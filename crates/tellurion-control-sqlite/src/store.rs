use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use tellurion_core::{
    apply_control_changes, validate_control_bootstrap_seed, AuditRequestContext, BootstrapOutcome,
    ControlAuditRecord, ControlBootstrapMode, ControlChangeSet, ControlCommit, ControlEvent,
    ControlEventCursor, ControlRevision, ControlSnapshot, ControlStore, Error, PrincipalIdentity,
    Result, VersionedControlSnapshot,
};

use crate::schema;

#[derive(Debug, Serialize, Deserialize)]
struct StoredIdempotencyCommit {
    commit: ControlCommit,
    #[serde(default)]
    request_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SqliteControlStore {
    path: Arc<PathBuf>,
    applying_instance: Arc<str>,
}

impl SqliteControlStore {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let opened_path = path.clone();
        tokio::task::spawn_blocking(move || schema::open(&opened_path).map(|_| ()))
            .await
            .map_err(join_error)??;
        Ok(Self {
            path: Arc::new(path),
            applying_instance: Arc::from("sqlite-control-store"),
        })
    }

    async fn with_connection<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            let mut connection = schema::open(&path)?;
            work(&mut connection)
        })
        .await
        .map_err(join_error)?
    }
}

#[async_trait]
impl ControlStore for SqliteControlStore {
    async fn bootstrap_if_empty(
        &self,
        seed: &ControlSnapshot,
        actor: &PrincipalIdentity,
        mode: ControlBootstrapMode,
    ) -> Result<BootstrapOutcome> {
        let seed = seed.clone();
        let actor = actor.clone();
        let applying_instance = self.applying_instance.to_string();
        self.with_connection(move |connection| {
            let transaction = immediate(connection)?;
            if let Some(revision) = current_revision(&transaction)? {
                transaction.commit().map_err(schema::storage)?;
                return Ok(BootstrapOutcome::AlreadyInitialized(revision));
            }
            seed.validate()?;
            validate_actor(&actor)?;
            validate_control_bootstrap_seed(&seed, mode)?;

            let revision = 1;
            let recorded_at = now_unix_ms();
            insert_revision(&transaction, revision, &seed, recorded_at)?;
            transaction
                .execute(
                    "INSERT INTO control_state (singleton, current_revision) VALUES (1, ?1)",
                    [to_i64(revision)?],
                )
                .map_err(schema::storage)?;
            replace_current_resources(&transaction, &seed)?;
            insert_event(&transaction, revision, 0, &["snapshot".to_string()])?;
            insert_audit(
                &transaction,
                revision,
                &actor,
                &AuditRequestContext {
                    method: "BOOTSTRAP".to_string(),
                    canonical_path: "/_control/v1/platform".to_string(),
                    correlation_id: "bootstrap".to_string(),
                },
                None,
                &["snapshot".to_string()],
                recorded_at,
                &applying_instance,
            )?;
            transaction.commit().map_err(schema::storage)?;
            Ok(BootstrapOutcome::Bootstrapped(revision))
        })
        .await
    }

    async fn current_revision(&self) -> Result<Option<ControlRevision>> {
        self.with_connection(|connection| current_revision(connection))
            .await
    }

    async fn load_snapshot(&self) -> Result<VersionedControlSnapshot> {
        self.with_connection(|connection| load_snapshot(connection))
            .await
    }

