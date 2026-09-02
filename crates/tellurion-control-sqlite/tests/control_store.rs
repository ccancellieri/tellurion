use std::sync::Arc;

use tellurion_control_sqlite::SqliteControlStore;
use tellurion_core::{
    assert_control_bootstrap_contract, assert_control_store_contract, authorize_control_mutation,
    AuditRequestContext, AuthenticatedSubject, BootEnvelope, BootstrapOutcome,
    ControlBootstrapMode, ControlChangeSet, ControlCommit, ControlOperation,
    ControlRouteDescriptor, ControlRouteRegistry, ControlScope, ControlSnapshot, ControlStore,
    Error, PathPolicy, PolicyEffect, PrincipalIdentity, RoleBinding, VersionedControlOperation,
    VersionedControlSnapshot,
};

fn actor() -> PrincipalIdentity {
    PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "sysadmin".to_string(),
    }
}

fn seed() -> ControlSnapshot {
    ControlSnapshot {
        config: serde_yaml::from_str(
            "auth:\n  trusted_issuers:\n    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }",
        )
        .unwrap(),
        role_bindings: vec![RoleBinding {
            principal: actor(),
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    }
}

fn request() -> AuditRequestContext {
    AuditRequestContext {
        method: "PUT".to_string(),
        canonical_path: "/_control/v1/platform/policies/demo".to_string(),
        correlation_id: "test-correlation".to_string(),
    }
}

fn changes(key: &str, policy_id: &str) -> ControlChangeSet {
    ControlChangeSet {
        idempotency_key: Some(key.to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutPathPolicy(PathPolicy::new(
                policy_id,
                "service_account",
                ControlScope::Platform,
                PolicyEffect::Allow,
                ["GET"],
                ["/_control/v1/platform/**"],
            )),
        }],
    }
}

fn checkpoint_authorization(
    subject: &AuthenticatedSubject,
    mut request: AuditRequestContext,
    versioned: &VersionedControlSnapshot,
    changes: &ControlChangeSet,
) -> tellurion_core::AuthorizedControlMutation {
    let route = match &changes.operations[0].operation {
        ControlOperation::PutPathPolicy(policy) => {
            request.canonical_path = format!("/_control/v1/platform/policies/{}", policy.id);
            ControlRouteDescriptor::PlatformPathPolicy
        }
        ControlOperation::ReplacePlatformSettings(_) => {
            request.method = "POST".to_string();
            request.canonical_path = "/_control/v1/platform/import".to_string();
            ControlRouteDescriptor::PlatformBatchImport
        }
        ControlOperation::PutTenant(_) => {
            request.method = "POST".to_string();
            request.canonical_path = "/_control/v1/tenants".to_string();
            ControlRouteDescriptor::Tenants
        }
        ControlOperation::PutCollection(_) => {
            request.method = "POST".to_string();
            request.canonical_path = "/_control/v1/tenants/tenant-a/collection-moves".to_string();
            ControlRouteDescriptor::TenantCollectionMove
        }
        operation => panic!("unsupported test mutation: {operation:?}"),
    };
    let registry = ControlRouteRegistry::new([route]).unwrap();
    authorize_control_mutation(
        subject,
        &request.method,
        request.canonical_path.as_bytes(),
        route.template(),
        &registry,
        "",
        versioned,
        changes,
        &request.correlation_id,
    )
    .unwrap()
}

async fn authorization(
    store: &dyn ControlStore,
    changes: &ControlChangeSet,
) -> tellurion_core::AuthorizedControlMutation {
    let versioned = store.load_snapshot().await.unwrap();
    let request = request();
    checkpoint_authorization(
        &AuthenticatedSubject {
            principal: actor(),
            claims: Default::default(),
        },
        request,
        &versioned,
        changes,
    )
}

#[tokio::test]
async fn migration_is_idempotent_and_restart_preserves_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");

    let store = SqliteControlStore::open(&path).await.unwrap();
    assert_eq!(
        store
            .bootstrap_if_empty(
                &seed(),
                &actor(),
                ControlBootstrapMode::RequireInitialSysadmin,
            )
            .await
            .unwrap(),
        BootstrapOutcome::Bootstrapped(1)
    );
    drop(store);

    let reopened = SqliteControlStore::open(&path).await.unwrap();
    assert_eq!(reopened.current_revision().await.unwrap(), Some(1));
    assert_eq!(reopened.load_snapshot().await.unwrap().snapshot, seed());
}

