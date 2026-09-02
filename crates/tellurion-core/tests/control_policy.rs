use std::collections::{BTreeMap, HashMap};

use tellurion_core::{
    apply_control_changes, authorize_control as authorize_validated_control,
    authorize_control_mutation, explain_control, preview_control_changes, role_binding_target_id,
    validate_delegated_policy, validate_delegated_role_binding, AppConfig, AuditRequestContext,
    AuthenticatedSubject, CatalogDecl, CollectionDecl, ControlChangeSet, ControlOperation,
    ControlRouteDescriptor, ControlRouteRegistry, ControlScope, ControlSnapshot, DelegationError,
    MutationControlDecision as ControlDecision,
    MutationControlRequestContext as ControlRequestContext, PathPolicy, PolicyCondition,
    PolicyEffect, PrincipalIdentity, RoleBinding, SettingsDecl, TenantDecl,
    VersionedControlOperation, VersionedControlSnapshot,
};

fn subject() -> AuthenticatedSubject {
    AuthenticatedSubject {
        principal: PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "operator-1".to_string(),
        },
        claims: HashMap::new(),
    }
}

fn snapshot(role: &str, binding_scope: ControlScope) -> ControlSnapshot {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a } ]
catalogs: [ { id: catalog-a, tenant: tenant-a } ]
collections:
  - id: collection-a
    catalog: catalog-a
    storage: main
"#,
    )
    .unwrap();
    ControlSnapshot {
        config,
        role_bindings: vec![RoleBinding {
            principal: subject().principal.clone(),
            role: role.to_string(),
            scope: binding_scope,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    }
}

fn hierarchy_snapshot(role: &str, binding_scope: ControlScope) -> ControlSnapshot {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a } ]
catalogs: [ { id: catalog-a, tenant: tenant-a } ]
collections:
  - id: collection-a
    catalog: catalog-a
    storage: main
"#,
    )
    .unwrap();
    ControlSnapshot {
        config,
        role_bindings: vec![RoleBinding {
            principal: subject().principal.clone(),
            role: role.to_string(),
            scope: binding_scope,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    }
}

fn request(method: &str, canonical_path: &str, scope: ControlScope) -> ControlRequestContext {
    ControlRequestContext {
        method: method.to_string(),
        canonical_path: canonical_path.to_string(),
        route_template: canonical_path.to_string(),
        scope,
    }
}

fn authorize_control(
    subject: &AuthenticatedSubject,
    candidate: &ControlRequestContext,
    snapshot: &ControlSnapshot,
) -> ControlDecision {
    let validated = snapshot.validated().expect("test snapshot must validate");
    authorize_validated_control(subject, candidate, &validated)
}

fn checkpoint_authorization(
    subject: &AuthenticatedSubject,
    candidate: &ControlRequestContext,
    audit: AuditRequestContext,
    snapshot: &VersionedControlSnapshot,
    descriptor: ControlRouteDescriptor,
    changes: &ControlChangeSet,
) -> Result<tellurion_core::AuthorizedControlMutation, tellurion_core::ControlMiddlewareError> {
    let registry = ControlRouteRegistry::new([descriptor]).unwrap();
    authorize_control_mutation(
        subject,
        &candidate.method,
        candidate.canonical_path.as_bytes(),
        descriptor.template(),
        &registry,
        "",
        snapshot,
        changes,
        &audit.correlation_id,
    )
}

fn checkpoint_with_descriptor(
    authoritative: &ControlSnapshot,
    entity_versions: BTreeMap<String, String>,
    descriptor: ControlRouteDescriptor,
    method: &str,
    canonical_path: &str,
    changes: &ControlChangeSet,
) -> Result<tellurion_core::AuthorizedControlMutation, tellurion_core::ControlMiddlewareError> {
    let versioned =
        VersionedControlSnapshot::new(authoritative.clone(), 1, entity_versions).unwrap();
    let registry = ControlRouteRegistry::new([descriptor]).unwrap();
    authorize_control_mutation(
        &subject(),
        method,
        canonical_path.as_bytes(),
        descriptor.template(),
        &registry,
        "",
        &versioned,
        changes,
        "explicit-route-contract",
    )
}

fn mutation_authorization(
    snapshot: &ControlSnapshot,
    descriptor: ControlRouteDescriptor,
    method: &str,
    canonical_path: &str,
    changes: &ControlChangeSet,
) -> tellurion_core::AuthorizedControlMutation {
    let versioned = VersionedControlSnapshot::new(snapshot.clone(), 1, BTreeMap::new()).unwrap();
    let control_request = request(method, canonical_path, ControlScope::Platform);
    checkpoint_authorization(
        &subject(),
        &control_request,
        AuditRequestContext {
            method: method.to_string(),
            canonical_path: canonical_path.to_string(),
            correlation_id: "test-mutation".to_string(),
        },
        &versioned,
        descriptor,
        changes,
    )
    .unwrap()
}

#[test]
fn preview_reports_the_prospective_change_without_mutating_the_authoritative_snapshot() {
    let authoritative = snapshot("sysadmin", ControlScope::Platform);
    let versioned =
        VersionedControlSnapshot::new(authoritative.clone(), 1, BTreeMap::new()).unwrap();
    let changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutTenant(TenantDecl {
                id: "tenant-b".to_string(),
                external_id: Some("bravo".to_string()),
                settings: SettingsDecl::default(),
            }),
        }],
    };
    let authorization = mutation_authorization(
        &authoritative,
        ControlRouteDescriptor::Tenants,
        "POST",
        "/_control/v1/tenants",
        &changes,
    );
    let before = versioned.clone();

    let preview = preview_control_changes(&versioned, &authorization, &changes).unwrap();

    assert_eq!(preview.base_revision, 1);
    assert_eq!(preview.prospective_revision, 2);
    assert_eq!(
        preview.changed_resources,
        vec!["tenant/tenant-b".to_string()]
    );
    assert_eq!(
        preview.entity_versions,
        BTreeMap::from([("tenant/tenant-b".to_string(), "2".to_string())])
    );
    assert!(preview
        .prospective_snapshot()
        .config
        .tenants
        .iter()
        .any(|tenant| tenant.external_id() == "bravo"));
    assert_eq!(versioned, before);
}

#[test]
fn mutation_checkpoint_rejects_route_operation_substitution_and_generic_batches() {
    let authoritative = snapshot("sysadmin", ControlScope::Platform);
    let versioned =
        VersionedControlSnapshot::new(authoritative.clone(), 1, BTreeMap::new()).unwrap();
    let settings_path = "/_control/v1/platform/settings";
    let settings_request = request("PUT", settings_path, ControlScope::Platform);

    let unrelated = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutPathPolicy(PathPolicy::new(
                "substituted-policy",
                "sysadmin",
                ControlScope::Platform,
                PolicyEffect::Allow,
                ["GET"],
                ["/_control/v1/platform/**"],
            )),
        }],
    };
    assert!(checkpoint_authorization(
        &subject(),
        &settings_request,
        AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: settings_path.to_string(),
            correlation_id: "settings-substitution".to_string(),
        },
        &versioned,
        ControlRouteDescriptor::PlatformSettings,
        &unrelated,
    )
    .is_err());

    let mixed = ControlChangeSet {
        idempotency_key: None,
        operations: vec![
            VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(authoritative.config.clone()),
            },
            VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutRoleBinding(RoleBinding {
                    principal: subject().principal,
                    role: "viewer".to_string(),
                    scope: ControlScope::Platform,
                }),
            },
        ],
    };
    assert!(checkpoint_authorization(
        &subject(),
        &settings_request,
        AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: settings_path.to_string(),
            correlation_id: "settings-batch".to_string(),
        },
        &versioned,
        ControlRouteDescriptor::PlatformSettings,
        &mixed,
    )
    .is_err());

    let patch_request = request("PATCH", settings_path, ControlScope::Platform);
    let replace = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(authoritative.config.clone()),
        }],
    };
    assert!(checkpoint_authorization(
        &subject(),
        &patch_request,
        AuditRequestContext {
            method: "PATCH".to_string(),
            canonical_path: settings_path.to_string(),
            correlation_id: "settings-method".to_string(),
        },
        &versioned,
        ControlRouteDescriptor::PlatformSettings,
        &replace,
    )
    .is_ok());

    let options_request = request("OPTIONS", settings_path, ControlScope::Platform);
    assert!(checkpoint_authorization(
        &subject(),
        &options_request,
        AuditRequestContext {
            method: "OPTIONS".to_string(),
            canonical_path: settings_path.to_string(),
            correlation_id: "settings-options".to_string(),
        },
        &versioned,
        ControlRouteDescriptor::PlatformSettings,
        &replace,
    )
    .is_err());

    let assets_path =
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/a";
    let assets_request = request(
        "PUT",
        assets_path,
        ControlScope::Collection {
            tenant_id: "tenant-a".to_string(),
            catalog_id: "catalog-a".to_string(),
            collection_id: "collection-a".to_string(),
        },
    );
    let put_collection = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCollection(authoritative.config.collections[0].clone()),
        }],
    };
    assert!(checkpoint_authorization(
        &subject(),
        &assets_request,
        AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: assets_path.to_string(),
            correlation_id: "assets-substitution".to_string(),
        },
        &versioned,
        ControlRouteDescriptor::CollectionAsset,
        &put_collection,
    )
    .is_err());
}

#[test]
fn settings_and_metadata_descriptors_reject_field_substitution() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);

    let mut platform_settings = authoritative.config.clone();
    platform_settings.settings.cache_ttl_s = Some(30);
    let platform_patch = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(platform_settings),
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformSettings,
        "PATCH",
        "/_control/v1/platform/settings",
        &platform_patch,
    )
    .is_ok());

    let mut substituted_platform = authoritative.config.clone();
    substituted_platform.server.port += 1;
    let substituted_platform = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(substituted_platform),
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformSettings,
        "PUT",
        "/_control/v1/platform/settings",
        &substituted_platform,
    )
    .is_err());

    let mut tenant = authoritative.config.tenants[0].clone();
    tenant.settings.cache_ttl_s = Some(30);
    tenant.external_id = Some("substituted-tenant".to_string());
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantSettings,
        "PATCH",
        "/_control/v1/tenants/tenant-a/settings",
        &put_tenant(tenant),
    )
    .is_err());

    let mut catalog = authoritative.config.catalogs[0].clone();
    catalog.settings.cache_ttl_s = Some(30);
    catalog.visibility.public = true;
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::CatalogSettings,
        "PATCH",
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/settings",
        &put_catalog(catalog),
    )
    .is_err());

    let mut collection = authoritative.config.collections[0].clone();
    collection.settings.cache_ttl_s = Some(30);
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::CollectionMetadata,
        "PATCH",
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/metadata",
        &put_collection(collection),
    )
    .is_err());
}

