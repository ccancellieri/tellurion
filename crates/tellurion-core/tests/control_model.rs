use tellurion_core::{
    validate_control_event_page, AppConfig, AuditRequestContext, CatalogDecl, ControlChangeSet,
    ControlEvent, ControlEventCursor, ControlOperation, ControlScope, ControlSnapshot, Error,
    PathPolicy, PolicyEffect, PrincipalIdentity, RoleBinding, SettingsDecl, TenantDecl,
    VersionedControlOperation, VisibilityDecl,
};

fn principal() -> PrincipalIdentity {
    PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "operator-1".to_string(),
    }
}

fn policy(id: &str) -> PathPolicy {
    PathPolicy::legacy(
        id,
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/platform/**"],
        Vec::new(),
    )
}

fn snapshot(config: AppConfig) -> ControlSnapshot {
    ControlSnapshot {
        config,
        role_bindings: Vec::new(),
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    }
}

#[test]
fn snapshot_rejects_a_binding_to_an_unknown_tenant() {
    let mut candidate = snapshot(AppConfig::default());
    candidate.role_bindings.push(RoleBinding {
        principal: principal(),
        role: "tenant_admin".to_string(),
        scope: ControlScope::Tenant {
            tenant_id: "missing".to_string(),
        },
    });

    let error = candidate.validate().expect_err("unknown scope must fail");

    assert!(matches!(error, Error::ControlValidation(_)));
    assert!(error.to_string().contains("unknown tenant 'missing'"));
}

#[test]
fn snapshot_rejects_a_catalog_scope_under_the_wrong_tenant() {
    let mut config = AppConfig::default();
    config.tenants.push(TenantDecl {
        id: "tenant-a".to_string(),
        external_id: None,
        settings: SettingsDecl::default(),
    });
    config.tenants.push(TenantDecl {
        id: "tenant-b".to_string(),
        external_id: None,
        settings: SettingsDecl::default(),
    });
    config.catalogs.push(CatalogDecl {
        id: "catalog-a".to_string(),
        external_id: None,
        tenant: "tenant-a".to_string(),
        settings: SettingsDecl::default(),
        visibility: VisibilityDecl::default(),
    });
    let mut candidate = snapshot(config);
    candidate.role_bindings.push(RoleBinding {
        principal: principal(),
        role: "catalog_admin".to_string(),
        scope: ControlScope::Catalog {
            tenant_id: "tenant-b".to_string(),
            catalog_id: "catalog-a".to_string(),
        },
    });

    let error = candidate.validate().expect_err("false ownership must fail");

    assert!(matches!(error, Error::ControlValidation(_)));
    assert!(error.to_string().contains("belongs to tenant 'tenant-a'"));
}

#[test]
fn snapshot_rejects_duplicate_policy_ids() {
    let mut candidate = snapshot(AppConfig::default());
    candidate.path_policies = vec![policy("read-platform"), policy("read-platform")];

    let error = candidate.validate().expect_err("duplicate ids must fail");

    assert!(matches!(error, Error::ControlValidation(_)));
    assert!(error.to_string().contains("duplicate path policy id"));
}

#[test]
fn snapshot_rejects_non_segment_policy_patterns() {
    let mut candidate = snapshot(AppConfig::default());
    candidate.path_policies.push(PathPolicy::new(
        "ambiguous-pattern",
        "viewer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/ac*/**"],
    ));

    let error = candidate
        .validate()
        .expect_err("partial-segment wildcard must fail");

    assert!(matches!(error, Error::ControlValidation(_)));
    assert!(error.to_string().contains("anchored segment pattern"));
}

