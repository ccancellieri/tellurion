#![cfg(feature = "test-support")]

use std::collections::BTreeMap;
use std::sync::Arc;

use tellurion_core::{
    apply_control_changes, assert_control_store_contract, authorize_control_mutation,
    AuthenticatedSubject, BootstrapOutcome, ControlBootstrapMode, ControlChangeSet,
    ControlOperation, ControlRouteDescriptor, ControlRouteRegistry, ControlScope, ControlSnapshot,
    ControlStore, InMemoryControlStore, PrincipalIdentity, RoleBinding, VersionedControlOperation,
    VersionedControlSnapshot,
};

fn reachable_config() -> tellurion_core::AppConfig {
    serde_yaml::from_str(
        "auth:\n  trusted_issuers:\n    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }",
    )
    .unwrap()
}

fn platform_import_authorization(
    actor: &PrincipalIdentity,
    snapshot: &VersionedControlSnapshot,
    changes: &ControlChangeSet,
    correlation_id: &str,
) -> tellurion_core::AuthorizedControlMutation {
    let route = ControlRouteDescriptor::PlatformBatchImport;
    let registry = ControlRouteRegistry::new([route]).unwrap();
    authorize_control_mutation(
        &AuthenticatedSubject {
            principal: actor.clone(),
            claims: Default::default(),
        },
        "POST",
        b"/_control/v1/platform/import",
        route.template(),
        &registry,
        "",
        snapshot,
        changes,
        correlation_id,
    )
    .unwrap()
}

fn route_authorization(
    actor: &PrincipalIdentity,
    snapshot: &VersionedControlSnapshot,
    descriptor: ControlRouteDescriptor,
    method: &str,
    path: &str,
    changes: &ControlChangeSet,
    correlation_id: &str,
) -> tellurion_core::AuthorizedControlMutation {
    let registry = ControlRouteRegistry::new([descriptor]).unwrap();
    authorize_control_mutation(
        &AuthenticatedSubject {
            principal: actor.clone(),
            claims: Default::default(),
        },
        method,
        path.as_bytes(),
        descriptor.template(),
        &registry,
        "",
        snapshot,
        changes,
        correlation_id,
    )
    .unwrap()
}

#[tokio::test]
async fn in_memory_store_satisfies_the_backend_neutral_contract() {
    assert_control_store_contract(Arc::new(InMemoryControlStore::new())).await;
}