#[test]
fn ordinary_delete_is_tombstone_only_and_permanent_delete_is_staged() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let collection_scope = collection();
    let collection_path =
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a";
    let tombstone = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::TombstoneResource {
                scope: collection_scope.clone(),
            },
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::Collection,
        "DELETE",
        collection_path,
        &tombstone,
    )
    .is_ok());

    let permanent_without_etag = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PermanentlyDeleteResource {
                scope: collection_scope.clone(),
            },
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::Collection,
        "DELETE",
        collection_path,
        &permanent_without_etag,
    )
    .is_err());

    let permanent_direct = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("7".to_string()),
            operation: ControlOperation::PermanentlyDeleteResource {
                scope: collection_scope,
            },
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::from([(
            "tenant/tenant-a/catalog/catalog-a/collection/collection-a".to_string(),
            "7".to_string(),
        )]),
        ControlRouteDescriptor::Collection,
        "DELETE",
        collection_path,
        &permanent_direct,
    )
    .is_err());
}

#[test]
fn permanent_delete_descriptor_requires_tombstone_etag_and_exact_target() {
    let collection_scope = collection();
    let permanent_path = "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/permanent-delete";
    let entity_versions = BTreeMap::from([(
        "tenant/tenant-a/catalog/catalog-a/collection/collection-a".to_string(),
        "7".to_string(),
    )]);
    let permanent = |scope: ControlScope, expected_entity_version: Option<&str>| ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: expected_entity_version.map(str::to_string),
            operation: ControlOperation::PermanentlyDeleteResource { scope },
        }],
    };

    let direct = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    assert!(checkpoint_with_descriptor(
        &direct,
        entity_versions.clone(),
        ControlRouteDescriptor::CollectionPermanentDelete,
        "DELETE",
        permanent_path,
        &permanent(collection_scope.clone(), Some("7")),
    )
    .is_err());

    let mut tombstoned = direct;
    tombstoned
        .tombstoned_resources
        .push(collection_scope.clone());
    assert!(checkpoint_with_descriptor(
        &tombstoned,
        entity_versions.clone(),
        ControlRouteDescriptor::CollectionPermanentDelete,
        "DELETE",
        permanent_path,
        &permanent(collection_scope.clone(), None),
    )
    .is_err());
    assert!(checkpoint_with_descriptor(
        &tombstoned,
        entity_versions.clone(),
        ControlRouteDescriptor::CollectionPermanentDelete,
        "DELETE",
        permanent_path,
        &permanent(tenant(), Some("7")),
    )
    .is_err());
    assert!(checkpoint_with_descriptor(
        &tombstoned,
        entity_versions,
        ControlRouteDescriptor::CollectionPermanentDelete,
        "DELETE",
        permanent_path,
        &permanent(collection_scope, Some("7")),
    )
    .is_ok());
}

#[test]
fn platform_import_permanent_delete_requires_authoritative_tombstone_and_etag() {
    let target = collection();
    let target_key = target.resource_key();
    let versions = BTreeMap::from([
        (target_key.clone(), "7".to_string()),
        (
            "tenant/tenant-b/catalog/catalog-b/collection/collection-b".to_string(),
            "9".to_string(),
        ),
    ]);
    let permanent = |scope: ControlScope, expected: Option<&str>| ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: expected.map(str::to_string),
            operation: ControlOperation::PermanentlyDeleteResource { scope },
        }],
    };

    let direct = two_tenant_snapshot("sysadmin", ControlScope::Platform);
    let direct_changes = permanent(target.clone(), Some("7"));
    let direct_authorization = mutation_authorization_with_state(
        &direct,
        1,
        versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &direct_changes,
    );
    assert!(apply_control_changes(
        direct,
        versions.clone(),
        2,
        &direct_authorization,
        &direct_changes,
    )
    .is_err());

    let same_batch = ControlChangeSet {
        idempotency_key: None,
        operations: vec![
            VersionedControlOperation {
                expected_entity_version: Some("7".to_string()),
                operation: ControlOperation::TombstoneResource {
                    scope: target.clone(),
                },
            },
            VersionedControlOperation {
                expected_entity_version: Some("7".to_string()),
                operation: ControlOperation::PermanentlyDeleteResource {
                    scope: target.clone(),
                },
            },
        ],
    };
    let same_batch_snapshot = two_tenant_snapshot("sysadmin", ControlScope::Platform);
    let same_batch_authorization = mutation_authorization_with_state(
        &same_batch_snapshot,
        1,
        versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &same_batch,
    );
    assert!(apply_control_changes(
        same_batch_snapshot,
        versions.clone(),
        2,
        &same_batch_authorization,
        &same_batch,
    )
    .is_err());

    let mut tombstoned = two_tenant_snapshot("sysadmin", ControlScope::Platform);
    tombstoned.tombstoned_resources.push(target.clone());
    let missing_etag = permanent(target.clone(), None);
    let missing_etag_authorization = mutation_authorization_with_state(
        &tombstoned,
        1,
        versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &missing_etag,
    );
    assert!(apply_control_changes(
        tombstoned.clone(),
        versions.clone(),
        2,
        &missing_etag_authorization,
        &missing_etag,
    )
    .is_err());

    let staged = permanent(target.clone(), Some("7"));
    let staged_authorization = mutation_authorization_with_state(
        &tombstoned,
        1,
        versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &staged,
    );
    let mut empty_etag = staged.clone();
    empty_etag.operations[0].expected_entity_version = Some(" ".to_string());
    assert!(apply_control_changes(
        tombstoned.clone(),
        versions.clone(),
        2,
        &staged_authorization,
        &empty_etag,
    )
    .is_err());

    let stale = permanent(target.clone(), Some("6"));
    let stale_authorization = mutation_authorization_with_state(
        &tombstoned,
        1,
        versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &stale,
    );
    assert!(matches!(
        apply_control_changes(
            tombstoned.clone(),
            versions.clone(),
            2,
            &stale_authorization,
            &stale,
        ),
        Err(tellurion_core::Error::ControlEntityVersionConflict { .. })
    ));

    let wrong_scope = ControlScope::Collection {
        tenant_id: "tenant-b".to_string(),
        catalog_id: "catalog-b".to_string(),
        collection_id: "collection-b".to_string(),
    };
    let wrong_target = permanent(wrong_scope, Some("9"));
    let wrong_target_authorization = mutation_authorization_with_state(
        &tombstoned,
        1,
        versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &wrong_target,
    );
    assert!(apply_control_changes(
        tombstoned.clone(),
        versions.clone(),
        2,
        &wrong_target_authorization,
        &wrong_target,
    )
    .is_err());

    let applied = apply_control_changes(tombstoned, versions, 2, &staged_authorization, &staged)
        .expect("a platform import may permanently delete the exact tombstoned ETag target");
    assert!(applied
        .snapshot
        .config
        .collections
        .iter()
        .all(|collection| collection.id != "collection-a"));
    assert!(!applied.snapshot.tombstoned_resources.contains(&target));
    assert!(!applied.entity_versions.contains_key(&target_key));
}

#[test]
fn scoped_batch_descriptor_is_rejected() {
    let tenant_scope = tenant();
    let mut authoritative = hierarchy_snapshot("tenant_admin", tenant_scope.clone());
    authoritative.path_policies.push(PathPolicy::new(
        "legacy-scoped-import",
        "tenant_admin",
        tenant_scope,
        PolicyEffect::Allow,
        ["POST"],
        ["/_control/v1/tenants/tenant-a/import"],
    ));
    let mut tenant = authoritative.config.tenants[0].clone();
    tenant.settings.cache_ttl_s = Some(30);
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/tenants/tenant-a/import",
        &put_tenant(tenant),
    )
    .is_err());
}

#[test]
fn binding_target_substitution_is_rejected() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let delete = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::DeleteRoleBinding {
                principal: subject().principal,
                scope: ControlScope::Platform,
                role: "sysadmin".to_string(),
            },
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformRoleBinding,
        "DELETE",
        "/_control/v1/platform/role-bindings/not-the-binding-target",
        &delete,
    )
    .is_err());
}

#[test]
fn role_binding_create_and_delete_use_separate_opaque_targets() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let binding = RoleBinding {
        principal: PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "viewer-operator".to_string(),
        },
        role: "viewer".to_string(),
        scope: ControlScope::Platform,
    };
    let target = role_binding_target_id(&binding);
    assert_eq!(
        target,
        "ac6f27e0aec5260ec3a5e7f049399b0040bc459633ac04a15a0a9b37bf515d88"
    );

    let create = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutRoleBinding(binding.clone()),
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformRoleBindings,
        "POST",
        "/_control/v1/platform/role-bindings",
        &create,
    )
    .is_ok());

    let delete = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::DeleteRoleBinding {
                principal: binding.principal,
                scope: binding.scope,
                role: binding.role,
            },
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformRoleBinding,
        "DELETE",
        &format!("/_control/v1/platform/role-bindings/{target}"),
        &delete,
    )
    .is_ok());
}

#[test]
fn role_binding_targets_structurally_frame_all_scope_variants() {
    let target = |scope| {
        role_binding_target_id(&RoleBinding {
            principal: PrincipalIdentity {
                issuer: "https://issuer.example".to_string(),
                subject: "viewer-operator".to_string(),
            },
            role: "viewer".to_string(),
            scope,
        })
    };

    assert_eq!(
        target(ControlScope::Platform),
        "ac6f27e0aec5260ec3a5e7f049399b0040bc459633ac04a15a0a9b37bf515d88"
    );
    assert_eq!(
        target(ControlScope::Tenant {
            tenant_id: "tenant-a".to_string(),
        }),
        "ecaadbccea868e44e94c65388e7300028d8b3814c747e38eb1cf254cb1ee13b1"
    );
    assert_eq!(
        target(ControlScope::Catalog {
            tenant_id: "tenant-a".to_string(),
            catalog_id: "catalog-a".to_string(),
        }),
        "76b52a75cedb5c9d58475cd81465fdb03e0f62f44e8b78eb243cfc6bafcc2da8"
    );
    assert_eq!(
        target(ControlScope::Collection {
            tenant_id: "tenant-a".to_string(),
            catalog_id: "catalog-a".to_string(),
            collection_id: "collection-a".to_string(),
        }),
        "f6f3fc4a28d32bf0b6ff956d87fbfe3d56463c769e1a3837507520c01fd7e848"
    );

    let adversarial_tenant = target(ControlScope::Tenant {
        tenant_id: "a/catalog/b".to_string(),
    });
    let adversarial_catalog = target(ControlScope::Catalog {
        tenant_id: "a".to_string(),
        catalog_id: "b".to_string(),
    });
    assert_eq!(
        adversarial_tenant,
        "770e8eb10392009761221159af56724c2652603c2cb8f9e0d5975d18ead733c0"
    );
    assert_eq!(
        adversarial_catalog,
        "bf3d09ed200a3fad8038a6ba36a4c930764bc26c3c85f359d01ae21934536cc9"
    );
    assert_ne!(adversarial_tenant, adversarial_catalog);
}

