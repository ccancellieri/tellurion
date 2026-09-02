use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use tellurion_core::{Error, Result};

const SCHEMA_VERSION: i64 = 1;
const MIGRATION_V1: &str = include_str!("../migrations/001_control_store.sql");

pub(crate) fn open(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path).map_err(storage)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(storage)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(storage)?;
    migrate(&connection)?;
    Ok(connection)
}

fn migrate(connection: &Connection) -> Result<()> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(storage)?;
    match version {
        0 => connection.execute_batch(MIGRATION_V1).map_err(storage)?,
        SCHEMA_VERSION => {}
        unsupported => {
            return Err(Error::Config(format!(
                "unsupported SQLite control-store schema version {unsupported}; this binary supports version {SCHEMA_VERSION}"
            )))
        }
    }

    let recorded: Option<i64> = connection
        .query_row(
            "SELECT version FROM control_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            Error::Config(format!(
                "SQLite control-store schema version {SCHEMA_VERSION} is incomplete: {error}"
            ))
        })?;
    if recorded != Some(SCHEMA_VERSION) {
        return Err(Error::Config(format!(
            "SQLite control-store schema marker is invalid: expected {SCHEMA_VERSION}, found {recorded:?}"
        )));
    }
    for table in [
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
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !exists {
            return Err(Error::Config(format!(
                "SQLite control-store schema version {SCHEMA_VERSION} is incomplete: missing table '{table}'"
            )));
        }
    }
    Ok(())
}

pub(crate) fn storage(error: rusqlite::Error) -> Error {
    Error::Storage(Box::new(error))
}
