//! Builds and atomically publishes one validated runtime configuration and
//! its immutable authorization bindings.
//!
//! File reload and dynamic control-store convergence use this exact path, so
//! neither can expose a partially rebuilt router, resolver, authorizer, or
//! registry reader.

// `#162` removed this import along with the `Option<&Arc<dyn Factory>>`
// parameters it existed for; `#215` needs it back for a different reason —
// the compiled policy set below is shared into `ContextState` by `Arc`, the
// same way the router and authorizer beside it are.
use std::sync::Arc;

use tellurion_core::{
    build_authorizer_with_bindings, build_registry_reader, build_router_and_resolver,
    build_tenant_reader, AppConfig, AppContext, ConfigVersion, ControlPolicySet, ControlRevision,
    PathPolicy, Registry, RegistryValidationMode, RelationalRegistryFactories,
    RelationalTenantFactories, RoleBinding,
};

use crate::metrics;
use crate::readiness::Readiness;

#[derive(Debug, Clone, Copy)]
pub struct ActivationSummary {
    pub tenants: usize,
    pub catalogs: usize,
    pub collections: usize,
    pub storages: usize,
}

pub struct RuntimeCandidate {
    pub config: AppConfig,
    pub role_bindings: Vec<RoleBinding>,
    pub control_revision: Option<ControlRevision>,
    /// `#215`: the path-scoped administration statements this candidate
    /// serves. Empty for every candidate built from a configuration document
    /// alone, which is what `From<AppConfig>` below produces and what the
    /// legacy file backend always produces — an empty statement list is the
    /// pre-`#215` behaviour exactly, not an approximation of it.
    pub path_policies: Vec<PathPolicy>,
}

impl From<AppConfig> for RuntimeCandidate {
    fn from(config: AppConfig) -> Self {
        Self {
            config,
            role_bindings: Vec::new(),
            control_revision: None,
            path_policies: Vec::new(),
        }
    }
}

pub async fn activate_config(
    ctx: &AppContext,
    candidate: RuntimeCandidate,
    version: ConfigVersion,
    registry: &Registry,
    relational_registry_factories: &RelationalRegistryFactories,
    relational_tenant_factories: &RelationalTenantFactories,
    readiness: &Readiness,
) -> anyhow::Result<ActivationSummary> {
    let RuntimeCandidate {
        mut config,
        role_bindings,
        control_revision,
        path_policies,
    } = candidate;
    apply_process_overrides(&mut config);
    reject_restart_required_changes(&ctx.current().config, &config)?;
    let registry_reader = build_registry_reader(&config, relational_registry_factories).await?;
    let tenant_reader = build_tenant_reader(&config, relational_tenant_factories).await?;
    let (router, resolver, tenants) = build_router_and_resolver(
        &config,
        registry,
        registry_reader.as_ref(),
        tenant_reader.as_ref(),
    )
    .await?;
    if config.registry.validation == RegistryValidationMode::Eager {
        router.validate_catalog().await?;
    }
    // `#144`: resolving `auth.bearer_tokens` is part of building the
    // candidate, and it happens before the swap below like every other
    // fallible step here — a `token_env` that stopped being set is a named
    // failed activation with the previous configuration still serving, never
    // a live authorizer that silently lost a principal.
    let authorizer = build_authorizer_with_bindings(&config.auth, &role_bindings)?;
    // `#215`: patterns are compiled here, before the swap, like every other
    // fallible step above — an unparseable pattern is a named failed
    // activation with the previous policy still serving, never a live policy
    // set with a statement quietly missing from it.
    let control_policy = Arc::new(ControlPolicySet::compile(&role_bindings, &path_policies)?);
    // Named, not silent: a statement nobody can reach and a condition nobody
    // evaluates both still bring their paths under default-deny, so an
    // operator hears about them once per activation rather than deducing
    // them from a refusal.
    if !control_policy.is_empty() {
        tracing::info!(
            statements = control_policy.statement_count(),
            bindings = control_policy.binding_count(),
            "hierarchical path-scoped administration policy activated"
        );
        if !control_policy.unhonoured_conditions().is_empty() {
            tracing::warn!(
                policies = ?control_policy.unhonoured_conditions(),
                "path policies declare conditions of a kind this build does not implement; \
                 such a statement can deny but can never allow (#215)"
            );
        }
        if !control_policy.roleless_statements().is_empty() {
            tracing::warn!(
                policies = ?control_policy.roleless_statements(),
                "path policies name no role, so no principal can satisfy them; \
                 the paths they match remain default-deny (#215)"
            );
        }
    }
    let summary = ActivationSummary {
        tenants: tenants.len(),
        catalogs: resolver.catalog_count(),
        collections: router.collection_count(),
        storages: config.storages.len(),
    };
    readiness.reload_and_invalidate(|| {
        ctx.reload_with_registry_version_and_policy(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry_reader,
            version.clone(),
            control_policy,
            control_revision,
        );
    });
    metrics::set_config_version_gauge(&version);
    tracing::info!(
        tenants = summary.tenants,
        catalogs = summary.catalogs,
        collections = summary.collections,
        storages = summary.storages,
        %version,
        "runtime configuration activated atomically"
    );
    Ok(summary)
}

