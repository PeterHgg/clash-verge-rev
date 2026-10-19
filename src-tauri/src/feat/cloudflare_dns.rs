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
use std::collections::BTreeSet;

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

fn bindings_ready_for_activation(
    metadata: &cloudflare_dns::CloudflareDnsRuntime,
    previous: &[CloudflareDnsBinding],
    bindings: &[CloudflareDnsBinding],
) -> Result<bool> {
    let eligibility = cloudflare_dns::validate_eligibility(metadata, bindings);
    let removal_only = bindings.len() < previous.len() && bindings.iter().all(|binding| previous.contains(binding));
    if eligibility.is_err() && removal_only {
        return Ok(false);
    }
    eligibility?;
    Ok(!bindings.is_empty())
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
    ensure!(bindings.len() <= 128, "At most 128 hostname bindings are supported");
    let verge = Config::verge().await;
    let previous = verge
        .data_arc()
        .profile_cloudflare_dns_settings
        .as_ref()
        .and_then(|settings| settings.get(&profile_uid))
        .cloned()
        .unwrap_or_default();
    let activate = bindings_ready_for_activation(metadata, &previous.bindings, &bindings)?;
    let mut domains = BTreeSet::new();
    for binding in &bindings {
        ensure!(domains.insert(&binding.domain), "Duplicate binding hostname");
        ensure!(
            cloudflare_dns::hostname(&binding.domain).as_deref() == Some(binding.domain.as_str()),
            "Invalid binding hostname"
        );
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
        cloudflare_dns::overlay(metadata.base.clone(), &settings.bindings)?;
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
    let applied = manager.update_config_checked().await;
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
            error: (enabled && !activate).then(|| {
                "Remaining saved bindings are inactive; remove hostnames that are no longer locally resolved proxy servers".into()
            }),
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
    fn invalid_legacy_remainders_can_only_be_saved_inactive_by_exact_removal() -> Result<()> {
        let metadata = cloudflare_dns::CloudflareDnsRuntime {
            profile_uid: "profile".into(),
            fingerprint: "source".into(),
            base: Default::default(),
            domains: Vec::new(),
            unknown_sources: Vec::new(),
            warnings: Vec::new(),
            active: false,
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
        assert!(bindings_ready_for_activation(&metadata, &previous, &previous).is_err());
        assert!(bindings_ready_for_activation(&metadata, &previous, &[binding("injected.example")]).is_err());
        let mut modified = previous[0].clone();
        modified.target = CloudflareBindingTarget::PreferredDomain {
            domain: PREFERRED_DOMAINS[1].into(),
        };
        assert!(bindings_ready_for_activation(&metadata, &previous, &[modified]).is_err());
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
