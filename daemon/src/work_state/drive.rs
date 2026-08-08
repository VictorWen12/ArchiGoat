//! Drive launches once, then observes or reattaches the same native runner until verified delivery or owner Stop.

use std::time::Duration;

use super::{NativeOwnership, completion::CompletionError};
use crate::{
    host, process,
    state::{DaemonState, RunPhase},
    work::RuntimeWork,
};

const RETRY: Duration = Duration::from_millis(250);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// A replacement runner never waits longer than this: the cooldown exists only so a runner that dies
/// the instant it launches cannot relaunch in a hot loop.
const MAX_COOLDOWN: Duration = Duration::from_secs(2);

/// A turn ends for exactly one reason, and the conversation names it.
const DELIVERED: &str = "delivered";
const STEERED: &str = "steered";
const STOPPED: &str = "stopped";

/// Run consumes one stable host admission for fresh or pre-launch recovery without creating another Work.
pub(super) async fn run(state: DaemonState, mut runtime: RuntimeWork) {
    // One task owns every sequential runner generation, preventing overlapping repair execution. A
    // runner already proven dead is replaced with no pause at all, so the screen never goes quiet
    // waiting for a runner that is already gone.
    let mut cooldown: Option<Duration> = None;
    loop {
        let current = runtime.clone();
        let state_ref = &state;
        let turn = async move {
            if current.rotating {
                resume_steer_rotation(state_ref, &current).await
            } else {
                run_turn(state_ref, &current).await
            }
        };
        let next = turn.await;
        let Some(next) = next else {
            return;
        };
        if runtime.rotating
            || runtime.steer.as_ref().map(|steer| &steer.id)
                != next.steer.as_ref().map(|steer| &steer.id)
        {
            cooldown = None;
            runtime = next;
            continue;
        }
        // Only a generation that already relaunched once pauses, and never past the anti-hot-loop cap.
        if let Some(pause) = cooldown {
            tokio::time::sleep(pause).await;
        }
        cooldown = Some(cooldown.map_or(RETRY, |pause| (pause * 2).min(MAX_COOLDOWN)));
        runtime = next;
    }
}

/// RunTurn launches and settles one physical native turn for the durable Work owner.
async fn run_turn(state: &DaemonState, runtime: &RuntimeWork) -> Option<RuntimeWork> {
    let stop = state.owner_stop(&runtime.work_id)?;
    let turn = state.turn_stop(&runtime.work_id)?;
    state.request_pending_rotation(&runtime.work_id);
    // Every retry reuses the same signed runner ID; the host claim admits native execution once.
    let mut retry = RETRY;
    let (mut runner, physically_live) = if runtime.launched {
        loop {
            if turn.requested() {
                return None;
            }
            match host::reattach(&runtime.work_id, &runtime.session, state) {
                Ok(Some(mut runner)) => {
                    let physically_live = runner.proves_liveness();
                    break (runner, physically_live);
                }
                Ok(None) if stop.requested() => {
                    if owner_stopped(state, &runtime.work_id) {
                        return None;
                    }
                    backoff(&mut retry).await;
                }
                Ok(None) => backoff(&mut retry).await,
                Err(error) => {
                    eprintln!("Product native runner reattach retry: {error}");
                    backoff(&mut retry).await;
                }
            }
        }
    } else {
        loop {
            if turn.requested() {
                return None;
            }
            if stop.requested() {
                if owner_stopped(state, &runtime.work_id) {
                    return None;
                }
                backoff(&mut retry).await;
                continue;
            }
            match runtime.launch(state).await {
                Ok(mut runner) => {
                    if state.mark_launched(&runtime.work_id).is_err() {
                        backoff(&mut retry).await;
                        continue;
                    }
                    let physically_live = fresh_liveness(&mut runner).await;
                    break (runner, physically_live);
                }
                Err(error) => {
                    eprintln!("Product native runner launch retry: {error}");
                    // A signed durable runner is always reattached before launch may be retried.
                    let recovered = loop {
                        if turn.requested() {
                            return None;
                        }
                        match host::reattach(&runtime.work_id, &runtime.session, state) {
                            Ok(Some(runner)) => break Some(runner),
                            Ok(None) => {
                                backoff(&mut retry).await;
                                break None;
                            }
                            Err(error) => {
                                eprintln!("Product native runner reattach retry: {error}");
                                backoff(&mut retry).await;
                            }
                        }
                    };
                    if let Some(runner) = recovered {
                        if state.mark_launched(&runtime.work_id).is_err() {
                            backoff(&mut retry).await;
                            continue;
                        }
                        let mut runner = runner;
                        let physically_live = runner.proves_liveness();
                        break (runner, physically_live);
                    }
                }
            }
        }
    };
    let mut native = state.own_native_run(&runtime.work_id, physically_live);
    if !runtime.launched {
        let sequence = state.work_progress_sequence(&runtime.work_id);
        if let Err(error) =
            state.replace_work_progress(&runtime.work_id, sequence, "Running".to_owned())
        {
            eprintln!("Product turn stage reset retry: {error}");
        }
    }
    observe_until_terminal(state, runtime, &stop, &turn, &mut runner, &mut native).await
}

