use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use deadpool_postgres::{
    Config as PoolConfig, GenericClient, ManagerConfig, Pool, RecyclingMethod, Runtime,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tellurion_core::{
    apply_control_changes, validate_control_bootstrap_seed, AuditRequestContext, BootstrapOutcome,
    ControlAuditRecord, ControlBootstrapMode, ControlChangeSet, ControlCommit, ControlEvent,
    ControlEventCursor, ControlRevision, ControlSnapshot, ControlStore, Error, PrincipalIdentity,
    Result, VersionedControlSnapshot,
};
use tokio_postgres::NoTls;

use crate::schema;

const DEFAULT_SCHEMA: &str = "tellurion_control";

#[derive(Debug, Serialize, Deserialize)]
struct StoredIdempotencyCommit {
    commit: ControlCommit,
    #[serde(default)]
    request_fingerprint: Option<String>,
}

#[derive(Clone)]
pub struct PostgresControlStore {
    pool: Pool,
    schema: Arc<str>,
    applying_instance: Arc<str>,
}

impl std::fmt::Debug for PostgresControlStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresControlStore")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl PostgresControlStore {
    pub async fn connect(database_url: &str) -> Result<Self> {
        Self::connect_in_schema(database_url, DEFAULT_SCHEMA).await
    }

    pub async fn connect_in_schema(database_url: &str, schema_name: &str) -> Result<Self> {
        schema::validate_name(schema_name)?;
        let mut config = PoolConfig::new();
        config.url = Some(database_url.to_string());
        config.manager = Some(ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        });
        let pool = config
            .builder(NoTls)
            .map_err(schema::storage)?
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(schema::storage)?;
        schema::migrate(&pool, schema_name).await?;
        Ok(Self {
            pool,
            schema: Arc::from(schema::quoted(schema_name)),
            applying_instance: Arc::from("postgres-control-store"),
        })
    }

    fn table(&self, name: &str) -> String {
        format!("{}.{name}", self.schema)
    }
}