    async fn transact(
        &self,
        authorization: &tellurion_core::AuthorizedControlMutation,
        changes: &ControlChangeSet,
    ) -> Result<ControlCommit> {
        validate_actor(authorization.principal())?;
        validate_request(authorization.audit_request())?;
        changes.validate()?;
        authorization.validate_intent(changes)?;
        let authorization = authorization.clone();
        let expected = authorization.snapshot_revision();
        let changes = changes.clone();
        let applying_instance = self.applying_instance.to_string();
        self.with_connection(move |connection| {
            let transaction = immediate(connection)?;

            if let Some(key) = &changes.idempotency_key {
                if let Some((recorded_changes, mut recorded_commit, request_fingerprint)) =
                    load_idempotency(&transaction, key)?
                {
                    if recorded_changes != changes {
                        return Err(Error::ControlIdempotencyConflict { key: key.clone() });
                    }
                    if request_fingerprint.as_deref()
                        != Some(authorization.request_fingerprint())
                    {
                        return Err(Error::ControlIdempotencyAuthorizationConflict {
                            key: key.clone(),
                        });
                    }
                    recorded_commit.replayed = true;
                    transaction.commit().map_err(schema::storage)?;
                    return Ok(recorded_commit);
                }
            }
            if authorization.is_replay_only() {
                return Err(Error::ControlIdempotencyAuthorizationConflict {
                    key: changes.idempotency_key.clone().unwrap_or_default(),
                });
            }

            let versioned = load_snapshot(&transaction)?;
            if versioned.revision != expected {
                return Err(Error::ControlRevisionConflict {
                    expected,
                    current: versioned.revision,
                });
            }
            let revision = expected.checked_add(1).ok_or_else(|| {
                Error::ControlValidation("control revision overflow".to_string())
            })?;
            let applied = apply_control_changes(
                versioned.snapshot,
                versioned.entity_versions,
                revision,
                &authorization,
                &changes,
            )?;
            let recorded_at = now_unix_ms();
            insert_revision(&transaction, revision, &applied.snapshot, recorded_at)?;
            transaction
                .execute(
                    "UPDATE control_state SET current_revision = ?1 WHERE singleton = 1",
                    [to_i64(revision)?],
                )
                .map_err(schema::storage)?;
            replace_current_resources(&transaction, &applied.snapshot)?;
            replace_entity_versions(&transaction, &applied.entity_versions)?;
            insert_event(&transaction, revision, 0, &applied.changed_resources)?;
            insert_audit(
                &transaction,
                revision,
                authorization.principal(),
                authorization.audit_request(),
                changes.idempotency_key.as_deref(),
                &applied.changed_resources,
                recorded_at,
                &applying_instance,
            )?;
            let commit = ControlCommit {
                revision,
                changed_resources: applied.changed_resources,
                replayed: false,
            };
            if let Some(key) = &changes.idempotency_key {
                let stored_commit = StoredIdempotencyCommit {
                    commit: commit.clone(),
                    request_fingerprint: Some(authorization.request_fingerprint().to_string()),
                };
                transaction
                    .execute(
                        "INSERT INTO control_idempotency (idempotency_key, changeset_json, commit_json) VALUES (?1, ?2, ?3)",
                        params![key, json(&changes)?, json(&stored_commit)?],
                    )
                    .map_err(schema::storage)?;
            }
            transaction.commit().map_err(schema::storage)?;
            Ok(commit)
        })
        .await
    }

    async fn changes_since(
        &self,
        after: Option<ControlEventCursor>,
        limit: u32,
    ) -> Result<Vec<ControlEvent>> {
        validate_limit(limit)?;
        self.with_connection(move |connection| {
            let (revision, ordinal) = after
                .map(|cursor| (cursor.revision, cursor.ordinal))
                .unwrap_or((0, 0));
            let mut statement = connection
                .prepare(
                    "SELECT revision, ordinal, changed_resources_json FROM control_outbox
                     WHERE revision > ?1 OR (revision = ?1 AND ordinal > ?2)
                     ORDER BY revision, ordinal LIMIT ?3",
                )
                .map_err(schema::storage)?;
            let rows = statement
                .query_map(
                    params![to_i64(revision)?, i64::from(ordinal), i64::from(limit)],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(schema::storage)?;
            let mut events = Vec::new();
            for row in rows {
                let (revision, ordinal, changed) = row.map_err(schema::storage)?;
                events.push(ControlEvent {
                    revision: to_u64(revision)?,
                    ordinal: u32::try_from(ordinal).map_err(|_| corrupt("outbox ordinal"))?,
                    changed_resources: from_json(&changed)?,
                });
            }
            Ok(events)
        })
        .await
    }

    async fn audit_since(
        &self,
        after: ControlRevision,
        limit: u32,
    ) -> Result<Vec<ControlAuditRecord>> {
        validate_limit(limit)?;
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT revision, actor_json, request_json, idempotency_key,
                            changed_resources_json, recorded_at_unix_ms, applying_instance
                     FROM control_audit WHERE revision > ?1 ORDER BY revision LIMIT ?2",
                )
                .map_err(schema::storage)?;
            let rows = statement
                .query_map(params![to_i64(after)?, i64::from(limit)], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(schema::storage)?;
            let mut records = Vec::new();
            for row in rows {
                let (revision, actor, request, key, changed, recorded_at, instance) =
                    row.map_err(schema::storage)?;
                records.push(ControlAuditRecord {
                    revision: to_u64(revision)?,
                    actor: from_json(&actor)?,
                    request: from_json(&request)?,
                    idempotency_key: key,
                    changed_resources: from_json(&changed)?,
                    recorded_at_unix_ms: to_u64(recorded_at)?,
                    applying_instance: instance,
                });
            }
            Ok(records)
        })
        .await
    }
}

