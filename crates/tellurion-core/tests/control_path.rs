use tellurion_core::{canonicalize_control_path, CompiledPathPattern, ControlPathError};

#[test]
fn canonical_paths_reject_normalization_and_segment_attacks() {
    let cases: &[(&[u8], ControlPathError)] = &[
        (
            b"/_control/v1/tenants/acme%2Fadmin/catalogs",
            ControlPathError::EncodedComponent,
        ),
        (
            b"/_control/v1/tenants/acme%5cadmin/catalogs",
            ControlPathError::EncodedComponent,
        ),
        (
            b"/_control/v1/tenants/../platform/settings",
            ControlPathError::DotSegment,
        ),
        (
            b"/_control/v1/tenants//catalogs",
            ControlPathError::RepeatedSeparator,
        ),
        (
            b"/_control/v1/tenants/acme/",
            ControlPathError::TrailingSeparator,
        ),
        (
            b"/_control/v1/tenant/acme/settings",
            ControlPathError::InvalidHierarchy,
        ),
        (
            b"/_control/v1/tenants/acme\\catalogs",
            ControlPathError::InvalidSeparator,
        ),
        (
            b"/_control/v1/tenants/\xff/settings",
            ControlPathError::InvalidUtf8,
        ),
    ];

    for (raw_path, expected) in cases {
        assert_eq!(
            canonicalize_control_path(raw_path, "").unwrap_err(),
            *expected,
            "path bytes {raw_path:?}"
        );
    }
}

#[test]
fn canonicalization_strips_only_the_configured_application_root() {
    let canonical = canonicalize_control_path(
        b"/gateway/_control/v1/tenants/acme/catalogs/cadastre",
        "/gateway",
    )
    .expect("configured root is stripped");

    assert_eq!(
        canonical.as_str(),
        "/_control/v1/tenants/acme/catalogs/cadastre"
    );
    assert_eq!(
        canonicalize_control_path(
            b"/other/_control/v1/tenants/acme/catalogs/cadastre",
            "/gateway"
        ),
        Err(ControlPathError::OutsideApplicationRoot)
    );
}

#[test]
fn compiled_patterns_are_anchored_and_segment_aware() {
    let exact = CompiledPathPattern::compile(
        "/_control/v1/tenants/acme/catalogs/cadastre/collections/*/styles/**",
    )
    .unwrap();
    let matching = canonicalize_control_path(
        b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads/styles/day/legend",
        "",
    )
    .unwrap();
    let prefix_collision = canonicalize_control_path(
        b"/_control/v1/tenants/acme/catalogs/cadastre-evil/collections/roads/styles/day",
        "",
    )
    .unwrap();

    assert!(exact.matches(&matching));
    assert!(!exact.matches(&prefix_collision));
    assert!(!exact.matches(
        &canonicalize_control_path(
            b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads/assets/day",
            ""
        )
        .unwrap()
    ));
}

#[test]
fn pattern_compilation_rejects_ambiguous_or_non_segment_wildcards() {
    for pattern in [
        "_control/v1/**",
        "/_control/v1/tenants//**",
        "/_control/v1/tenants/acme/",
        "/_control/v1/tenants/ac*",
        "/_control/v1/tenants/../**",
    ] {
        assert!(
            CompiledPathPattern::compile(pattern).is_err(),
            "pattern {pattern}"
        );
    }
}
