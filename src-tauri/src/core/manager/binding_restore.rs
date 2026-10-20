use super::{CoreManager, lifecycle::ProxyRestoreExpectation};
use crate::{
    config::{Config, cloudflare_speed_test::CloudflareDnsRestoreStatus, runtime::IRuntime},
    constants::timing,
    core::{handle::Handle, service},
    utils::retry::{RetryError, RetryPolicy, retry},
};
use anyhow::Result;
use std::{num::NonZeroUsize, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RestoreCapture {
    core: ProxyRestoreExpectation,
    profile_uid: String,
}

#[derive(Default)]
pub(super) struct BindingRestore {
    capture: Option<RestoreCapture>,
    status: Option<CloudflareDnsRestoreStatus>,
    reason: Option<String>,
    failed_runtime: Option<Arc<Box<IRuntime>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl BindingRestore {
    fn queue(&mut self, capture: RestoreCapture) -> bool {
        if self.capture.as_ref() == Some(&capture) {
            return false;
        }
        self.capture = Some(capture);
        self.status = Some(CloudflareDnsRestoreStatus::Pending);
        self.reason = None;
        self.failed_runtime = None;
        true
    }

    fn finish(&mut self, capture: &RestoreCapture, result: Result<()>, original_runtime: Arc<Box<IRuntime>>) {
        if self.capture.as_ref() == Some(capture) {
            self.status = result.as_ref().err().map(|_| CloudflareDnsRestoreStatus::Error);
            self.reason = result.as_ref().err().map(|error| format!("{error:#}"));
            self.failed_runtime = result.err().map(|_| original_runtime);
        }
    }
}

impl std::fmt::Debug for BindingRestore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BindingRestore")
            .field("capture", &self.capture)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl CoreManager {
    pub(super) fn binding_restore_core_matches(&self, capture: &RestoreCapture) -> bool {
        capture.core.is_valid(
            self.get_running_mode().as_ref(),
            self.current_core_readiness_generation(),
            service::owner_monitor_generation(),
        )
    }

    pub(super) async fn binding_restore_is_current(&self, capture: &RestoreCapture) -> bool {
        !Handle::global().is_exiting()
            && self.binding_restore_core_matches(capture)
            && Config::profiles().await.data_arc().current.as_deref() == Some(capture.profile_uid.as_str())
    }

    pub(super) async fn schedule_binding_restore(&self, core: ProxyRestoreExpectation) {
        let Some(profile_uid) = Config::profiles().await.data_arc().current.clone() else {
            return;
        };
        let capture = RestoreCapture {
            core,
            profile_uid: profile_uid.to_string(),
        };
        let mut restore = self.binding_restore.lock();
        if !restore.queue(capture) {
            return;
        }
        if restore.task.as_ref().is_none_or(|task| task.is_finished()) {
            restore.task = Some(tokio::spawn(async {
                CoreManager::global().run_binding_restore().await;
            }));
        }
    }

    async fn run_binding_restore(&self) {
        loop {
            let capture = {
                let mut restore = self.binding_restore.lock();
                if restore.status != Some(CloudflareDnsRestoreStatus::Pending) {
                    restore.task.take();
                    return;
                }
                restore.status = Some(CloudflareDnsRestoreStatus::Restoring);
                restore.capture.clone().expect("pending restore has a capture")
            };
            // The trigger holds lifecycle ownership. Wait for that boundary without holding
            // configuration ownership, then acquire the transaction locks in their normal order.
            drop(self.lifecycle_lock.lock().await);
            let result = retry(
                RetryPolicy::fixed(NonZeroUsize::MIN.saturating_add(2), timing::CONFIG_UPDATE_DEBOUNCE),
                |_| async {
                    if !self.binding_restore_is_current(&capture).await {
                        return Ok(());
                    }
                    let write = Config::lock_config_write().await;
                    let update = self.claim_config_update(&write).map_err(RetryError::Retry)?;
                    let original_runtime = Config::runtime().await.data_arc();
                    let result = self.restore_bindings_in_update(&capture, &update).await;
                    self.binding_restore.lock().finish(&capture, result, original_runtime);
                    Ok(())
                },
            )
            .await;
            if let Err(error) = result {
                let original_runtime = Config::runtime().await.data_arc();
                self.binding_restore
                    .lock()
                    .finish(&capture, Err(error), original_runtime);
            }
            {
                let mut restore = self.binding_restore.lock();
                if restore.capture.as_ref() == Some(&capture)
                    && restore.status == Some(CloudflareDnsRestoreStatus::Restoring)
                {
                    restore.status = None;
                }
            }
            Handle::refresh_clash();
        }
    }

    pub(crate) async fn get_binding_restore_status(
        &self,
        profile_uid: &str,
    ) -> Option<(CloudflareDnsRestoreStatus, Option<String>)> {
        let runtime = Config::runtime().await.data_arc();
        let restore = self.binding_restore.lock();
        let capture = restore.capture.as_ref()?;
        if capture.profile_uid != profile_uid
            || Handle::global().is_exiting()
            || !self.binding_restore_core_matches(capture)
            || restore
                .failed_runtime
                .as_ref()
                .is_some_and(|failed| !Arc::ptr_eq(failed, &runtime))
        {
            return None;
        }
        restore.status.map(|status| (status, restore.reason.clone()))
    }

    pub(super) async fn saved_bindings_need_restore(&self, capture: &RestoreCapture) -> Option<bool> {
        if !self.binding_restore_is_current(capture).await {
            return None;
        }
        let verge = Config::verge().await.data_arc();
        let Some(settings) = verge
            .profile_cloudflare_dns_settings
            .as_ref()
            .and_then(|settings| settings.get(&capture.profile_uid))
            .filter(|settings| settings.enabled && !settings.bindings.is_empty())
        else {
            return None;
        };
        let runtime = Config::runtime().await.data_arc();
        let Some(metadata) = runtime
            .cloudflare_dns
            .as_ref()
            .filter(|metadata| metadata.profile_uid == capture.profile_uid)
        else {
            return None;
        };
        let already_reconciled = matches!(
            metadata.restore_status,
            CloudflareDnsRestoreStatus::Active | CloudflareDnsRestoreStatus::Dormant
        ) && settings.bindings.iter().all(|binding| {
            metadata.applied_domains.contains(&binding.domain) || metadata.dormant_domains.contains(&binding.domain)
        });
        Some(!already_reconciled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::manager::RunningMode;

    fn capture(mode: RunningMode, generation: u64, owner: u64) -> RestoreCapture {
        RestoreCapture {
            core: ProxyRestoreExpectation::capture(mode, Some(generation), owner).unwrap(),
            profile_uid: "profile-a".into(),
        }
    }

    #[test]
    fn ready_generation_is_owned_once_and_old_completion_cannot_finish_its_replacement() {
        let old = capture(RunningMode::Service, 3, 1);
        let new = capture(RunningMode::Service, 5, 2);
        let mut restore = BindingRestore::default();
        assert!(restore.queue(old.clone()));
        assert_eq!(restore.status, Some(CloudflareDnsRestoreStatus::Pending));
        assert!(!restore.queue(old.clone()));
        restore.finish(
            &old,
            Err(anyhow::anyhow!("reload rejected")),
            Arc::new(Box::new(IRuntime::default())),
        );
        assert_eq!(restore.status, Some(CloudflareDnsRestoreStatus::Error));
        assert!(!restore.queue(old.clone()));
        assert!(restore.queue(new.clone()));
        restore.finish(&old, Ok(()), Arc::new(Box::new(IRuntime::default())));
        assert_eq!(restore.status, Some(CloudflareDnsRestoreStatus::Pending));
        restore.finish(&new, Ok(()), Arc::new(Box::new(IRuntime::default())));
        assert_eq!(restore.status, None);
        assert!(!restore.queue(new));
    }

    #[test]
    fn stop_replacement_and_service_owner_loss_invalidate_the_captured_core() {
        let manager = CoreManager::isolated();
        for mode in [RunningMode::Service, RunningMode::Sidecar] {
            let generation = manager.mark_core_ready();
            manager.core_started(mode);
            let captured = capture(mode, generation, service::owner_monitor_generation());
            assert!(manager.binding_restore_core_matches(&captured));
            if mode == RunningMode::Service {
                let wrong_owner = capture(mode, generation, service::owner_monitor_generation().wrapping_add(1));
                assert!(!manager.binding_restore_core_matches(&wrong_owner));
            }
            manager.core_stopped();
            assert!(!manager.binding_restore_core_matches(&captured));
            manager.mark_core_ready();
            manager.core_started(mode);
            assert!(!manager.binding_restore_core_matches(&captured));
        }
    }
}