#[async_trait]
impl ControlStore for PostgresControlStore {
    async fn bootstrap_if_empty(
        &self,
        seed: &ControlSnapshot,
        actor: &PrincipalIdentity,
        mode: ControlBootstrapMode,
    ) -> Result<BootstrapOutcome> {
        let mut client = self.pool.get().await.map_err(schema::storage)?;
        let transaction = client.transaction().await.map_err(schema::storage)?;
        let lock_key = format!("{}:bootstrap", self.schema);
        transaction
            .execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&lock_key],
            )
            .await
            .map_err(schema::storage)?;
        if let Some(revision) = current_revision(&transaction, &self.schema).await? {
            transaction.commit().await.map_err(schema::storage)?;
            return Ok(BootstrapOutcome::AlreadyInitialized(revision));
        }
        seed.validate()?;
        validate_actor(actor)?;
        validate_control_bootstrap_seed(seed, mode)?;

        let revision = 1;
        let recorded_at = now_unix_ms();
        insert_revision(&transaction, &self.schema, revision, seed, recorded_at).await?;
        transaction
            .execute(
                &format!(
                    "INSERT INTO {} (singleton, current_revision) VALUES (TRUE, $1)",
                    self.table("control_state")
                ),
                &[&to_i64(revision)?],
            )
            .await
            .map_err(schema::storage)?;
        replace_current_resources(&transaction, &self.schema, seed).await?;
        let changed = vec!["snapshot".to_string()];
        insert_event(&transaction, &self.schema, revision, 0, &changed).await?;
        insert_audit(
            &transaction,
            &self.schema,
            revision,
            actor,
            &AuditRequestContext {
                method: "BOOTSTRAP".to_string(),
                canonical_path: "/_control/v1/platform".to_string(),
                correlation_id: "bootstrap".to_string(),
            },
            None,
            &changed,
            recorded_at,
            &self.applying_instance,
        )
        .await?;
        transaction.commit().await.map_err(schema::storage)?;
        Ok(BootstrapOutcome::Bootstrapped(revision))
    }

    async fn current_revision(&self) -> Result<Option<ControlRevision>> {
        let client = self.pool.get().await.map_err(schema::storage)?;
        current_revision(&client, &self.schema).await
    }

    async fn load_snapshot(&self) -> Result<VersionedControlSnapshot> {
        let client = self.pool.get().await.map_err(schema::storage)?;
        load_snapshot(&client, &self.schema).await
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
        let expected = authorization.snapshot_revision();
        let mut client = self.pool.get().await.map_err(schema::storage)?;
        let transaction = client.transaction().await.map_err(schema::storage)?;
        let current: Option<i64> = transaction
            .query_opt(
                &format!(
                    "SELECT current_revision FROM {} WHERE singleton = TRUE FOR UPDATE",
                    self.table("control_state")
                ),
                &[],
            )
            .await
            .map_err(schema::storage)?
            .map(|row| row.get(0));
        let current = current
            .map(to_u64)
            .transpose()?
            .ok_or(Error::ControlUninitialized)?;

        if let Some(key) = &changes.idempotency_key {
            if let Some((recorded_changes, mut recorded_commit, request_fingerprint)) =
                load_idempotency(&transaction, &self.schema, key).await?
            {
                if recorded_changes != *changes {
                    return Err(Error::ControlIdempotencyConflict { key: key.clone() });
                }
                if request_fingerprint.as_deref() != Some(authorization.request_fingerprint()) {
                    return Err(Error::ControlIdempotencyAuthorizationConflict {
                        key: key.clone(),
                    });
                }
                recorded_commit.replayed = true;
                transaction.commit().await.map_err(schema::storage)?;
                return Ok(recorded_commit);
            }
        }
        if authorization.is_replay_only() {
            return Err(Error::ControlIdempotencyAuthorizationConflict {
                key: changes.idempotency_key.clone().unwrap_or_default(),
            });
        }
        if current != expected {
            return Err(Error::ControlRevisionConflict { expected, current });
        }
        let versioned = load_snapshot(&transaction, &self.schema).await?;
        let revision = expected
            .checked_add(1)
            .ok_or_else(|| Error::ControlValidation("control revision overflow".to_string()))?;
        let applied = apply_control_changes(
            versioned.snapshot,
            versioned.entity_versions,
            revision,
            authorization,
            changes,
        )?;
        let recorded_at = now_unix_ms();
        insert_revision(
            &transaction,
            &self.schema,
            revision,
            &applied.snapshot,
            recorded_at,
        )
        .await?;
        transaction
            .execute(
                &format!(
                    "UPDATE {} SET current_revision = $1 WHERE singleton = TRUE",
                    self.table("control_state")
                ),
                &[&to_i64(revision)?],
            )
            .await
            .map_err(schema::storage)?;
        replace_current_resources(&transaction, &self.schema, &applied.snapshot).await?;
        replace_entity_versions(&transaction, &self.schema, &applied.entity_versions).await?;
        insert_event(
            &transaction,
            &self.schema,
            revision,
            0,
            &applied.changed_resources,
        )
        .await?;
        insert_audit(
            &transaction,
            &self.schema,
            revision,
            authorization.principal(),
            authorization.audit_request(),
            changes.idempotency_key.as_deref(),
            &applied.changed_resources,
            recorded_at,
            &self.applying_instance,
        )
        .await?;
        let commit = ControlCommit {
            revision,
            changed_resources: applied.changed_resources,
            replayed: false,
        };
        if let Some(key) = &changes.idempotency_key {
            let changes_json = json(changes)?;
            let commit_json = json(&StoredIdempotencyCommit {
                commit: commit.clone(),
                request_fingerprint: Some(authorization.request_fingerprint().to_string()),
            })?;
            transaction
                .execute(
                    &format!(
                        "INSERT INTO {} (idempotency_key, changeset_json, commit_json) VALUES ($1, $2, $3)",
                        self.table("control_idempotency")
                    ),
                    &[key, &changes_json, &commit_json],
                )
                .await
                .map_err(schema::storage)?;
        }
        transaction.commit().await.map_err(schema::storage)?;
        Ok(commit)
    }

    async fn changes_since(
        &self,
        after: Option<ControlEventCursor>,
        limit: u32,
    ) -> Result<Vec<ControlEvent>> {
        validate_limit(limit)?;
        let query = changes_since_query(&self.table("control_outbox"), after, limit)?;
        let client = self.pool.get().await.map_err(schema::storage)?;
        let rows = client
            .query(&query.sql, &[&query.revision, &query.ordinal, &query.limit])
            .await
            .map_err(schema::storage)?;
        rows.into_iter()
            .map(|row| {
                Ok(ControlEvent {
                    revision: to_u64(row.get(0))?,
                    ordinal: u32::try_from(row.get::<_, i32>(1))
                        .map_err(|_| corrupt("outbox ordinal"))?,
                    changed_resources: from_json(row.get(2))?,
                })
            })
            .collect()
    }

    async fn audit_since(
        &self,
        after: ControlRevision,
        limit: u32,
    ) -> Result<Vec<ControlAuditRecord>> {
        validate_limit(limit)?;
        let client = self.pool.get().await.map_err(schema::storage)?;
        let rows = client
            .query(
                &format!(
                    "SELECT revision, actor_json, request_json, idempotency_key,
                            changed_resources_json, recorded_at_unix_ms, applying_instance
                     FROM {} WHERE revision > $1 ORDER BY revision LIMIT $2",
                    self.table("control_audit")
                ),
                &[&to_i64(after)?, &i64::from(limit)],
            )
            .await
            .map_err(schema::storage)?;
        rows.into_iter()
            .map(|row| {
                Ok(ControlAuditRecord {
                    revision: to_u64(row.get(0))?,
                    actor: from_json(row.get(1))?,
                    request: from_json(row.get(2))?,
                    idempotency_key: row.get(3),
                    changed_resources: from_json(row.get(4))?,
                    recorded_at_unix_ms: to_u64(row.get(5))?,
                    applying_instance: row.get(6),
                })
            })
            .collect()
    }
}

