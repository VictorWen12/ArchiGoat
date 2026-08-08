//! Steering state queues exact follow-ups and rotates physical turns without changing Work ownership.

use std::collections::VecDeque;

use crate::{state::RunProgress, work::RuntimeSteer};

use super::{
    model::{Entry, TurnStop},
    store::WorkStore,
};

/// SteeringRollback restores every steering fact when its durable save fails.
pub(super) struct SteeringRollback {
    steer: Option<RuntimeSteer>,
    steers: VecDeque<RuntimeSteer>,
    steering: bool,
    steer_delivered: bool,
    rotating: bool,
    stopping: bool,
    launched: bool,
    repair: bool,
    repairs: u32,
    attention: bool,
    failure: Option<String>,
    progress: Option<RunProgress>,
    turn_stop: TurnStop,
}

// This store mutates steering only through rollback-safe durable transition primitives.
impl WorkStore {
    /// QueueSteer accepts one idempotent follow-up as head or ordered tail.
    pub(super) fn queue_steer(
        &mut self,
        work_id: &str,
        steer: RuntimeSteer,
    ) -> Result<Option<(SteeringRollback, bool)>, String> {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return Err("Work is not Running".to_owned());
        };
        steer.validate(work_id, &work.session)?;
        if work.stopping {
            return Err("Work is stopping".to_owned());
        }
        if work.steer.as_ref().is_some_and(|item| item.id == steer.id)
            || work.steers.iter().any(|item| item.id == steer.id)
        {
            return Ok(None);
        }
        let rollback = SteeringRollback::capture(work);
        let resumed = work.attention;
        if resumed {
            work.rotating = false;
            work.launched = false;
            work.repairs = 0;
            work.attention = false;
            // Hide the parked label without resetting the monotonic cursor the browser already observed.
            if let Some(progress) = &mut work.progress {
                progress.text.clear();
            }
            // A parked Work keeps every undelivered message: the new turn arms the queue head
            // and the rest stay queued — no user words are ever dropped.
            if let Some(queued) = work.steer.take() {
                work.steers.push_front(queued);
            }
            work.steers.push_back(steer);
            let head = work
                .steers
                .pop_front()
                .expect("steer queue holds at least the new message");
            work.steer = Some(head);
            work.steering = true;
            // A head no runner has heard yet is delivered by the launch this nudge arms.
            work.steer_delivered = false;
            work.repair = false;
            work.failure = None;
            work.turn_stop = TurnStop::new();
            return Ok(Some((rollback, true)));
        }
        if work.steer.is_none() {
            work.steer = Some(steer);
        } else {
            work.steers.push_back(steer);
        }
        Ok(Some((rollback, false)))
    }

    /// RotationAuthority exposes only an eligible queued Build turn's internal stop signal.
    pub(super) fn rotation_authority(&self, work_id: &str) -> Option<TurnStop> {
        let Entry::Running(work) = self.entries.get(work_id)? else {
            return None;
        };
        let queued =
            (!work.steering && work.steer.is_some()) || (work.steering && !work.steers.is_empty());
        (queued
            && work.launched
            && !work.native_session.is_empty()
            && !work.rotating
            && !work.stopping)
            .then(|| work.turn_stop.clone())
    }

    /// BeginRotation records a physically ended turn and advances an active head to its tail.
    pub(super) fn begin_rotation(&mut self, work_id: &str) -> Option<SteeringRollback> {
        let Entry::Running(work) = self.entries.get_mut(work_id)? else {
            return None;
        };
        if work.rotating || work.stopping {
            return None;
        }
        let queued =
            (!work.steering && work.steer.is_some()) || (work.steering && !work.steers.is_empty());
        if !queued {
            return None;
        }
        let rollback = SteeringRollback::capture(work);
        if work.steering {
            work.steer = work.steers.pop_front();
            work.steering = false;
        }
        work.rotating = true;
        Some(rollback)
    }

    /// ArmHead starts the durable head only after prior physical cleanup completed.
    pub(super) fn arm_head(&mut self, work_id: &str) -> Option<SteeringRollback> {
        let Entry::Running(work) = self.entries.get_mut(work_id)? else {
            return None;
        };
        if !work.rotating || work.stopping || work.steer.is_none() {
            return None;
        }
        let rollback = SteeringRollback::capture(work);
        work.steering = true;
        work.rotating = false;
        work.launched = false;
        // This head's words have not reached a runner yet; the launch this arms is what delivers them.
        work.steer_delivered = false;
        work.repair = false;
        work.turn_stop = TurnStop::new();
        Some(rollback)
    }

    /// BeginStop durably clears every continuation before owner Stop can settle the Work.
    pub(super) fn begin_stop(&mut self, work_id: &str) -> Option<SteeringRollback> {
        let Entry::Running(work) = self.entries.get_mut(work_id)? else {
            return None;
        };
        if work.stopping {
            return None;
        }
        let rollback = SteeringRollback::capture(work);
        work.stopping = true;
        work.steer = None;
        work.steers.clear();
        work.steering = false;
        work.rotating = false;
        Some(rollback)
    }

    /// RollbackSteering restores the exact in-memory facts preceding a failed durable save.
    pub(super) fn rollback_steering(&mut self, work_id: &str, rollback: SteeringRollback) {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return;
        };
        work.steer = rollback.steer;
        work.steers = rollback.steers;
        work.steering = rollback.steering;
        work.steer_delivered = rollback.steer_delivered;
        work.rotating = rollback.rotating;
        work.stopping = rollback.stopping;
        work.launched = rollback.launched;
        work.repair = rollback.repair;
        work.repairs = rollback.repairs;
        work.attention = rollback.attention;
        work.failure = rollback.failure;
        work.progress = rollback.progress;
        work.turn_stop = rollback.turn_stop;
    }

    /// TurnStopAuthority returns the current physical turn's internal interruption signal.
    pub(super) fn turn_stop_authority(&self, work_id: &str) -> Option<TurnStop> {
        match self.entries.get(work_id) {
            Some(Entry::Running(work)) => Some(work.turn_stop.clone()),
            _ => None,
        }
    }
}

// This snapshot makes every steering transition exactly reversible before durable commit.
impl SteeringRollback {
    /// Capture copies only steering facts changed by this leaf.
    fn capture(work: &super::model::Running) -> Self {
        Self {
            steer: work.steer.clone(),
            steers: work.steers.clone(),
            steering: work.steering,
            steer_delivered: work.steer_delivered,
            rotating: work.rotating,
            stopping: work.stopping,
            launched: work.launched,
            repair: work.repair,
            repairs: work.repairs,
            attention: work.attention,
            failure: work.failure.clone(),
            progress: work.progress.clone(),
            turn_stop: work.turn_stop.clone(),
        }
    }
}