#[tokio::test]
async fn normal_first_boot_rejects_a_sysadmin_unreachable_through_auth() {
    let actor = PrincipalIdentity {
        issuer: "https://identity.example".to_string(),
        subject: "platform-operator".to_string(),
    };
    let seed = ControlSnapshot {
        config: Default::default(),
        role_bindings: vec![RoleBinding {
            principal: actor.clone(),
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };
    let store = InMemoryControlStore::new();

    assert!(store
        .bootstrap_if_empty(&seed, &actor, ControlBootstrapMode::RequireInitialSysadmin,)
        .await
        .is_err());
    assert_eq!(store.current_revision().await.unwrap(), None);
}

#[tokio::test]
async fn initialized_store_ignores_semantically_invalid_changed_seed_and_actor() {
    let actor = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "sysadmin".to_string(),
    };
    let authoritative = ControlSnapshot {
        config: reachable_config(),
        role_bindings: vec![RoleBinding {
            principal: actor.clone(),
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };
    let store = InMemoryControlStore::new();
    store
        .bootstrap_if_empty(
            &authoritative,
            &actor,
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();

    let mut changed = authoritative.clone();
    changed.config.server.max_concurrency = Some(0);
    changed.role_bindings.push(changed.role_bindings[0].clone());
    let invalid_actor = PrincipalIdentity {
        issuer: String::new(),
        subject: String::new(),
    };
    assert_eq!(
        store
            .bootstrap_if_empty(
                &changed,
                &invalid_actor,
                ControlBootstrapMode::RequireInitialSysadmin,
            )
            .await
            .unwrap(),
        BootstrapOutcome::AlreadyInitialized(1)
    );
    assert_eq!(store.load_snapshot().await.unwrap().snapshot, authoritative);

    let empty = InMemoryControlStore::new();
    assert!(empty
        .bootstrap_if_empty(
            &changed,
            &invalid_actor,
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .is_err());
    assert_eq!(empty.current_revision().await.unwrap(), None);
}

#[tokio::test]
async fn in_memory_store_rejects_a_token_from_different_same_revision_state() {
    let actor = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "sysadmin".to_string(),
    };
    let authoritative = ControlSnapshot {
        config: reachable_config(),
        role_bindings: vec![RoleBinding {
            principal: actor.clone(),
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };
    let store = InMemoryControlStore::new();
    store
        .bootstrap_if_empty(
            &authoritative,
            &actor,
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();

    let mut different = authoritative.clone();
    different.config.server.port = 9_101;
    let versioned = VersionedControlSnapshot::new(different, 1, BTreeMap::new()).unwrap();
    let path = "/_control/v1/platform/import";
    let route = ControlRouteDescriptor::PlatformBatchImport;
    let registry = ControlRouteRegistry::new([route]).unwrap();
    let changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(authoritative.config),
        }],
    };
    let authorization = authorize_control_mutation(
        &AuthenticatedSubject {
            principal: actor,
            claims: Default::default(),
        },
        "POST",
        path.as_bytes(),
        route.template(),
        &registry,
        "",
        &versioned,
        &changes,
        "different-same-revision",
    )
    .unwrap();

    let error = store.transact(&authorization, &changes).await.unwrap_err();
    assert!(error.to_string().contains("authoritative"));
}

#[tokio::test]
async fn raw_read_or_unregistered_request_cannot_mint_a_store_accepted_mutation_proof() {
    for (method, path) in [
        ("GET", "/_control/v1/platform/settings"),
        ("PUT", "/_control/v1/platform/unregistered"),
    ] {
        let role = if method == "GET" {
            "viewer"
        } else {
            "sysadmin"
        };
        let actor = PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "sysadmin".to_string(),
        };
        let authoritative = ControlSnapshot {
            config: reachable_config(),
            role_bindings: vec![RoleBinding {
                principal: actor.clone(),
                role: role.to_string(),
                scope: ControlScope::Platform,
            }],
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        };
        let versioned =
            VersionedControlSnapshot::new(authoritative.clone(), 1, BTreeMap::new()).unwrap();
        let registered = "/_control/v1/platform/settings";
        let registry =
            ControlRouteRegistry::new([ControlRouteDescriptor::PlatformSettings]).unwrap();
        let route_template = if method == "GET" { registered } else { path };
        let changes = ControlChangeSet {
            idempotency_key: None,
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(authoritative.config),
            }],
        };

        assert!(authorize_control_mutation(
            &AuthenticatedSubject {
                principal: actor,
                claims: Default::default(),
            },
            method,
            path.as_bytes(),
            route_template,
            &registry,
            "",
            &versioned,
            &changes,
            "raw-bypass",
        )
        .is_err());
    }
}

#[tokio::test]
async fn rejected_unstaged_permanent_delete_is_atomic_and_does_not_claim_idempotency() {
    let actor = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "sysadmin".to_string(),
    };
    let config = serde_yaml::from_str(
        r#"
auth:
  trusted_issuers:
    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a } ]
