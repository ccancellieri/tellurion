use std::sync::Arc;

use tellurion_core::{
    initialize_control_store_with_mode, BootEnvelope, ControlStartup, ControlStore,
    ControlStoreLocator, Error, PrincipalIdentity, Result,
};

pub struct OpenedControlStore {
    pub store: Arc<dyn ControlStore>,
    pub startup: ControlStartup,
}

pub async fn open_control_store(envelope: &BootEnvelope) -> Result<Arc<dyn ControlStore>> {
    match &envelope.control_store {
        ControlStoreLocator::LegacyFile => Err(Error::Config(
            "legacy_file uses FileConfigStore and cannot be opened as a dynamic control store"
                .to_string(),
        )),
        ControlStoreLocator::Sqlite { path, .. } => {
            #[cfg(feature = "control-sqlite")]
            {
                let store = tellurion_control_sqlite::SqliteControlStore::open(path).await?;
                Ok(Arc::new(store))
            }
            #[cfg(not(feature = "control-sqlite"))]
            {
                let _ = path;
                Err(Error::Config(
                    "control_store selects sqlite, but this binary was built without the `control-sqlite` feature"
                        .to_string(),
                ))
            }
        }
        ControlStoreLocator::Postgres { url_env, .. } => {
            #[cfg(feature = "control-postgres")]
            {
                let database_url = std::env::var(url_env).map_err(|_| {
                    Error::Config(format!(
                        "control_store.url_env names '{url_env}', but that environment variable is not set"
                    ))
                })?;
                let store =
                    tellurion_control_postgres::PostgresControlStore::connect(&database_url)
                        .await?;
                Ok(Arc::new(store))
            }
            #[cfg(not(feature = "control-postgres"))]
            {
                let _ = url_env;
                Err(Error::Config(
                    "control_store selects postgres, but this binary was built without the `control-postgres` feature"
                        .to_string(),
                ))
            }
        }
    }
}

pub async fn open_and_initialize(envelope: &BootEnvelope) -> Result<OpenedControlStore> {
    envelope.validate_locator()?;
    let store = open_control_store(envelope).await?;
    let seed = envelope.seed_snapshot();
    let actor = PrincipalIdentity {
        issuer: "urn:tellurion:bootstrap".to_string(),
        subject: "server-startup".to_string(),
    };
    let startup = initialize_control_store_with_mode(
        store.as_ref(),
        seed.as_ref(),
        &actor,
        envelope.bootstrap_mode(),
    )
    .await?;
    Ok(OpenedControlStore { store, startup })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn legacy_mode_is_never_opened_as_a_dynamic_store() {
        let envelope: BootEnvelope =
            serde_yaml::from_str("control_store: { backend: legacy_file }\nserver: { port: 8080 }")
                .unwrap();
        assert!(matches!(
            open_control_store(&envelope).await,
            Err(Error::Config(message)) if message.contains("legacy_file")
        ));
    }

    #[cfg(feature = "control-sqlite")]
    #[tokio::test]
    async fn restart_ignores_invalid_changed_seed_and_duplicate_initial_admins() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restart-invalid-seed.sqlite");
        let valid: BootEnvelope = serde_yaml::from_str(&format!(
            r#"
control_store: {{ backend: sqlite, path: {} }}
initial_sysadmins:
  - {{ issuer: https://issuer.example, subject: operator }}
auth:
  trusted_issuers:
    - {{ issuer: https://issuer.example, audience: tellurion-test, claims: {{ tenants: tenants }} }}
server: {{ port: 8081 }}
"#,
            path.display()
        ))
        .unwrap();
        let mut invalid_first_boot = valid.clone();
        invalid_first_boot.seed.server.max_concurrency = Some(0);
        invalid_first_boot
            .initial_sysadmins
            .push(invalid_first_boot.initial_sysadmins[0].clone());
        assert!(open_and_initialize(&invalid_first_boot).await.is_err());
        let still_empty = open_control_store(&valid).await.unwrap();
        assert_eq!(still_empty.current_revision().await.unwrap(), None);
        drop(still_empty);

        let first = open_and_initialize(&valid).await.unwrap();
        assert_eq!(first.startup.authoritative.revision, 1);
        drop(first);

        let mut changed = valid.clone();
        changed.seed.server.max_concurrency = Some(0);
        changed
            .initial_sysadmins
            .push(changed.initial_sysadmins[0].clone());
        let restarted = open_and_initialize(&changed).await.unwrap();
        assert_eq!(restarted.startup.authoritative.revision, 1);
        assert_eq!(
            restarted.startup.authoritative.snapshot.config.server.port,
            8081
        );
    }

    #[cfg(feature = "control-sqlite")]
    #[tokio::test]
    async fn restart_rejects_inline_bearer_token_before_accepting_seed_drift() {
        const SECRET: &str = "restart-inline-secret";
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restart-inline-token.sqlite");
        let valid: BootEnvelope = serde_yaml::from_str(&format!(
            r#"
control_store: {{ backend: sqlite, path: {} }}
initial_sysadmins:
  - {{ issuer: https://issuer.example, subject: operator }}
tenants: [{{ id: tenant-a }}]
auth:
  trusted_issuers:
    - {{ issuer: https://issuer.example, audience: tellurion-test, claims: {{ tenants: tenants }} }}
  bearer_tokens:
    - {{ token_env: TELLURION_TENANT_A_TOKEN, tenants: [tenant-a] }}
"#,
            path.display()
        ))
        .unwrap();

        let first = open_and_initialize(&valid).await.unwrap();
        assert_eq!(first.startup.authoritative.revision, 1);
        drop(first);

        let mut inline = valid.clone();
        inline.seed.auth.bearer_tokens[0].token_env = None;
        inline.seed.auth.bearer_tokens[0].token = SECRET.to_string();
        let error = match open_and_initialize(&inline).await {
            Ok(_) => panic!("durable restart must reject an inline bearer-token value"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("token_env"));
        assert!(!error.to_string().contains(SECRET));

        let reopened = open_control_store(&valid).await.unwrap();
        assert_eq!(reopened.current_revision().await.unwrap(), Some(1));
    }

    #[cfg(not(feature = "control-postgres"))]
    #[tokio::test]
    async fn compiled_out_postgres_backend_fails_by_name_without_reading_a_secret() {
        let envelope: BootEnvelope = serde_yaml::from_str(
            "control_store: { backend: postgres, url_env: MUST_NOT_EXIST }\nserver: { port: 8080 }",
        )
        .unwrap();
        assert!(matches!(
            open_control_store(&envelope).await,
            Err(Error::Config(message)) if message.contains("control-postgres")
        ));
    }
}
