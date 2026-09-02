//! `variants materialize` (`#201`), end to end through the real binary,
//! against the one thing the issue's own acceptance criterion names: a
//! collection whose declared `geometry_variants` column does not exist yet
//! is refused by `Router::validate_catalog`, and running this subcommand is
//! what makes that same config boot.
//!
//! The check being exercised (`router::refuse_invalid_geometry_variants`) is
//! driver-neutral and reads whatever the storage's own `CatalogSource`
//! reports, so proving it against the GeoPackage driver proves the shape for
//! both — and GeoPackage is the arm with the genuinely interesting
//! precondition, since a `.gpkg` provisioned to spec cannot register a
//! second geometry column at all until this command relaxes exactly one
//! constraint (see `variants.rs`'s own doc).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use tellurion_core::{AppConfig, Registry, Router as CoreRouter};
use tellurion_geopackage::GeopackageDriverFactory;

fn ingest(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tellurion-ingest"))
        .args(args)
        .output()
        .expect("runs the ingest binary")
}

fn succeed(args: &[&str]) -> String {
    let output = ingest(args);
    assert!(
        output.status.success(),
        "'{}' failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is UTF-8")
}

/// The config both halves of the acceptance check load: one GeoPackage
/// storage, one collection, one declared variant covering zooms 0-6.
fn write_config(dir: &Path, gpkg: &Path, env_var: &str) -> std::path::PathBuf {
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        format!(
            r#"
storages: [ {{ id: main, driver: geopackage, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    srid: 3857
    tiles: {{ minzoom: 0, maxzoom: 14 }}
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 6
"#
        ),
    )
    .unwrap();
    // The driver resolves the file through the storage's own `url_env`, and
    // so does the subcommand — one variable, read the same way by both.
    std::env::set_var(env_var, gpkg);
    path
}

async fn validate(config_path: &Path) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(config_path).unwrap();
    let config: AppConfig = serde_yaml::from_str(&text).unwrap();
    config.validate().unwrap();
    let mut registry = Registry::new();
    registry.register(Arc::new(GeopackageDriverFactory::new()));
    let router = CoreRouter::build(&config, &registry).unwrap();
    router.validate_catalog().await.map_err(|err| err.into())
}

#[tokio::test]
async fn materializing_a_declared_variant_turns_a_refused_config_into_a_booting_one() {
    let dir = tempfile::tempdir().unwrap();
    let gpkg = dir.path().join("variants_cli.gpkg");
    let env_var = "TELLURION_VARIANTS_CLI_TEST_GPKG";

    succeed(&[
        "geopackage",
        "create-tables",
        "--path",
        gpkg.to_str().unwrap(),
        "--table",
        "demo",
        "--srid",
        "3857",
        "--geometry-type",
        "POINT",
        "--columns",
        "name:TEXT,observed_at:DATETIME",
    ]);
    // One real feature, so the populate pass has something to rewrite.
    succeed(&[
        "geopackage",
        "seed",
        "--path",
        gpkg.to_str().unwrap(),
        "--table",
        "demo",
    ]);

    let config = write_config(dir.path(), &gpkg, env_var);

    // Before: the declared column does not exist, so boot refuses the
    // config by name — the exact state `#201` exists to get an operator out
    // of.
    let refused = validate(&config)
        .await
        .expect_err("a declared but missing variant column must fail boot validation");
    let refused = format!("{refused}");
    assert!(
        refused.contains("geom_z6") && refused.contains("does not report"),
        "expected a named missing-variant refusal, got: {refused}"
    );

    // A dry run prints the plan and changes nothing: the config is still
    // refused afterwards.
    let printed = succeed(&[
        "variants",
        "materialize",
        "--config",
        config.to_str().unwrap(),
        "--collection",
        "demo",
        "--dry-run",
    ]);
    assert!(
        printed.contains("ALTER TABLE \"demo\" ADD COLUMN \"geom_z6\""),
        "dry run should print its DDL, got: {printed}"
    );
    assert!(
        validate(&config).await.is_err(),
        "a dry run changes nothing"
    );

    // Without consent, the GeoPackage spec's one-geometry-column-per-table
    // constraint is reported rather than quietly worked around.
    let output = ingest(&[
        "variants",
        "materialize",
        "--config",
        config.to_str().unwrap(),
        "--collection",
        "demo",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("uk_gc_table_name") && stderr.contains("--allow-second-geometry-column"),
        "expected the constraint refusal to explain itself, got: {stderr}"
    );

    // After: the column exists, is registered, and is populated.
    let materialize = [
        "variants",
        "materialize",
        "--config",
        config.to_str().unwrap(),
        "--collection",
        "demo",
        "--allow-second-geometry-column",
    ];
    succeed(&materialize);
    validate(&config)
        .await
        .expect("the materialized variant column makes the same config boot");

    // Rerunning is idempotent — still one column, still one registration
    // row, still a booting config.
    succeed(&materialize);
    validate(&config)
        .await
        .expect("rerunning the subcommand keeps the config valid");

    std::env::remove_var(env_var);
}