catalogs: [ { id: catalog-a, tenant: tenant-a } ]
collections: [ { id: collection-a, catalog: catalog-a, storage: main } ]
"#,
    )
    .unwrap();
    let seed = ControlSnapshot {
        config,
        role_bindings: vec![RoleBinding {
            principal: actor.clone(),
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };
    let store = InMemoryControlStore::new();
    store
        .bootstrap_if_empty(&seed, &actor, ControlBootstrapMode::RequireInitialSysadmin)
        .await
        .unwrap();

    let initial = store.load_snapshot().await.unwrap();
    let mut collection = initial.snapshot.config.collections[0].clone();
    collection.external_id = Some("versioned".to_string());
    let establish_version = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCollection(collection),
        }],
    };
    let establish_authorization =
        platform_import_authorization(&actor, &initial, &establish_version, "establish-version");
    store
        .transact(&establish_authorization, &establish_version)
        .await
        .unwrap();

    let collection_scope = ControlScope::Collection {
        tenant_id: "tenant-a".to_string(),
        catalog_id: "catalog-a".to_string(),
        collection_id: "collection-a".to_string(),
    };
    let baseline = store.load_snapshot().await.unwrap();
    assert_eq!(
        baseline
            .entity_versions
            .get(&collection_scope.resource_key())
            .map(String::as_str),
        Some("2")
    );
    let baseline_events = store.changes_since(None, 100).await.unwrap();
    let baseline_audit = store.audit_since(0, 100).await.unwrap();
    let rejected = ControlChangeSet {
        idempotency_key: Some("permanent-delete-retry".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("2".to_string()),
            operation: ControlOperation::PermanentlyDeleteResource {
                scope: collection_scope.clone(),
            },
        }],
    };
    let rejected_authorization =
        platform_import_authorization(&actor, &baseline, &rejected, "reject-unstaged");
    store
        .transact(&rejected_authorization, &rejected)
        .await
        .unwrap_err();

    assert_eq!(store.load_snapshot().await.unwrap(), baseline);
    assert_eq!(store.current_revision().await.unwrap(), Some(2));
    assert_eq!(
        store.changes_since(None, 100).await.unwrap(),
        baseline_events
    );
    assert_eq!(store.audit_since(0, 100).await.unwrap(), baseline_audit);

    let tombstone = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("2".to_string()),
            operation: ControlOperation::TombstoneResource {
                scope: collection_scope.clone(),
            },
        }],
    };
    let tombstone_authorization =
        platform_import_authorization(&actor, &baseline, &tombstone, "stage-delete");
    store
        .transact(&tombstone_authorization, &tombstone)
        .await
        .unwrap();

    let staged = store.load_snapshot().await.unwrap();
    let retry = ControlChangeSet {
        idempotency_key: Some("permanent-delete-retry".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("3".to_string()),
            operation: ControlOperation::PermanentlyDeleteResource {
                scope: collection_scope.clone(),
            },
        }],
    };
    let retry_authorization =
        platform_import_authorization(&actor, &staged, &retry, "retry-staged");
    let commit = store.transact(&retry_authorization, &retry).await.unwrap();
    assert_eq!(commit.revision, 4);
    assert!(store
        .load_snapshot()
        .await
        .unwrap()
        .snapshot
        .config
        .collections
        .is_empty());
}