/// FreshLiveness admits public Running only after this platform proves its spawned native process.
#[cfg(target_os = "macos")]
async fn fresh_liveness(runner: &mut host::AgentRun) -> bool {
    runner.fresh_liveness().await
}

/// Windows launch retains a live supervisor, so its existing proof is immediate and physical.
#[cfg(target_os = "windows")]
async fn fresh_liveness(runner: &mut host::AgentRun) -> bool {
    runner.proves_liveness()
}

/// ObserveUntilTerminal releases each finished observation epoch before settling or reattaching.
async fn observe_until_terminal(
    state: &DaemonState,
    runtime: &RuntimeWork,
    stop: &crate::state::OwnerStop,
    turn: &crate::state::TurnStop,
    runner: &mut host::AgentRun,
    native: &mut Option<NativeOwnership>,
) -> Option<RuntimeWork> {
    let update_base = if runtime.launched {
        0
    } else {
        state.work_progress_sequence(&runtime.work_id)
    };
    let mut observer = process::Observer::new(runtime.provider, update_base);
    let mut last_error_sequence = 0;
    let mut retry = RETRY;
    loop {
        let outcome = observer
            .observe(
                runner,
                stop.clone(),
                turn.clone(),
                |index, update| state.record_work_stage(&runtime.work_id, index, update),
                |session| state.bind_native_session(&runtime.work_id, session),
                |total| state.replace_work_tokens(&runtime.work_id, total),
                |model| state.replace_work_model(&runtime.work_id, model),
                |output| state.record_protected_literal(&runtime.work_id, output),
                |id, answer| state.record_agent_message(&runtime.work_id, id, answer),
            )
            .await;
        *native = None;
        let mut evidence_retry = RETRY;
        while let Err(error) =
            state.record_protected_outputs(&runtime.work_id, observer.private_outputs())
        {
            eprintln!("Product protected output commit retry: {error}");
            backoff(&mut evidence_retry).await;
        }
        match outcome {
            Ok(Some(observed)) => {
                let mut rotation_retry = RETRY;
                loop {
                    if stop.requested() {
                        if owner_stopped(state, &runtime.work_id) {
                            complete_runner(runner, observed.terminal_sequence).await;
                            return None;
                        }
                    } else {
                        match state.record_steer_rotation(&runtime.work_id) {
                            Ok(true) => {
                                state.push_work_turn_boundary(&runtime.work_id, STEERED);
                                complete_runner(runner, observed.terminal_sequence).await;
                                return arm_steer(state, runtime, stop).await;
                            }
                            Ok(false) => break,
                            Err(error) => {
                                eprintln!("Product follow-up rotation commit retry: {error}")
                            }
                        }
                    }
                    backoff(&mut rotation_retry).await;
                }
                // Native completion cannot become Done until answer or artifact evidence commits durably.
                let mut delivery_retry = RETRY;
                loop {
                    if stop.requested() {
                        if owner_stopped(state, &runtime.work_id) {
                            complete_runner(runner, observed.terminal_sequence).await;
                            return None;
                        }
                        backoff(&mut delivery_retry).await;
                        continue;
                    }
                    match state.complete_observed(&runtime.work_id, &observed) {
                        Ok(()) => {
                            if let Some(delivered) = state.run_snapshot(&runtime.work_id) {
                                state.record_artifacts(&runtime.work_id, &delivered.files);
                            }
                            state.push_work_turn_boundary(&runtime.work_id, DELIVERED);
                            complete_runner(runner, observed.terminal_sequence).await;
                            return None;
                        }
                        Err(CompletionError::Retry(reason)) => {
                            // Durable commit diagnostics keep verified output pending while the machine repairs.
                            eprintln!("Product Work delivery commit retry: {reason}");
                            backoff(&mut delivery_retry).await;
                        }
                        Err(CompletionError::Repair(reason)) => {
                            return repair_turn(
                                state,
                                runtime,
                                stop,
                                runner,
                                observed.terminal_sequence,
                                observed.native_session,
                                reason,
                            )
                            .await;
                        }
                    }
                }
            }
            Ok(None) if stop.requested() => {
                if owner_stopped(state, &runtime.work_id) {
                    if let Some(sequence) = runner.terminal_sequence() {
                        complete_runner(runner, sequence).await;
                    }
                    return None;
                }
                backoff(&mut retry).await;
            }
            Ok(None) if observer.rotated() => {
                let Some(sequence) = runner.terminal_sequence() else {
                    continue;
                };
                if state
                    .run_snapshot(&runtime.work_id)
                    .is_none_or(|snapshot| snapshot.phase != RunPhase::Running)
                {
                    complete_runner(runner, sequence).await;
                    return None;
                }
                loop {
                    if stop.requested() {
                        if owner_stopped(state, &runtime.work_id) {
                            complete_runner(runner, sequence).await;
                            return None;
                        }
                    } else {
                        match state.record_steer_rotation(&runtime.work_id) {
                            Ok(true) => {
                                state.push_work_turn_boundary(&runtime.work_id, STEERED);
                                complete_runner(runner, sequence).await;
                                return arm_steer(state, runtime, stop).await;
                            }
                            Ok(false) => {
                                eprintln!("Product follow-up rotation lost its queued message")
                            }
                            Err(error) => {
                                eprintln!("Product follow-up rotation commit retry: {error}")
                            }
                        }
                    }
                    backoff(&mut retry).await;
                }
            }
            // An ended runner earns bounded same-session repair; only exhaustion asks the creator.
            // PHYSICS: the native process is gone, so this turn has no runner left to observe.
            Ok(None) if observer.stopped() => {
                let Some(sequence) = runner.terminal_sequence() else {
                    continue;
                };
                let session = observer
                    .native_session()
                    .unwrap_or(&runtime.native_session)
                    .to_owned();
                return repair_turn(
                    state,
                    runtime,
                    stop,
                    runner,
                    sequence,
                    session,
                    super::model::RUNNER_END.to_owned(),
                )
                .await;
            }
            Ok(None) => {
                if !reattach(state, runtime, stop, turn, runner, native).await {
                    return None;
                }
            }
            Err(error) => {
                if observer.terminal_failure() {
                    // A terminal frame proves the old runner is gone before a continuation is admitted.
                    let Some(sequence) = runner.terminal_sequence() else {
                        continue;
                    };
                    let session = observer
                        .native_session()
                        .unwrap_or(&runtime.native_session)
                        .to_owned();
                    return repair_turn(state, runtime, stop, runner, sequence, session, error)
                        .await;
                }
                // Capped backoff prevents a persistent journal defect from consuming the machine.
                if observer.sequence() > last_error_sequence {
                    retry = RETRY;
                    last_error_sequence = observer.sequence();
                } else {
                    retry = (retry * 2).min(MAX_RETRY);
                }
                tokio::time::sleep(retry).await;
                if !reattach(state, runtime, stop, turn, runner, native).await {
                    return None;
                }
            }
        }
    }
}