fn immediate(connection: &mut Connection) -> Result<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(schema::storage)
}

fn current_revision(connection: &Connection) -> Result<Option<ControlRevision>> {
    connection
        .query_row(
            "SELECT current_revision FROM control_state WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(schema::storage)?
        .map(to_u64)
        .transpose()
}

fn load_snapshot(connection: &Connection) -> Result<VersionedControlSnapshot> {
    let revision = current_revision(connection)?.ok_or(Error::ControlUninitialized)?;
    let snapshot_json: String = connection
        .query_row(
            "SELECT snapshot_json FROM control_revisions WHERE revision = ?1",
            [to_i64(revision)?],
            |row| row.get(0),
        )
        .map_err(schema::storage)?;
    let snapshot: ControlSnapshot = from_json(&snapshot_json)?;
    snapshot.validate()?;
    let mut statement = connection
        .prepare("SELECT resource_key, entity_version FROM control_entity_versions ORDER BY resource_key")
        .map_err(schema::storage)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(schema::storage)?;
    let mut entity_versions = BTreeMap::new();
    for row in rows {
        let (resource, version) = row.map_err(schema::storage)?;
        entity_versions.insert(resource, version);
    }
    VersionedControlSnapshot::new(snapshot, revision, entity_versions)
}

fn insert_revision(
    transaction: &Transaction<'_>,
    revision: ControlRevision,
    snapshot: &ControlSnapshot,
    recorded_at: u64,
) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO control_revisions (revision, snapshot_json, recorded_at_unix_ms) VALUES (?1, ?2, ?3)",
            params![to_i64(revision)?, json(snapshot)?, to_i64(recorded_at)?],
        )
        .map_err(schema::storage)?;
    Ok(())
}

fn insert_event(
    transaction: &Transaction<'_>,
    revision: ControlRevision,
    ordinal: u32,
    changed_resources: &[String],
) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO control_outbox (revision, ordinal, changed_resources_json) VALUES (?1, ?2, ?3)",
            params![to_i64(revision)?, i64::from(ordinal), json(changed_resources)?],
        )
        .map_err(schema::storage)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_audit(
    transaction: &Transaction<'_>,
    revision: ControlRevision,
    actor: &PrincipalIdentity,
    request: &AuditRequestContext,
    idempotency_key: Option<&str>,
    changed_resources: &[String],
    recorded_at: u64,
    applying_instance: &str,
) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO control_audit
             (revision, actor_json, request_json, idempotency_key, changed_resources_json,
              recorded_at_unix_ms, applying_instance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                to_i64(revision)?,
                json(actor)?,
                json(request)?,
                idempotency_key,
                json(changed_resources)?,
                to_i64(recorded_at)?,
                applying_instance,
            ],
        )
        .map_err(schema::storage)?;
    Ok(())
}

fn load_idempotency(
    connection: &Connection,
    key: &str,
) -> Result<Option<(ControlChangeSet, ControlCommit, Option<String>)>> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT changeset_json, commit_json FROM control_idempotency WHERE idempotency_key = ?1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(schema::storage)?;
    row.map(|(changes, commit)| {
        let changes: ControlChangeSet = from_json(&changes)?;
        match from_json::<StoredIdempotencyCommit>(&commit) {
            Ok(stored) => Ok((changes, stored.commit, stored.request_fingerprint)),
            Err(_) => Ok((changes, from_json::<ControlCommit>(&commit)?, None)),
        }
    })
    .transpose()
}