#[tokio::test]
async fn sqlite_rejects_first_boot_without_a_platform_sysadmin() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(directory.path().join("no-admin.sqlite"))
        .await
        .unwrap();
    let unadministrable = ControlSnapshot {
        config: tellurion_core::AppConfig::default(),
        role_bindings: Vec::new(),
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };

    assert!(store
        .bootstrap_if_empty(
            &unadministrable,
            &actor(),
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .is_err());
    assert_eq!(store.current_revision().await.unwrap(), None);
}

#[tokio::test]
async fn first_boot_persists_initial_sysadmin_and_restart_only_reports_seed_drift() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("initial-sysadmin.sqlite");
    let first_yaml = format!(
        "control_store: {{ backend: sqlite, path: {} }}\ninitial_sysadmins:\n  - {{ issuer: https://identity.example, subject: first-admin }}\nauth:\n  trusted_issuers:\n    - {{ issuer: https://identity.example, audience: tellurion-test, claims: {{ tenants: tenants }} }}\nserver: {{ port: 8081 }}",
        path.display()
    );
    let first: BootEnvelope = serde_yaml::from_str(&first_yaml).unwrap();
    first.validate().unwrap();
    let first_seed = first.seed_snapshot().unwrap();
    assert_eq!(
        first_seed.role_bindings,
        vec![RoleBinding {
            principal: PrincipalIdentity {
                issuer: "https://identity.example".to_string(),
                subject: "first-admin".to_string(),
            },
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }]
    );
    let audit_actor = PrincipalIdentity {
        issuer: "urn:tellurion:bootstrap".to_string(),
        subject: "server-startup".to_string(),
    };

    let store = SqliteControlStore::open(&path).await.unwrap();
    assert_eq!(
        store
            .bootstrap_if_empty(
                &first_seed,
                &audit_actor,
                ControlBootstrapMode::RequireInitialSysadmin,
            )
            .await
            .unwrap(),
        BootstrapOutcome::Bootstrapped(1)
    );
    drop(store);

    let changed_yaml = format!(
        "control_store: {{ backend: sqlite, path: {} }}\ninitial_sysadmins:\n  - {{ issuer: https://identity.example, subject: replacement-admin }}\nauth:\n  trusted_issuers:\n    - {{ issuer: https://identity.example, audience: tellurion-test, claims: {{ tenants: tenants }} }}\nserver: {{ port: 8082 }}",
        path.display()
    );
    let changed: BootEnvelope = serde_yaml::from_str(&changed_yaml).unwrap();
    let changed_seed = changed.seed_snapshot().unwrap();
    let reopened = SqliteControlStore::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .bootstrap_if_empty(
                &changed_seed,
                &audit_actor,
                ControlBootstrapMode::RequireInitialSysadmin,
            )
            .await
            .unwrap(),
        BootstrapOutcome::AlreadyInitialized(1)
    );
    let authoritative = reopened.load_snapshot().await.unwrap();
    assert_eq!(authoritative.snapshot, first_seed);
    assert_ne!(authoritative.snapshot, changed_seed);
    assert!(authoritative
        .snapshot
        .role_bindings
        .iter()
        .all(|binding| binding.principal != audit_actor));
}

#[tokio::test]
async fn sqlite_store_satisfies_the_backend_neutral_contract() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(directory.path().join("contract.sqlite"))
        .await
        .unwrap();
    assert_control_store_contract(Arc::new(store)).await;
}

#[tokio::test]
async fn sqlite_store_satisfies_the_shared_bootstrap_contract() {
    let directory = tempfile::tempdir().unwrap();
    let invalid = Arc::new(
        SqliteControlStore::open(directory.path().join("invalid-bootstrap.sqlite"))
            .await
            .unwrap(),
    );
    let racing = Arc::new(
        SqliteControlStore::open(directory.path().join("racing-bootstrap.sqlite"))
            .await
            .unwrap(),
    );
    let restart = Arc::new(
        SqliteControlStore::open(directory.path().join("restart-bootstrap.sqlite"))
            .await
            .unwrap(),
    );
    assert_control_bootstrap_contract(invalid, racing, restart).await;
}

#[tokio::test]
async fn sqlite_store_rejects_a_token_from_different_same_revision_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(directory.path().join("state-binding.sqlite"))
        .await
        .unwrap();
    let authoritative = seed();
    store
        .bootstrap_if_empty(
            &authoritative,
            &actor(),
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();

    let mut different = authoritative.clone();
    different.config.server.port = 9_201;
    let versioned = VersionedControlSnapshot::new(different, 1, Default::default()).unwrap();
    let request = request();
    let changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(authoritative.config),
        }],
    };
    let authorization = checkpoint_authorization(
        &AuthenticatedSubject {
            principal: actor(),
            claims: Default::default(),
        },
        request,
        &versioned,
        &changes,
    );

    let error = store.transact(&authorization, &changes).await.unwrap_err();
    assert!(error.to_string().contains("authoritative"));
}