/// ResumeSteerRotation finishes a durably ended old turn before arming its queued follow-up.
async fn resume_steer_rotation(state: &DaemonState, runtime: &RuntimeWork) -> Option<RuntimeWork> {
    let stop = state.owner_stop(&runtime.work_id)?;
    let turn = state.turn_stop(&runtime.work_id)?;
    let mut retry = RETRY;
    loop {
        if turn.requested()
            || state
                .run_snapshot(&runtime.work_id)
                .is_none_or(|snapshot| snapshot.phase != RunPhase::Running)
        {
            return None;
        }
        match host::reattach(&runtime.work_id, &runtime.session, state) {
            Ok(Some(mut runner)) => {
                let physically_live = runner.proves_liveness();
                let _native = state.own_native_run(&runtime.work_id, physically_live);
                let mut observer = process::Observer::new(runtime.provider, 0);
                let settled = observer
                    .observe(
                        &mut runner,
                        stop.clone(),
                        turn.clone(),
                        |_, _| Ok(()),
                        |session| {
                            (session == runtime.native_session)
                                .then_some(())
                                .ok_or_else(|| {
                                    "Provider native session changed during follow-up rotation"
                                        .to_owned()
                                })
                        },
                        |_| Ok(()),
                        |_| Ok(()),
                        |output| state.record_protected_literal(&runtime.work_id, output),
                        // The ended turn already published its messages; this replay only reaches its end.
                        |_, answer| state.append_work_answer(&runtime.work_id, answer),
                    )
                    .await;
                drop(_native);
                while let Err(error) =
                    state.record_protected_outputs(&runtime.work_id, observer.private_outputs())
                {
                    eprintln!("Product protected rotation evidence retry: {error}");
                    backoff(&mut retry).await;
                }
                match settled {
                    Ok(Some(_)) | Ok(None)
                        if observer.rotated() || turn.requested() || stop.requested() =>
                    {
                        if let Some(sequence) = runner.terminal_sequence() {
                            complete_runner(&mut runner, sequence).await;
                            break;
                        }
                    }
                    Ok(None) => {
                        eprintln!("Product follow-up rotation retry: native runner stopped")
                    }
                    Err(error) => eprintln!("Product follow-up rotation retry: {error}"),
                    Ok(Some(_)) => {}
                }
            }
            // Missing private runner state proves its terminal cleanup already committed.
            Ok(None) => break,
            Err(error) => eprintln!("Product follow-up rotation reattach retry: {error}"),
        }
        backoff(&mut retry).await;
    }
    if stop.requested() {
        while !owner_stopped(state, &runtime.work_id) {
            backoff(&mut retry).await;
        }
        return None;
    }
    if state
        .run_snapshot(&runtime.work_id)
        .is_none_or(|snapshot| snapshot.phase != RunPhase::Running)
    {
        return None;
    }
    arm_steer(state, runtime, &stop).await
}

