use crate::{
    config::{Config, cloudflare_speed_test::*, profiles::PROFILE_WRITE_LOCK},
    core::{
        CoreManager, cloudflare_speed_test,
        notify::{Refresh, announce},
    },
    enhance::cloudflare_dns,
};
use anyhow::{Result, ensure};
use clash_verge_draft::DraftTransaction;

fn stale(
    profile_uid: &str,
    fingerprint: &str,
    current_uid: &str,
    current_fingerprint: &str,
) -> Option<CloudflareDnsOutcome> {
    (profile_uid != current_uid || fingerprint != current_fingerprint).then(|| CloudflareDnsOutcome::Stale {
        reason:
            "The active profile or its committed sources changed; refresh the inventory and review the bindings again"
                .into(),
    })
}

#[derive(Debug)]
struct ManualBindingStale;

impl std::fmt::Display for ManualBindingStale {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Profile sources changed while preparing the binding update; refresh the inventory")
    }
}

impl std::error::Error for ManualBindingStale {}

fn validate_manual_generation(
    runtime: &crate::config::runtime::IRuntime,
    profile_uid: &str,
    fingerprint: &str,
    settings: &ProfileCloudflareDnsSettings,
    activate: bool,
) -> Result<()> {
    let Some(metadata) = runtime.cloudflare_dns.as_ref() else {
        return Err(ManualBindingStale.into());
    };
    if metadata.profile_uid != profile_uid
        || metadata.fingerprint != fingerprint
        || (activate
            && settings.confirmation.as_deref() != Some(cloudflare_dns::confirmation(metadata, settings)?.as_str()))
    {
        return Err(ManualBindingStale.into());
    }
    ensure!(
        !activate || metadata.active,
        "The generated runtime could not activate the requested DNS bindings"
    );
    Ok(())
}

fn bindings_ready_for_activation(
    metadata: &cloudflare_dns::CloudflareDnsRuntime,
    previous: &[CloudflareDnsBinding],
    bindings: &[CloudflareDnsBinding],
) -> Result<bool> {
    for binding in bindings.iter().filter(|binding| !previous.contains(binding)) {
        cloudflare_dns::validate_eligibility(metadata, std::slice::from_ref(binding))?;
    }
    Ok(bindings
        .iter()
        .any(|binding| cloudflare_dns::validate_eligibility(metadata, std::slice::from_ref(binding)).is_ok()))
}

