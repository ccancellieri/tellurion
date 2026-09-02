//! One-shot bootstrap and explicit migration for dynamic control stores.
//!
//! The YAML document is a boot envelope, not a continuously authoritative
//! configuration source. Once a durable store has a revision, its snapshot
//! always wins; a changed seed is reported as drift and is never applied by
//! startup.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    validate_control_bootstrap_seed, AppConfig, BootstrapOutcome, ControlBootstrapMode,
    ControlScope, ControlSnapshot, ControlStore, Error, PathPolicy, PrincipalIdentity, Result,
    RoleBinding, VersionedControlSnapshot,
};

const MIN_POLL_INTERVAL_MS: u64 = 250;
const MAX_POLL_INTERVAL_MS: u64 = 60_000;

fn default_poll_interval_ms() -> u64 {
    1_000
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlStoreLocator {
    /// The YAML document is itself the configuration, re-read on change —
    /// every deployment's behavior before durable control stores existed,
    /// and therefore the [`Default`]: a config written before this module
    /// existed describes exactly this arrangement, so absence of a
    /// `control_store` block means it rather than meaning "unconfigured".
    #[default]
    LegacyFile,
    Sqlite {
        path: PathBuf,
        #[serde(default = "default_poll_interval_ms")]
        poll_interval_ms: u64,
    },
    Postgres {
        url_env: String,
        #[serde(default = "default_poll_interval_ms")]
        poll_interval_ms: u64,
        #[serde(default)]
        pooled_proxy: bool,
    },
}

impl ControlStoreLocator {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::LegacyFile => Ok(()),
            Self::Sqlite {
                path,
                poll_interval_ms,
            } => {
                if path.as_os_str().is_empty() {
                    return Err(Error::Config(
                        "control_store.path must not be empty".to_string(),
                    ));
                }
                validate_poll_interval(*poll_interval_ms)
            }
            Self::Postgres {
                url_env,
                poll_interval_ms,
                ..
            } => {
                if url_env.trim().is_empty() {
                    return Err(Error::Config(
                        "control_store.url_env must not be empty".to_string(),
                    ));
                }
                validate_poll_interval(*poll_interval_ms)
            }
        }
    }

    pub fn poll_interval_ms(&self) -> Option<u64> {
        match self {
            Self::LegacyFile => None,
            Self::Sqlite {
                poll_interval_ms, ..
            }
            | Self::Postgres {
                poll_interval_ms, ..
            } => Some(*poll_interval_ms),
        }
    }
}

fn validate_poll_interval(value: u64) -> Result<()> {
    if !(MIN_POLL_INTERVAL_MS..=MAX_POLL_INTERVAL_MS).contains(&value) {
        return Err(Error::Config(format!(
            "control_store.poll_interval_ms ({value}) must be within [{MIN_POLL_INTERVAL_MS}, {MAX_POLL_INTERVAL_MS}]"
        )));
    }
    Ok(())
}

/// Static process settings plus the optional first-run seed. Configuration
/// fields are flattened so an existing Tellurion YAML needs only the new
/// `control_store` block; the locator itself is not persisted in snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootEnvelope {
    /// Which durable control store backs this process, defaulting to
    /// [`ControlStoreLocator::LegacyFile`] when the block is absent.
    ///
    /// The default is what makes the sentence above true: every config
    /// written before this module existed has no `control_store` block, and
    /// a required field would stop all of them from booting rather than
    /// leaving them on the legacy path they already describe. Opting in to
    /// a durable store stays an explicit, declared act; opting out was
    /// never something an operator had to say.
    #[serde(default)]
    pub control_store: ControlStoreLocator,
    #[serde(default)]
    pub allow_empty_platform: bool,
    /// Opaque external identities granted platform `sysadmin` only during
    /// the first durable-store transaction. These are identifiers, never
    /// credentials or secrets.
    #[serde(default)]
    pub initial_sysadmins: Vec<PrincipalIdentity>,
    /// Additional role bindings seeded only by the first durable-store
    /// transaction. The persisted snapshot is authoritative afterwards.
    #[serde(default)]
    pub role_bindings: Vec<RoleBinding>,
    /// Path policies seeded with the initial durable snapshot.
    #[serde(default)]
    pub path_policies: Vec<PathPolicy>,
    #[serde(flatten)]
    pub seed: AppConfig,
}