async fn current_revision<C>(client: &C, schema: &str) -> Result<Option<ControlRevision>>
where
    C: GenericClient + Sync,
{
    client
        .query_opt(
            &format!("SELECT current_revision FROM {schema}.control_state WHERE singleton = TRUE"),
            &[],
        )
        .await
        .map_err(schema::storage)?
        .map(|row| to_u64(row.get(0)))
        .transpose()
}

async fn load_snapshot<C>(client: &C, schema: &str) -> Result<VersionedControlSnapshot>
where
    C: GenericClient + Sync,
{
    let revision = current_revision(client, schema)
        .await?
        .ok_or(Error::ControlUninitialized)?;
    let row = client
        .query_one(
            &format!("SELECT snapshot_json FROM {schema}.control_revisions WHERE revision = $1"),
            &[&to_i64(revision)?],
        )
        .await
        .map_err(schema::storage)?;
    let snapshot: ControlSnapshot = from_json(row.get(0))?;
    snapshot.validate()?;
    let rows = client
        .query(
            &format!(
                "SELECT resource_key, entity_version FROM {schema}.control_entity_versions ORDER BY resource_key"
            ),
            &[],
        )
        .await
        .map_err(schema::storage)?;
    let entity_versions = rows
        .into_iter()
        .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
        .collect();
    VersionedControlSnapshot::new(snapshot, revision, entity_versions)
}

async fn insert_revision<C>(
    client: &C,
    schema: &str,
    revision: ControlRevision,
    snapshot: &ControlSnapshot,
    recorded_at: u64,
) -> Result<()>
where
    C: GenericClient + Sync,
{
    client
        .execute(
            &format!(
                "INSERT INTO {schema}.control_revisions
                 (revision, snapshot_json, recorded_at_unix_ms) VALUES ($1, $2, $3)"
            ),
            &[&to_i64(revision)?, &json(snapshot)?, &to_i64(recorded_at)?],
        )
        .await
        .map_err(schema::storage)?;
    Ok(())
}