#[tokio::test]
async fn fresh_equivalent_create_and_update_requests_replay_but_conflicts_fail_closed() {
    let actor = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "sysadmin".to_string(),
    };
    let other = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "other-sysadmin".to_string(),
    };
    let seed = ControlSnapshot {
        config: reachable_config(),
        role_bindings: vec![
            RoleBinding {
                principal: actor.clone(),
                role: "sysadmin".to_string(),
                scope: ControlScope::Platform,
            },
            RoleBinding {
                principal: other.clone(),
                role: "sysadmin".to_string(),
                scope: ControlScope::Platform,
            },
        ],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };
    let store = InMemoryControlStore::new();
    store
        .bootstrap_if_empty(&seed, &actor, ControlBootstrapMode::RequireInitialSysadmin)
        .await
        .unwrap();

    let create = ControlChangeSet {
        idempotency_key: Some("create-tenant".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutTenant(tellurion_core::TenantDecl {
                id: "tenant-replay".to_string(),
                external_id: None,
                settings: Default::default(),
            }),
        }],
    };
    let initial = store.load_snapshot().await.unwrap();
    let create_authorization = route_authorization(
        &actor,
        &initial,
        ControlRouteDescriptor::Tenants,
        "POST",
        "/_control/v1/tenants",
        &create,
        "create-first",
    );
    let created = store
        .transact(&create_authorization, &create)
        .await
        .unwrap();
    assert_eq!(created.revision, 2);
    assert!(!created.replayed);

    let restarted = store.load_snapshot().await.unwrap();
    let replay_authorization = route_authorization(
        &actor,
        &restarted,
        ControlRouteDescriptor::Tenants,
        "POST",
        "/_control/v1/tenants",
        &create,
        "create-after-restart",
    );
    let replayed = store
        .transact(&replay_authorization, &create)
        .await
        .unwrap();
    assert_eq!(replayed.revision, 2);
    assert!(replayed.replayed);

    let no_record = ControlChangeSet {
        idempotency_key: Some("missing-create-record".to_string()),
        ..create.clone()
    };
    let no_record_authorization = route_authorization(
        &actor,
        &restarted,
        ControlRouteDescriptor::Tenants,
        "POST",
        "/_control/v1/tenants",
        &no_record,
        "missing-create-record",
    );
    assert!(store
        .transact(&no_record_authorization, &no_record)
        .await
        .is_err());
    assert_eq!(store.current_revision().await.unwrap(), Some(2));

    let cross_principal = route_authorization(
        &other,
        &restarted,
        ControlRouteDescriptor::Tenants,
        "POST",
        "/_control/v1/tenants",
        &create,
        "other-principal",
    );
    assert!(matches!(
        store.transact(&cross_principal, &create).await,
        Err(tellurion_core::Error::ControlIdempotencyAuthorizationConflict { .. })
    ));

    let mut updated_config = restarted.snapshot.config.clone();
    updated_config.server.port = 9_321;
    let update = ControlChangeSet {
        idempotency_key: Some("update-platform".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(updated_config),
        }],
    };
    let update_authorization = route_authorization(
        &actor,
        &restarted,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &update,
        "update-first",
    );
    let updated = store
        .transact(&update_authorization, &update)
        .await
        .unwrap();
    assert_eq!(updated.revision, 3);

    let updated_snapshot = store.load_snapshot().await.unwrap();
    let update_replay_authorization = route_authorization(
        &actor,
        &updated_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &update,
        "update-after-restart",
    );
    let update_replay = store
        .transact(&update_replay_authorization, &update)
        .await
        .unwrap();
    assert_eq!(update_replay.revision, 3);
    assert!(update_replay.replayed);

    let mut changed_intent = update.clone();
    changed_intent.operations[0].expected_entity_version = Some("3".to_string());
    let changed_intent_authorization = route_authorization(
        &actor,
        &updated_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &changed_intent,
        "changed-intent",
    );
    assert!(matches!(
        store
            .transact(&changed_intent_authorization, &changed_intent)
            .await,
        Err(tellurion_core::Error::ControlIdempotencyConflict { .. })
    ));
}

#[tokio::test]
async fn reconstructed_collection_move_replays_but_unrecorded_no_ops_fail_closed() {
    let actor = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "sysadmin".to_string(),
    };
    let config = serde_yaml::from_str(
        r#"
auth:
  trusted_issuers:
    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a } ]
catalogs:
  - { id: catalog-a, tenant: tenant-a }
  - { id: catalog-a2, tenant: tenant-a }
collections:
  - { id: collection-moved, catalog: catalog-a, storage: main }
  - { id: collection-unmoved, catalog: catalog-a, storage: main }
