//! `demo`: one-command composition of `geopackage create-tables` +
//! `geopackage seed`, followed by serving the result — for a user who has
//! just the two binaries this workspace builds (`tellurion-ingest`,
//! `tellurion`) and wants a running instance with no other steps.
//!
//! Provisioning and seeding call straight into this crate's own
//! `geopackage`/`geopackage_seed` modules — the same code path
//! `tellurion-ingest geopackage create-tables`/`geopackage seed` already
//! run, not a reimplementation. Serving hands off to the real `tellurion`
//! binary as a child process rather than reimplementing any of it here:
//! this crate owns DDL (`main.rs`'s own top-level doc), `tellurion` owns
//! serving, and this command only sequences the two. That split is also why
//! this stays a *runtime* sibling-binary lookup (`sibling_server_binary`)
//! rather than a Cargo dependency edge between the two binary crates in
//! either direction — the same call `tellurion-server`'s own
//! `geopackage_binary.rs` test already makes for the reverse direction (see
//! that file's "Provisioning choice" doc): a dependency added only to
//! resolve a sibling path isn't worth the coupling when the two binaries
//! are already built together (`cargo build -p tellurion -p
//! tellurion-ingest`, the README's own Quickstart invocation).
//!
//! The config served is the repository's `config/example-geopackage.yaml`
//! byte-for-byte, embedded at compile time so a deployment of just the two
//! binaries doesn't also need a source checkout on disk — the same minimal
//! embedded-first reference config the README's Quickstart already points
//! at, parameterized only by the `PORT` environment variable it already
//! documents supporting. The embedded copy lives inside this crate
//! (`config/example-geopackage.yaml` next to `src/`) because a packaged
//! crate cannot ship a file from outside its own root; the
//! `embedded_demo_config_matches_the_repository_example` test pins the two
//! files identical so they cannot drift.

use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::process::{Child, Command};

use crate::geopackage::{self, CreateTablesArgs};
use crate::geopackage_seed::{self, SeedArgs};

const DEMO_CONFIG_YAML: &str = include_str!("../config/example-geopackage.yaml");

const DEMO_TABLE: &str = "demo";
const DEMO_GEOMETRY: &str = "geom";
const DEMO_GEOMETRY_TYPE: &str = "POINT";
// Web Mercator: required for the tiles lane, matching the README's own
// Quickstart choice for the exact same table/geometry/column shape.
const DEMO_SRID: i32 = 3857;
const DEMO_CATALOG: &str = "default";
const DEMO_STORAGE: &str = "main";

pub struct DemoArgs {
    pub path: PathBuf,
    pub port: Option<u16>,
}

pub async fn run(args: DemoArgs) -> anyhow::Result<()> {
    provision_and_seed(&args.path).await?;

    let server_bin = sibling_server_binary()?;
    let config_path = write_demo_config()?;

    let mut command = Command::new(&server_bin);
    command
        .env("TELLURION_GEOPACKAGE_PATH", &args.path)
        .env("TELLURION_CONFIG", &config_path);
    if let Some(port) = args.port {
        command.env("PORT", port.to_string());
    }

    let mut child = command.spawn().with_context(|| {
        format!(
            "starting the tellurion server at '{}'",
            server_bin.display()
        )
    })?;
    tracing::info!(bin = %server_bin.display(), path = %args.path.display(), "serving the demo geopackage");

    let status = run_until_child_exits_or_signaled(&mut child).await;
    let _ = std::fs::remove_file(&config_path);
    let status = status?;

    if !status.success() {
        anyhow::bail!("tellurion server exited with {status}");
    }
    Ok(())
}

/// Runs `geopackage create-tables` + `geopackage seed` against `path`,
/// exactly as the two standalone subcommands do. Idempotent both ways
/// (`create-tables` confirms an existing table rather than erroring,
/// `seed` re-upserts the same 500 deterministic rows), so re-running `demo`
/// against an already-provisioned file is safe: it serves what's there
/// rather than refusing or duplicating data.
async fn provision_and_seed(path: &Path) -> anyhow::Result<()> {
    geopackage::create_tables(CreateTablesArgs {
        path: path.to_path_buf(),
        table: DEMO_TABLE.to_string(),
        geometry: DEMO_GEOMETRY.to_string(),
        srid: DEMO_SRID,
        geometry_type: DEMO_GEOMETRY_TYPE.to_string(),
        columns: vec![("name".to_string(), "TEXT".to_string())],
        dry_run: false,
    })
    .await?;

    geopackage_seed::run(SeedArgs {
        path: path.to_path_buf(),
        table: DEMO_TABLE.to_string(),
        catalog: DEMO_CATALOG.to_string(),
        storage: DEMO_STORAGE.to_string(),
    })
    .await
}