async fn insert_event<C>(
    client: &C,
    schema: &str,
    revision: ControlRevision,
    ordinal: u32,
    changed_resources: &[String],
) -> Result<()>
where
    C: GenericClient + Sync,
{
    client
        .execute(
            &format!(
                "INSERT INTO {schema}.control_outbox
                 (revision, ordinal, changed_resources_json) VALUES ($1, $2, $3)"
            ),
            &[
                &to_i64(revision)?,
                &i32::try_from(ordinal).map_err(|_| corrupt("outbox ordinal"))?,
                &json(changed_resources)?,
            ],
        )
        .await
        .map_err(schema::storage)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_audit<C>(
    client: &C,
    schema: &str,
    revision: ControlRevision,
    actor: &PrincipalIdentity,
    request: &AuditRequestContext,
    idempotency_key: Option<&str>,
    changed_resources: &[String],
    recorded_at: u64,
    applying_instance: &str,
) -> Result<()>
where
    C: GenericClient + Sync,
{
    client
        .execute(
            &format!(
                "INSERT INTO {schema}.control_audit
                 (revision, actor_json, request_json, idempotency_key, changed_resources_json,
                  recorded_at_unix_ms, applying_instance)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)"
            ),
            &[
                &to_i64(revision)?,
                &json(actor)?,
                &json(request)?,
                &idempotency_key,
                &json(changed_resources)?,
                &to_i64(recorded_at)?,
                &applying_instance,
            ],
        )
        .await
        .map_err(schema::storage)?;
    Ok(())
}

async fn load_idempotency<C>(
    client: &C,
    schema: &str,
    key: &str,
) -> Result<Option<(ControlChangeSet, ControlCommit, Option<String>)>>
where
    C: GenericClient + Sync,
{
    client
        .query_opt(
            &format!(
                "SELECT changeset_json, commit_json FROM {schema}.control_idempotency
                 WHERE idempotency_key = $1"
            ),
            &[&key],
        )
        .await
        .map_err(schema::storage)?
        .map(|row| {
            let changes: ControlChangeSet = from_json(row.get(0))?;
            let commit_json: Value = row.get(1);
            match serde_json::from_value::<StoredIdempotencyCommit>(commit_json.clone()) {
                Ok(stored) => Ok((changes, stored.commit, stored.request_fingerprint)),
                Err(_) => Ok((changes, from_json::<ControlCommit>(commit_json)?, None)),
            }
        })
        .transpose()
}

async fn replace_current_resources<C>(
    client: &C,
    schema: &str,
    snapshot: &ControlSnapshot,
) -> Result<()>
where
    C: GenericClient + Sync,
{
    client
        .batch_execute(&format!(
            "DELETE FROM {schema}.control_resources;
             DELETE FROM {schema}.control_role_bindings;
             DELETE FROM {schema}.control_path_policies;
             DELETE FROM {schema}.control_tombstones;"
        ))
        .await
        .map_err(schema::storage)?;
    insert_resource(client, schema, "platform", "platform", &snapshot.config).await?;
    for tenant in &snapshot.config.tenants {
        insert_resource(
            client,
            schema,
            &format!("tenant/{}", tenant.id),
            "tenant",
            tenant,
        )
        .await?;
    }
    for catalog in &snapshot.config.catalogs {
        insert_resource(
            client,
            schema,
            &format!("tenant/{}/catalog/{}", catalog.tenant, catalog.id),
            "catalog",
            catalog,
        )
        .await?;
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
            client,
            schema,
            &format!(
                "tenant/{}/catalog/{}/collection/{}",
                catalog.tenant, catalog.id, collection.id
            ),
            "collection",
            collection,
        )
        .await?;
    }
    for role in &snapshot.config.policy.roles {
        insert_resource(
            client,
            schema,
            &format!("role/platform/{}", role.name),
            "role",
            role,
        )
        .await?;
    }
    for tenant_policy in &snapshot.config.policy.tenant_policies {
        for role in &tenant_policy.roles {
            insert_resource(
                client,
                schema,
                &format!("role/tenant/{}/{}", tenant_policy.tenant, role.name),
                "role",
                role,
            )
            .await?;
        }
    }
    for binding in &snapshot.role_bindings {
        client
            .execute(
                &format!(
                    "INSERT INTO {schema}.control_role_bindings
                     (issuer, subject, role, scope_key, binding_json)
                     VALUES ($1, $2, $3, $4, $5)"
                ),
                &[
                    &binding.principal.issuer,
                    &binding.principal.subject,
                    &binding.role,
                    &binding.scope.resource_key(),
                    &json(binding)?,
                ],
            )
            .await
            .map_err(schema::storage)?;
    }
    for policy in &snapshot.path_policies {
        client
            .execute(
                &format!(
                    "INSERT INTO {schema}.control_path_policies (policy_id, policy_json)
                     VALUES ($1, $2)"
                ),
                &[&policy.id, &json(policy)?],
            )
            .await
            .map_err(schema::storage)?;
    }
    for scope in &snapshot.tombstoned_resources {
        client
            .execute(
                &format!(
                    "INSERT INTO {schema}.control_tombstones (scope_key, scope_json)
                     VALUES ($1, $2)"
                ),
                &[&scope.resource_key(), &json(scope)?],
            )
            .await
            .map_err(schema::storage)?;
    }
    Ok(())
}