#[test]
fn snapshot_rejects_disagreeing_dynamic_and_checkpoint_roles() {
    let mut candidate = snapshot(AppConfig::default());
    let mut statement = PathPolicy::new(
        "split-role",
        "tenant_admin",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/platform/**"],
    );
    statement.roles = vec!["sysadmin".to_string()];
    candidate.path_policies.push(statement);

    let error = candidate
        .validate()
        .expect_err("the two policy engines must consume the same role");
    assert!(error.to_string().contains("role representations disagree"));
}

#[test]
fn snapshot_rejects_a_policy_pattern_outside_its_effective_scope() {
    let mut config = AppConfig::default();
    config.tenants.push(TenantDecl {
        id: "tenant-a".to_string(),
        external_id: Some("acme".to_string()),
        settings: SettingsDecl::default(),
    });
    let mut candidate = snapshot(config);
    candidate.path_policies.push(PathPolicy::new(
        "wrong-tenant",
        "viewer",
        ControlScope::Tenant {
            tenant_id: "tenant-a".to_string(),
        },
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/other/**"],
    ));

    let error = candidate
        .validate()
        .expect_err("policy path must stay inside its typed scope");

    assert!(matches!(error, Error::ControlValidation(_)));
    assert!(error.to_string().contains("outside its effective scope"));
}

#[test]
fn snapshot_delegates_configuration_validation() {
    let mut config = AppConfig::default();
    config.cache.memory_percent = 101.0;

    let error = snapshot(config)
        .validate()
        .expect_err("invalid AppConfig must fail");

    assert!(matches!(error, Error::Config(_)));
}

#[test]
fn snapshot_rejects_inline_bearer_token_secrets() {
    const SECRET: &str = "control-plane-inline-secret";
    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
tenants: [{{ id: tenant-a }}]
auth:
  bearer_tokens:
    - {{ token: {SECRET}, tenants: [tenant-a] }}
"#,
    ))
    .unwrap();

    let error = snapshot(config)
        .validate()
        .expect_err("a durable control snapshot must not contain a bearer-token value");

    assert!(matches!(error, Error::ControlValidation(_)));
    assert!(error.to_string().contains("token_env"));
    assert!(!error.to_string().contains(SECRET));
}

#[test]
fn snapshot_accepts_bearer_token_secret_references() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
tenants: [{ id: tenant-a }]
auth:
  bearer_tokens:
    - { token_env: TELLURION_TENANT_A_TOKEN, tenants: [tenant-a] }
"#,
    )
    .unwrap();

    snapshot(config)
        .validate()
        .expect("a control snapshot may persist a secret reference");
}

#[test]
fn changeset_rejects_an_empty_operation_list() {
    let changes = ControlChangeSet {
        idempotency_key: Some("request-1".to_string()),
        operations: Vec::new(),
    };

    let error = changes.validate().expect_err("empty mutation must fail");

    assert!(matches!(error, Error::ControlValidation(_)));
    assert!(error.to_string().contains("at least one operation"));
}

#[test]
fn event_page_rejects_non_monotonic_positions() {
    let events = vec![
        ControlEvent {
            revision: 4,
            ordinal: 1,
            changed_resources: vec!["policy/one".to_string()],
        },
        ControlEvent {
            revision: 4,
            ordinal: 0,
            changed_resources: vec!["policy/two".to_string()],
        },
    ];

    let error = validate_control_event_page(
        Some(ControlEventCursor {
            revision: 3,
            ordinal: 0,
        }),
        &events,
    )
    .expect_err("a page must be strictly ordered");

    assert!(matches!(error, Error::ControlEventOrder { .. }));
}

#[test]
fn a_well_formed_platform_changeset_is_valid() {
    let changes = ControlChangeSet {
        idempotency_key: Some("request-2".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutPathPolicy(policy("read-platform")),
        }],
    };

    changes.validate().expect("well-formed changeset");

    let request = AuditRequestContext {
        method: "PUT".to_string(),
        canonical_path: "/_control/v1/platform/policies/read-platform".to_string(),
        correlation_id: "correlation-1".to_string(),
    };
    assert_eq!(request.method, "PUT");
}
