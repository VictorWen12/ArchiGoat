//! Connection status owns atomic Provider selection without changing public Work truth.

use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::Notify;

use crate::{
    provider::Provider,
    state::{DaemonState, Phase, RunSlot, Status},
};

/// ConnectGuard releases only the connection attempt it admitted.
pub(crate) struct ConnectGuard {
    run_slot: Arc<StdMutex<RunSlot>>,
    work_notify: Arc<Notify>,
    id: u64,
}

// This guard wakes observers when a connection attempt ends or is superseded.
impl Drop for ConnectGuard {
    /// Drop releases Provider admission so the owner can retry after this attempt ends.
    fn drop(&mut self) {
        let mut slot = self
            .run_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.connecting == Some(self.id) {
            slot.connecting = None;
        }
        self.work_notify.notify_waiters();
    }
}

// This state exposes the current recoverable device-connection truth.
impl DaemonState {
    /// BeginConnect prevents a Provider switch from splitting an active Work across Agents.
    pub(crate) async fn begin_connect(
        &self,
        provider: Provider,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<(ConnectGuard, u64), String> {
        self.close_foreign_login(provider);
        let guard = {
            let mut slot = self
                .run_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !slot.active.is_empty() {
                return Err("A running Work keeps its current Provider connection".to_owned());
            }
            slot.next_connection = slot.next_connection.checked_add(1).unwrap_or(1);
            let id = slot.next_connection;
            slot.connecting = Some(id);
            ConnectGuard {
                run_slot: self.run_slot.clone(),
                work_notify: self.work_notify.clone(),
                id,
            }
        };
        let mut status = self.status.write().await;
        let epoch = status.epoch.checked_add(1).unwrap_or(1);
        *status = Status {
            phase: Phase::Authorizing,
            provider: Some(provider),
            model,
            effort,
            epoch,
        };
        drop(status);
        // A new epoch wakes every obsolete native check before it can publish stale truth.
        self.connection_events.notify_waiters();
        let _ = self.persist().await;
        Ok((guard, epoch))
    }

    /// BeginObserve re-proves a saved Provider without claiming the owner's connect admission.
    pub(crate) async fn begin_observe(&self, provider: Provider) -> Option<u64> {
        let status = self.status.read().await;
        // A silent probe opens no login terminal, so it publishes no intent and opens no epoch;
        // it inherits the current one, and the owner's next Connect supersedes it immediately.
        (status.provider == Some(provider)).then_some(status.epoch)
    }

    /// SetConnection accepts facts only from the current Provider attempt.
    pub(crate) async fn set_connection(
        &self,
        epoch: u64,
        phase: Phase,
        provider: Provider,
    ) -> bool {
        let mut status = self.status.write().await;
        if status.epoch != epoch || status.provider != Some(provider) {
            return false;
        }
        let model = status.model.clone();
        let effort = status.effort.clone();
        *status = Status {
            phase,
            provider: Some(provider),
            model,
            effort,
            epoch,
        };
        drop(status);
        // Every accepted native fact wakes obsolete observers after their listener is armed.
        self.connection_events.notify_waiters();
        // Persistence never invents public connection truth; the next real fact repairs it.
        let _ = self.persist().await;
        true
    }

    /// Disconnect removes native access after explicit signed-out evidence without exposing internals.
    pub(crate) async fn disconnect(&self, epoch: u64, provider: Provider) -> bool {
        let mut status = self.status.write().await;
        if status.epoch != epoch || status.provider != Some(provider) {
            return false;
        }
        *status = Status {
            phase: Phase::Disconnected,
            provider: None,
            model: None,
            effort: None,
            epoch,
        };
        drop(status);
        // Provider removal wakes stale checks and clears relay admission from native evidence.
        self.connection_events.notify_waiters();
        let _ = self.persist().await;
        true
    }

    /// ConnectionCurrent stops superseded native checks from publishing stale facts.
    pub(crate) async fn connection_current(&self, epoch: u64, provider: Provider) -> bool {
        let status = self.status.read().await;
        status.epoch == epoch && status.provider == Some(provider)
    }

    /// ConnectedProvider admits Work only against a currently proven native connection.
    pub(crate) async fn connected_provider(&self) -> Option<(Provider, u64)> {
        let status = self.status.read().await;
        (status.phase == Phase::Connected)
            .then_some(status.provider.map(|provider| (provider, status.epoch)))
            .flatten()
    }

    /// ConnectedSelection returns the selected Provider and its exact optional native tiers.
    pub(crate) async fn connected_selection(
        &self,
    ) -> Option<(Provider, Option<String>, Option<String>)> {
        let status = self.status.read().await;
        (status.phase == Phase::Connected)
            .then_some(
                status
                    .provider
                    .map(|provider| (provider, status.model.clone(), status.effort.clone())),
            )
            .flatten()
    }

    /// PublishModels keeps a Provider's own catalog only while its connection is still current.
    pub(crate) async fn publish_models(
        &self,
        epoch: u64,
        provider: Provider,
        models: Vec<crate::provider::ModelChoice>,
    ) {
        {
            let status = self.status.read().await;
            if status.epoch != epoch || status.provider != Some(provider) {
                return;
            }
            let mut catalog = self.model_catalog.write().await;
            *catalog = Some((provider, models));
        }
        self.connection_events.notify_waiters();
    }

    /// Models returns the current Provider's own catalog for the status surface.
    pub(crate) async fn models(
        &self,
        provider: Option<Provider>,
    ) -> Vec<crate::provider::ModelChoice> {
        let catalog = self.model_catalog.read().await;
        match (catalog.as_ref(), provider) {
            (Some((owner, models)), Some(current)) if *owner == current => models.clone(),
            _ => Vec::new(),
        }
    }

    /// PublishPresets keeps the published tiers only while their connection is still current.
    pub(crate) async fn publish_presets(
        &self,
        epoch: u64,
        provider: Provider,
        presets: crate::provider::PresetPair,
    ) {
        {
            let status = self.status.read().await;
            if status.epoch != epoch || status.provider != Some(provider) {
                return;
            }
            let mut map = self.preset_map.write().await;
            *map = Some((provider, presets));
        }
        self.connection_events.notify_waiters();
    }

    /// Presets returns the current Provider's tiers, which every Agent always has.
    pub(crate) async fn presets(
        &self,
        provider: Option<Provider>,
    ) -> Option<crate::provider::PresetPair> {
        let current = provider?;
        let map = self.preset_map.read().await;
        Some(match map.as_ref() {
            Some((owner, presets)) if *owner == current => presets.clone(),
            _ => super::presets::shipped(current),
        })
    }

    /// CloseForeignLogin ends a sign-in the owner abandoned by choosing another Agent.
    fn close_foreign_login(&self, provider: Provider) {
        let mut slot = self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.as_ref().is_some_and(|(held, _)| *held != provider) {
            *slot = None;
        }
    }

    /// AdoptLogin keeps this Agent's open sign-in window instead of opening a second one.
    pub(crate) fn adopt_login(&self, provider: Provider) -> bool {
        let mut slot = self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot.as_mut() {
            Some((held, login)) => *held == provider && login.running(),
            None => false,
        }
    }

    /// ParkLogin holds the one live sign-in flow so a pasted code can reach it.
    pub(crate) fn park_login(&self, provider: Provider, login: crate::cli::Login) {
        *self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((provider, login));
    }

    /// EndLogin closes this Agent's sign-in window once it has nothing left to finish.
    pub(crate) fn end_login(&self, provider: Provider) {
        let mut slot = self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.as_ref().is_some_and(|(held, _)| *held == provider) {
            *slot = None;
        }
    }

    /// SubmitCode hands the owner's one-time code to the live sign-in flow.
    pub(crate) async fn submit_code(&self, code: &str) -> Result<(), String> {
        let taken = self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some((provider, mut login)) = taken else {
            return Err("Start the sign-in first".to_owned());
        };
        let result = login.submit(code).await;
        let mut slot = self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A newer flow that arrived meanwhile owns the slot; this one ends with its own attempt.
        if slot.is_none() {
            *slot = Some((provider, login));
        }
        result
    }
}