/// ArmSteer retries the durable continuation edge after the prior runner is physically gone.
async fn arm_steer(
    state: &DaemonState,
    runtime: &RuntimeWork,
    stop: &crate::state::OwnerStop,
) -> Option<RuntimeWork> {
    let mut retry = RETRY;
    let mut cleared = false;
    loop {
        if state
            .run_snapshot(&runtime.work_id)
            .is_none_or(|snapshot| snapshot.phase != RunPhase::Running)
        {
            return None;
        }
        if stop.requested() {
            if owner_stopped(state, &runtime.work_id) {
                return None;
            }
        } else if !cleared {
            match crate::delivery::discard_private_tree(&runtime.freeze_root) {
                Ok(()) => cleared = true,
                Err(error) => eprintln!("Product follow-up freeze cleanup retry: {error}"),
            }
        } else {
            match state.prepare_steer(&runtime.work_id) {
                Ok(steer) => return Some(steer),
                Err(error) => eprintln!("Product follow-up arm retry: {error}"),
            }
        }
        backoff(&mut retry).await;
    }
}

/// RepairTurn continues a bound native session; a pre-session death asks the creator instead of looping.
async fn repair_turn(
    state: &DaemonState,
    runtime: &RuntimeWork,
    stop: &crate::state::OwnerStop,
    runner: &mut host::AgentRun,
    sequence: u64,
    native_session: String,
    reason: String,
) -> Option<RuntimeWork> {
    // The diagnostic is local; public Work remains Running and its turn is still open while the
    // product repairs itself, so the conversation names no ending the creator never saw.
    eprintln!("Product Work self-repair: {reason}");
    complete_runner(runner, sequence).await;
    let mut retry = RETRY;
    loop {
        if state
            .run_snapshot(&runtime.work_id)
            .is_none_or(|snapshot| snapshot.phase != RunPhase::Running)
        {
            return None;
        }
        if stop.requested() {
            while !owner_stopped(state, &runtime.work_id) {
                backoff(&mut retry).await;
            }
            return None;
        }
        let repair = if native_session.is_empty() {
            state
                .prepare_attention(&runtime.work_id, reason.clone())
                .map(|()| None)
        } else {
            state.prepare_repair(&runtime.work_id, native_session.clone(), reason.clone())
        };
        match repair {
            // An exhausted repair budget ends this driver while the Work stays durably resumable.
            Ok(repair) => return repair,
            Err(error) => {
                eprintln!("Product Work repair commit retry: {error}");
                backoff(&mut retry).await;
            }
        }
    }
}