#[test]
fn idempotency_request_fingerprint_is_stable_and_binds_route_principal_and_intent() {
    let mut authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let other = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "operator-2".to_string(),
    };
    authoritative.role_bindings.push(RoleBinding {
        principal: other.clone(),
        role: "sysadmin".to_string(),
        scope: ControlScope::Platform,
    });
    let changes = ControlChangeSet {
        idempotency_key: Some("stable-request".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::ReplacePlatformSettings(authoritative.config.clone()),
        }],
    };
    let authorize = |principal: PrincipalIdentity,
                     descriptor: ControlRouteDescriptor,
                     method: &str,
                     path: &str,
                     revision: u64,
                     correlation: &str,
                     changes: &ControlChangeSet| {
        let versioned =
            VersionedControlSnapshot::new(authoritative.clone(), revision, BTreeMap::new())
                .unwrap();
        authorize_control_mutation(
            &AuthenticatedSubject {
                principal,
                claims: HashMap::new(),
            },
            method,
            path.as_bytes(),
            descriptor.template(),
            &ControlRouteRegistry::new([descriptor]).unwrap(),
            "",
            &versioned,
            changes,
            correlation,
        )
        .unwrap()
    };

    let first = authorize(
        subject().principal,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        1,
        "first-correlation",
        &changes,
    );
    let restarted = authorize(
        subject().principal,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        99,
        "after-restart",
        &changes,
    );
    assert_eq!(first.request_fingerprint(), restarted.request_fingerprint());

    let other_principal = authorize(
        other,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        1,
        "other-principal",
        &changes,
    );
    assert_ne!(
        first.request_fingerprint(),
        other_principal.request_fingerprint()
    );

    let other_route = authorize(
        subject().principal,
        ControlRouteDescriptor::PlatformSettings,
        "PUT",
        "/_control/v1/platform/settings",
        1,
        "other-route",
        &changes,
    );
    assert_ne!(
        first.request_fingerprint(),
        other_route.request_fingerprint()
    );

    let mut other_intent = changes.clone();
    other_intent.idempotency_key = Some("different-intent".to_string());
    let other_intent = authorize(
        subject().principal,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        1,
        "other-intent",
        &other_intent,
    );
    assert_ne!(
        first.request_fingerprint(),
        other_intent.request_fingerprint()
    );
}

fn mutation_authorization_for_descriptor(
    snapshot: &ControlSnapshot,
    descriptor: ControlRouteDescriptor,
    method: &str,
    canonical_path: &str,
    changes: &ControlChangeSet,
) -> tellurion_core::AuthorizedControlMutation {
    mutation_authorization_with_state(
        snapshot,
        1,
        BTreeMap::new(),
        descriptor,
        method,
        canonical_path,
        changes,
    )
}

fn mutation_authorization_with_state(
    snapshot: &ControlSnapshot,
    revision: u64,
    entity_versions: BTreeMap<String, String>,
    descriptor: ControlRouteDescriptor,
    method: &str,
    canonical_path: &str,
    changes: &ControlChangeSet,
) -> tellurion_core::AuthorizedControlMutation {
    let versioned =
        VersionedControlSnapshot::new(snapshot.clone(), revision, entity_versions).unwrap();
    let registry = ControlRouteRegistry::new([descriptor]).unwrap();
    authorize_control_mutation(
        &subject(),
        method,
        canonical_path.as_bytes(),
        descriptor.template(),
        &registry,
        "",
        &versioned,
        changes,
        "scope-mutation",
    )
    .unwrap()
}

fn two_tenant_snapshot(role: &str, binding_scope: ControlScope) -> ControlSnapshot {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a }, { id: tenant-b } ]
catalogs:
  - { id: catalog-a, tenant: tenant-a }
  - { id: catalog-a2, tenant: tenant-a }
  - { id: catalog-b, tenant: tenant-b }
collections:
  - { id: collection-a, catalog: catalog-a, storage: main }
  - { id: collection-b, catalog: catalog-b, storage: main }
"#,
    )
    .unwrap();
    ControlSnapshot {
        config,
        role_bindings: vec![RoleBinding {
            principal: subject().principal.clone(),
            role: role.to_string(),
            scope: binding_scope,
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    }
}

fn put_tenant(tenant: TenantDecl) -> ControlChangeSet {
    ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutTenant(tenant),
        }],
    }
}

fn put_catalog(catalog: CatalogDecl) -> ControlChangeSet {
    ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCatalog(catalog),
        }],
    }
}

fn put_collection(collection: CollectionDecl) -> ControlChangeSet {
    ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCollection(collection),
        }],
    }
}

fn tenant() -> ControlScope {
    ControlScope::Tenant {
        tenant_id: "tenant-a".to_string(),
    }
}

fn catalog() -> ControlScope {
    ControlScope::Catalog {
        tenant_id: "tenant-a".to_string(),
        catalog_id: "catalog-a".to_string(),
    }
}

fn collection() -> ControlScope {
    ControlScope::Collection {
        tenant_id: "tenant-a".to_string(),
        catalog_id: "catalog-a".to_string(),
        collection_id: "collection-a".to_string(),
    }
}

#[test]
fn built_in_role_matrix_applies_capabilities_at_each_scope() {
    let cases = [
        (
            "sysadmin",
            ControlScope::Platform,
            request(
                "PATCH",
                "/_control/v1/platform/settings",
                ControlScope::Platform,
            ),
            ControlDecision::Allow,
        ),
        (
            "tenant_admin",
            tenant(),
            request(
                "POST",
                "/_control/v1/tenants/tenant-a/catalogs",
                tenant(),
            ),
            ControlDecision::Allow,
        ),
        (
            "tenant_admin",
            tenant(),
            request(
                "POST",
                "/_control/v1/tenants/tenant-a/collection-moves",
                tenant(),
            ),
            ControlDecision::Allow,
        ),
        (
            "tenant_admin",
            tenant(),
            request(
                "PUT",
                "/_control/v1/tenants/tenant-a/collection-moves",
                tenant(),
            ),
            ControlDecision::Deny,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "DELETE",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a",
                collection(),
            ),
            ControlDecision::Allow,
        ),
        (
            "collection_editor",
            collection(),
            request(
                "PUT",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/a",
                collection(),
            ),
            ControlDecision::Allow,
        ),
        (
            "collection_editor",
            collection(),
            request(
                "PUT",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/data/bulk",
                collection(),
            ),
            ControlDecision::Allow,
        ),
        (
            "publisher",
            collection(),
            request(
                "PATCH",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/visibility",
                collection(),
            ),
            ControlDecision::Allow,
        ),
        (
            "viewer",
            collection(),
            request(
                "GET",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a",
                collection(),
            ),
            ControlDecision::Allow,
        ),
        (
            "viewer",
            collection(),
            request(
                "POST",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a",
                collection(),
            ),
            ControlDecision::Deny,
        ),
        (
            "publisher",
            collection(),
            request(
                "PUT",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/a",
                collection(),
            ),
            ControlDecision::Deny,
        ),
        (
            "service_account",
            collection(),
            request(
                "GET",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a",
                collection(),
            ),
            ControlDecision::Deny,
        ),
    ];

    for (role, binding_scope, candidate, expected) in cases {
        assert_eq!(
            authorize_control(&subject(), &candidate, &snapshot(role, binding_scope)),
            expected,
            "role {role}"
        );
    }
}

#[test]
fn built_in_administrators_receive_only_their_approved_capabilities() {
    let cases = [
        (
            "tenant_admin",
            tenant(),
            request("DELETE", "/_control/v1/tenants/tenant-a", tenant()),
            ControlDecision::Deny,
        ),
        (
            "tenant_admin",
            tenant(),
            request(
                "GET",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/a",
                collection(),
            ),
            ControlDecision::Deny,
        ),
        (
            "tenant_admin",
            tenant(),
            request("GET", "/_control/v1/tenants/tenant-a/audit", tenant()),
            ControlDecision::Deny,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "DELETE",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a",
                catalog(),
            ),
            ControlDecision::Deny,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "GET",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/data/items",
                collection(),
            ),
            ControlDecision::Deny,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "POST",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/principals",
                catalog(),
            ),
            ControlDecision::Deny,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "GET",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/audit",
                catalog(),
            ),
            ControlDecision::Deny,
        ),
        (
            "tenant_admin",
            tenant(),
            request(
                "PATCH",
                "/_control/v1/tenants/tenant-a/settings",
                tenant(),
            ),
            ControlDecision::Allow,
        ),
        (
            "tenant_admin",
            tenant(),
            request(
                "POST",
                "/_control/v1/tenants/tenant-a/catalogs",
                tenant(),
            ),
            ControlDecision::Allow,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "POST",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections",
                catalog(),
            ),
            ControlDecision::Allow,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "PATCH",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/settings",
                catalog(),
            ),
            ControlDecision::Allow,
        ),
        (
            "catalog_admin",
            catalog(),
            request(
                "POST",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/role-bindings",
                catalog(),
            ),
            ControlDecision::Allow,
        ),
    ];

    for (role, scope, candidate, expected) in cases {
        assert_eq!(
            authorize_control(&subject(), &candidate, &snapshot(role, scope)),
            expected,
            "role {role} on {} {}",
            candidate.method,
            candidate.canonical_path,
        );
    }
}

