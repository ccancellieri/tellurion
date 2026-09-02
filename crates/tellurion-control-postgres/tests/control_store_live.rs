use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tellurion_control_postgres::PostgresControlStore;
use tellurion_core::{
    assert_control_bootstrap_contract, assert_control_store_contract, authorize_control_mutation,
    AuditRequestContext, AuthenticatedSubject, ControlChangeSet, ControlOperation,
    ControlRouteDescriptor, ControlRouteRegistry, ControlScope, ControlStore, Error, PathPolicy,
    PolicyEffect, PrincipalIdentity, VersionedControlOperation,
};
use tokio_postgres::NoTls;

fn unique_schema() -> String {
    static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "tellurion_control_test_{}_{}_{}",
        std::process::id(),
        timestamp,
        NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed),
    )
}

fn actor() -> PrincipalIdentity {
    PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "sysadmin".to_string(),
    }
}

fn request() -> AuditRequestContext {
    AuditRequestContext {
        method: "PUT".to_string(),
        canonical_path: "/_control/v1/platform/policies/concurrent".to_string(),
        correlation_id: "postgres-live-concurrency".to_string(),
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

async fn authorization(
    store: &dyn ControlStore,
    changes: &ControlChangeSet,
) -> tellurion_core::AuthorizedControlMutation {
    let versioned = store.load_snapshot().await.unwrap();
    let mut request = request();
    if let ControlOperation::PutPathPolicy(policy) = &changes.operations[0].operation {
        request.canonical_path = format!("/_control/v1/platform/policies/{}", policy.id);
    }
    let route = ControlRouteDescriptor::PlatformPathPolicy;
    let registry = ControlRouteRegistry::new([route]).unwrap();
    authorize_control_mutation(
        &AuthenticatedSubject {
            principal: actor(),
            claims: Default::default(),
        },
        &request.method,
        request.canonical_path.as_bytes(),
        route.template(),
        &registry,
        "",
        &versioned,
        changes,
        &request.correlation_id,
    )
    .unwrap()
}

async fn drop_schema(database_url: &str, schema: &str) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
        .await
        .unwrap();
}

async fn create_schema_version(database_url: &str, schema: &str, version: i64) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(&format!(
            "CREATE SCHEMA \"{schema}\";
             CREATE TABLE \"{schema}\".control_schema (
                 singleton BOOLEAN PRIMARY KEY CHECK (singleton),
                 version BIGINT NOT NULL
             );"
        ))
        .await
        .unwrap();
    client
        .execute(
            &format!(
                "INSERT INTO \"{schema}\".control_schema (singleton, version) VALUES (TRUE, $1)"
            ),
            &[&version],
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TELLURION_TEST_CONTROL_DATABASE_URL"]
async fn migrations_are_restart_safe_and_the_shared_contract_passes() {
    let Ok(database_url) = env::var("TELLURION_TEST_CONTROL_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL control-store test: database URL is not configured");
        return;
    };
    let schema = unique_schema();
    let first = Arc::new(
        PostgresControlStore::connect_in_schema(&database_url, &schema)
            .await
            .unwrap(),
    );
    let second = PostgresControlStore::connect_in_schema(&database_url, &schema)
        .await
        .unwrap();

    assert_control_store_contract(first.clone()).await;
    assert_eq!(second.current_revision().await.unwrap(), Some(3));
    assert_eq!(second.changes_since(None, 10).await.unwrap().len(), 3);

    let first_changes = changes("postgres-concurrent-a", "postgres-concurrent-a");
    let second_changes = changes("postgres-concurrent-b", "postgres-concurrent-b");
    let first_authorization = authorization(first.as_ref(), &first_changes).await;
    let second_authorization = authorization(first.as_ref(), &second_changes).await;
    let first_write = first.transact(&first_authorization, &first_changes);
    let second_write = second.transact(&second_authorization, &second_changes);
    let results = tokio::join!(first_write, second_write);
    assert_eq!(
        [&results.0, &results.1]
            .into_iter()
            .filter(|result| result.is_ok())
            .count(),
        1
    );
    assert_eq!(
        [&results.0, &results.1]
            .into_iter()
            .filter(|result| matches!(result, Err(Error::ControlRevisionConflict { .. })))
            .count(),
        1
    );
    assert_eq!(second.current_revision().await.unwrap(), Some(4));
    assert_eq!(second.changes_since(None, 10).await.unwrap().len(), 4);

    drop(first);
    drop(second);
    drop_schema(&database_url, &schema).await;
}

#[tokio::test]
#[ignore = "requires TELLURION_TEST_CONTROL_DATABASE_URL"]
async fn unsupported_schema_version_fails_closed_with_an_actionable_error() {
    let Ok(database_url) = env::var("TELLURION_TEST_CONTROL_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL schema-version test: database URL is not configured");
        return;
    };
    let schema = unique_schema();
    create_schema_version(&database_url, &schema, 99).await;

    let error = PostgresControlStore::connect_in_schema(&database_url, &schema)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Config(message) if message.contains("unsupported PostgreSQL control-store schema version 99"))
    );

    drop_schema(&database_url, &schema).await;
}

#[tokio::test]
#[ignore = "requires TELLURION_TEST_CONTROL_DATABASE_URL"]
async fn postgres_store_satisfies_the_shared_bootstrap_contract() {
    let Ok(database_url) = env::var("TELLURION_TEST_CONTROL_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL bootstrap test: database URL is not configured");
        return;
    };
    let schemas = [unique_schema(), unique_schema(), unique_schema()];
    let invalid = Arc::new(
        PostgresControlStore::connect_in_schema(&database_url, &schemas[0])
            .await
            .unwrap(),
    );
    let racing = Arc::new(
        PostgresControlStore::connect_in_schema(&database_url, &schemas[1])
            .await
            .unwrap(),
    );
    let restart = Arc::new(
        PostgresControlStore::connect_in_schema(&database_url, &schemas[2])
            .await
            .unwrap(),
    );

    assert_control_bootstrap_contract(invalid, racing, restart).await;
    for schema in &schemas {
        drop_schema(&database_url, schema).await;
    }
}