pub fn apply_process_overrides(config: &mut AppConfig) {
    let raw_port = std::env::var("PORT").ok();
    apply_port_override(config, raw_port.as_deref());
}

fn apply_port_override(config: &mut AppConfig, raw_port: Option<&str>) {
    let Some(raw_port) = raw_port else {
        return;
    };
    match raw_port.parse::<u16>() {
        Ok(port) => config.server.port = port,
        Err(_) => tracing::warn!(
            value = raw_port,
            "ignoring invalid PORT environment variable"
        ),
    }
}

fn reject_restart_required_changes(
    current: &AppConfig,
    candidate: &AppConfig,
) -> anyhow::Result<()> {
    let mut changed = Vec::new();
    if current.cache != candidate.cache {
        changed.push("cache");
    }
    if current.styles != candidate.styles {
        changed.push("styles");
    }
    if current.webhooks != candidate.webhooks {
        changed.push("webhooks");
    }
    let old = &current.server;
    let new = &candidate.server;
    if old.port != new.port
        || old.request_timeout_s != new.request_timeout_s
        || old.log_json != new.log_json
        || old.max_concurrency != new.max_concurrency
        || old.index_applier != new.index_applier
        || old.tile_invalidation != new.tile_invalidation
        || old.webhook_delivery != new.webhook_delivery
        || old.outbox_retention != new.outbox_retention
        || old.drain_timeout_s != new.drain_timeout_s
        || old.readiness_probe_interval_s != new.readiness_probe_interval_s
        || old.readiness_probe_timeout_s != new.readiness_probe_timeout_s
    {
        changed.push("server boot settings");
    }
    if changed.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "candidate changes restart-required configuration: {}",
            changed.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::StyleRef;

    #[test]
    fn restart_required_dependencies_are_rejected_before_activation() {
        let current = AppConfig::default();

        let mut cache = current.clone();
        cache.cache.memory_percent = 1.0;
        assert!(reject_restart_required_changes(&current, &cache).is_err());

        let mut styles = current.clone();
        styles.styles.push(StyleRef {
            id: "new".to_string(),
            path: "new.json".to_string(),
        });
        assert!(reject_restart_required_changes(&current, &styles).is_err());

        let mut server = current.clone();
        server.server.port += 1;
        assert!(reject_restart_required_changes(&current, &server).is_err());
    }

    #[test]
    fn atomically_swapped_configuration_is_allowed() {
        let current = AppConfig::default();
        let mut candidate = current.clone();
        candidate.settings.cache_ttl_s = Some(90);
        assert!(reject_restart_required_changes(&current, &candidate).is_ok());
    }

    #[test]
    fn legacy_runtime_candidate_has_no_durable_control_revision() {
        let candidate = RuntimeCandidate::from(AppConfig::default());

        assert_eq!(candidate.control_revision, None);
    }

    #[test]
    fn process_port_override_is_normalized_for_every_candidate() {
        let mut current = AppConfig::default();
        let mut candidate = current.clone();
        apply_port_override(&mut current, Some("9090"));
        apply_port_override(&mut candidate, Some("9090"));

        assert_eq!(current.server.port, 9090);
        assert!(reject_restart_required_changes(&current, &candidate).is_ok());
    }
}
