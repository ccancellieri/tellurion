//! `harvest stac` (`#191`) refuses a request it cannot serve *before* it
//! touches the network or the database. That ordering is the contract these
//! tests pin: every case below runs with a connection-string environment
//! variable that is deliberately unset and a source host that does not
//! resolve, so a refusal that names the argument proves nothing was fetched
//! and nothing was connected to first.

use std::process::Command;

fn harvest(extra: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion-ingest"));
    command
        .args(["harvest", "stac"])
        .args(["--tenant", "acme", "--catalog", "default"])
        .args(["--database-url-env", "TELLURION_HARVEST_CLI_TEST_UNSET"])
        .args(extra);
    command.env_remove("TELLURION_HARVEST_CLI_TEST_UNSET");
    command.output().expect("runs the ingest binary")
}

fn refusal(extra: &[&str]) -> String {
    let output = harvest(extra);
    assert!(
        !output.status.success(),
        "harvest unexpectedly succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stderr).expect("stderr is UTF-8")
}

#[test]
fn a_non_http_source_is_refused_before_anything_is_fetched_or_connected() {
    let stderr = refusal(&["file:///tmp/catalog"]);
    assert!(
        stderr.contains("http(s) STAC API root"),
        "expected a named source refusal: {stderr}"
    );
}

#[test]
fn a_zero_max_items_is_refused_rather_than_read_as_harvest_everything() {
    let stderr = refusal(&["https://harvest.invalid/stac", "--max-items", "0"]);
    assert!(
        stderr.contains("--max-items must be at least 1"),
        "expected a named cap refusal: {stderr}"
    );
}

#[test]
fn a_map_entry_for_an_unrequested_collection_is_refused() {
    let stderr = refusal(&[
        "https://harvest.invalid/stac",
        "--collections",
        "a",
        "--map",
        "b=c",
    ]);
    assert!(
        stderr.contains("which --collections does not request"),
        "expected a named mapping refusal: {stderr}"
    );
}

#[test]
fn a_bookmark_from_another_source_is_refused_rather_than_resumed() {
    let directory = tempfile::tempdir().expect("creates a temp directory");
    let bookmark = directory.path().join("harvest.bookmark");
    std::fs::write(
        &bookmark,
        serde_json::json!({
            "source": "https://elsewhere.invalid/stac",
            "tenant": "acme",
            "catalog": "default",
            "collections": {}
        })
        .to_string(),
    )
    .expect("writes the bookmark");

    let stderr = refusal(&[
        "https://harvest.invalid/stac",
        "--bookmark",
        bookmark.to_str().expect("temp path is UTF-8"),
    ]);
    assert!(
        stderr.contains("bookmark was written for source"),
        "expected a named bookmark refusal: {stderr}"
    );
}

/// The connection string is read before the first fetch, so a harvest never
/// walks a remote catalog it could not possibly write the result of.
#[test]
fn a_missing_connection_string_is_named_before_the_catalog_is_walked() {
    let stderr = refusal(&["https://harvest.invalid/stac"]);
    assert!(
        stderr.contains("TELLURION_HARVEST_CLI_TEST_UNSET' is not set"),
        "expected a named environment refusal: {stderr}"
    );
}