#[test]
fn grants_flow_downward_but_never_upward_or_across_parents() {
    let down = snapshot("viewer", tenant());
    assert_eq!(
        authorize_control(
            &subject(),
            &request(
                "GET",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a",
                collection(),
            ),
            &down,
        ),
        ControlDecision::Allow
    );

    let up = snapshot("viewer", collection());
    assert_eq!(
        authorize_control(
            &subject(),
            &request("GET", "/_control/v1/tenants/tenant-a", tenant()),
            &up,
        ),
        ControlDecision::Deny
    );

    let sibling = request(
        "GET",
        "/_control/v1/tenants/tenant-b/catalogs/catalog-a",
        ControlScope::Catalog {
            tenant_id: "tenant-b".to_string(),
            catalog_id: "catalog-a".to_string(),
        },
    );
    assert_eq!(
        authorize_control(&subject(), &sibling, &down),
        ControlDecision::Deny
    );
}

#[test]
fn explicit_deny_wins_and_absence_of_allow_denies() {
    let mut candidate = snapshot("viewer", ControlScope::Platform);
    candidate.path_policies.push(PathPolicy::new(
        "hide-audit",
        "viewer",
        ControlScope::Platform,
        PolicyEffect::Deny,
        ["GET"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/audit/**"],
    ));
    candidate.validate().unwrap();
    let audit = request(
        "GET",
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/audit/events",
        collection(),
    );

    assert_eq!(
        authorize_control(&subject(), &audit, &candidate),
        ControlDecision::Deny
    );
    let validated = candidate.validated().unwrap();
    let explanation = explain_control(&subject(), &audit, &validated);
    assert_eq!(explanation.decision, ControlDecision::Deny);
    assert_eq!(explanation.evaluated_roles, vec!["viewer"]);
    assert_eq!(explanation.matched_denies, vec!["hide-audit"]);
    assert!(explanation
        .matched_allows
        .contains(&"builtin:viewer:read".to_string()));

    assert_eq!(
        authorize_control(&subject(), &audit, &snapshot("unknown", collection())),
        ControlDecision::Deny
    );
}

#[test]
fn decision_only_authorization_matches_explanations() {
    let allowed_snapshot = snapshot("viewer", ControlScope::Platform);
    let allowed = request(
        "GET",
        "/_control/v1/platform/settings",
        ControlScope::Platform,
    );
    let unknown_snapshot = snapshot("unknown", ControlScope::Platform);
    let denied = request(
        "GET",
        "/_control/v1/platform/settings",
        ControlScope::Platform,
    );

    for (candidate, request, expected) in [
        (&allowed_snapshot, &allowed, ControlDecision::Allow),
        (&unknown_snapshot, &denied, ControlDecision::Deny),
    ] {
        let validated = candidate.validated().unwrap();
        assert_eq!(
            authorize_validated_control(&subject(), request, &validated),
            expected
        );
        assert_eq!(
            authorize_validated_control(&subject(), request, &validated),
            explain_control(&subject(), request, &validated).decision
        );
    }
}

#[test]
fn unevaluated_conditions_fail_closed_for_explicit_denies() {
    let mut candidate = snapshot("viewer", ControlScope::Platform);
    let mut deny = PathPolicy::new(
        "conditional-deny",
        "viewer",
        ControlScope::Platform,
        PolicyEffect::Deny,
        ["GET"],
        ["/_control/v1/platform/audit/**"],
    );
    deny.conditions.push(PolicyCondition {
        kind: "future_condition".to_string(),
        config: serde_json::json!({"enabled": true}),
    });
    candidate.path_policies.push(deny);
    candidate.validate().unwrap();

    assert_eq!(
        authorize_control(
            &subject(),
            &request(
                "GET",
                "/_control/v1/platform/audit/events",
                ControlScope::Platform,
            ),
            &candidate,
        ),
        ControlDecision::Deny
    );
}

#[test]
fn unvalidated_or_stale_denies_never_disappear_behind_a_builtin_allow() {
    let denied_request = request(
        "GET",
        "/_control/v1/platform/audit/events",
        ControlScope::Platform,
    );
    let mut unvalidated = snapshot("viewer", ControlScope::Platform);
    unvalidated.path_policies.push(PathPolicy::new(
        "deny-audit",
        "viewer",
        ControlScope::Platform,
        PolicyEffect::Deny,
        ["GET"],
        ["/_control/v1/platform/audit/**"],
    ));
    let validated = unvalidated.validated().unwrap();
    assert_eq!(
        authorize_validated_control(&subject(), &denied_request, &validated),
        ControlDecision::Deny,
    );
    unvalidated.path_policies[0].patterns =
        vec!["/_control/v1/platform/somewhere-else/**".to_string()];
    assert_eq!(
        authorize_validated_control(&subject(), &denied_request, &validated),
        ControlDecision::Deny,
        "mutating the raw snapshot cannot alter an immutable authorization view",
    );

    let mut invalid = snapshot("viewer", ControlScope::Platform);
    invalid.path_policies.push(PathPolicy::new(
        "invalid-deny",
        "viewer",
        ControlScope::Platform,
        PolicyEffect::Deny,
        ["GET"],
        ["/legacy/audit/**"],
    ));
    assert!(invalid.validated().is_err());
}

#[test]
fn custom_policy_bundles_give_service_accounts_only_explicit_capabilities() {
    let mut candidate = snapshot("service_account", ControlScope::Platform);
    candidate.path_policies.push(PathPolicy::new(
        "asset-reader",
        "service_account",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/**"],
    ));
    candidate.validate().unwrap();
    assert_eq!(
        authorize_control(
            &subject(),
            &request(
                "GET",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/a",
                collection(),
            ),
            &candidate,
        ),
        ControlDecision::Allow
    );
    assert_eq!(
        authorize_control(
            &subject(),
            &request(
                "DELETE",
                "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/a",
                collection(),
            ),
            &candidate,
        ),
        ControlDecision::Deny
    );
}

#[test]
fn delegation_cannot_exceed_the_actors_effective_allow_envelope() {
    let actor_snapshot = snapshot("catalog_admin", catalog());
    actor_snapshot.validate().unwrap();
    let allowed = PathPolicy::new(
        "delegated-viewer",
        "viewer",
        catalog(),
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-a/settings"],
    );
    validate_delegated_policy(&subject(), &allowed, &actor_snapshot)
        .expect("catalog admin may delegate reads beneath its catalog");

    let upward = PathPolicy::new(
        "delegated-platform",
        "viewer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/platform/audit/**"],
    );
    assert!(validate_delegated_policy(&subject(), &upward, &actor_snapshot).is_err());

    let publisher_snapshot = snapshot("publisher", collection());
    let assets = PathPolicy::new(
        "delegated-assets",
        "collection_editor",
        collection(),
        PolicyEffect::Allow,
        ["PUT"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/assets/**"],
    );
    assert!(validate_delegated_policy(&subject(), &assets, &publisher_snapshot).is_err());

    let sibling_path = PathPolicy::new(
        "delegated-sibling",
        "viewer",
        collection(),
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-b/collections/**"],
    );
    assert!(validate_delegated_policy(&subject(), &sibling_path, &actor_snapshot).is_err());
}

#[test]
fn descendant_denies_constrain_broad_policy_delegation() {
    let mut catalog_actor = hierarchy_snapshot("sysadmin", catalog());
    catalog_actor.path_policies.push(PathPolicy::new(
        "deny-collection-styles",
        "sysadmin",
        collection(),
        PolicyEffect::Deny,
        ["GET"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/styles/**"],
    ));
    catalog_actor.validate().unwrap();
    let catalog_delegation = PathPolicy::new(
        "delegate-catalog-read",
        "viewer",
        catalog(),
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-a/**"],
    );
    assert_eq!(
        validate_delegated_policy(&subject(), &catalog_delegation, &catalog_actor),
        Err(DelegationError::IntersectsExplicitDeny),
    );

    let mut tenant_actor = hierarchy_snapshot("sysadmin", tenant());
    tenant_actor.path_policies.push(PathPolicy::new(
        "deny-catalog-settings",
        "sysadmin",
        catalog(),
        PolicyEffect::Deny,
        ["PATCH"],
        ["/_control/v1/tenants/tenant-a/catalogs/catalog-a/settings"],
    ));
    tenant_actor.validate().unwrap();
    let tenant_delegation = PathPolicy::new(
        "delegate-tenant-write",
        "custom_admin",
        tenant(),
        PolicyEffect::Allow,
        ["PATCH"],
        ["/_control/v1/tenants/tenant-a/**"],
    );
    assert_eq!(
        validate_delegated_policy(&subject(), &tenant_delegation, &tenant_actor),
        Err(DelegationError::IntersectsExplicitDeny),
    );
}

#[test]
fn policy_mutations_cannot_self_widen_or_remove_effective_denies() {
    let mut actor_snapshot = snapshot("policy_manager", ControlScope::Platform);
    actor_snapshot.path_policies.extend([
        PathPolicy::new(
            "manage-policies",
            "policy_manager",
            ControlScope::Platform,
            PolicyEffect::Allow,
            ["POST", "PUT", "DELETE"],
            ["/_control/v1/platform/policies/**"],
        ),
        PathPolicy::new(
            "settings-envelope",
            "policy_manager",
            ControlScope::Platform,
            PolicyEffect::Allow,
            ["PATCH"],
            ["/_control/v1/platform/settings/**"],
        ),
        PathPolicy::new(
            "deny-secrets",
            "policy_manager",
            ControlScope::Platform,
            PolicyEffect::Deny,
            ["PATCH"],
            ["/_control/v1/platform/settings/secrets/**"],
        ),
    ]);
    actor_snapshot.validate().unwrap();

    let self_widening = ControlChangeSet {
        idempotency_key: Some("self-widen".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutPathPolicy(PathPolicy::new(
                "become-sysadmin",
                "policy_manager",
                ControlScope::Platform,
                PolicyEffect::Allow,
                ["DELETE"],
                ["/_control/v1/**"],
            )),
        }],
    };
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformPathPolicy,
        "PUT",
        "/_control/v1/platform/policies/become-sysadmin",
        &self_widening,
    );
    assert!(apply_control_changes(
        actor_snapshot.clone(),
        BTreeMap::new(),
        2,
        &authorization,
        &self_widening,
    )
    .is_err());

    let remove_deny = ControlChangeSet {
        idempotency_key: Some("remove-deny".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::DeletePathPolicy {
                id: "deny-secrets".to_string(),
            },
        }],
    };
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformPathPolicy,
        "DELETE",
        "/_control/v1/platform/policies/deny-secrets",
        &remove_deny,
    );
    assert!(apply_control_changes(
        actor_snapshot,
        BTreeMap::new(),
        2,
        &authorization,
        &remove_deny,
    )
    .is_err());
}

#[test]
fn mutation_checkpoint_rejects_mismatched_operation_scopes_and_tokens_reject_stale_revisions() {
    let actor_snapshot = hierarchy_snapshot("sysadmin", collection());
    let versioned =
        VersionedControlSnapshot::new(actor_snapshot.clone(), 1, BTreeMap::new()).unwrap();
    let collection_path =
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/policies/change";
    let control_request = request("PUT", collection_path, collection());
    let platform_changes = ControlChangeSet {
        idempotency_key: Some("scope-mismatch".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutPathPolicy(PathPolicy::new(
                "platform-policy",
                "viewer",
                ControlScope::Platform,
                PolicyEffect::Allow,
                ["GET"],
                ["/_control/v1/platform/settings"],
            )),
        }],
    };
    assert!(checkpoint_authorization(
        &subject(),
        &control_request,
        AuditRequestContext {
            method: "PUT".to_string(),
            canonical_path: collection_path.to_string(),
            correlation_id: "scope-bound-token".to_string(),
        },
        &versioned,
        ControlRouteDescriptor::CollectionPathPolicy,
        &platform_changes,
    )
    .is_err());

    let legitimate_changes = put_collection(actor_snapshot.config.collections[0].clone());
    let authorization = mutation_authorization_for_descriptor(
        &actor_snapshot,
        ControlRouteDescriptor::Collection,
        "PUT",
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a",
        &legitimate_changes,
    );
    assert!(matches!(
        apply_control_changes(
            actor_snapshot,
            BTreeMap::new(),
            3,
            &authorization,
            &legitimate_changes,
        ),
        Err(tellurion_core::Error::ControlRevisionConflict {
            expected: 1,
            current: 2,
        })
    ));
}

#[test]
fn mutation_token_rejects_a_different_authoritative_snapshot_at_the_same_revision() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let mut caller_snapshot = authoritative.clone();
    caller_snapshot.config.server.port = 9_001;
    let changes = put_tenant(authoritative.config.tenants[0].clone());
    let authorization = mutation_authorization_for_descriptor(
        &caller_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &changes,
    );

    assert!(matches!(
        apply_control_changes(
            authoritative,
            BTreeMap::new(),
            2,
            &authorization,
            &changes,
        ),
        Err(tellurion_core::Error::ControlValidation(message))
            if message.contains("authoritative")
    ));
}

#[test]
fn public_snapshot_mutation_cannot_change_the_private_authority_binding() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let mut caller =
        VersionedControlSnapshot::new(authoritative.clone(), 1, BTreeMap::new()).unwrap();
    caller.revision = 99;
    caller.snapshot.config.server.port = 9_002;
    caller.snapshot.role_bindings.clear();
    let control_request = request(
        "POST",
        "/_control/v1/platform/import",
        ControlScope::Platform,
    );
    let changes = put_tenant(authoritative.config.tenants[0].clone());
    let authorization = checkpoint_authorization(
        &subject(),
        &control_request,
        AuditRequestContext {
            method: "POST".to_string(),
            canonical_path: "/_control/v1/platform/import".to_string(),
            correlation_id: "private-snapshot-binding".to_string(),
        },
        &caller,
        ControlRouteDescriptor::PlatformBatchImport,
        &changes,
    )
    .expect("the immutable validated state still carries the original authority");

    assert!(apply_control_changes(
        caller.snapshot,
        BTreeMap::new(),
        2,
        &authorization,
        &changes,
    )
    .is_err());
    apply_control_changes(authoritative, BTreeMap::new(), 2, &authorization, &changes)
        .expect("the token remains bound to the originally validated state");
}

#[test]
fn deserialized_versioned_snapshots_cannot_mint_mutation_tokens() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let serialized = serde_json::to_string(
        &VersionedControlSnapshot::new(authoritative.clone(), 1, BTreeMap::new()).unwrap(),
    )
    .unwrap();
    let deserialized: VersionedControlSnapshot = serde_json::from_str(&serialized).unwrap();
    let changes = put_tenant(authoritative.config.tenants[0].clone());
    let control_request = request(
        "POST",
        "/_control/v1/platform/import",
        ControlScope::Platform,
    );

    assert!(checkpoint_authorization(
        &subject(),
        &control_request,
        AuditRequestContext {
            method: "POST".to_string(),
            canonical_path: "/_control/v1/platform/import".to_string(),
            correlation_id: "deserialized-snapshot".to_string(),
        },
        &deserialized,
        ControlRouteDescriptor::PlatformBatchImport,
        &changes,
    )
    .is_err());
}

#[test]
fn mutation_token_binds_the_private_entity_version_state() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let resource = "tenant/tenant-a".to_string();
    let authoritative_versions = BTreeMap::from([(resource.clone(), "5".to_string())]);
    let mut caller =
        VersionedControlSnapshot::new(authoritative.clone(), 1, authoritative_versions.clone())
            .unwrap();
    caller
        .entity_versions
        .insert(resource.clone(), "6".to_string());
    let changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("5".to_string()),
            operation: ControlOperation::PutTenant(authoritative.config.tenants[0].clone()),
        }],
    };
    let control_request = request(
        "POST",
        "/_control/v1/platform/import",
        ControlScope::Platform,
    );
    let authorization = checkpoint_authorization(
        &subject(),
        &control_request,
        AuditRequestContext {
            method: "POST".to_string(),
            canonical_path: "/_control/v1/platform/import".to_string(),
            correlation_id: "private-entity-versions".to_string(),
        },
        &caller,
        ControlRouteDescriptor::PlatformBatchImport,
        &changes,
    )
    .unwrap();

    apply_control_changes(
        authoritative.clone(),
        authoritative_versions,
        2,
        &authorization,
        &changes,
    )
    .expect("the token uses the entity versions captured at validated construction");
    assert!(matches!(
        apply_control_changes(
            authoritative,
            caller.entity_versions,
            2,
            &authorization,
            &changes,
        ),
        Err(tellurion_core::Error::ControlValidation(message))
            if message.contains("authoritative store state")
    ));
}

#[test]
fn mutation_token_rejects_modified_and_destructive_changesets() {
    let authoritative = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let mut intended_tenant = authoritative.config.tenants[0].clone();
    intended_tenant.settings.cache_ttl_s = Some(30);
    let intended = put_tenant(intended_tenant);
    let authorization = mutation_authorization_for_descriptor(
        &authoritative,
        ControlRouteDescriptor::TenantSettings,
        "PUT",
        "/_control/v1/tenants/tenant-a/settings",
        &intended,
    );

    let mut modified_tenant = authoritative.config.tenants[0].clone();
    modified_tenant.settings.cache_ttl_s = Some(60);
    assert!(apply_control_changes(
        authoritative.clone(),
        BTreeMap::new(),
        2,
        &authorization,
        &put_tenant(modified_tenant),
    )
    .is_err());

    let collection_scope = collection();
    let intended_collection = put_collection(authoritative.config.collections[0].clone());
    let collection_authorization = mutation_authorization_for_descriptor(
        &authoritative,
        ControlRouteDescriptor::Collection,
        "PUT",
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a",
        &intended_collection,
    );
    let destructive = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PermanentlyDeleteResource {
                scope: collection_scope,
            },
        }],
    };
    assert!(apply_control_changes(
        authoritative,
        BTreeMap::new(),
        2,
        &collection_authorization,
        &destructive,
    )
    .is_err());
}

#[test]
fn policy_replacement_requires_authority_over_existing_and_candidate_scopes() {
    let mut authoritative = two_tenant_snapshot(
        "sysadmin",
        ControlScope::Tenant {
            tenant_id: "tenant-a".to_string(),
        },
    );
    authoritative.path_policies.push(PathPolicy::new(
        "globally-unique-policy",
        "viewer",
        ControlScope::Tenant {
            tenant_id: "tenant-b".to_string(),
        },
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/tenant-b/**"],
    ));
    authoritative.validate().unwrap();
    let tenant_a = ControlScope::Tenant {
        tenant_id: "tenant-a".to_string(),
    };
    let cross_scope_replacement = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutPathPolicy(PathPolicy::new(
                "globally-unique-policy",
                "viewer",
                tenant_a.clone(),
                PolicyEffect::Allow,
                ["GET"],
                ["/_control/v1/tenants/tenant-a/**"],
            )),
        }],
    };
    assert!(checkpoint_with_descriptor(
        &authoritative,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &cross_scope_replacement,
    )
    .is_err());

    let mut platform_authoritative = authoritative.clone();
    platform_authoritative.role_bindings[0].scope = ControlScope::Platform;
    let platform_authorization = mutation_authorization_for_descriptor(
        &platform_authoritative,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &cross_scope_replacement,
    );
    apply_control_changes(
        platform_authoritative,
        BTreeMap::new(),
        2,
        &platform_authorization,
        &cross_scope_replacement,
    )
    .expect("platform authority can replace a policy after normal delegation checks");
}

#[test]
fn delegated_administrators_can_update_tenant_and_catalog_settings() {
    let tenant_snapshot = hierarchy_snapshot("tenant_admin", tenant());
    let mut tenant_update = tenant_snapshot.config.tenants[0].clone();
    tenant_update.settings.cache_ttl_s = Some(30);
    let tenant_changes = put_tenant(tenant_update);
    let tenant_authorization = mutation_authorization_for_descriptor(
        &tenant_snapshot,
        ControlRouteDescriptor::TenantSettings,
        "PATCH",
        "/_control/v1/tenants/tenant-a/settings",
        &tenant_changes,
    );
    apply_control_changes(
        tenant_snapshot,
        BTreeMap::new(),
        2,
        &tenant_authorization,
        &tenant_changes,
    )
    .expect("tenant administrator updates its tenant settings");

    let catalog_snapshot = hierarchy_snapshot("catalog_admin", catalog());
    let mut catalog_update = catalog_snapshot.config.catalogs[0].clone();
    catalog_update.settings.cache_ttl_s = Some(45);
    let catalog_changes = put_catalog(catalog_update);
    let catalog_authorization = mutation_authorization_for_descriptor(
        &catalog_snapshot,
        ControlRouteDescriptor::CatalogSettings,
        "PATCH",
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/settings",
        &catalog_changes,
    );
    apply_control_changes(
        catalog_snapshot,
        BTreeMap::new(),
        2,
        &catalog_authorization,
        &catalog_changes,
    )
    .expect("catalog administrator updates its catalog settings");
}

#[test]
fn parent_moves_require_authority_over_both_old_and_new_parents() {
    let tenant_b = ControlScope::Tenant {
        tenant_id: "tenant-b".to_string(),
    };
    let catalog_b = ControlScope::Catalog {
        tenant_id: "tenant-b".to_string(),
        catalog_id: "catalog-b".to_string(),
    };

    let catalog_snapshot = two_tenant_snapshot("sysadmin", tenant_b.clone());
    let mut moved_catalog = catalog_snapshot.config.catalogs[0].clone();
    moved_catalog.tenant = "tenant-b".to_string();
    let catalog_changes = put_catalog(moved_catalog);
    assert!(checkpoint_with_descriptor(
        &catalog_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &catalog_changes,
    )
    .is_err());

    let collection_snapshot = two_tenant_snapshot("sysadmin", catalog_b.clone());
    let mut moved_collection = collection_snapshot.config.collections[0].clone();
    moved_collection.catalog = "catalog-b".to_string();
    let collection_changes = put_collection(moved_collection);
    assert!(checkpoint_with_descriptor(
        &collection_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &collection_changes,
    )
    .is_err());
}

#[test]
fn tenant_collection_move_is_monosemantic_and_requires_both_parents() {
    let route = "/_control/v1/tenants/tenant-a/collection-moves";
    let tenant_snapshot = two_tenant_snapshot("tenant_admin", tenant());
    let mut moved = tenant_snapshot.config.collections[0].clone();
    moved.catalog = "catalog-a2".to_string();
    let move_changes = put_collection(moved.clone());

    let authorization = mutation_authorization_for_descriptor(
        &tenant_snapshot,
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &move_changes,
    );
    let applied = apply_control_changes(
        tenant_snapshot.clone(),
        BTreeMap::new(),
        2,
        &authorization,
        &move_changes,
    )
    .expect("a tenant administrator may move an existing collection between its catalogs");
    assert_eq!(applied.snapshot.config.collections[0].catalog, "catalog-a2");

    let replay_changes = ControlChangeSet {
        idempotency_key: Some("move-replay".to_string()),
        ..move_changes.clone()
    };
    let replay_authorization = checkpoint_with_descriptor(
        &applied.snapshot,
        applied.entity_versions.clone(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &replay_changes,
    )
    .expect("an exact already-applied move may mint only a replay lookup proof");
    assert!(replay_authorization.is_replay_only());
    assert!(checkpoint_with_descriptor(
        &applied.snapshot,
        applied.entity_versions.clone(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &move_changes,
    )
    .is_err());

    let mut mismatched_replay = applied.snapshot.config.collections[0].clone();
    mismatched_replay.external_id = Some("substitution".to_string());
    let mismatched_replay_changes = ControlChangeSet {
        idempotency_key: Some("move-replay".to_string()),
        ..put_collection(mismatched_replay)
    };
    assert!(checkpoint_with_descriptor(
        &applied.snapshot,
        applied.entity_versions.clone(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &mismatched_replay_changes,
    )
    .is_err());
    assert!(checkpoint_with_descriptor(
        &applied.snapshot,
        applied.entity_versions.clone(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        "/_control/v1/tenants/tenant-b/collection-moves",
        &replay_changes,
    )
    .is_err());

    let unchanged = put_collection(tenant_snapshot.config.collections[0].clone());
    assert!(checkpoint_with_descriptor(
        &tenant_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &unchanged,
    )
    .is_err());

    let mut field_substitution = moved.clone();
    field_substitution.external_id = Some("substitution".to_string());
    assert!(checkpoint_with_descriptor(
        &tenant_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &put_collection(field_substitution),
    )
    .is_err());

    let mut cross_tenant = moved;
    cross_tenant.catalog = "catalog-b".to_string();
    assert!(checkpoint_with_descriptor(
        &tenant_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &put_collection(cross_tenant),
    )
    .is_err());

    let mut nonexistent = tenant_snapshot.config.collections[0].clone();
    nonexistent.id = "missing".to_string();
    nonexistent.catalog = "catalog-a2".to_string();
    assert!(checkpoint_with_descriptor(
        &tenant_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &put_collection(nonexistent),
    )
    .is_err());

    let platform_snapshot = two_tenant_snapshot("sysadmin", ControlScope::Platform);
    let mut wrong_route = platform_snapshot.config.collections[0].clone();
    wrong_route.catalog = "catalog-a2".to_string();
    assert!(checkpoint_with_descriptor(
        &platform_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        "/_control/v1/tenants/tenant-b/collection-moves",
        &put_collection(wrong_route),
    )
    .is_err());

    let two_moves = ControlChangeSet {
        idempotency_key: None,
        operations: vec![
            move_changes.operations[0].clone(),
            move_changes.operations[0].clone(),
        ],
    };
    assert!(checkpoint_with_descriptor(
        &tenant_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &two_moves,
    )
    .is_err());

    let catalog_snapshot = two_tenant_snapshot("catalog_admin", catalog());
    let mut one_parent_only = catalog_snapshot.config.collections[0].clone();
    one_parent_only.catalog = "catalog-a2".to_string();
    assert!(checkpoint_with_descriptor(
        &catalog_snapshot,
        BTreeMap::new(),
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        route,
        &put_collection(one_parent_only),
    )
    .is_err());
}

#[test]
fn moves_migrate_entity_versions_and_report_old_and_new_resource_paths() {
    let authoritative = two_tenant_snapshot("sysadmin", ControlScope::Platform);
    let old_catalog = "tenant/tenant-a/catalog/catalog-a".to_string();
    let new_catalog = "tenant/tenant-b/catalog/catalog-a".to_string();
    let old_collection = "tenant/tenant-a/catalog/catalog-a/collection/collection-a".to_string();
    let new_collection = "tenant/tenant-b/catalog/catalog-a/collection/collection-a".to_string();
    let versions = BTreeMap::from([
        (old_catalog.clone(), "7".to_string()),
        (old_collection.clone(), "6".to_string()),
        ("unrelated".to_string(), "4".to_string()),
    ]);
    let mut moved_catalog = authoritative.config.catalogs[0].clone();
    moved_catalog.tenant = "tenant-b".to_string();
    let changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("7".to_string()),
            operation: ControlOperation::PutCatalog(moved_catalog),
        }],
    };
    let authorization = mutation_authorization_with_state(
        &authoritative,
        1,
        versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &changes,
    );
    let moved = apply_control_changes(authoritative, versions, 2, &authorization, &changes)
        .expect("catalog move checks the authoritative ETag and migrates descendants");
    assert_eq!(
        moved.changed_resources,
        vec![
            old_catalog.clone(),
            old_collection.clone(),
            new_catalog.clone(),
            new_collection.clone(),
        ]
    );
    assert!(!moved.entity_versions.contains_key(&old_catalog));
    assert!(!moved.entity_versions.contains_key(&old_collection));
    assert_eq!(
        moved.entity_versions.get(&new_catalog).map(String::as_str),
        Some("2")
    );
    assert_eq!(
        moved
            .entity_versions
            .get(&new_collection)
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        moved.entity_versions.get("unrelated").map(String::as_str),
        Some("4")
    );

    let mut moved_back = moved.snapshot.config.catalogs[0].clone();
    moved_back.tenant = "tenant-a".to_string();
    let move_back_changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("2".to_string()),
            operation: ControlOperation::PutCatalog(moved_back),
        }],
    };
    let move_back_authorization = mutation_authorization_with_state(
        &moved.snapshot,
        2,
        moved.entity_versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &move_back_changes,
    );
    let restored = apply_control_changes(
        moved.snapshot,
        moved.entity_versions,
        3,
        &move_back_authorization,
        &move_back_changes,
    )
    .expect("moving back checks the current path and does not resurrect stale keys");
    assert!(!restored.entity_versions.contains_key(&new_catalog));
    assert!(!restored.entity_versions.contains_key(&new_collection));
    assert_eq!(
        restored
            .entity_versions
            .get(&old_catalog)
            .map(String::as_str),
        Some("3")
    );
    assert_eq!(
        restored
            .entity_versions
            .get(&old_collection)
            .map(String::as_str),
        Some("3")
    );

    let collection_snapshot = two_tenant_snapshot("sysadmin", ControlScope::Platform);
    let old_collection = "tenant/tenant-a/catalog/catalog-a/collection/collection-a".to_string();
    let new_collection = "tenant/tenant-a/catalog/catalog-a2/collection/collection-a".to_string();
    let collection_versions = BTreeMap::from([(old_collection.clone(), "11".to_string())]);
    let mut moved_collection = collection_snapshot.config.collections[0].clone();
    moved_collection.catalog = "catalog-a2".to_string();
    let collection_changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: Some("11".to_string()),
            operation: ControlOperation::PutCollection(moved_collection),
        }],
    };
    let collection_authorization = mutation_authorization_with_state(
        &collection_snapshot,
        1,
        collection_versions.clone(),
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &collection_changes,
    );
    let moved_collection = apply_control_changes(
        collection_snapshot,
        collection_versions,
        2,
        &collection_authorization,
        &collection_changes,
    )
    .expect("collection move checks its authoritative ETag");
    assert_eq!(
        moved_collection.changed_resources,
        vec![old_collection.clone(), new_collection.clone()]
    );
    assert!(!moved_collection
        .entity_versions
        .contains_key(&old_collection));
    assert_eq!(
        moved_collection
            .entity_versions
            .get(&new_collection)
            .map(String::as_str),
        Some("2")
    );
}

#[test]
fn creates_and_same_parent_updates_keep_their_delegated_scopes() {
    let tenant_scope = tenant();
    let tenant_snapshot = hierarchy_snapshot("tenant_admin", tenant_scope.clone());
    let create_catalog = CatalogDecl {
        id: "catalog-new".to_string(),
        external_id: None,
        tenant: "tenant-a".to_string(),
        settings: SettingsDecl::default(),
        visibility: Default::default(),
    };
    let catalog_changes = put_catalog(create_catalog);
    let tenant_authorization = mutation_authorization_for_descriptor(
        &tenant_snapshot,
        ControlRouteDescriptor::TenantCatalogs,
        "POST",
        "/_control/v1/tenants/tenant-a/catalogs",
        &catalog_changes,
    );
    apply_control_changes(
        tenant_snapshot,
        BTreeMap::new(),
        2,
        &tenant_authorization,
        &catalog_changes,
    )
    .expect("tenant administrator creates a catalog in its tenant");

    let atomic_snapshot = hierarchy_snapshot("sysadmin", ControlScope::Platform);
    let mut atomic_collection = atomic_snapshot.config.collections[0].clone();
    atomic_collection.id = "collection-batch".to_string();
    atomic_collection.external_id = None;
    atomic_collection.catalog = "catalog-batch".to_string();
    let atomic_changes = ControlChangeSet {
        idempotency_key: None,
        operations: vec![
            VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutCatalog(CatalogDecl {
                    id: "catalog-batch".to_string(),
                    external_id: None,
                    tenant: "tenant-a".to_string(),
                    settings: SettingsDecl::default(),
                    visibility: Default::default(),
                }),
            },
            VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutCollection(atomic_collection),
            },
        ],
    };
    let atomic_authorization = mutation_authorization_for_descriptor(
        &atomic_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &atomic_changes,
    );
    apply_control_changes(
        atomic_snapshot,
        BTreeMap::new(),
        2,
        &atomic_authorization,
        &atomic_changes,
    )
    .expect("the explicit platform import atomically creates a catalog and child collection");

    let catalog_scope = catalog();
    let catalog_snapshot = hierarchy_snapshot("catalog_admin", catalog_scope.clone());
    let mut create_collection = catalog_snapshot.config.collections[0].clone();
    create_collection.id = "collection-new".to_string();
    create_collection.external_id = None;
    let collection_changes = put_collection(create_collection);
    let catalog_authorization = mutation_authorization_for_descriptor(
        &catalog_snapshot,
        ControlRouteDescriptor::CatalogCollections,
        "POST",
        "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections",
        &collection_changes,
    );
    apply_control_changes(
        catalog_snapshot,
        BTreeMap::new(),
        2,
        &catalog_authorization,
        &collection_changes,
    )
    .expect("catalog administrator creates a collection in its catalog");

    let tenant_changes = put_tenant(TenantDecl {
        id: "tenant-new".to_string(),
        external_id: None,
        settings: SettingsDecl::default(),
    });
    let tenant_only_snapshot = two_tenant_snapshot("tenant_admin", tenant_scope.clone());
    let tenant_only_versioned =
        VersionedControlSnapshot::new(tenant_only_snapshot, 1, BTreeMap::new()).unwrap();
    let platform_import = ControlRouteDescriptor::PlatformBatchImport;
    let registry = ControlRouteRegistry::new([platform_import]).unwrap();
    assert!(authorize_control_mutation(
        &subject(),
        "POST",
        b"/_control/v1/platform/import",
        platform_import.template(),
        &registry,
        "",
        &tenant_only_versioned,
        &tenant_changes,
        "tenant-cannot-create-tenant",
    )
    .is_err());

    let platform_snapshot = two_tenant_snapshot("sysadmin", ControlScope::Platform);
    let platform_authorization = mutation_authorization_for_descriptor(
        &platform_snapshot,
        ControlRouteDescriptor::Tenants,
        "POST",
        "/_control/v1/tenants",
        &tenant_changes,
    );
    apply_control_changes(
        platform_snapshot,
        BTreeMap::new(),
        2,
        &platform_authorization,
        &tenant_changes,
    )
    .expect("platform administrator creates a tenant");

    let move_snapshot = two_tenant_snapshot("tenant_admin", tenant_scope);
    let mut moved_collection = move_snapshot.config.collections[0].clone();
    moved_collection.catalog = "catalog-a2".to_string();
    let move_changes = put_collection(moved_collection);
    let move_authorization = mutation_authorization_for_descriptor(
        &move_snapshot,
        ControlRouteDescriptor::TenantCollectionMove,
        "POST",
        "/_control/v1/tenants/tenant-a/collection-moves",
        &move_changes,
    );
    apply_control_changes(
        move_snapshot,
        BTreeMap::new(),
        2,
        &move_authorization,
        &move_changes,
    )
    .expect("tenant collection move covers both catalogs in the same tenant");
}

#[test]
fn delegation_rejects_nonexistent_scopes_even_for_platform_actors() {
    let actor_snapshot = snapshot("sysadmin", ControlScope::Platform);
    let delegated = PathPolicy::new(
        "ghost-tenant",
        "viewer",
        ControlScope::Tenant {
            tenant_id: "missing".to_string(),
        },
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/tenants/missing/**"],
    );

    assert_eq!(
        validate_delegated_policy(&subject(), &delegated, &actor_snapshot),
        Err(tellurion_core::DelegationError::InvalidStatement)
    );
}

#[test]
fn role_binding_mutations_cannot_delegate_a_more_powerful_role() {
    let mut actor_snapshot = snapshot("binding_writer", ControlScope::Platform);
    actor_snapshot.path_policies.push(PathPolicy::new(
        "binding-writer",
        "binding_writer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["POST"],
        ["/_control/v1/platform/role-bindings/**"],
    ));
    actor_snapshot.validate().unwrap();
    let delegated = RoleBinding {
        principal: PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "other-operator".to_string(),
        },
        role: "sysadmin".to_string(),
        scope: ControlScope::Platform,
    };

    assert_eq!(
        validate_delegated_role_binding(&subject().principal, &delegated, &actor_snapshot),
        Err(DelegationError::OutsideAllowEnvelope)
    );

    let changes = ControlChangeSet {
        idempotency_key: Some("bind-sysadmin".to_string()),
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutRoleBinding(delegated),
        }],
    };
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformRoleBindings,
        "POST",
        "/_control/v1/platform/role-bindings",
        &changes,
    );
    assert!(
        apply_control_changes(actor_snapshot, BTreeMap::new(), 2, &authorization, &changes,)
            .is_err()
    );

    let mut target_snapshot = snapshot("binding_writer", ControlScope::Platform);
    target_snapshot.path_policies.push(PathPolicy::new(
        "binding-writer",
        "binding_writer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["POST"],
        ["/_control/v1/platform/role-bindings/**"],
    ));
    let powerful_policy = PathPolicy::new(
        "platform-settings-writer",
        "powerful_custom_role",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["PATCH"],
        ["/_control/v1/platform/settings"],
    );
    target_snapshot.path_policies.push(powerful_policy.clone());
    target_snapshot.validate().unwrap();
    let custom_binding = RoleBinding {
        principal: PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "other-operator".to_string(),
        },
        role: "powerful_custom_role".to_string(),
        scope: ControlScope::Platform,
    };
    assert_eq!(
        validate_delegated_role_binding(&subject().principal, &custom_binding, &target_snapshot),
        Err(DelegationError::OutsideAllowEnvelope)
    );

    let base_snapshot = snapshot("binding_writer", ControlScope::Platform);
    let mut base_snapshot = base_snapshot;
    base_snapshot.path_policies.push(PathPolicy::new(
        "binding-writer",
        "binding_writer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["POST"],
        ["/_control/v1/platform/role-bindings/**"],
    ));
    base_snapshot.path_policies.push(PathPolicy::new(
        "typed-platform-batch-fixture",
        "binding_writer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["POST"],
        ["/_control/v1/platform/import"],
    ));
    base_snapshot.validate().unwrap();
    let same_batch = ControlChangeSet {
        idempotency_key: Some("create-and-bind-powerful-role".to_string()),
        operations: vec![
            VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutPathPolicy(powerful_policy),
            },
            VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutRoleBinding(custom_binding),
            },
        ],
    };
    let authorization = mutation_authorization(
        &base_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &same_batch,
    );
    assert!(apply_control_changes(
        base_snapshot,
        BTreeMap::new(),
        2,
        &authorization,
        &same_batch,
    )
    .is_err());
}

#[test]
fn role_binding_delegation_allows_roles_within_the_actor_envelope() {
    let actor_snapshot = snapshot("sysadmin", ControlScope::Platform);
    let delegated = RoleBinding {
        principal: PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "viewer-operator".to_string(),
        },
        role: "viewer".to_string(),
        scope: ControlScope::Platform,
    };

    validate_delegated_role_binding(&subject().principal, &delegated, &actor_snapshot)
        .expect("sysadmin can delegate the read-only viewer role");
}

fn binding_deletion(principal: PrincipalIdentity, role: &str) -> ControlChangeSet {
    ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::DeleteRoleBinding {
                principal,
                scope: ControlScope::Platform,
                role: role.to_string(),
            },
        }],
    }
}

fn binding_deletion_path(changes: &ControlChangeSet) -> String {
    let [operation] = changes.operations.as_slice() else {
        panic!("binding deletion fixture must contain exactly one operation");
    };
    let ControlOperation::DeleteRoleBinding {
        principal,
        scope,
        role,
    } = &operation.operation
    else {
        panic!("binding deletion fixture must contain DeleteRoleBinding");
    };
    let target = role_binding_target_id(&RoleBinding {
        principal: principal.clone(),
        role: role.clone(),
        scope: scope.clone(),
    });
    format!("/_control/v1/platform/role-bindings/{target}")
}

fn scoped_binding_deletion(
    principal: PrincipalIdentity,
    scope: ControlScope,
    role: &str,
) -> ControlChangeSet {
    ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::DeleteRoleBinding {
                principal,
                scope,
                role: role.to_string(),
            },
        }],
    }
}

fn scoped_deny_removal_snapshot(
    removed_scope: ControlScope,
) -> (ControlSnapshot, PrincipalIdentity) {
    let mut actor_snapshot = snapshot("batch_operator", ControlScope::Platform);
    actor_snapshot.role_bindings.push(RoleBinding {
        principal: subject().principal.clone(),
        role: "collection_deny_remover".to_string(),
        scope: ControlScope::Collection {
            tenant_id: "tenant-a".to_string(),
            catalog_id: "catalog-a".to_string(),
            collection_id: "collection-a".to_string(),
        },
    });
    let restricted = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "restricted-operator".to_string(),
    };
    actor_snapshot.role_bindings.push(RoleBinding {
        principal: restricted.clone(),
        role: "restricted_role".to_string(),
        scope: removed_scope,
    });
    actor_snapshot.path_policies.extend([
        PathPolicy::new(
            "batch-authorizer",
            "batch_operator",
            ControlScope::Platform,
            PolicyEffect::Allow,
            ["POST"],
            ["/_control/v1/platform/import"],
        ),
        PathPolicy::new(
            "collection-deny-removal-capability",
            "collection_deny_remover",
            ControlScope::Platform,
            PolicyEffect::Allow,
            ["PATCH"],
            ["/_control/v1/**"],
        ),
        PathPolicy::new(
            "restricted-role-deny",
            "restricted_role",
            ControlScope::Platform,
            PolicyEffect::Deny,
            ["PATCH"],
            ["/_control/v1/**"],
        ),
    ]);
    (actor_snapshot, restricted)
}

fn binding_deletion_snapshot() -> ControlSnapshot {
    let mut actor_snapshot = snapshot("binding_writer", ControlScope::Platform);
    actor_snapshot.path_policies.push(PathPolicy::new(
        "binding-delete",
        "binding_writer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["DELETE"],
        ["/_control/v1/platform/role-bindings/**"],
    ));
    actor_snapshot.path_policies.push(PathPolicy::new(
        "typed-platform-batch-fixture",
        "binding_writer",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["POST"],
        ["/_control/v1/platform/import"],
    ));
    actor_snapshot.path_policies.push(PathPolicy::new(
        "binding-writer-deny",
        "binding_writer",
        ControlScope::Platform,
        PolicyEffect::Deny,
        ["PATCH"],
        ["/_control/v1/platform/settings/secrets/**"],
    ));
    actor_snapshot.path_policies.push(PathPolicy::new(
        "restricted-role-deny",
        "restricted_role",
        ControlScope::Platform,
        PolicyEffect::Deny,
        ["PATCH"],
        ["/_control/v1/platform/settings/secrets/**"],
    ));
    actor_snapshot
}

#[test]
fn delete_role_binding_cannot_remove_the_actors_effective_deny() {
    let actor_snapshot = binding_deletion_snapshot();
    let changes = binding_deletion(subject().principal.clone(), "binding_writer");
    let path = binding_deletion_path(&changes);
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformRoleBinding,
        "DELETE",
        &path,
        &changes,
    );

    assert!(
        apply_control_changes(actor_snapshot, BTreeMap::new(), 2, &authorization, &changes,)
            .is_err()
    );
}

#[test]
fn delete_role_binding_cannot_remove_another_principals_effective_deny() {
    let mut actor_snapshot = binding_deletion_snapshot();
    let other = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "restricted-operator".to_string(),
    };
    actor_snapshot.role_bindings.push(RoleBinding {
        principal: other.clone(),
        role: "restricted_role".to_string(),
        scope: ControlScope::Platform,
    });
    let changes = binding_deletion(other, "restricted_role");
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &changes,
    );

    assert!(
        apply_control_changes(actor_snapshot, BTreeMap::new(), 2, &authorization, &changes,)
            .is_err()
    );
}

#[test]
fn same_batch_deny_binding_deletion_then_privilege_widening_is_rejected() {
    let actor_snapshot = binding_deletion_snapshot();
    let mut changes = binding_deletion(subject().principal.clone(), "binding_writer");
    changes.operations.push(VersionedControlOperation {
        expected_entity_version: None,
        operation: ControlOperation::PutPathPolicy(PathPolicy::new(
            "widen-after-self-removal",
            "binding_writer",
            ControlScope::Platform,
            PolicyEffect::Allow,
            ["PATCH"],
            ["/_control/v1/platform/settings/secrets/**"],
        )),
    });
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &changes,
    );

    assert!(
        apply_control_changes(actor_snapshot, BTreeMap::new(), 2, &authorization, &changes,)
            .is_err()
    );
}

#[test]
fn authorized_admin_can_delete_an_allow_only_binding() {
    let mut actor_snapshot = binding_deletion_snapshot();
    let viewer = PrincipalIdentity {
        issuer: "https://issuer.example".to_string(),
        subject: "viewer-operator".to_string(),
    };
    actor_snapshot.role_bindings.push(RoleBinding {
        principal: viewer.clone(),
        role: "viewer".to_string(),
        scope: ControlScope::Platform,
    });
    let changes = binding_deletion(viewer.clone(), "viewer");
    let path = binding_deletion_path(&changes);
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformRoleBinding,
        "DELETE",
        &path,
        &changes,
    );

    let applied =
        apply_control_changes(actor_snapshot, BTreeMap::new(), 2, &authorization, &changes)
            .expect("deleting an allow-only binding reduces authority");
    assert!(applied
        .snapshot
        .role_bindings
        .iter()
        .all(|binding| binding.principal != viewer));
}

#[test]
fn collection_scoped_binding_cannot_cover_tenant_or_catalog_deny_removal() {
    let attacks = [
        ControlScope::Tenant {
            tenant_id: "tenant-a".to_string(),
        },
        ControlScope::Catalog {
            tenant_id: "tenant-a".to_string(),
            catalog_id: "catalog-a".to_string(),
        },
    ];

    for removed_scope in attacks {
        let (actor_snapshot, restricted) = scoped_deny_removal_snapshot(removed_scope.clone());
        let changes = scoped_binding_deletion(restricted, removed_scope, "restricted_role");
        let authorization = mutation_authorization(
            &actor_snapshot,
            ControlRouteDescriptor::PlatformBatchImport,
            "POST",
            "/_control/v1/platform/import",
            &changes,
        );

        assert!(
            apply_control_changes(actor_snapshot, BTreeMap::new(), 2, &authorization, &changes)
                .is_err(),
            "a collection binding must not cover a broader deny removal"
        );
    }
}

#[test]
fn collection_scoped_binding_can_cover_a_contained_collection_deny_removal() {
    let removed_scope = ControlScope::Collection {
        tenant_id: "tenant-a".to_string(),
        catalog_id: "catalog-a".to_string(),
        collection_id: "collection-a".to_string(),
    };
    let (actor_snapshot, restricted) = scoped_deny_removal_snapshot(removed_scope.clone());
    let changes = scoped_binding_deletion(restricted, removed_scope, "restricted_role");
    let authorization = mutation_authorization(
        &actor_snapshot,
        ControlRouteDescriptor::PlatformBatchImport,
        "POST",
        "/_control/v1/platform/import",
        &changes,
    );

    apply_control_changes(actor_snapshot, BTreeMap::new(), 2, &authorization, &changes)
        .expect("a collection binding may cover deny removal inside the same collection");
}

#[test]
fn legacy_snapshot_json_deserializes_without_role_or_scope_fields() {
    let legacy = r#"{
        "config": {},
        "role_bindings": [],
        "path_policies": [{
            "id": "legacy",
            "effect": "allow",
            "methods": ["GET"],
            "patterns": ["/_control/v1/platform/**"],
            "conditions": []
        }],
        "tombstoned_resources": []
    }"#;

    let snapshot: ControlSnapshot = serde_json::from_str(legacy).unwrap();
    assert_eq!(snapshot.path_policies[0].role, None);
    assert_eq!(snapshot.path_policies[0].scope, None);
    snapshot.validate().expect("legacy snapshot remains valid");
}

#[test]
fn inert_legacy_policies_keep_the_pre_hierarchy_absolute_pattern_grammar() {
    let legacy = r#"{
        "config": {},
        "role_bindings": [],
        "path_policies": [{
            "id": "legacy-absolute",
            "effect": "allow",
            "methods": ["GET"],
            "patterns": ["/administration/tenants/**"],
            "conditions": []
        }],
        "tombstoned_resources": []
    }"#;

    let snapshot: ControlSnapshot = serde_json::from_str(legacy).unwrap();
    snapshot
        .validate()
        .expect("role-less legacy policy is inert and remains loadable");
}

#[test]
fn inert_legacy_policies_remain_inert_but_use_the_active_pattern_grammar() {
    let compatible = [
        "/",
        "/_control/v1/**",
        "/administration/tenants",
        "/administration/*/tenants",
        "/administration/%2F/tenants",
    ];
    for (index, pattern) in compatible.iter().enumerate() {
        let mut candidate = hierarchy_snapshot("legacy-role", ControlScope::Platform);
        candidate.path_policies.push(PathPolicy::legacy(
            format!("legacy-{index}"),
            PolicyEffect::Allow,
            ["GET"],
            [*pattern],
            Vec::new(),
        ));
        candidate
            .validate()
            .unwrap_or_else(|error| panic!("legacy pattern {pattern:?} must load: {error}"));
        assert_eq!(
            authorize_control(
                &subject(),
                &request(
                    "GET",
                    "/_control/v1/platform/settings",
                    ControlScope::Platform,
                ),
                &candidate,
            ),
            ControlDecision::Deny,
            "legacy pattern {pattern:?} must remain inert",
        );
    }

    for invalid in [
        "",
        "administration/tenants",
        "**",
        "/administration/ten*nts",
        "/administration//tenants",
        "/administration/./tenants",
        "/administration/../tenants",
        "/administration/**/tenants",
    ] {
        let mut candidate = hierarchy_snapshot("legacy-role", ControlScope::Platform);
        candidate.path_policies.push(PathPolicy::legacy(
            "legacy-relative",
            PolicyEffect::Allow,
            ["GET"],
            [invalid],
            Vec::new(),
        ));
        assert!(candidate.validate().is_err(), "invalid pattern {invalid:?}");
    }
}

#[test]
fn snapshot_validation_compiles_deserialized_custom_patterns() {
    let serialized = r#"{
        "config": {},
        "role_bindings": [{
            "principal": {
                "issuer": "https://issuer.example",
                "subject": "operator-1"
            },
            "role": "service_account",
            "scope": {"kind": "platform"}
        }],
        "path_policies": [{
            "id": "compiler-probe",
            "role": "service_account",
            "scope": {"kind": "platform"},
            "effect": "allow",
            "methods": ["GET"],
            "patterns": ["/_control/v1/platform/compiler-probe"],
            "conditions": []
        }],
        "tombstoned_resources": []
    }"#;
    let snapshot: ControlSnapshot = serde_json::from_str(serialized).unwrap();
    snapshot.validate().expect("snapshot compilation succeeds");

    assert_eq!(
        authorize_control(
            &subject(),
            &request(
                "GET",
                "/_control/v1/platform/compiler-probe",
                ControlScope::Platform,
            ),
            &snapshot,
        ),
        ControlDecision::Allow
    );
}

#[test]
fn a_policy_mutated_after_validation_fails_closed_until_revalidated() {
    let mut snapshot = snapshot("service_account", ControlScope::Platform);
    snapshot.path_policies.push(PathPolicy::new(
        "validated-reader",
        "service_account",
        ControlScope::Platform,
        PolicyEffect::Allow,
        ["GET"],
        ["/_control/v1/platform/compiler-probe"],
    ));
    let validated = snapshot.validated().unwrap();
    snapshot.path_policies[0].methods = vec!["DELETE".to_string()];

    assert_eq!(
        authorize_validated_control(
            &subject(),
            &request(
                "DELETE",
                "/_control/v1/platform/compiler-probe",
                ControlScope::Platform,
            ),
            &validated,
        ),
        ControlDecision::Deny
    );
}