#[tokio::test]
async fn legacy_idempotency_rows_without_replay_proof_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-idempotency.sqlite");
    let store = SqliteControlStore::open(&path).await.unwrap();
    store
        .bootstrap_if_empty(
            &seed(),
            &actor(),
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();
    let legacy_changes = changes("legacy-row", "legacy-policy");
    let authorization = authorization(&store, &legacy_changes).await;
    let legacy_commit = ControlCommit {
        revision: 2,
        changed_resources: vec!["path-policy/legacy-policy".to_string()],
        replayed: false,
    };
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO control_idempotency (idempotency_key, changeset_json, commit_json) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "legacy-row",
                serde_json::to_string(&legacy_changes).unwrap(),
                serde_json::to_string(&legacy_commit).unwrap(),
            ],
        )
        .unwrap();
    drop(connection);

    assert!(store
        .transact(&authorization, &legacy_changes)
        .await
        .is_err());
    assert_eq!(store.current_revision().await.unwrap(), Some(1));
}

#[tokio::test]
async fn concurrent_stale_writers_produce_one_named_conflict() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteControlStore::open(directory.path().join("concurrent.sqlite"))
            .await
            .unwrap(),
    );
    store
        .bootstrap_if_empty(
            &seed(),
            &actor(),
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();
    let first_changes = changes("first", "first");
    let second_changes = changes("second", "second");
    let first_authorization = authorization(store.as_ref(), &first_changes).await;
    let second_authorization = authorization(store.as_ref(), &second_changes).await;

    let first_store = Arc::clone(&store);
    let second_store = Arc::clone(&store);
    let first = tokio::spawn(async move {
        first_store
            .transact(&first_authorization, &first_changes)
            .await
    });
    let second = tokio::spawn(async move {
        second_store
            .transact(&second_authorization, &second_changes)
            .await
    });
    let (first, second) = tokio::join!(first, second);
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::ControlRevisionConflict { .. })))
            .count(),
        1
    );
}