fn replace_current_resources(
    transaction: &Transaction<'_>,
    snapshot: &ControlSnapshot,
) -> Result<()> {
    transaction
        .execute_batch(
            "DELETE FROM control_resources;
             DELETE FROM control_role_bindings;
             DELETE FROM control_path_policies;
             DELETE FROM control_tombstones;",
        )
        .map_err(schema::storage)?;
    insert_resource(transaction, "platform", "platform", &snapshot.config)?;
    for tenant in &snapshot.config.tenants {
        insert_resource(
            transaction,
            &format!("tenant/{}", tenant.id),
            "tenant",
            tenant,
        )?;
    }
    for catalog in &snapshot.config.catalogs {
        insert_resource(
            transaction,
            &format!("tenant/{}/catalog/{}", catalog.tenant, catalog.id),
            "catalog",
            catalog,
        )?;
    }
    for collection in &snapshot.config.collections {
        let catalog = snapshot
            .config
            .catalogs
            .iter()
            .find(|catalog| catalog.id == collection.catalog)
            .ok_or_else(|| {
                Error::ControlValidation(format!(
                    "collection '{}' references unknown catalog '{}'",
                    collection.id, collection.catalog
                ))
            })?;
        insert_resource(
            transaction,
            &format!(
                "tenant/{}/catalog/{}/collection/{}",
                catalog.tenant, catalog.id, collection.id
            ),
            "collection",
            collection,
        )?;
    }
    for role in &snapshot.config.policy.roles {
        insert_resource(
            transaction,
            &format!("role/platform/{}", role.name),
            "role",
            role,
        )?;
    }
    for tenant_policy in &snapshot.config.policy.tenant_policies {
        for role in &tenant_policy.roles {
            insert_resource(
                transaction,
                &format!("role/tenant/{}/{}", tenant_policy.tenant, role.name),
                "role",
                role,
            )?;
        }
    }
    for binding in &snapshot.role_bindings {
        transaction
            .execute(
                "INSERT INTO control_role_bindings
                 (issuer, subject, role, scope_key, binding_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    binding.principal.issuer,
                    binding.principal.subject,
                    binding.role,
                    binding.scope.resource_key(),
                    json(binding)?,
                ],
            )
            .map_err(schema::storage)?;
    }
    for policy in &snapshot.path_policies {
        transaction
            .execute(
                "INSERT INTO control_path_policies (policy_id, policy_json) VALUES (?1, ?2)",
                params![policy.id, json(policy)?],
            )
            .map_err(schema::storage)?;
    }
    for scope in &snapshot.tombstoned_resources {
        transaction
            .execute(
                "INSERT INTO control_tombstones (scope_key, scope_json) VALUES (?1, ?2)",
                params![scope.resource_key(), json(scope)?],
            )
            .map_err(schema::storage)?;
    }
    Ok(())
}

fn insert_resource<T: serde::Serialize + ?Sized>(
    transaction: &Transaction<'_>,
    key: &str,
    kind: &str,
    resource: &T,
) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO control_resources (resource_key, resource_kind, resource_json)
             VALUES (?1, ?2, ?3)",
            params![key, kind, json(resource)?],
        )
        .map_err(schema::storage)?;
    Ok(())
}

fn replace_entity_versions(
    transaction: &Transaction<'_>,
    versions: &BTreeMap<String, String>,
) -> Result<()> {
    transaction
        .execute("DELETE FROM control_entity_versions", [])
        .map_err(schema::storage)?;
    for (resource, version) in versions {
        transaction
            .execute(
                "INSERT INTO control_entity_versions (resource_key, entity_version) VALUES (?1, ?2)",
                params![resource, version],
            )
            .map_err(schema::storage)?;
    }
    Ok(())
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

fn json<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::Storage(Box::new(error)))
}

fn from_json<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|error| Error::Storage(Box::new(error)))
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| corrupt("unsigned integer"))
}

fn to_u64(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| corrupt("negative integer"))
}

fn corrupt(field: &str) -> Error {
    Error::Config(format!("SQLite control-store contains an invalid {field}"))
}

fn join_error(error: tokio::task::JoinError) -> Error {
    Error::Storage(Box::new(error))
}
