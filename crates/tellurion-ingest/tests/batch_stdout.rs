use std::process::Command;

#[test]
fn batch_load_stdout_remains_ndjson_when_info_logging_is_enabled() {
    let temp = tempfile::tempdir().expect("creates temp directory");
    let gpkg = temp.path().join("batch.gpkg");
    let source = temp.path().join("features.geojsons");
    std::fs::write(
        &source,
        b"\x1e{\"type\":\"Feature\",\"id\":\"1\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[10,20]},\"properties\":{}}\n",
    )
    .expect("writes source sequence");
    let provisioned = Command::new(env!("CARGO_BIN_EXE_tellurion-ingest"))
        .args(["geopackage", "create-tables", "--path"])
        .arg(&gpkg)
        .args(["--table", "demo", "--geometry-type", "POINT"])
        .output()
        .expect("runs geopackage provisioning");
    assert!(
        provisioned.status.success(),
        "provisioning failed: {}",
        String::from_utf8_lossy(&provisioned.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_tellurion-ingest"))
        .args(["geopackage", "load", "--path"])
        .arg(&gpkg)
        .args(["--table", "demo"])
        .arg(&source)
        .env("RUST_LOG", "info")
        .output()
        .expect("runs batch load");
    assert!(
        output.status.success(),
        "load failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("every stdout line is NDJSON"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["type"], "applied");
    assert_eq!(lines[1]["type"], "summary");

    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(
        stderr.contains("starting batch load"),
        "RUST_LOG=info must exercise the tracing writer: {stderr}"
    );
}