/// Locates the `tellurion` server binary next to this one — see this
/// module's own top-level doc for why that stays a runtime lookup.
fn sibling_server_binary() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("resolving this binary's own path")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("'{}' has no parent directory", exe.display()))?;
    let candidate = dir.join(format!("tellurion{}", std::env::consts::EXE_SUFFIX));
    if !candidate.is_file() {
        anyhow::bail!(
            "expected the tellurion server binary at '{}'; build both together with \
             `cargo build -p tellurion -p tellurion-ingest`",
            candidate.display()
        );
    }
    Ok(candidate)
}

/// Writes the embedded `example-geopackage.yaml` content to a process-local
/// temp file and returns its path — `TELLURION_CONFIG` only ever accepts a
/// file path, not inline content.
fn write_demo_config() -> anyhow::Result<PathBuf> {
    let mut config_path = std::env::temp_dir();
    config_path.push(format!("tellurion-demo-config-{}.yaml", std::process::id()));
    std::fs::write(&config_path, DEMO_CONFIG_YAML).with_context(|| {
        format!(
            "writing the embedded demo config to '{}'",
            config_path.display()
        )
    })?;
    Ok(config_path)
}

/// Waits for the server child to exit on its own, or forwards SIGINT/SIGTERM
/// to it the moment this process receives one itself, then waits for the
/// real exit — an operator's Ctrl-C already reaches both processes directly
/// (child processes share their parent's process group unless told
/// otherwise), but a supervisor that signals only this PID (a test harness,
/// systemd, `kill <pid>`) would otherwise leave the server orphaned.
async fn run_until_child_exits_or_signaled(
    child: &mut Child,
) -> anyhow::Result<std::process::ExitStatus> {
    tokio::select! {
        status = child.wait() => status.context("waiting on the tellurion server process"),
        _ = wait_for_shutdown_signal() => {
            tracing::info!("forwarding shutdown to the tellurion server");
            forward_shutdown(child).await?;
            child
                .wait()
                .await
                .context("waiting on the tellurion server process after shutdown")
        }
    }
}

async fn wait_for_shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(unix)]
async fn forward_shutdown(child: &Child) -> anyhow::Result<()> {
    let Some(pid) = child.id() else {
        return Ok(()); // already exited
    };
    Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .await
        .context("forwarding shutdown to the tellurion server process")?;
    Ok(())
}

#[cfg(not(unix))]
async fn forward_shutdown(child: &mut Child) -> anyhow::Result<()> {
    child
        .kill()
        .await
        .context("forwarding shutdown to the tellurion server process")
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tellurion_core::BootEnvelope;

    /// The crate-local config copy exists only because a packaged crate
    /// cannot `include_str!` a file outside its own root. The repository's
    /// `config/example-geopackage.yaml` stays the canonical example the
    /// README points at; this pins the embedded copy to it byte-for-byte.
    #[test]
    fn embedded_demo_config_matches_the_repository_example() {
        let canonical = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/example-geopackage.yaml"
        );
        let Ok(canonical_content) = std::fs::read_to_string(canonical) else {
            // Outside the workspace (packaged source), the canonical file
            // is not present; the embedded copy is then the only copy.
            return;
        };
        assert_eq!(
            super::DEMO_CONFIG_YAML,
            canonical_content,
            "crate-local config copy drifted from config/example-geopackage.yaml — re-copy it"
        );
    }

    #[test]
    fn embedded_quickstart_is_explicitly_anonymous() {
        let envelope: BootEnvelope = serde_yaml::from_str(super::DEMO_CONFIG_YAML).unwrap();
        assert!(envelope.allow_empty_platform);
        assert!(envelope.initial_sysadmins.is_empty());
        assert!(envelope.seed.auth.oidc.is_none());
        assert!(envelope.seed.auth.trusted_issuers.is_empty());
        assert!(envelope.seed.auth.bearer_tokens.is_empty());
        envelope.validate_initial_seed().unwrap();
    }

    #[tokio::test]
    async fn provision_and_seed_can_be_repeated_without_duplicate_features_or_rtree_rows() {
        let dir = tempfile::tempdir().expect("creates a temporary directory");
        let path = dir.path().join("demo.gpkg");

        super::provision_and_seed(&path)
            .await
            .expect("first demo provisioning and seed succeeds");
        super::provision_and_seed(&path)
            .await
            .expect("repeated demo provisioning and seed succeeds");

        let conn = Connection::open(&path).expect("opens the seeded GeoPackage");
        let feature_count: i64 = conn
            .query_row("SELECT count(*) FROM demo", [], |row| row.get(0))
            .expect("counts seeded features");
        let rtree_count: i64 = conn
            .query_row("SELECT count(*) FROM rtree_demo_geom", [], |row| row.get(0))
            .expect("counts R*Tree entries");

        assert_eq!(feature_count, 500);
        assert_eq!(rtree_count, feature_count);
    }
}
