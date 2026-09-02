use deadpool_postgres::Pool;
use tellurion_core::{Error, Result};

const SCHEMA_VERSION: i64 = 1;
const MIGRATION_V1: &str = include_str!("../migrations/001_control_store.sql");

const TABLES: &[&str] = &[
    "control_revisions",
    "control_state",
    "control_resources",
    "control_role_bindings",
    "control_path_policies",
    "control_tombstones",
    "control_entity_versions",
    "control_audit",
    "control_outbox",
    "control_idempotency",
];

pub(crate) fn validate_name(schema: &str) -> Result<()> {
    let mut characters = schema.chars();
    let valid_first = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    if !valid_first
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
        || schema.len() > 63
    {
        return Err(Error::Config(format!(
            "invalid PostgreSQL control-store schema name '{schema}'"
        )));
    }
    Ok(())
}

pub(crate) fn quoted(schema: &str) -> String {
    format!("\"{schema}\"")
}

pub(crate) async fn migrate(pool: &Pool, schema: &str) -> Result<()> {
    validate_name(schema)?;
    let qualified = quoted(schema);
    let mut client = pool.get().await.map_err(storage)?;
    let transaction = client.transaction().await.map_err(storage)?;
    let lock_key = format!("tellurion-control-schema:{schema}");
    transaction
        .execute(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            &[&lock_key],
        )
        .await
        .map_err(storage)?;
    transaction
        .batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {qualified}"))
        .await
        .map_err(storage)?;

    let marker = format!("{schema}.control_schema");
    let exists: bool = transaction
        .query_one("SELECT to_regclass($1) IS NOT NULL", &[&marker])
        .await
        .map_err(storage)?
        .get(0);
    if !exists {
        transaction
            .batch_execute(&MIGRATION_V1.replace("{{schema}}", &qualified))
            .await
            .map_err(storage)?;
    }

    let version: i64 = transaction
        .query_one(
            &format!("SELECT version FROM {qualified}.control_schema WHERE singleton = TRUE"),
            &[],
        )
        .await
        .map_err(storage)?
        .get(0);
    if version != SCHEMA_VERSION {
        return Err(Error::Config(format!(
            "unsupported PostgreSQL control-store schema version {version}; this binary supports version {SCHEMA_VERSION}"
        )));
    }
    for table in TABLES {
        let name = format!("{schema}.{table}");
        let exists: bool = transaction
            .query_one("SELECT to_regclass($1) IS NOT NULL", &[&name])
            .await
            .map_err(storage)?
            .get(0);
        if !exists {
            return Err(Error::Config(format!(
                "PostgreSQL control-store schema version {SCHEMA_VERSION} is incomplete: missing table '{table}'"
            )));
        }
    }
    transaction.commit().await.map_err(storage)?;
    Ok(())
}

pub(crate) fn storage<E>(error: E) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    Error::Storage(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_names_are_restricted_to_safe_postgresql_identifiers() {
        for valid in ["tellurion_control", "_private", "tenant42"] {
            validate_name(valid).unwrap();
        }
        for invalid in ["", "42tenant", "public; DROP SCHEMA public", "has-dash"] {
            assert!(matches!(validate_name(invalid), Err(Error::Config(_))));
        }
    }
}