"#,
    )
    .unwrap();
    let seed = ControlSnapshot {
        config,
        role_bindings: vec![RoleBinding {
            principal: actor.clone(),
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };
    let store = InMemoryControlStore::new();
    store
        .bootstrap_if_empty(&seed, &actor, ControlBootstrapMode::RequireInitialSysadmin)
        .await
        .unwrap();

    let mut moved = seed.config.collections[0].clone();
    moved.catalog = "catalog-a2".to_string();
    let changes = ControlChangeSet {
        idempotency_key: Some("move-collection".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCollection(moved),
        }],
    };
    let route = ControlRouteDescriptor::TenantCollectionMove;
    let path = "/_control/v1/tenants/tenant-a/collection-moves";
    let initial = store.load_snapshot().await.unwrap();
    let first_authorization = route_authorization(
        &actor,
        &initial,
        route,
        "POST",
        path,
        &changes,
        "move-first",
    );
    let first = store
        .transact(&first_authorization, &changes)
        .await
        .unwrap();
    assert_eq!(first.revision, 2);
    assert!(!first.replayed);

    let after_move = store.load_snapshot().await.unwrap();
    let reconstructed = route_authorization(
        &actor,
        &after_move,
        route,
        "POST",
        path,
        &changes,
        "move-reconstructed",
    );
    let replay = store.transact(&reconstructed, &changes).await.unwrap();
    assert_eq!(replay.revision, 2);
    assert!(replay.replayed);

    let different_key = ControlChangeSet {
        idempotency_key: Some("move-collection-different-key".to_string()),
        ..changes.clone()
    };
    let different_key_authorization = route_authorization(
        &actor,
        &after_move,
        route,
        "POST",
        path,
        &different_key,
        "move-different-key",
    );
    assert!(matches!(
        store
            .transact(&different_key_authorization, &different_key)
            .await,
        Err(tellurion_core::Error::ControlIdempotencyAuthorizationConflict { .. })
    ));

    let first_time_no_op = ControlChangeSet {
        idempotency_key: Some("move-first-time-no-op".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCollection(seed.config.collections[1].clone()),
        }],
    };
    let no_op_authorization = route_authorization(
        &actor,
        &after_move,
        route,
        "POST",
        path,
        &first_time_no_op,
        "move-first-time-no-op",
    );
    assert!(matches!(
        store
            .transact(&no_op_authorization, &first_time_no_op)
            .await,
        Err(tellurion_core::Error::ControlIdempotencyAuthorizationConflict { .. })
    ));
    assert_eq!(store.current_revision().await.unwrap(), Some(2));
}

#[test]
fn collection_move_migrates_dependent_control_state_and_moves_back_atomically() {
    let actor = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "tenant-admin".to_string(),
    };
    let dependent = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "collection-reader".to_string(),
    };
    let config = serde_yaml::from_str(
        r#"
auth:
  trusted_issuers:
    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a }, { id: tenant-b } ]
catalogs:
  - { id: catalog-a, tenant: tenant-a }
  - { id: catalog-a2, tenant: tenant-a }
  - { id: catalog-b, tenant: tenant-b }