/// OwnerStopped commits the creator's Stop and closes the turn it interrupted.
// PHYSICS: the owner's own Stop is one of the two ends a turn has.
fn owner_stopped(state: &DaemonState, work_id: &str) -> bool {
    if !state.mark_owner_stopped(work_id) {
        return false;
    }
    state.push_work_turn_boundary(work_id, STOPPED);
    true
}

/// CompleteRunner retries terminal cleanup so private native state is never silently orphaned.
async fn complete_runner(runner: &mut host::AgentRun, sequence: u64) {
    let mut retry = RETRY;
    while runner.complete(sequence).is_err() {
        tokio::time::sleep(retry).await;
        retry = (retry * 2).min(MAX_RETRY);
    }
}

/// Backoff keeps persistent machine faults recoverable without hot or fixed-rate polling.
async fn backoff(retry: &mut Duration) {
    tokio::time::sleep(*retry).await;
    *retry = (*retry * 2).min(MAX_RETRY);
}

/// Reattach replays only unconfirmed native journal frames and honors owner Stop while unavailable.
async fn reattach(
    state: &DaemonState,
    runtime: &RuntimeWork,
    stop: &crate::state::OwnerStop,
    turn: &crate::state::TurnStop,
    runner: &mut host::AgentRun,
    native: &mut Option<NativeOwnership>,
) -> bool {
    *native = None;
    let mut retry = RETRY;
    loop {
        if turn.requested()
            || state
                .run_snapshot(&runtime.work_id)
                .is_none_or(|snapshot| snapshot.phase != RunPhase::Running)
        {
            return false;
        }
        match host::reattach(&runtime.work_id, &runtime.session, state) {
            Ok(Some(reattached)) => {
                *runner = reattached;
                let physically_live = runner.proves_liveness();
                *native = state.own_native_run(&runtime.work_id, physically_live);
                return true;
            }
            Ok(None) if stop.requested() => {
                if owner_stopped(state, &runtime.work_id) {
                    return false;
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("Product native runner reattach retry: {error}"),
        }
        backoff(&mut retry).await;
    }
}