#[tokio::test]
async fn replaying_one_idempotency_key_one_thousand_times_commits_once() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(directory.path().join("idempotency.sqlite"))
        .await
        .unwrap();
    store
        .bootstrap_if_empty(
            &seed(),
            &actor(),
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();
    let changes = changes("repeat", "repeated-policy");
    let authorization = authorization(&store, &changes).await;

    for attempt in 0..1_000 {
        let commit = store.transact(&authorization, &changes).await.unwrap();
        assert_eq!(commit.revision, 2);
        assert_eq!(commit.replayed, attempt != 0);
    }
    assert_eq!(store.current_revision().await.unwrap(), Some(2));
    assert_eq!(store.audit_since(0, 10).await.unwrap().len(), 2);
    assert_eq!(store.changes_since(None, 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn reopened_store_replays_fresh_create_and_update_proofs_with_concurrent_retries() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fresh-create-replay.sqlite");
    let store = SqliteControlStore::open(&path).await.unwrap();
    store
        .bootstrap_if_empty(
            &seed(),
            &actor(),
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();
    let create = ControlChangeSet {
        idempotency_key: Some("fresh-create".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutTenant(tellurion_core::TenantDecl {
                id: "tenant-created".to_string(),
                external_id: None,
                settings: Default::default(),
            }),
        }],
    };
    let first_authorization = authorization(&store, &create).await;
    let first = store.transact(&first_authorization, &create).await.unwrap();
    assert_eq!(first.revision, 2);
    assert!(!first.replayed);
    drop(store);

    let reopened = Arc::new(SqliteControlStore::open(&path).await.unwrap());
    let fresh_authorization = authorization(reopened.as_ref(), &create).await;
    let replay = reopened
        .transact(&fresh_authorization, &create)
        .await
        .unwrap();
    assert_eq!(replay.revision, 2);
    assert!(replay.replayed);

    let mut retries = Vec::new();
    for attempt in 0..1_000 {
        let snapshot = reopened.load_snapshot().await.unwrap();
        let mut retry_request = request();
        retry_request.correlation_id = format!("fresh-retry-{attempt}");
        let proof = checkpoint_authorization(
            &AuthenticatedSubject {
                principal: actor(),
                claims: Default::default(),
            },
            retry_request,
            &snapshot,
            &create,
        );
        let retry_store = Arc::clone(&reopened);
        let retry_changes = create.clone();
        retries.push(tokio::spawn(async move {
            retry_store.transact(&proof, &retry_changes).await
        }));
    }
    for retry in retries {
        let commit = retry.await.unwrap().unwrap();
        assert_eq!(commit.revision, 2);
        assert!(commit.replayed);
    }
    assert_eq!(reopened.current_revision().await.unwrap(), Some(2));
    assert_eq!(reopened.audit_since(0, 10).await.unwrap().len(), 2);
    assert_eq!(reopened.changes_since(None, 10).await.unwrap().len(), 2);

    let mut updated_config = reopened.load_snapshot().await.unwrap().snapshot.config;
    updated_config.server.port += 1;
    let update = ControlChangeSet {
        idempotency_key: Some("fresh-update".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(updated_config),
        }],
    };
    let first_update_authorization = authorization(reopened.as_ref(), &update).await;
    let first_update = reopened
        .transact(&first_update_authorization, &update)
        .await
        .unwrap();
    assert_eq!(first_update.revision, 3);
    assert!(!first_update.replayed);
    drop(reopened);

    let reopened = SqliteControlStore::open(&path).await.unwrap();
    let fresh_update_authorization = authorization(&reopened, &update).await;
    let update_replay = reopened
        .transact(&fresh_update_authorization, &update)
        .await
        .unwrap();
    assert_eq!(update_replay.revision, 3);
    assert!(update_replay.replayed);
    assert_eq!(reopened.current_revision().await.unwrap(), Some(3));
    assert_eq!(reopened.audit_since(0, 10).await.unwrap().len(), 3);
    assert_eq!(reopened.changes_since(None, 10).await.unwrap().len(), 3);
}

#[tokio::test]
async fn reopened_store_replays_a_reconstructed_collection_move_and_rejects_a_new_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fresh-move-replay.sqlite");
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
collections: [ { id: collection-a, catalog: catalog-a, storage: main } ]
"#,
    )
    .unwrap();
    let seed = ControlSnapshot {
        config,
        role_bindings: vec![RoleBinding {
            principal: actor(),
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    };
    let store = SqliteControlStore::open(&path).await.unwrap();
    store
        .bootstrap_if_empty(
            &seed,
            &actor(),
            ControlBootstrapMode::RequireInitialSysadmin,
        )
        .await
        .unwrap();
    let mut moved = seed.config.collections[0].clone();
    moved.catalog = "catalog-a2".to_string();
    let changes = ControlChangeSet {
        idempotency_key: Some("fresh-move".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCollection(moved),
        }],
    };
    let first_authorization = authorization(&store, &changes).await;
    let first = store
        .transact(&first_authorization, &changes)
        .await
        .unwrap();
    assert_eq!(first.revision, 2);
    assert!(!first.replayed);
    drop(store);

    let reopened = SqliteControlStore::open(&path).await.unwrap();
    let reconstructed = authorization(&reopened, &changes).await;
    let replay = reopened.transact(&reconstructed, &changes).await.unwrap();
    assert_eq!(replay.revision, 2);
    assert!(replay.replayed);

    let different_key = ControlChangeSet {
        idempotency_key: Some("fresh-move-different-key".to_string()),
        ..changes.clone()
    };
    let different_key_authorization = authorization(&reopened, &different_key).await;
    assert!(matches!(
        reopened
            .transact(&different_key_authorization, &different_key)
            .await,
        Err(Error::ControlIdempotencyAuthorizationConflict { .. })
    ));
    assert_eq!(reopened.current_revision().await.unwrap(), Some(2));
    assert_eq!(reopened.audit_since(0, 10).await.unwrap().len(), 2);
    assert_eq!(reopened.changes_since(None, 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn unsupported_or_incomplete_schema_versions_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let unsupported = directory.path().join("unsupported.sqlite");
    let connection = rusqlite::Connection::open(&unsupported).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    let error = SqliteControlStore::open(&unsupported).await.unwrap_err();
    assert!(
        matches!(error, Error::Config(message) if message.contains("unsupported SQLite control-store schema version 99"))
    );

    let incomplete = directory.path().join("incomplete.sqlite");
    let connection = rusqlite::Connection::open(&incomplete).unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    drop(connection);
    let error = SqliteControlStore::open(&incomplete).await.unwrap_err();
    assert!(
        matches!(error, Error::Config(message) if message.contains("schema version 1 is incomplete"))
    );
}