collections: [ { id: collection-a, catalog: catalog-a, storage: main } ]
"#,
    )
    .unwrap();
    let old_scope = ControlScope::Collection {
        tenant_id: "tenant-a".to_string(),
        catalog_id: "catalog-a".to_string(),
        collection_id: "collection-a".to_string(),
    };
    let new_scope = ControlScope::Collection {
        tenant_id: "tenant-a".to_string(),
        catalog_id: "catalog-a2".to_string(),
        collection_id: "collection-a".to_string(),
    };
    let old_prefix = "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a";
    let new_prefix = "/_control/v1/tenants/tenant-a/catalogs/catalog-a2/collections/collection-a";
    let authoritative = ControlSnapshot {
        config,
        role_bindings: vec![
            RoleBinding {
                principal: actor.clone(),
                role: "tenant_admin".to_string(),
                scope: ControlScope::Tenant {
                    tenant_id: "tenant-a".to_string(),
                },
            },
            RoleBinding {
                principal: dependent,
                role: "viewer".to_string(),
                scope: old_scope.clone(),
            },
        ],
        path_policies: vec![
            tellurion_core::PathPolicy::new(
                "collection-allow",
                "viewer",
                old_scope.clone(),
                tellurion_core::PolicyEffect::Allow,
                ["GET"],
                [format!("{old_prefix}/metadata")],
            ),
            tellurion_core::PathPolicy::new(
                "collection-deny",
                "viewer",
                old_scope.clone(),
                tellurion_core::PolicyEffect::Deny,
                ["DELETE"],
                [format!("{old_prefix}/assets/**")],
            ),
        ],
        tombstoned_resources: vec![old_scope.clone()],
    };
    authoritative.validate().unwrap();
    let old_key = old_scope.resource_key();
    let new_key = new_scope.resource_key();
    let versions = BTreeMap::from([(old_key.clone(), "11".to_string())]);
    let mut candidate = authoritative.config.collections[0].clone();
    candidate.catalog = "catalog-a2".to_string();
    let changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("11".to_string()),
            operation: ControlOperation::PutCollection(candidate),
        }],
    };
    let versioned =
        VersionedControlSnapshot::new(authoritative.clone(), 1, versions.clone()).unwrap();
    let authorization = route_authorization(
        &actor,
        &versioned,
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        "/_control/v1/tenants/tenant-a/collection-moves",
        &changes,
        "move-forward",
    );

    let mut stale = changes.clone();
    stale.operations[0].expected_entity_version = Some("10".to_string());
    let stale_authorization = route_authorization(
        &actor,
        &versioned,
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        "/_control/v1/tenants/tenant-a/collection-moves",
        &stale,
        "move-stale",
    );
    assert!(apply_control_changes(
        authoritative.clone(),
        versions.clone(),
        2,
        &stale_authorization,
        &stale,
    )
    .is_err());

    let moved = apply_control_changes(authoritative.clone(), versions, 2, &authorization, &changes)
        .expect("a same-tenant collection move migrates every dependent scope");
    assert!(moved
        .snapshot
        .role_bindings
        .iter()
        .any(|binding| binding.scope == new_scope));
    for policy in &moved.snapshot.path_policies {
        assert_eq!(policy.scope.as_ref(), Some(&new_scope));
        assert!(policy
            .patterns
            .iter()
            .all(|pattern| pattern.starts_with(new_prefix)));
    }
    assert_eq!(moved.snapshot.tombstoned_resources, vec![new_scope.clone()]);
    assert!(!moved.entity_versions.contains_key(&old_key));
    assert_eq!(
        moved.entity_versions.get(&new_key).map(String::as_str),
        Some("2")
    );

    let mut restored_collection = moved.snapshot.config.collections[0].clone();
    restored_collection.catalog = "catalog-a".to_string();
    let restore = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("2".to_string()),
            operation: ControlOperation::PutCollection(restored_collection),
        }],
    };
    let moved_versioned =
        VersionedControlSnapshot::new(moved.snapshot.clone(), 2, moved.entity_versions.clone())
            .unwrap();
    let restore_authorization = route_authorization(
        &actor,
        &moved_versioned,
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        "/_control/v1/tenants/tenant-a/collection-moves",
        &restore,
        "move-back",
    );
    let restored = apply_control_changes(
        moved.snapshot,
        moved.entity_versions,
        3,
        &restore_authorization,
        &restore,
    )
    .expect("moving back restores dependent scopes and paths atomically");
    assert!(restored
        .snapshot
        .role_bindings
        .iter()
        .any(|binding| binding.scope == old_scope));
    for policy in &restored.snapshot.path_policies {
        assert_eq!(policy.scope.as_ref(), Some(&old_scope));
        assert!(policy
            .patterns
            .iter()
            .all(|pattern| pattern.starts_with(old_prefix)));
    }
    assert_eq!(restored.snapshot.tombstoned_resources, vec![old_scope]);
}