impl BootEnvelope {
    pub fn validate_locator(&self) -> Result<()> {
        self.control_store.validate()?;
        if !matches!(self.control_store, ControlStoreLocator::LegacyFile) {
            crate::control_model::validate_durable_bearer_token_sources(&self.seed)?;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_locator()?;
        self.seed.validate()?;
        if matches!(self.control_store, ControlStoreLocator::LegacyFile)
            && !(self.initial_sysadmins.is_empty()
                && self.role_bindings.is_empty()
                && self.path_policies.is_empty())
        {
            return Err(Error::Config(
                "initial_sysadmins, role_bindings, and path_policies require a durable control store; set control_store.backend to sqlite or postgres"
                    .to_string(),
            ));
        }
        let mut principals = std::collections::HashSet::new();
        for principal in &self.initial_sysadmins {
            principal.validate("initial sysadmin")?;
            if !principals.insert(principal) {
                return Err(Error::Config(
                    "initial_sysadmins must not contain duplicate principals".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Validates the one-shot seed requirements before an empty durable
    /// store can be initialized. Restarts validate the envelope but treat
    /// changed initial administrators as drift only.
    pub fn validate_initial_seed(&self) -> Result<()> {
        self.validate()?;
        if !matches!(self.control_store, ControlStoreLocator::LegacyFile) {
            let seed = self.seed_snapshot().ok_or_else(|| {
                Error::Config(
                    "first durable control-store initialization requires initial_sysadmins; set allow_empty_platform: true only for an intentionally unadministrable seed"
                        .to_string(),
                )
            })?;
            validate_control_bootstrap_seed(&seed, self.bootstrap_mode())?;
        }
        Ok(())
    }

    pub fn bootstrap_mode(&self) -> ControlBootstrapMode {
        if self.allow_empty_platform {
            ControlBootstrapMode::AllowEmptyPlatform
        } else {
            ControlBootstrapMode::RequireInitialSysadmin
        }
    }

    /// A default-only config is treated as no seed unless the operator has
    /// explicitly enabled an empty platform.
    pub fn seed_snapshot(&self) -> Option<ControlSnapshot> {
        if self.seed == AppConfig::default()
            && self.initial_sysadmins.is_empty()
            && self.role_bindings.is_empty()
            && self.path_policies.is_empty()
            && !self.allow_empty_platform
        {
            return None;
        }
        Some(ControlSnapshot {
            config: self.seed.clone(),
            role_bindings: self
                .role_bindings
                .iter()
                .cloned()
                .chain(
                    self.initial_sysadmins
                        .iter()
                        .cloned()
                        .map(|principal| RoleBinding {
                            principal,
                            role: "sysadmin".to_string(),
                            scope: ControlScope::Platform,
                        }),
                )
                .collect(),
            path_policies: self.path_policies.clone(),
            tombstoned_resources: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedStatus {
    Bootstrapped,
    MatchesAuthoritative,
    Drift { changed_sections: Vec<String> },
    AuthoritativeWithoutSeed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlStartup {
    pub authoritative: VersionedControlSnapshot,
    pub seed_status: SeedStatus,
}

/// Initializes an empty store exactly once, then always returns the durable
/// authoritative snapshot. Calling this concurrently is safe when the store
/// satisfies the `ControlStore` bootstrap contract.
pub async fn initialize_control_store(
    store: &dyn ControlStore,
    seed: Option<&ControlSnapshot>,
    actor: &PrincipalIdentity,
) -> Result<ControlStartup> {
    initialize_control_store_with_mode(
        store,
        seed,
        actor,
        ControlBootstrapMode::RequireInitialSysadmin,
    )
    .await
}

pub async fn initialize_control_store_with_mode(
    store: &dyn ControlStore,
    seed: Option<&ControlSnapshot>,
    actor: &PrincipalIdentity,
    mode: ControlBootstrapMode,
) -> Result<ControlStartup> {
    if let Some(seed) = seed {
        let outcome = store.bootstrap_if_empty(seed, actor, mode).await?;
        let authoritative = store.load_snapshot().await?;
        let seed_status = match outcome {
            BootstrapOutcome::Bootstrapped(_) => SeedStatus::Bootstrapped,
            BootstrapOutcome::AlreadyInitialized(_) if authoritative.snapshot == *seed => {
                SeedStatus::MatchesAuthoritative
            }
            BootstrapOutcome::AlreadyInitialized(_) => SeedStatus::Drift {
                changed_sections: diff_control_snapshots(seed, &authoritative.snapshot),
            },
        };
        return Ok(ControlStartup {
            authoritative,
            seed_status,
        });
    }

    if store.current_revision().await?.is_none() {
        return Err(Error::ControlUninitialized);
    }
    Ok(ControlStartup {
        authoritative: store.load_snapshot().await?,
        seed_status: SeedStatus::AuthoritativeWithoutSeed,
    })
}

/// Reports top-level seed differences without exposing values or secrets.
pub fn diff_control_snapshots(seed: &ControlSnapshot, current: &ControlSnapshot) -> Vec<String> {
    let seed = serde_json::to_value(seed).unwrap_or(serde_json::Value::Null);
    let current = serde_json::to_value(current).unwrap_or(serde_json::Value::Null);
    let mut changed = Vec::new();
    let keys = [
        "config",
        "role_bindings",
        "path_policies",
        "tombstoned_resources",
    ];
    for key in keys {
        if seed.get(key) != current.get(key) {
            changed.push(key.to_string());
        }
    }
    changed
}

pub fn export_control_snapshot(snapshot: &ControlSnapshot) -> Result<String> {
    serde_yaml::to_string(snapshot)
        .map_err(|error| Error::Config(format!("serializing control snapshot: {error}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlMigrationPlan {
    pub source_revision: u64,
    pub applied: bool,
}

/// Copies one authoritative snapshot to an empty destination. Replacing an
/// initialized destination is intentionally unsupported: that requires an
/// explicit versioned changeset, not a boot-time shortcut.
pub async fn migrate_control_store(
    source: &dyn ControlStore,
    destination: &dyn ControlStore,
    actor: &PrincipalIdentity,
    apply: bool,
) -> Result<ControlMigrationPlan> {
    migrate_control_store_with_mode(
        source,
        destination,
        actor,
        apply,
        ControlBootstrapMode::RequireInitialSysadmin,
    )
    .await
}

pub async fn migrate_control_store_with_mode(
    source: &dyn ControlStore,
    destination: &dyn ControlStore,
    actor: &PrincipalIdentity,
    apply: bool,
    mode: ControlBootstrapMode,
) -> Result<ControlMigrationPlan> {
    let source = source.load_snapshot().await?;
    if destination.current_revision().await?.is_some() {
        return Err(Error::Config(
            "control-store migration destination is already initialized".to_string(),
        ));
    }
    if apply {
        destination
            .bootstrap_if_empty(&source.snapshot, actor, mode)
            .await?;
    }
    Ok(ControlMigrationPlan {
        source_revision: source.revision,
        applied: apply,
    })
}

#[cfg(any(test, feature = "test-support"))]
pub async fn assert_control_bootstrap_contract(
    invalid_store: std::sync::Arc<dyn ControlStore>,
    racing_store: std::sync::Arc<dyn ControlStore>,
    restart_store: std::sync::Arc<dyn ControlStore>,
) {
    let actor = PrincipalIdentity {
        issuer: "urn:tellurion:bootstrap-contract".to_string(),
        subject: "operator".to_string(),
    };
    let make_seed = |port| {
        let mut config = AppConfig::default();
        config.server.port = port;
        ControlSnapshot {
            config,
            role_bindings: Vec::new(),
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        }
    };

    let mut invalid = make_seed(8_000);
    invalid.config.server.max_concurrency = Some(0);
    assert!(initialize_control_store_with_mode(
        invalid_store.as_ref(),
        Some(&invalid),
        &actor,
        ControlBootstrapMode::AllowEmptyPlatform,
    )
    .await
    .is_err());
    assert_eq!(invalid_store.current_revision().await.unwrap(), None);
    assert!(invalid_store
        .bootstrap_if_empty(
            &make_seed(8_001),
            &actor,
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .is_err());
    assert_eq!(invalid_store.current_revision().await.unwrap(), None);
    let invalid_actor = PrincipalIdentity {
        issuer: String::new(),
        subject: String::new(),
    };
    assert!(invalid_store
        .bootstrap_if_empty(
            &make_seed(8_002),
            &invalid_actor,
            ControlBootstrapMode::AllowEmptyPlatform,
        )
        .await
        .is_err());
    assert_eq!(invalid_store.current_revision().await.unwrap(), None);

    let first_store = std::sync::Arc::clone(&racing_store);
    let second_store = std::sync::Arc::clone(&racing_store);
    let first_actor = actor.clone();
    let second_actor = actor.clone();
    let first = tokio::spawn(async move {
        initialize_control_store_with_mode(
            first_store.as_ref(),
            Some(&make_contract_seed(8_100)),
            &first_actor,
            ControlBootstrapMode::AllowEmptyPlatform,
        )
        .await
    });
    let second = tokio::spawn(async move {
        initialize_control_store_with_mode(
            second_store.as_ref(),
            Some(&make_contract_seed(8_200)),
            &second_actor,
            ControlBootstrapMode::AllowEmptyPlatform,
        )
        .await
    });
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert_eq!(first.authoritative, second.authoritative);
    assert_eq!(first.authoritative.revision, 1);

    let initial = make_seed(8_300);
    initialize_control_store_with_mode(
        restart_store.as_ref(),
        Some(&initial),
        &actor,
        ControlBootstrapMode::AllowEmptyPlatform,
    )
    .await
    .unwrap();
    let changed_seed = make_seed(8_301);
    let restarted = initialize_control_store_with_mode(
        restart_store.as_ref(),
        Some(&changed_seed),
        &actor,
        ControlBootstrapMode::AllowEmptyPlatform,
    )
    .await
    .unwrap();
    assert!(matches!(restarted.seed_status, SeedStatus::Drift { .. }));
    assert_eq!(restarted.authoritative.snapshot, initial);

    let mut invalid_changed_seed = changed_seed;
    invalid_changed_seed.config.server.max_concurrency = Some(0);
    let invalid_changed_actor = PrincipalIdentity {
        issuer: String::new(),
        subject: String::new(),
    };
    let invalid_restarted = initialize_control_store_with_mode(
        restart_store.as_ref(),
        Some(&invalid_changed_seed),
        &invalid_changed_actor,
        ControlBootstrapMode::AllowEmptyPlatform,
    )
    .await
    .unwrap();
    assert!(matches!(
        invalid_restarted.seed_status,
        SeedStatus::Drift { .. }
    ));
    assert_eq!(invalid_restarted.authoritative.snapshot, initial);
}

#[cfg(any(test, feature = "test-support"))]
fn make_contract_seed(port: u16) -> ControlSnapshot {
    let mut config = AppConfig::default();
    config.server.port = port;
    ControlSnapshot {
        config,
        role_bindings: Vec::new(),
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        ControlChangeSet, ControlOperation, InMemoryControlStore, VersionedControlOperation,
    };

    fn actor() -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "https://identity.example".to_string(),
            subject: "operator".to_string(),
        }
    }

    fn seed(port: u16) -> ControlSnapshot {
        let mut config: AppConfig = serde_yaml::from_str(
            "auth:\n  trusted_issuers:\n    - { issuer: https://identity.example, audience: tellurion-test, claims: { tenants: tenants } }",
        )
        .unwrap();
        config.server.port = port;
        ControlSnapshot {
            config,
            role_bindings: vec![crate::RoleBinding {
                principal: actor(),
                role: "sysadmin".to_string(),
                scope: crate::ControlScope::Platform,
            }],
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        }
    }

    /// A configuration written before this module existed carries no
    /// `control_store` block at all, and must still boot on the legacy
    /// path — the arrangement it already describes. Pinned with a document
    /// shaped like a real deployment's rather than an empty one, so the
    /// flattened seed is exercised alongside the defaulted locator.
    #[test]
    fn a_config_without_a_control_store_block_boots_on_the_legacy_path() {
        let envelope: BootEnvelope = serde_yaml::from_str(
            "\
server: { port: 8081 }
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]",
        )
        .expect("a config with no control_store block must parse");
        assert_eq!(envelope.control_store, ControlStoreLocator::LegacyFile);
        assert_eq!(envelope.seed.server.port, 8081);
        envelope.validate().expect("and must validate");
    }

    #[test]
    fn a_legacy_file_envelope_still_accepts_a_pre_credential_seam_inline_token() {
        let envelope: BootEnvelope = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token: legacy-inline-token, tenants: [tenant-a], platform_admin: true }
"#,
        )
        .expect("a legacy configuration with an inline token must parse");

        assert_eq!(envelope.control_store, ControlStoreLocator::LegacyFile);
        envelope
            .validate()
            .expect("legacy file mode must retain inline-token compatibility");
    }

    #[test]
    fn durable_store_locators_reject_inline_bearer_token_secrets() {
        const SECRET: &str = "durable-bootstrap-inline-secret";
        for locator in [
            "{ backend: sqlite, path: control.db }",
            "{ backend: postgres, url_env: CONTROL_DATABASE_URL }",
        ] {
            let envelope: BootEnvelope = serde_yaml::from_str(&format!(
                r#"
control_store: {locator}
tenants: [{{ id: tenant-a }}]
auth:
  bearer_tokens:
    - {{ token: {SECRET}, tenants: [tenant-a] }}
"#,
            ))
            .expect("a durable envelope with an inline token must parse");

            let error = envelope
                .validate_locator()
                .expect_err("durable startup must reject inline bearer-token values");
            assert!(error.to_string().contains("token_env"));
            assert!(!error.to_string().contains(SECRET));
        }
    }

    #[test]
    fn parses_all_store_locators_and_rejects_inline_credentials() {
        let legacy: BootEnvelope =
            serde_yaml::from_str("control_store: { backend: legacy_file }\nserver: { port: 8081 }")
                .unwrap();
        assert_eq!(legacy.control_store, ControlStoreLocator::LegacyFile);

        let sqlite: BootEnvelope = serde_yaml::from_str(
            "control_store: { backend: sqlite, path: control.db, poll_interval_ms: 250 }\nallow_empty_platform: true",
        )
        .unwrap();
        sqlite.validate().unwrap();

        let postgres: BootEnvelope = serde_yaml::from_str(
            "control_store: { backend: postgres, url_env: CONTROL_DATABASE_URL, pooled_proxy: true }\nallow_empty_platform: true",
        )
        .unwrap();
        postgres.validate().unwrap();

        // An absent `control_store` block is NOT an error: it is the legacy
        // path, which is what every config written before this module
        // existed already describes. Pinned positively by
        // `a_config_without_a_control_store_block_boots_on_the_legacy_path`
        // above; requiring the block here would stop all of them booting.
        assert!(serde_yaml::from_str::<BootEnvelope>(
            "control_store: { backend: postgres, url: postgres://secret@example/db }"
        )
        .is_err());
    }

    #[test]
    fn rejects_poll_intervals_outside_the_operational_bounds() {
        for interval in [249, 60_001] {
            let envelope: BootEnvelope = serde_yaml::from_str(&format!(
                "control_store: {{ backend: sqlite, path: control.db, poll_interval_ms: {interval} }}"
            ))
            .unwrap();
            assert!(envelope.validate().is_err());
        }
    }

    #[test]
    fn empty_platform_requires_an_explicit_opt_in() {
        let without_opt_in: BootEnvelope =
            serde_yaml::from_str("control_store: { backend: sqlite, path: control.db }").unwrap();
        assert!(without_opt_in.seed_snapshot().is_none());

        let with_opt_in: BootEnvelope = serde_yaml::from_str(
            "control_store: { backend: sqlite, path: control.db }\nallow_empty_platform: true",
        )
        .unwrap();
        assert_eq!(
            with_opt_in.seed_snapshot().unwrap().config,
            AppConfig::default()
        );
    }

    #[test]
    fn durable_bootstrap_requires_and_seeds_an_initial_sysadmin() {
        let missing: BootEnvelope = serde_yaml::from_str(
            "control_store: { backend: sqlite, path: control.db }\nserver: { port: 8081 }",
        )
        .unwrap();
        missing.validate().unwrap();
        assert!(missing.validate_initial_seed().is_err());

        let configured: BootEnvelope = serde_yaml::from_str(
            r#"
control_store: { backend: sqlite, path: control.db }
initial_sysadmins:
  - issuer: https://identity.example
    subject: platform-operator
auth:
  trusted_issuers:
    - issuer: https://identity.example
      audience: tellurion-placeholder
      claims: { tenants: tenants }
server: { port: 8081 }
"#,
        )
        .unwrap();
        configured.validate_initial_seed().unwrap();
        assert_eq!(
            configured.bootstrap_mode(),
            ControlBootstrapMode::RequireInitialSysadmin
        );
        let seed = configured.seed_snapshot().unwrap();
        assert_eq!(
            seed.role_bindings,
            vec![crate::RoleBinding {
                principal: PrincipalIdentity {
                    issuer: "https://identity.example".to_string(),
                    subject: "platform-operator".to_string(),
                },
                role: "sysadmin".to_string(),
                scope: crate::ControlScope::Platform,
            }]
        );

        let intentional_lockout: BootEnvelope = serde_yaml::from_str(
            "control_store: { backend: sqlite, path: control.db }\nallow_empty_platform: true",
        )
        .unwrap();
        intentional_lockout.validate_initial_seed().unwrap();
        assert_eq!(
            intentional_lockout.bootstrap_mode(),
            ControlBootstrapMode::AllowEmptyPlatform
        );
    }

    #[test]
    fn duplicate_initial_sysadmins_are_rejected() {
        let duplicate: BootEnvelope = serde_yaml::from_str(
            r#"
control_store: { backend: sqlite, path: control.db }
initial_sysadmins:
  - { issuer: https://identity.example, subject: platform-operator }
  - { issuer: https://identity.example, subject: platform-operator }
"#,
        )
        .unwrap();

        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn durable_bootstrap_requires_a_sysadmin_reachable_through_configured_auth() {
        let no_auth: BootEnvelope = serde_yaml::from_str(
            r#"
control_store: { backend: sqlite, path: control.db }
initial_sysadmins:
  - { issuer: https://identity.example, subject: platform-operator }
server: { port: 8081 }
"#,
        )
        .unwrap();
        assert!(no_auth.validate_initial_seed().is_err());

        let oidc_exact: BootEnvelope = serde_yaml::from_str(
            r#"
control_store: { backend: sqlite, path: control.db }
initial_sysadmins:
  - { issuer: https://identity.example, subject: platform-operator }
auth:
  trusted_issuers:
    - issuer: https://identity.example
      audience: tellurion-placeholder
      claims: { tenants: tenants }
server: { port: 8081 }
"#,
        )
        .unwrap();
        oidc_exact.validate_initial_seed().unwrap();

        let oidc_mismatch: BootEnvelope = serde_yaml::from_str(
            r#"
control_store: { backend: sqlite, path: control.db }
initial_sysadmins:
  - { issuer: https://other-identity.example, subject: platform-operator }
auth:
  trusted_issuers:
    - issuer: https://identity.example
      audience: tellurion-placeholder
      claims: { tenants: tenants }
server: { port: 8081 }
"#,
        )
        .unwrap();
        assert!(oidc_mismatch.validate_initial_seed().is_err());

        let static_exact: BootEnvelope = serde_yaml::from_str(
            r#"
control_store: { backend: sqlite, path: control.db }
initial_sysadmins:
  - { issuer: "urn:tellurion:static", subject: recovery-operator }
auth:
  bearer_tokens:
    - token_env: TELLURION_RECOVERY_TOKEN
      tenants: [tenant-a]
      principal: recovery-operator
tenants: [ { id: tenant-a } ]
"#,
        )
        .unwrap();
        static_exact.validate_initial_seed().unwrap();

        let static_mismatch: BootEnvelope = serde_yaml::from_str(
            r#"
control_store: { backend: sqlite, path: control.db }
initial_sysadmins:
  - { issuer: "urn:tellurion:static", subject: other-operator }
auth:
  bearer_tokens:
    - token_env: TELLURION_RECOVERY_TOKEN
      tenants: [tenant-a]
      principal: recovery-operator
tenants: [ { id: tenant-a } ]
"#,
        )
        .unwrap();
        assert!(static_mismatch.validate_initial_seed().is_err());
    }

    #[tokio::test]
    async fn restart_reports_drift_without_reapplying_the_seed() {
        let store = InMemoryControlStore::new();
        let first = initialize_control_store(&store, Some(&seed(8000)), &actor())
            .await
            .unwrap();
        assert_eq!(first.seed_status, SeedStatus::Bootstrapped);

        let changes = ControlChangeSet {
            idempotency_key: None,
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(seed(9000).config),
            }],
        };
        let versioned = store.load_snapshot().await.unwrap();
        let request = crate::MutationControlRequestContext {
            method: "PUT".to_string(),
            canonical_path: "/_control/v1/platform".to_string(),
            route_template: "/_control/v1/platform".to_string(),
            scope: crate::ControlScope::Platform,
        };
        let authorization = crate::control_admin_policy::authorize_control_mutation_from_context(
            &crate::AuthenticatedSubject {
                principal: actor(),
                claims: std::collections::HashMap::new(),
            },
            &request,
            crate::AuditRequestContext {
                method: request.method.clone(),
                canonical_path: request.canonical_path.clone(),
                correlation_id: "test".to_string(),
            },
            &versioned,
            &changes,
        )
        .unwrap();
        store.transact(&authorization, &changes).await.unwrap();

        let restarted = initialize_control_store(&store, Some(&seed(8001)), &actor())
            .await
            .unwrap();
        assert!(matches!(restarted.seed_status, SeedStatus::Drift { .. }));
        assert_eq!(restarted.authoritative.snapshot.config.server.port, 9000);
        assert_eq!(restarted.authoritative.revision, 2);
    }

    #[tokio::test]
    async fn racing_seeds_converge_on_one_authoritative_snapshot() {
        let store = Arc::new(InMemoryControlStore::new());
        let first_store = Arc::clone(&store);
        let second_store = Arc::clone(&store);
        let first = tokio::spawn(async move {
            initialize_control_store(first_store.as_ref(), Some(&seed(8100)), &actor()).await
        });
        let second = tokio::spawn(async move {
            initialize_control_store(second_store.as_ref(), Some(&seed(8200)), &actor()).await
        });
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first.authoritative, second.authoritative);
        assert_eq!(first.authoritative.revision, 1);
        assert_eq!(
            [first.seed_status, second.seed_status]
                .iter()
                .filter(|status| matches!(status, SeedStatus::Bootstrapped))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn no_seed_requires_an_initialized_store() {
        let store = InMemoryControlStore::new();
        assert!(matches!(
            initialize_control_store(&store, None, &actor(),).await,
            Err(Error::ControlUninitialized)
        ));
    }

    #[tokio::test]
    async fn invalid_seed_leaves_store_uninitialized() {
        let store = InMemoryControlStore::new();
        let mut invalid = seed(8000);
        invalid.config.server.max_concurrency = Some(0);
        assert!(initialize_control_store(&store, Some(&invalid), &actor(),)
            .await
            .is_err());
        assert_eq!(store.current_revision().await.unwrap(), None);
    }

    #[tokio::test]
    async fn migration_is_dry_run_or_apply_and_refuses_overwrite() {
        let source = InMemoryControlStore::new();
        let destination = InMemoryControlStore::new();
        initialize_control_store(&source, Some(&seed(8300)), &actor())
            .await
            .unwrap();
        assert_eq!(
            migrate_control_store(&source, &destination, &actor(), false)
                .await
                .unwrap(),
            ControlMigrationPlan {
                source_revision: 1,
                applied: false
            }
        );
        assert_eq!(destination.current_revision().await.unwrap(), None);
        migrate_control_store(&source, &destination, &actor(), true)
            .await
            .unwrap();
        assert_eq!(
            destination.load_snapshot().await.unwrap().snapshot,
            seed(8300)
        );
        assert!(migrate_control_store(&source, &destination, &actor(), true)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn empty_platform_migration_requires_the_explicit_allow_empty_mode() {
        let source = InMemoryControlStore::new();
        source
            .bootstrap_if_empty(
                &make_contract_seed(8_400),
                &actor(),
                ControlBootstrapMode::AllowEmptyPlatform,
            )
            .await
            .unwrap();

        let default_destination = InMemoryControlStore::new();
        assert!(
            migrate_control_store(&source, &default_destination, &actor(), true)
                .await
                .is_err()
        );
        assert_eq!(default_destination.current_revision().await.unwrap(), None);

        let explicit_destination = InMemoryControlStore::new();
        migrate_control_store_with_mode(
            &source,
            &explicit_destination,
            &actor(),
            true,
            ControlBootstrapMode::AllowEmptyPlatform,
        )
        .await
        .unwrap();
        assert_eq!(
            explicit_destination.load_snapshot().await.unwrap().snapshot,
            make_contract_seed(8_400)
        );
    }

    #[test]
    fn durable_seed_combines_explicit_bindings_policies_and_initial_administrators() {
        let envelope: BootEnvelope = serde_yaml::from_str(
            r#"
control_store:
  backend: sqlite
  path: /tmp/tellurion-control.db
allow_empty_platform: true
initial_sysadmins:
  - { issuer: https://identity.example, subject: operator }
role_bindings:
  - principal: { issuer: https://service.example, subject: publisher }
    role: service_account
    scope: { kind: platform }
path_policies:
  - id: publish-read
    role: service_account
    scope: { kind: platform }
    effect: allow
    methods: [GET]
    patterns: [/_control/v1/platform/**]
"#,
        )
        .unwrap();

        envelope.validate_initial_seed().unwrap();
        let snapshot = envelope.seed_snapshot().unwrap();
        assert_eq!(snapshot.role_bindings.len(), 2);
        assert_eq!(snapshot.path_policies.len(), 1);
        assert_eq!(
            snapshot.path_policies[0].role.as_deref(),
            Some("service_account")
        );
        crate::ControlPolicySet::compile(&snapshot.role_bindings, &snapshot.path_policies)
            .expect("checkpoint policy compiles the same persisted statement");
    }

    #[test]
    fn legacy_file_rejects_control_authority_that_it_cannot_persist() {
        let envelope: BootEnvelope = serde_yaml::from_str(
            r#"
role_bindings:
  - principal: { issuer: https://service.example, subject: publisher }
    role: service_account
    scope: { kind: platform }
"#,
        )
        .unwrap();

        let error = envelope.validate().expect_err("legacy authority must fail");
        assert!(error
            .to_string()
            .contains("require a durable control store"));
    }
}
