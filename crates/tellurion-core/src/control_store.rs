//! Asynchronous persistence contract for the dynamic control plane.

use async_trait::async_trait;

use crate::control_model::{
    AuditRequestContext, BootstrapOutcome, ControlChangeSet, ControlCommit, ControlEvent,
    ControlEventCursor, ControlRevision, ControlSnapshot, PrincipalIdentity,
    VersionedControlSnapshot,
};
use crate::error::{Error, Result};
use crate::AuthorizedControlMutation;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ControlAuditRecord {
    pub revision: ControlRevision,
    pub actor: PrincipalIdentity,
    pub request: AuditRequestContext,
    pub idempotency_key: Option<String>,
    pub changed_resources: Vec<String>,
    pub recorded_at_unix_ms: u64,
    pub applying_instance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlBootstrapMode {
    RequireInitialSysadmin,
    AllowEmptyPlatform,
}

pub fn validate_control_bootstrap_seed(
    seed: &ControlSnapshot,
    mode: ControlBootstrapMode,
) -> Result<()> {
    if mode == ControlBootstrapMode::AllowEmptyPlatform
        || seed.role_bindings.iter().any(|binding| {
            binding.role == "sysadmin"
                && binding.scope == crate::ControlScope::Platform
                && principal_is_reachable(&seed.config.auth, &binding.principal)
        })
    {
        return Ok(());
    }
    Err(Error::ControlValidation(
        "first control-store initialization requires a platform sysadmin reachable through configured authentication unless empty-platform mode is explicit"
            .to_string(),
    ))
}

fn principal_is_reachable(auth: &crate::config::AuthConfig, principal: &PrincipalIdentity) -> bool {
    auth.oidc
        .iter()
        .chain(auth.trusted_issuers.iter())
        .any(|issuer| issuer.issuer == principal.issuer)
        || (principal.issuer == "urn:tellurion:static"
            && auth
                .bearer_tokens
                .iter()
                .any(|token| token.principal.as_deref() == Some(principal.subject.as_str())))
}

#[async_trait]
pub trait ControlStore: Send + Sync {
    async fn bootstrap_if_empty(
        &self,
        seed: &ControlSnapshot,
        actor: &PrincipalIdentity,
        mode: ControlBootstrapMode,
    ) -> Result<BootstrapOutcome>;

    async fn current_revision(&self) -> Result<Option<ControlRevision>>;

    async fn load_snapshot(&self) -> Result<VersionedControlSnapshot>;

    async fn transact(
        &self,
        authorization: &AuthorizedControlMutation,
        changes: &ControlChangeSet,
    ) -> Result<ControlCommit>;

    async fn changes_since(
        &self,
        after: Option<ControlEventCursor>,
        limit: u32,
    ) -> Result<Vec<ControlEvent>>;

    async fn audit_since(
        &self,
        after: ControlRevision,
        limit: u32,
    ) -> Result<Vec<ControlAuditRecord>>;
}

#[cfg(any(test, feature = "test-support"))]
mod test_support {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use super::{
        validate_control_bootstrap_seed, ControlAuditRecord, ControlBootstrapMode, ControlStore,
    };
    use crate::control_model::{
        apply_control_changes, AuditRequestContext, BootstrapOutcome, ControlChangeSet,
        ControlCommit, ControlEvent, ControlEventCursor, ControlOperation, ControlRevision,
        ControlScope, ControlSnapshot, PathPolicy, PolicyEffect, PrincipalIdentity,
        VersionedControlOperation, VersionedControlSnapshot,
    };
    use crate::error::{Error, Result};

    #[derive(Default)]
    struct InMemoryState {
        snapshot: Option<ControlSnapshot>,
        revision: ControlRevision,
        entity_versions: BTreeMap<String, String>,
        events: Vec<ControlEvent>,
        audit: Vec<ControlAuditRecord>,
        idempotency: HashMap<String, (ControlChangeSet, ControlCommit, String)>,
    }

    pub struct InMemoryControlStore {
        state: Mutex<InMemoryState>,
        applying_instance: String,
    }

    impl Default for InMemoryControlStore {
        fn default() -> Self {
            Self::new()
        }
    }

    impl InMemoryControlStore {
        pub fn new() -> Self {
            Self {
                state: Mutex::new(InMemoryState::default()),
                applying_instance: "in-memory-control-store".to_string(),
            }
        }
    }

    #[async_trait]
    impl ControlStore for InMemoryControlStore {
        async fn bootstrap_if_empty(
            &self,
            seed: &ControlSnapshot,
            actor: &PrincipalIdentity,
            mode: ControlBootstrapMode,
        ) -> Result<BootstrapOutcome> {
            let mut state = self.state.lock().await;
            if state.snapshot.is_some() {
                return Ok(BootstrapOutcome::AlreadyInitialized(state.revision));
            }
            seed.validate()?;
            validate_actor(actor)?;
            validate_control_bootstrap_seed(seed, mode)?;

            state.revision = 1;
            state.snapshot = Some(seed.clone());
            let changed_resources = vec!["snapshot".to_string()];
            state.events.push(ControlEvent {
                revision: 1,
                ordinal: 0,
                changed_resources: changed_resources.clone(),
            });
            state.audit.push(ControlAuditRecord {
                revision: 1,
                actor: actor.clone(),
                request: AuditRequestContext {
                    method: "BOOTSTRAP".to_string(),
                    canonical_path: "/_control/v1/platform".to_string(),
                    correlation_id: "bootstrap".to_string(),
                },
                idempotency_key: None,
                changed_resources,
                recorded_at_unix_ms: now_unix_ms(),
                applying_instance: self.applying_instance.clone(),
            });
            Ok(BootstrapOutcome::Bootstrapped(1))
        }

        async fn current_revision(&self) -> Result<Option<ControlRevision>> {
            let state = self.state.lock().await;
            Ok(state.snapshot.as_ref().map(|_| state.revision))
        }

        async fn load_snapshot(&self) -> Result<VersionedControlSnapshot> {
            let state = self.state.lock().await;
            let snapshot = state.snapshot.clone().ok_or(Error::ControlUninitialized)?;
            VersionedControlSnapshot::new(snapshot, state.revision, state.entity_versions.clone())
        }

        async fn transact(
            &self,
            authorization: &crate::AuthorizedControlMutation,
            changes: &ControlChangeSet,
        ) -> Result<ControlCommit> {
            validate_actor(authorization.principal())?;
            validate_request(authorization.audit_request())?;
            changes.validate()?;
            authorization.validate_intent(changes)?;
            let mut state = self.state.lock().await;

            if let Some(key) = &changes.idempotency_key {
                if let Some((recorded_changes, recorded_commit, request_fingerprint)) =
                    state.idempotency.get(key)
                {
                    if recorded_changes != changes {
                        return Err(Error::ControlIdempotencyConflict { key: key.clone() });
                    }
                    if request_fingerprint != authorization.request_fingerprint() {
                        return Err(Error::ControlIdempotencyAuthorizationConflict {
                            key: key.clone(),
                        });
                    }
                    let mut replay = recorded_commit.clone();
                    replay.replayed = true;
                    return Ok(replay);
                }
            }
            if authorization.is_replay_only() {
                return Err(Error::ControlIdempotencyAuthorizationConflict {
                    key: changes.idempotency_key.clone().unwrap_or_default(),
                });
            }

            let current = state.snapshot.clone().ok_or(Error::ControlUninitialized)?;
            if authorization.snapshot_revision() != state.revision {
                return Err(Error::ControlRevisionConflict {
                    expected: authorization.snapshot_revision(),
                    current: state.revision,
                });
            }

            let revision = state
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::ControlValidation("control revision overflow".to_string()))?;
            let applied = apply_control_changes(
                current,
                state.entity_versions.clone(),
                revision,
                authorization,
                changes,
            )?;
            let changed_resources = applied.changed_resources;
            let commit = ControlCommit {
                revision,
                changed_resources: changed_resources.clone(),
                replayed: false,
            };
            state.snapshot = Some(applied.snapshot);
            state.entity_versions = applied.entity_versions;
            state.revision = revision;
            state.events.push(ControlEvent {
                revision,
                ordinal: 0,
                changed_resources: changed_resources.clone(),
            });
            state.audit.push(ControlAuditRecord {
                revision,
                actor: authorization.principal().clone(),
                request: authorization.audit_request().clone(),
                idempotency_key: changes.idempotency_key.clone(),
                changed_resources,
                recorded_at_unix_ms: now_unix_ms(),
                applying_instance: self.applying_instance.clone(),
            });
            if let Some(key) = &changes.idempotency_key {
                state.idempotency.insert(
                    key.clone(),
                    (
                        changes.clone(),
                        commit.clone(),
                        authorization.request_fingerprint().to_string(),
                    ),
                );
            }
            Ok(commit)
        }

        async fn changes_since(
            &self,
            after: Option<ControlEventCursor>,
            limit: u32,
        ) -> Result<Vec<ControlEvent>> {
            validate_limit(limit)?;
            let state = self.state.lock().await;
            Ok(state
                .events
                .iter()
                .filter(|event| after.is_none_or(|cursor| event.cursor() > cursor))
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn audit_since(
            &self,
            after: ControlRevision,
            limit: u32,
        ) -> Result<Vec<ControlAuditRecord>> {
            validate_limit(limit)?;
            let state = self.state.lock().await;
            Ok(state
                .audit
                .iter()
                .filter(|record| record.revision > after)
                .take(limit as usize)
                .cloned()
                .collect())
        }
    }

    fn validate_actor(actor: &PrincipalIdentity) -> Result<()> {
        if actor.issuer.trim().is_empty() || actor.subject.trim().is_empty() {
            return Err(Error::ControlValidation(
                "control actor issuer and subject must not be empty".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_request(request: &AuditRequestContext) -> Result<()> {
        if request.method.trim().is_empty()
            || !request.canonical_path.starts_with('/')
            || request.correlation_id.trim().is_empty()
        {
            return Err(Error::ControlValidation(
                "audit request method, canonical path, and correlation id are required".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_limit(limit: u32) -> Result<()> {
        if limit == 0 {
            return Err(Error::ControlValidation(
                "control page limit must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }

    fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn actor() -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "sysadmin".to_string(),
        }
    }

    fn limited_actor() -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "policy-manager".to_string(),
        }
    }

    fn policy(id: &str) -> PathPolicy {
        PathPolicy::new(
            id,
            "service_account",
            ControlScope::Platform,
            PolicyEffect::Allow,
            ["GET"],
            ["/_control/v1/platform/**"],
        )
    }

    fn authorization_for(
        principal: PrincipalIdentity,
        versioned: &VersionedControlSnapshot,
        request: &AuditRequestContext,
        changes: &ControlChangeSet,
    ) -> crate::AuthorizedControlMutation {
        authorization_for_scope(
            principal,
            versioned,
            request,
            ControlScope::Platform,
            changes,
        )
    }

    fn authorization_for_scope(
        principal: PrincipalIdentity,
        versioned: &VersionedControlSnapshot,
        request: &AuditRequestContext,
        scope: ControlScope,
        changes: &ControlChangeSet,
    ) -> crate::AuthorizedControlMutation {
        let subject = crate::AuthenticatedSubject {
            principal,
            claims: Default::default(),
        };
        let control_request = crate::MutationControlRequestContext {
            method: request.method.clone(),
            canonical_path: request.canonical_path.clone(),
            route_template: request.canonical_path.clone(),
            scope,
        };
        crate::control_admin_policy::authorize_control_mutation_from_context(
            &subject,
            &control_request,
            request.clone(),
            versioned,
            changes,
        )
        .expect("seed binding authorizes the platform mutation")
    }

    fn authorization(
        versioned: &VersionedControlSnapshot,
        request: &AuditRequestContext,
        changes: &ControlChangeSet,
    ) -> crate::AuthorizedControlMutation {
        authorization_for(actor(), versioned, request, changes)
    }

    fn changes(key: &str, policy_id: &str) -> ControlChangeSet {
        ControlChangeSet {
            idempotency_key: Some(key.to_string()),
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutPathPolicy(policy(policy_id)),
            }],
        }
    }

    pub async fn assert_control_store_contract(store: Arc<dyn ControlStore>) {
        let mut config = serde_yaml::from_str::<crate::AppConfig>(
            "auth:\n  trusted_issuers:\n    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }",
        )
        .unwrap();
        config.tenants.push(crate::TenantDecl {
            id: "tenant-a".to_string(),
            external_id: None,
            settings: Default::default(),
        });
        let seed = ControlSnapshot {
            config,
            role_bindings: vec![
                crate::RoleBinding {
                    principal: actor(),
                    role: "sysadmin".to_string(),
                    scope: ControlScope::Platform,
                },
                crate::RoleBinding {
                    principal: limited_actor(),
                    role: "policy_manager".to_string(),
                    scope: ControlScope::Platform,
                },
            ],
            path_policies: vec![
                PathPolicy::new(
                    "manage-policies",
                    "policy_manager",
                    ControlScope::Platform,
                    PolicyEffect::Allow,
                    ["POST", "PUT", "DELETE"],
                    ["/_control/v1/platform/policies/**"],
                ),
                PathPolicy::new(
                    "settings-envelope",
                    "policy_manager",
                    ControlScope::Platform,
                    PolicyEffect::Allow,
                    ["PATCH"],
                    ["/_control/v1/platform/settings/**"],
                ),
                PathPolicy::new(
                    "deny-secrets",
                    "policy_manager",
                    ControlScope::Platform,
                    PolicyEffect::Deny,
                    ["PATCH"],
                    ["/_control/v1/platform/settings/secrets/**"],
                ),
                PathPolicy::legacy(
                    "legacy-inert",
                    PolicyEffect::Allow,
                    ["GET"],
                    ["/administration/tenants/**"],
                    Vec::new(),
                ),
            ],
            tombstoned_resources: Vec::new(),
        };
        assert_eq!(store.current_revision().await.unwrap(), None);
        assert!(matches!(
            store
                .bootstrap_if_empty(
                    &seed,
                    &actor(),
                    ControlBootstrapMode::RequireInitialSysadmin,
                )
                .await
                .unwrap(),
            BootstrapOutcome::Bootstrapped(1)
        ));
        assert!(matches!(
            store
                .bootstrap_if_empty(
                    &seed,
                    &actor(),
                    ControlBootstrapMode::RequireInitialSysadmin,
                )
                .await
                .unwrap(),
            BootstrapOutcome::AlreadyInitialized(1)
        ));
        assert_eq!(store.load_snapshot().await.unwrap().snapshot, seed);

        let revision_one = store.load_snapshot().await.unwrap();
        let limited_put_request = AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: "/_control/v1/platform/policies/change".to_string(),
            correlation_id: "limited-put".to_string(),
        };
        let self_widening = ControlChangeSet {
            idempotency_key: Some("self-widening".to_string()),
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutPathPolicy(PathPolicy::new(
                    "become-sysadmin",
                    "policy_manager",
                    ControlScope::Platform,
                    PolicyEffect::Allow,
                    ["DELETE"],
                    ["/_control/v1/**"],
                )),
            }],
        };
        let limited_put = authorization_for(
            limited_actor(),
            &revision_one,
            &limited_put_request,
            &self_widening,
        );
        assert!(store.transact(&limited_put, &self_widening).await.is_err());
        assert_eq!(store.current_revision().await.unwrap(), Some(1));

        let limited_delete_request = AuditRequestContext {
            method: "DELETE".to_string(),
            canonical_path: "/_control/v1/platform/policies/deny-secrets".to_string(),
            correlation_id: "limited-delete".to_string(),
        };
        let remove_deny = ControlChangeSet {
            idempotency_key: Some("remove-deny".to_string()),
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::DeletePathPolicy {
                    id: "deny-secrets".to_string(),
                },
            }],
        };
        let limited_delete = authorization_for(
            limited_actor(),
            &revision_one,
            &limited_delete_request,
            &remove_deny,
        );
        assert!(store.transact(&limited_delete, &remove_deny).await.is_err());
        assert_eq!(store.current_revision().await.unwrap(), Some(1));

        let request = AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: "/_control/v1/platform/policies/read-one".to_string(),
            correlation_id: "correlation-1".to_string(),
        };
        let first_changes = changes("request-1", "read-one");
        let authorization_one = authorization(&revision_one, &request, &first_changes);
        let commit = store
            .transact(&authorization_one, &first_changes)
            .await
            .unwrap();
        assert_eq!(commit.revision, 2);
        assert!(!commit.replayed);

        let replay = store
            .transact(&authorization_one, &first_changes)
            .await
            .unwrap();
        assert_eq!(replay.revision, 2);
        assert!(replay.replayed);
        assert_eq!(store.current_revision().await.unwrap(), Some(2));

        let fabricated_revision_one =
            VersionedControlSnapshot::new(seed.clone(), 1, BTreeMap::new()).unwrap();
        let reconstructed_authorization =
            authorization(&fabricated_revision_one, &request, &first_changes);
        let reconstructed_replay = store
            .transact(&reconstructed_authorization, &first_changes)
            .await
            .unwrap();
        assert_eq!(reconstructed_replay.revision, 2);
        assert!(reconstructed_replay.replayed);

        let cross_principal =
            authorization_for(limited_actor(), &revision_one, &request, &first_changes);
        assert!(store
            .transact(&cross_principal, &first_changes)
            .await
            .is_err());

        let cross_route_request = AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: "/_control/v1/platform/policies/another-route".to_string(),
            correlation_id: "cross-route-replay".to_string(),
        };
        let cross_route = authorization(&revision_one, &cross_route_request, &first_changes);
        assert!(store.transact(&cross_route, &first_changes).await.is_err());

        let tenant_request = AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: "/_control/v1/tenants/tenant-a/settings".to_string(),
            correlation_id: "cross-scope-replay".to_string(),
        };
        let cross_scope = authorization_for_scope(
            actor(),
            &revision_one,
            &tenant_request,
            ControlScope::Tenant {
                tenant_id: "tenant-a".to_string(),
            },
            &first_changes,
        );
        assert!(store.transact(&cross_scope, &first_changes).await.is_err());

        let revision_two = store.load_snapshot().await.unwrap();
        let reused_changes = changes("request-1", "different");
        let reused_authorization = authorization(&revision_two, &request, &reused_changes);
        let reused_key = store
            .transact(&reused_authorization, &reused_changes)
            .await
            .unwrap_err();
        assert!(matches!(
            reused_key,
            Error::ControlIdempotencyConflict { .. }
        ));

        let versioned = store.load_snapshot().await.unwrap();
        assert_eq!(
            versioned
                .entity_versions
                .get("path-policy/read-one")
                .map(String::as_str),
            Some("2")
        );
        let stale_entity = ControlChangeSet {
            idempotency_key: Some("request-stale-entity".to_string()),
            operations: vec![VersionedControlOperation {
                expected_entity_version: Some("1".to_string()),
                operation: ControlOperation::PutPathPolicy(policy("read-one")),
            }],
        };
        let stale_authorization = authorization(&revision_two, &request, &stale_entity);
        assert!(matches!(
            store
                .transact(&stale_authorization, &stale_entity)
                .await
                .unwrap_err(),
            Error::ControlEntityVersionConflict { .. }
        ));

        let conflict_changes = changes("request-2", "read-two");
        let conflict_authorization = authorization(&revision_one, &request, &conflict_changes);
        let conflict = store
            .transact(&conflict_authorization, &conflict_changes)
            .await
            .unwrap_err();
        assert!(matches!(
            conflict,
            Error::ControlRevisionConflict {
                expected: 1,
                current: 2
            }
        ));

        let invalid = ControlChangeSet {
            idempotency_key: Some("request-invalid".to_string()),
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutRoleBinding(crate::RoleBinding {
                    principal: actor(),
                    role: "tenant_admin".to_string(),
                    scope: ControlScope::Tenant {
                        tenant_id: "missing".to_string(),
                    },
                }),
            }],
        };
        let invalid_authorization = authorization(&revision_two, &request, &invalid);
        assert!(store
            .transact(&invalid_authorization, &invalid)
            .await
            .is_err());
        assert_eq!(store.current_revision().await.unwrap(), Some(2));
        assert_eq!(store.changes_since(None, 10).await.unwrap().len(), 2);
        assert_eq!(store.audit_since(0, 10).await.unwrap().len(), 2);

        let mut tenant_update = revision_two.snapshot.config.tenants[0].clone();
        tenant_update.settings.cache_ttl_s = Some(60);
        let late_failure = ControlChangeSet {
            idempotency_key: Some("late-failure-rollback".to_string()),
            operations: vec![
                VersionedControlOperation {
                    expected_entity_version: None,
                    operation: ControlOperation::PutTenant(tenant_update),
                },
                VersionedControlOperation {
                    expected_entity_version: None,
                    operation: ControlOperation::PutRoleBinding(crate::RoleBinding {
                        principal: actor(),
                        role: "tenant_admin".to_string(),
                        scope: ControlScope::Tenant {
                            tenant_id: "missing".to_string(),
                        },
                    }),
                },
            ],
        };
        let late_failure_authorization = authorization(&revision_two, &request, &late_failure);
        assert!(store
            .transact(&late_failure_authorization, &late_failure)
            .await
            .is_err());
        let after_failure = store.load_snapshot().await.unwrap();
        assert_eq!(after_failure.snapshot, revision_two.snapshot);
        assert_eq!(after_failure.entity_versions, revision_two.entity_versions);
        assert_eq!(after_failure.revision, revision_two.revision);
        assert_eq!(store.changes_since(None, 10).await.unwrap().len(), 2);
        assert_eq!(store.audit_since(0, 10).await.unwrap().len(), 2);

        let third_changes = changes("late-failure-rollback", "read-three");
        let third_authorization = authorization(&revision_two, &request, &third_changes);
        store
            .transact(&third_authorization, &third_changes)
            .await
            .unwrap();
        let page = store
            .changes_since(
                Some(ControlEventCursor {
                    revision: 1,
                    ordinal: 0,
                }),
                1,
            )
            .await
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].revision, 2);
        let next_page = store
            .changes_since(Some(page[0].cursor()), 1)
            .await
            .unwrap();
        assert_eq!(next_page.len(), 1);
        assert_eq!(next_page[0].revision, 3);
    }

    pub use InMemoryControlStore as ExportedInMemoryControlStore;
}

#[cfg(any(test, feature = "test-support"))]
pub use test_support::{
    assert_control_store_contract, ExportedInMemoryControlStore as InMemoryControlStore,
};