async fn insert_resource<C, T>(
    client: &C,
    schema: &str,
    key: &str,
    kind: &str,
    resource: &T,
) -> Result<()>
where
    C: GenericClient + Sync,
    T: Serialize + ?Sized,
{
    client
        .execute(
            &format!(
                "INSERT INTO {schema}.control_resources
                 (resource_key, resource_kind, resource_json) VALUES ($1, $2, $3)"
            ),
            &[&key, &kind, &json(resource)?],
        )
        .await
        .map_err(schema::storage)?;
    Ok(())
}

async fn replace_entity_versions<C>(
    client: &C,
    schema: &str,
    versions: &BTreeMap<String, String>,
) -> Result<()>
where
    C: GenericClient + Sync,
{
    client
        .execute(
            &format!("DELETE FROM {schema}.control_entity_versions"),
            &[],
        )
        .await
        .map_err(schema::storage)?;
    for (resource, version) in versions {
        client
            .execute(
                &format!(
                    "INSERT INTO {schema}.control_entity_versions
                     (resource_key, entity_version) VALUES ($1, $2)"
                ),
                &[resource, version],
            )
            .await
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

struct ChangesSinceQuery {
    sql: String,
    revision: i64,
    ordinal: i64,
    limit: i64,
}

fn changes_since_query(
    table: &str,
    after: Option<ControlEventCursor>,
    limit: u32,
) -> Result<ChangesSinceQuery> {
    let (revision, ordinal) = after
        .map(|cursor| (cursor.revision, cursor.ordinal))
        .unwrap_or((0, 0));
    Ok(ChangesSinceQuery {
        sql: format!(
            "SELECT revision, ordinal, changed_resources_json FROM {table} WHERE revision > $1 OR (revision = $1 AND ordinal::bigint > $2) ORDER BY revision, ordinal LIMIT $3"
        ),
        revision: to_i64(revision)?,
        ordinal: i64::from(ordinal),
        limit: i64::from(limit),
    })
}

fn json<T: Serialize + ?Sized>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(schema::storage)
}

fn from_json<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(schema::storage)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| corrupt("unsigned integer"))
}

fn to_u64(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| corrupt("negative integer"))
}

fn corrupt(field: &str) -> Error {
    Error::Config(format!(
        "PostgreSQL control-store contains an invalid {field}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumer_revision_cursor_uses_a_bigint_compatible_query_position() {
        let query = changes_since_query(
            "tellurion_control.control_outbox",
            Some(ControlEventCursor {
                revision: 41,
                ordinal: u32::MAX,
            }),
            1_000,
        )
        .unwrap();

        assert_eq!(query.revision, 41_i64);
        assert_eq!(query.ordinal, 4_294_967_295_i64);
        assert_eq!(query.limit, 1_000_i64);
        assert_eq!(
            query.sql,
            "SELECT revision, ordinal, changed_resources_json FROM tellurion_control.control_outbox WHERE revision > $1 OR (revision = $1 AND ordinal::bigint > $2) ORDER BY revision, ordinal LIMIT $3"
        );
    }
}