pub async fn get_cloudflare_dns_inventory() -> Result<CloudflareDnsInventory> {
    let runtime = Config::runtime().await.data_arc();
    let metadata = runtime
        .cloudflare_dns
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No committed profile inventory is available"))?;
    let profiles = Config::profiles().await.data_arc();
    ensure!(
        profiles.current.as_deref().unwrap_or_default() == metadata.profile_uid,
        "The active profile changed; refresh the inventory"
    );
    let mut refreshed = cloudflare_dns::collect(metadata.base.clone(), &profiles).await?;
    refreshed.warnings.extend(
        metadata
            .warnings
            .iter()
            .filter(|warning| !refreshed.warnings.contains(warning))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let effective_enabled = metadata.active;
    let applied_domains = metadata.applied_domains.clone();
    let dormant_domains = metadata.dormant_domains.clone();
    let (restore_status, restore_reason) = CoreManager::global()
        .get_binding_restore_status(&metadata.profile_uid)
        .await
        .unwrap_or((metadata.restore_status, metadata.restore_reason.clone()));
    let metadata = &refreshed;
    let verge = Config::verge().await.data_arc();
    let settings = verge
        .profile_cloudflare_dns_settings
        .as_ref()
        .and_then(|settings| settings.get(&metadata.profile_uid))
        .cloned()
        .unwrap_or_default();
    Ok(CloudflareDnsInventory {
        profile_uid: metadata.profile_uid.clone(),
        fingerprint: metadata.fingerprint.clone(),
        domains: metadata.domains.clone(),
        bindings: settings.bindings,
        enabled: settings.enabled,
        effective_enabled,
        applied_domains,
        dormant_domains,
        restore_status,
        restore_reason,
        use_hosts: metadata
            .base
            .get("dns")
            .and_then(|dns| dns.get("use-hosts"))
            .and_then(serde_yaml_ng::Value::as_bool),
        unknown_sources: metadata.unknown_sources.clone(),
        warnings: metadata.warnings.clone(),
    })
}

pub async fn set_cloudflare_dns_bindings(
    profile_uid: String,
    fingerprint: String,
    bindings: Vec<CloudflareDnsBinding>,
    _enabled: bool,
    confirmation: Option<String>,
) -> Result<CloudflareDnsOutcome> {
    let enabled = !bindings.is_empty();
    let _profile_write = PROFILE_WRITE_LOCK.lock().await;
    let _write = Config::lock_config_write().await;
    let profiles = Config::profiles().await.data_arc();
    let runtime = Config::runtime().await.data_arc();
    let metadata = runtime
        .cloudflare_dns
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No committed profile inventory is available"))?;
    if let Some(outcome) = stale(
        &profile_uid,
        &fingerprint,
        profiles.current.as_deref().unwrap_or_default(),
        &fingerprint,
    ) {
        return Ok(outcome);
    }
    if let Some(outcome) = stale(&profile_uid, &fingerprint, &metadata.profile_uid, &fingerprint) {
        return Ok(outcome);
    }
    let refreshed = cloudflare_dns::collect(metadata.base.clone(), &profiles).await?;
    if let Some(outcome) = stale(
        &profile_uid,
        &fingerprint,
        &refreshed.profile_uid,
        &refreshed.fingerprint,
    ) {
        return Ok(outcome);
    }
    let metadata = &refreshed;
    cloudflare_dns::validate_targets(&bindings)?;
    let verge = Config::verge().await;
    let previous = verge
        .data_arc()
        .profile_cloudflare_dns_settings
        .as_ref()
        .and_then(|settings| settings.get(&profile_uid))
        .cloned()
        .unwrap_or_default();
    let activate = bindings_ready_for_activation(metadata, &previous.bindings, &bindings)?;
    for binding in &bindings {
        let unchanged = previous.bindings.contains(binding);
        if unchanged {
            continue;
        }
        match &binding.target {
            CloudflareBindingTarget::PreferredDomain { domain } => ensure!(
                PREFERRED_DOMAINS.contains(&domain.as_str()),
                "Only built-in preferred domains can be dynamic targets"
            ),
            CloudflareBindingTarget::FixedIp { ip, run_id } => ensure!(
                cloudflare_speed_test::trusted_ip(ip, run_id, &profile_uid).await?,
                "The fixed IP must be a measured Cloudflare address from this profile and network"
            ),
        }
    }
    let mut settings = ProfileCloudflareDnsSettings {
        enabled,
        bindings,
        confirmation: None,
    };
    if activate {
        let capability = cloudflare_speed_test::capability().await;
        ensure!(
            capability.compatible() && capability.dns_aliases && capability.fixed_address_family,
            "The running core does not support safe Cloudflare DNS bindings"
        );
        let eligible_bindings: Vec<_> = settings
            .bindings
            .iter()
            .filter(|binding| cloudflare_dns::validate_eligibility(metadata, std::slice::from_ref(*binding)).is_ok())
            .cloned()
            .collect();
        cloudflare_dns::overlay(metadata.base.clone(), &eligible_bindings)?;
        let token = cloudflare_dns::confirmation(metadata, &settings)?;
        if confirmation.as_ref() != Some(&token) {
            return Ok(CloudflareDnsOutcome::ConfirmationRequired {
                confirmation: token,
                conflicts: cloudflare_dns::conflicts(&metadata.base, &settings.bindings),
                enable_use_hosts: metadata
                    .base
                    .get("dns")
                    .and_then(|dns| dns.get("use-hosts"))
                    .and_then(serde_yaml_ng::Value::as_bool)
                    != Some(true),
                affected_domains: settings.bindings.iter().map(|binding| binding.domain.clone()).collect(),
                warnings: metadata.warnings.clone(),
            });
        }
        settings.confirmation = Some(token);
    }
    let transaction = DraftTransaction::begin(vec![&verge])?;
    verge.edit_draft(|draft| {
        draft
            .profile_cloudflare_dns_settings
            .get_or_insert_default()
            .insert(profile_uid.clone(), settings.clone());
    });
    let manager = CoreManager::global();
    let applied = manager
        .update_config_checked_with_runtime_guard(|generated| {
            validate_manual_generation(generated, &profile_uid, &fingerprint, &settings, activate)
        })
        .await;
    // The manager may retain an accepted runtime even if a later lifecycle effect failed.
    let applied_runtime = Config::runtime().await.data_arc();
    let retained = !std::sync::Arc::ptr_eq(&runtime, &applied_runtime);
    if let Err(error) = applied {
        if retained {
            transaction.commit();
            announce(Refresh::Clash);
            announce(Refresh::Verge);
            return Ok(CloudflareDnsOutcome::Applied {
                saved: false,
                error: Some(format!(
                    "The core accepted the runtime, but a later step failed: {error:#}"
                )),
            });
        }
        if error.is::<ManualBindingStale>() {
            return Ok(CloudflareDnsOutcome::Stale {
                reason: error.to_string(),
            });
        }
        return Err(error);
    }
    if activate
        && applied_runtime.cloudflare_dns.as_ref().is_none_or(|metadata| {
            !metadata.active
                || settings.confirmation.as_deref() != cloudflare_dns::confirmation(metadata, &settings).ok().as_deref()
        })
    {
        transaction.rollback();
        announce(Refresh::Clash);
        return Ok(CloudflareDnsOutcome::Stale {
            reason: "Profile sources changed while the core was validating; bindings were not activated".into(),
        });
    }
    transaction.commit();
    let saved = verge.data_arc().save_file().await;
    announce(Refresh::Verge);
    announce(Refresh::Clash);
    match saved {
        Ok(()) => Ok(CloudflareDnsOutcome::Applied {
            saved: true,
            error: None,
        }),
        Err(error) => Ok(CloudflareDnsOutcome::Applied {
            saved: false,
            error: Some(error.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_dormant_records_do_not_authorize_new_or_changed_non_node_bindings() -> Result<()> {
        let mut metadata = cloudflare_dns::CloudflareDnsRuntime {
            profile_uid: "profile".into(),
            fingerprint: "source".into(),
            base: Default::default(),
            domains: Vec::new(),
            unknown_sources: Vec::new(),
            warnings: Vec::new(),
            active: false,
            applied_domains: Vec::new(),
            dormant_domains: Vec::new(),
            restore_status: CloudflareDnsRestoreStatus::Dormant,
            restore_reason: None,
        };
        let binding = |domain: &str| CloudflareDnsBinding {
            domain: domain.into(),
            target: CloudflareBindingTarget::PreferredDomain {
                domain: PREFERRED_DOMAINS[0].into(),
            },
        };
        let previous = vec![binding("first.example"), binding("second.example")];
        assert!(!bindings_ready_for_activation(&metadata, &previous, &previous[..1])?);
        assert!(!bindings_ready_for_activation(&metadata, &previous, &[])?);
        assert!(!bindings_ready_for_activation(&metadata, &previous, &previous)?);
        assert!(bindings_ready_for_activation(&metadata, &previous, &[binding("injected.example")]).is_err());
        let mut modified = previous[0].clone();
        modified.target = CloudflareBindingTarget::PreferredDomain {
            domain: PREFERRED_DOMAINS[1].into(),
        };
        assert!(bindings_ready_for_activation(&metadata, &previous, &[modified]).is_err());
        metadata.domains.push(CloudflareDomain {
            domain: "good.example".into(),
            occurrences: vec![CloudflareOccurrence {
                id: "node".into(),
                display: "good.example".into(),
                field_path: "proxies[0].server".into(),
                role: CloudflareOccurrenceRole::ProxyServer,
                bindable: true,
                reason: None,
            }],
            bindable: true,
            reason: None,
        });
        let mut combined = previous.clone();
        combined.push(binding("good.example"));
        assert!(bindings_ready_for_activation(&metadata, &previous, &combined)?);
        Ok(())
    }

    #[test]
    fn manual_confirmation_tokens_remain_bound_to_sources_and_exact_targets() -> Result<()> {
        let mut metadata = cloudflare_dns::CloudflareDnsRuntime {
            profile_uid: "profile".into(),
            fingerprint: "old-source".into(),
            base: Default::default(),
            domains: Vec::new(),
            unknown_sources: Vec::new(),
            warnings: Vec::new(),
            active: false,
            applied_domains: Vec::new(),
            dormant_domains: Vec::new(),
            restore_status: CloudflareDnsRestoreStatus::Dormant,
            restore_reason: None,
        };
        let mut settings = ProfileCloudflareDnsSettings {
            enabled: true,
            bindings: vec![CloudflareDnsBinding {
                domain: "node.example".into(),
                target: CloudflareBindingTarget::FixedIp {
                    ip: "104.16.0.1".into(),
                    run_id: "old-run".into(),
                },
            }],
            confirmation: None,
        };
        let token = cloudflare_dns::confirmation(&metadata, &settings)?;
        metadata.fingerprint = "updated-source".into();
        assert_ne!(token, cloudflare_dns::confirmation(&metadata, &settings)?);
        metadata.fingerprint = "old-source".into();
        settings.bindings[0].target = CloudflareBindingTarget::FixedIp {
            ip: "104.16.0.2".into(),
            run_id: "new-run".into(),
        };
        assert_ne!(token, cloudflare_dns::confirmation(&metadata, &settings)?);
        Ok(())
    }

    #[test]
    fn generated_manual_update_rejects_source_profile_or_token_changes_before_apply() -> Result<()> {
        let mut metadata = cloudflare_dns::CloudflareDnsRuntime {
            profile_uid: "profile".into(),
            fingerprint: "reviewed-source".into(),
            base: Default::default(),
            domains: Vec::new(),
            unknown_sources: Vec::new(),
            warnings: Vec::new(),
            active: true,
            applied_domains: vec!["node.example".into()],
            dormant_domains: Vec::new(),
            restore_status: CloudflareDnsRestoreStatus::Active,
            restore_reason: None,
        };
        let mut settings = ProfileCloudflareDnsSettings {
            enabled: true,
            bindings: vec![CloudflareDnsBinding {
                domain: "node.example".into(),
                target: CloudflareBindingTarget::PreferredDomain {
                    domain: PREFERRED_DOMAINS[0].into(),
                },
            }],
            confirmation: None,
        };
        settings.confirmation = Some(cloudflare_dns::confirmation(&metadata, &settings)?);
        let runtime = |metadata| crate::config::runtime::IRuntime {
            cloudflare_dns: Some(metadata),
            ..Default::default()
        };
        assert!(
            validate_manual_generation(
                &runtime(metadata.clone()),
                "profile",
                "reviewed-source",
                &settings,
                true
            )
            .is_ok()
        );
        metadata.fingerprint = "provider-changed-during-generation".into();
        let generated = runtime(metadata.clone());
        let mut core_applied = false;
        let applied = validate_manual_generation(&generated, "profile", "reviewed-source", &settings, true).map(|()| {
            core_applied = true;
        });
        assert!(applied.unwrap_err().is::<ManualBindingStale>());
        assert!(!core_applied);
        metadata.fingerprint = "reviewed-source".into();
        metadata.profile_uid = "different-profile".into();
        assert!(
            validate_manual_generation(
                &runtime(metadata.clone()),
                "profile",
                "reviewed-source",
                &settings,
                true
            )
            .unwrap_err()
            .is::<ManualBindingStale>()
        );
        metadata.profile_uid = "profile".into();
        settings.bindings[0].target = CloudflareBindingTarget::PreferredDomain {
            domain: PREFERRED_DOMAINS[1].into(),
        };
        assert!(
            validate_manual_generation(
                &runtime(metadata.clone()),
                "profile",
                "reviewed-source",
                &settings,
                true
            )
            .unwrap_err()
            .is::<ManualBindingStale>()
        );
        settings.enabled = false;
        settings.bindings.clear();
        metadata.fingerprint = "changed-before-removal".into();
        assert!(
            validate_manual_generation(&runtime(metadata), "profile", "reviewed-source", &settings, false)
                .unwrap_err()
                .is::<ManualBindingStale>()
        );
        Ok(())
    }

    #[test]
    fn stale_profile_or_fingerprint_is_a_typed_non_apply_outcome() {
        assert!(matches!(
            stale("first", "a", "second", "a"),
            Some(CloudflareDnsOutcome::Stale { .. })
        ));
        assert!(matches!(
            stale("first", "a", "first", "b"),
            Some(CloudflareDnsOutcome::Stale { .. })
        ));
        assert!(stale("first", "a", "first", "a").is_none());
    }
}
