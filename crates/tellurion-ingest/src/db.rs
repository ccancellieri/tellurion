//! Direct `tokio-postgres` connections. Ingest talks to Postgres itself (no
//! pool, no server-side abstractions) because it is a one-shot CLI, not a
//! long-lived service.

use tokio_postgres::{Client, NoTls};

/// Reads the connection string out of the named environment variable. The
/// variable name (not the secret) is what callers pass on the command line.
pub fn read_url(database_url_env: &str) -> anyhow::Result<String> {
    std::env::var(database_url_env)
        .map_err(|_| anyhow::anyhow!("environment variable '{database_url_env}' is not set"))
}

pub async fn connect(database_url_env: &str) -> anyhow::Result<Client> {
    let url = read_url(database_url_env)?;
    connect_url(&url).await
}

pub async fn connect_url(url: &str) -> anyhow::Result<Client> {
    let (client, connection) = tokio_postgres::connect(url, NoTls).await?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::error!(%err, "postgres connection terminated");
        }
    });
    Ok(client)
}
