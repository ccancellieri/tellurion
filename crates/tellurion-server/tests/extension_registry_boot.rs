//! The extension inventory must be available even when a selected extension
//! is not in this binary.  That inventory is part of the diagnostic for the
//! refusal, so it cannot be emitted only after the fallible cache setup.

#![cfg(not(feature = "valkey"))]

mod common;

use std::process::Command;

#[test]
fn a_compiled_out_cache_backend_still_logs_every_extension_seam() {
    let mut config_path = common::unique_temp_path("tellurion-extension-registry-boot");
    config_path.set_extension("yaml");
    std::fs::write(
        &config_path,
        common::legacy_config(
            r#"
server:
  log_json: true
cache:
  memory_percent: 10.0
  l2:
    backend: valkey
    url_env: TELLURION_EXTENSION_REGISTRY_VALKEY_URL
storages: []
"#,
        ),
    )
    .expect("writes the throwaway config");

    let output = Command::new(env!("CARGO_BIN_EXE_tellurion"))
        .env("TELLURION_CONFIG", &config_path)
        .env("RUST_LOG", "info")
        .output()
        .expect("spawns the tellurion binary");
    let _ = std::fs::remove_file(&config_path);

    assert!(
        !output.status.success(),
        "a valkey backend compiled out of this binary must fail boot"
    );
    let stdout = String::from_utf8(output.stdout).expect("the binary emits UTF-8 logs");
    for seam in [
        "extension registry: config store",
        "extension registry: storage drivers",
        "extension registry: catalog/collection registry backend",
        "extension registry: tile cache tiers",
        "extension registry: style store",
        // `#186`: the cross-protocol link-contributor seam, registered
        // before the fallible cache setup for exactly this reason.
        "extension registry: link contributors",
    ] {
        assert!(
            stdout.contains(seam),
            "the boot failure omitted the {seam:?} inventory: {stdout}"
        );
    }

    // `#162`: the registry-backend seam does not just announce itself, it
    // enumerates what this binary actually contains — `file` as the direct
    // built-in backend, plus every relational implementation registered under
    // its own declared name. Asserted on THAT line rather than on the whole
    // of stdout, because "postgis" also appears in the storage-driver
    // inventory and a whole-stdout match would pass while this line said
    // nothing at all.
    let backend_line = stdout
        .lines()
        .find(|line| line.contains("extension registry: catalog/collection registry backend"))
        .expect("the registry-backend inventory line was just asserted to exist");

    // What that enumeration must CONTAIN is a property of the feature set this
    // binary was built with, not a constant. `postgis` is the only relational
    // implementation any driver crate registers, and it is compiled out of
    // every `--no-default-features --features <driver>` leg. This file is
    // gated only `#![cfg(not(feature = "valkey"))]`, so it builds and runs in
    // nine of those legs — and from #162 (`5208a89`) until this comment it
    // asserted "postgis" unconditionally, which failed all nine of them with
    // `the registry-backend inventory omitted "postgis"`. The assertion was
    // never correct as written: it described a default build, while the seam
    // it checks exists precisely to describe *this* build.
    //
    // Deriving the expectation from `cfg` rather than skipping the check under
    // `#[cfg(feature = "postgis")]` keeps the assertion live everywhere this
    // file is built at all: those nine legs, plus the default-feature `test`
    // job, which is the only place it runs with `postgis` compiled IN.
    // "This binary contains postgis, so the seam names it" and "this binary
    // contains no relational driver, so the seam names none" are equally
    // checkable claims, and the second is the one that catches a seam
    // enumerating a backend that is not actually compiled in — the exact
    // direction #162's zero-factory refusal path depends on being able to
    // trust. A check that goes silent in nine legs would assert nothing there.
    #[cfg(feature = "postgis")]
    const EXPECTED_RELATIONAL: &[&str] = &["postgis"];
    #[cfg(not(feature = "postgis"))]
    const EXPECTED_RELATIONAL: &[&str] = &[];

    // `server.log_json: true` above, so this line is a JSON object and the
    // inventory can be read as structured fields rather than matched as a
    // substring. `demo-smoke.sh` phase 19 asserts the loose, token-shaped form
    // against the terminal-styled log; this is the strict form it defers to.
    let event: serde_json::Value =
        serde_json::from_str(backend_line).expect("`log_json: true` makes the boot log JSON");
    let fields = event
        .get("fields")
        .and_then(serde_json::Value::as_object)
        .expect("every tracing JSON event carries its fields");

    // `file` is the direct built-in backend: there is no driver crate it could
    // be compiled out with, so it is named in every leg without exception.
    assert_eq!(
        fields.get("builtin").and_then(serde_json::Value::as_str),
        Some("file"),
        "the registry-backend inventory omitted the built-in backend: {backend_line}"
    );

    // Both halves of the one `registry.backend` knob are selected by one name,
    // so both must enumerate exactly the driver crates that provide them.
    for field in [
        "relational_registry_implementations",
        "relational_tenant_implementations",
    ] {
        let rendered = fields
            .get(field)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                panic!("the registry-backend inventory omitted {field:?}: {backend_line}")
            });
        // Recorded with `?`, so the value is the `Debug` of a `Vec<&str>` —
        // which, for a list of strings, is also its JSON form.
        let listed: Vec<String> = serde_json::from_str(rendered).unwrap_or_else(|err| {
            panic!("{field:?} is not a rendered list of names ({err}): {rendered}")
        });
        assert_eq!(
            listed, EXPECTED_RELATIONAL,
            "the registry-backend inventory disagrees with what this binary was built \
             with: {field:?} should enumerate {EXPECTED_RELATIONAL:?}: {backend_line}"
        );
    }
}
