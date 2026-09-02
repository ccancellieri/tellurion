use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tellurion_control::{
    control_mutation_checkpoint, control_read_checkpoint, ControlMiddlewareError,
    ControlRouteDescriptor, ControlRouteRegistry,
};
use tellurion_core::{
    AppConfig, AuthenticatedSubject, ControlChangeSet, ControlOperation, ControlScope,
    ControlSnapshot, Error, MutationControlDecision as ControlDecision, PrincipalIdentity,
    Resolver as CoreResolver, RoleBinding, VersionedControlOperation, VersionedControlSnapshot,
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

fn snapshot() -> ControlSnapshot {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-internal, external_id: acme } ]
catalogs: [ { id: catalog-internal, external_id: cadastre, tenant: tenant-internal } ]
collections:
  - id: collection-internal
    external_id: roads
    catalog: catalog-internal
    storage: main
"#,
    )
    .unwrap();
    ControlSnapshot {
        config,
        role_bindings: vec![RoleBinding {
            principal: subject().principal.clone(),
            role: "catalog_admin".to_string(),
            scope: ControlScope::Collection {
                tenant_id: "tenant-internal".to_string(),
                catalog_id: "catalog-internal".to_string(),
                collection_id: "collection-internal".to_string(),
            },
        }],
        path_policies: Vec::new(),
        tombstoned_resources: Vec::new(),
    }
}

fn versioned(revision: u64) -> VersionedControlSnapshot {
    VersionedControlSnapshot::new(snapshot(), revision, BTreeMap::new()).unwrap()
}

fn route_registry() -> ControlRouteRegistry {
    ControlRouteRegistry::new([
        ControlRouteDescriptor::PlatformSettings,
        ControlRouteDescriptor::Catalog,
        ControlRouteDescriptor::Collection,
        ControlRouteDescriptor::CollectionMetadata,
        ControlRouteDescriptor::CollectionAsset,
    ])
    .unwrap()
}

fn platform_versioned(revision: u64) -> VersionedControlSnapshot {
    let mut candidate = snapshot();
    candidate.role_bindings[0].role = "sysadmin".to_string();
    candidate.role_bindings[0].scope = ControlScope::Platform;
    VersionedControlSnapshot::new(candidate, revision, BTreeMap::new()).unwrap()
}

fn mutation_changes() -> ControlChangeSet {
    ControlChangeSet {
        idempotency_key: None,
        operations: vec![VersionedControlOperation {
            expected_entity_version: None,
            operation: ControlOperation::PutCollection(snapshot().config.collections[0].clone()),
        }],
    }
}

#[derive(Default)]
struct Resolver {
    calls: AtomicUsize,
    alias_tenant: AtomicBool,
    false_catalog_owner: AtomicBool,
    false_collection_owner: AtomicBool,
}

struct OwnershipMismatchResolver;

#[async_trait::async_trait]
impl CoreResolver for OwnershipMismatchResolver {
    async fn resolve_tenant(&self, external_id: &str) -> tellurion_core::Result<String> {
        (external_id == "acme")
            .then(|| "tenant-a".to_string())
            .ok_or(Error::NotFound)
    }

    async fn resolve_catalog(
        &self,
        tenant_internal_id: &str,
        external_id: &str,
    ) -> tellurion_core::Result<String> {
        (tenant_internal_id == "tenant-a" && external_id == "cadastre")
            .then(|| "catalog-internal".to_string())
            .ok_or(Error::NotFound)
    }

    async fn resolve_collection(
        &self,
        catalog_internal_id: &str,
        external_id: &str,
    ) -> tellurion_core::Result<String> {
        (catalog_internal_id == "catalog-internal" && external_id == "roads")
            .then(|| "collection-internal".to_string())
            .ok_or(Error::NotFound)
    }

    fn tenant_external_id(&self, tenant_internal_id: &str) -> Option<&str> {
        (tenant_internal_id == "tenant-a").then_some("acme")
    }

    fn catalog_external_id(&self, catalog_internal_id: &str) -> Option<&str> {
        (catalog_internal_id == "catalog-internal").then_some("cadastre")
    }

    fn collection_external_id(&self, collection_internal_id: &str) -> Option<&str> {
        (collection_internal_id == "collection-internal").then_some("roads")
    }

    fn catalogs_for_tenant(&self, _: &str) -> Vec<(&str, &str)> {
        Vec::new()
    }

    fn catalog_count(&self) -> usize {
        1
    }
}

fn ownership_mismatch_versioned() -> VersionedControlSnapshot {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a, external_id: acme }, { id: tenant-b, external_id: beta } ]
catalogs: [ { id: catalog-internal, external_id: cadastre, tenant: tenant-b } ]
collections:
  - id: collection-internal
    external_id: roads
    catalog: catalog-internal
    storage: main
"#,
    )
    .unwrap();
    VersionedControlSnapshot::new(
        ControlSnapshot {
            config,
            role_bindings: vec![RoleBinding {
                principal: subject().principal.clone(),
                role: "sysadmin".to_string(),
                scope: ControlScope::Platform,
            }],
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        },
        1,
        BTreeMap::new(),
    )
    .unwrap()
}

#[async_trait::async_trait]
impl CoreResolver for Resolver {
    async fn resolve_tenant(&self, external_id: &str) -> tellurion_core::Result<String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if external_id != "acme"
            && !(external_id == "acme-alias" && self.alias_tenant.load(Ordering::Relaxed))
        {
            return Err(Error::NotFound);
        }
        Ok("tenant-internal".to_string())
    }

    async fn resolve_catalog(
        &self,
        tenant_internal_id: &str,
        external_id: &str,
    ) -> tellurion_core::Result<String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if external_id != "cadastre"
            || tenant_internal_id != "tenant-internal"
            || self.false_catalog_owner.load(Ordering::Relaxed)
        {
            return Err(Error::NotFound);
        }
        Ok("catalog-internal".to_string())
    }

    async fn resolve_collection(
        &self,
        catalog_internal_id: &str,
        external_id: &str,
    ) -> tellurion_core::Result<String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if external_id != "roads"
            || catalog_internal_id != "catalog-internal"
            || self.false_collection_owner.load(Ordering::Relaxed)
        {
            return Err(Error::NotFound);
        }
        Ok("collection-internal".to_string())
    }

    fn tenant_external_id(&self, tenant_internal_id: &str) -> Option<&str> {
        (tenant_internal_id == "tenant-internal").then_some("acme")
    }

    fn catalog_external_id(&self, catalog_internal_id: &str) -> Option<&str> {
        (catalog_internal_id == "catalog-internal").then_some("cadastre")
    }

    fn collection_external_id(&self, collection_internal_id: &str) -> Option<&str> {
        (collection_internal_id == "collection-internal").then_some("roads")
    }

    fn catalogs_for_tenant(&self, _: &str) -> Vec<(&str, &str)> {
        Vec::new()
    }

    fn catalog_count(&self) -> usize {
        1
    }
}

#[tokio::test]
async fn authorized_mutation_runs_one_checkpoint_and_attaches_audit_context() {
    let resolver = Resolver::default();
    let handler_calls = AtomicUsize::new(0);
    let context = RefCell::new(None);

    control_mutation_checkpoint(
        &subject(),
        "PUT",
        b"/gateway/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}",
        &route_registry(),
        "/gateway",
        &versioned(42),
        &mutation_changes(),
        &resolver,
        "correlation-42",
        |authorized| {
            handler_calls.fetch_add(1, Ordering::Relaxed);
            context.replace(Some(authorized));
        },
    )
    .await
    .expect("authorized request reaches handler");

    assert_eq!(resolver.calls.load(Ordering::Relaxed), 3);
    assert_eq!(handler_calls.load(Ordering::Relaxed), 1);
    let context = context.into_inner().unwrap();
    assert_eq!(context.principal(), &subject().principal);
    assert_eq!(context.decision_context().decision, ControlDecision::Allow);
    assert_eq!(context.snapshot_revision(), 42);
    assert_eq!(context.audit_request().method, "PUT");
    assert_eq!(context.audit_request().correlation_id, "correlation-42");
    assert_eq!(
        context.audit_request().canonical_path,
        "/_control/v1/tenants/acme/catalogs/cadastre/collections/roads"
    );
    assert_eq!(
        context.effective_scope(),
        &ControlScope::Collection {
            tenant_id: "tenant-internal".to_string(),
            catalog_id: "catalog-internal".to_string(),
            collection_id: "collection-internal".to_string(),
        }
    );
}

#[tokio::test]
async fn mutation_checkpoint_accepts_the_runtime_resolver_trait_object() {
    let concrete = Resolver::default();
    let resolver: &dyn CoreResolver = &concrete;

    let result = control_mutation_checkpoint(
        &subject(),
        "PUT",
        b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}",
        &route_registry(),
        "",
        &versioned(42),
        &mutation_changes(),
        resolver,
        "correlation-42",
        |authorized| authorized.snapshot_revision(),
    )
    .await;

    assert_eq!(result.unwrap(), 42);
}

#[tokio::test]
async fn mutation_revision_is_derived_from_the_versioned_snapshot() {
    let versioned = VersionedControlSnapshot::new(snapshot(), 77, BTreeMap::new()).unwrap();
    let resolver = Resolver::default();

    control_mutation_checkpoint(
        &subject(),
        "PUT",
        b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}",
        &route_registry(),
        "",
        &versioned,
        &mutation_changes(),
        &resolver,
        "derived-revision",
        |authorized| {
            assert_eq!(authorized.snapshot_revision(), 77);
            assert_eq!(authorized.principal(), &subject().principal);
            assert_eq!(
                authorized.audit_request().correlation_id,
                "derived-revision"
            );
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn read_and_mutation_checkpoints_reject_the_wrong_request_kind() {
    let resolver = Resolver::default();
    let mutation_through_read = control_read_checkpoint(
        &subject(),
        "PUT",
        b"/_control/v1/platform/settings",
        "/_control/v1/platform/settings",
        &route_registry(),
        "",
        &versioned(1),
        &resolver,
        |_| (),
    )
    .await;
    assert_eq!(
        mutation_through_read,
        Err(ControlMiddlewareError::WrongCheckpointKind)
    );

    let read_through_mutation = control_mutation_checkpoint(
        &subject(),
        "GET",
        b"/_control/v1/platform/settings",
        "/_control/v1/platform/settings",
        &route_registry(),
        "",
        &versioned(1),
        &mutation_changes(),
        &resolver,
        "correlation",
        |_| (),
    )
    .await;
    assert_eq!(
        read_through_mutation,
        Err(ControlMiddlewareError::WrongCheckpointKind)
    );

    let missing_correlation = control_mutation_checkpoint(
        &subject(),
        "PUT",
        b"/_control/v1/platform/settings",
        "/_control/v1/platform/settings",
        &route_registry(),
        "",
        &versioned(1),
        &mutation_changes(),
        &resolver,
        " ",
        |_| (),
    )
    .await;
    assert_eq!(
        missing_correlation,
        Err(ControlMiddlewareError::InvalidCorrelationId)
    );
    assert_eq!(resolver.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn malformed_paths_fail_before_resolution_or_handler_execution() {
    let attacks: &[&[u8]] = &[
        b"/_control/v1/tenants/acme%2Fadmin/catalogs/cadastre",
        b"/_control/v1/tenants/acme//catalogs/cadastre",
        b"/_control/v1/tenants/../platform/settings",
        b"/_control/v1/tenants/acme/catalogs/cadastre/",
        b"/_control/v1/tenants/\xff/catalogs/cadastre",
    ];

    for raw_path in attacks {
        let resolver = Resolver::default();
        let handler_calls = AtomicUsize::new(0);
        let result = control_read_checkpoint(
            &subject(),
            "GET",
            raw_path,
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}",
            &route_registry(),
            "",
            &versioned(1),
            &resolver,
            |_| {
                handler_calls.fetch_add(1, Ordering::Relaxed);
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(ControlMiddlewareError::InvalidPath(_))
        ));
        assert_eq!(
            resolver.calls.load(Ordering::Relaxed),
            0,
            "path bytes {raw_path:?}"
        );
        assert_eq!(
            handler_calls.load(Ordering::Relaxed),
            0,
            "path bytes {raw_path:?}"
        );
    }
}

#[tokio::test]
async fn unmatched_routes_and_false_ownership_fail_before_handler_execution() {
    let resolver = Resolver::default();
    let handler_calls = AtomicUsize::new(0);
    let unmatched = control_read_checkpoint(
        &subject(),
        "GET",
        b"/_control/v1/tenants/acme/catalogs/cadastre-evil",
        "/_control/v1/tenants/{tenant}/catalogs/cadastre",
        &route_registry(),
        "",
        &versioned(1),
        &resolver,
        |_| {
            handler_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;
    assert!(matches!(
        unmatched,
        Err(ControlMiddlewareError::UnmatchedRoute)
    ));
    assert_eq!(resolver.calls.load(Ordering::Relaxed), 0);
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);

    resolver
        .false_collection_owner
        .store(true, Ordering::Relaxed);
    let false_owner = control_read_checkpoint(
        &subject(),
        "GET",
        b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}",
        &route_registry(),
        "",
        &versioned(1),
        &resolver,
        |_| {
            handler_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;
    assert!(matches!(
        false_owner,
        Err(ControlMiddlewareError::Resolution)
    ));
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn denied_requests_do_not_execute_the_handler() {
    let resolver = Resolver::default();
    let handler_calls = AtomicUsize::new(0);
    let mut denied_snapshot = snapshot();
    denied_snapshot.role_bindings.clear();
    let denied_snapshot =
        VersionedControlSnapshot::new(denied_snapshot, 1, BTreeMap::new()).unwrap();

    let result = control_mutation_checkpoint(
        &subject(),
        "PUT",
        b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}",
        &route_registry(),
        "",
        &denied_snapshot,
        &mutation_changes(),
        &resolver,
        "denied-request",
        |_| {
            handler_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;

    assert!(matches!(result, Err(ControlMiddlewareError::Denied(_))));
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn identifier_aliases_fail_before_handler_execution() {
    let resolver = Resolver::default();
    resolver.alias_tenant.store(true, Ordering::Relaxed);
    let handler_calls = AtomicUsize::new(0);
    let result = control_read_checkpoint(
        &subject(),
        "GET",
        b"/_control/v1/tenants/acme-alias/catalogs/cadastre",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}",
        &route_registry(),
        "",
        &versioned(1),
        &resolver,
        |_| {
            handler_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;

    assert!(matches!(
        result,
        Err(ControlMiddlewareError::NonCanonicalIdentifier)
    ));
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn resolver_ownership_must_match_the_private_validated_snapshot() {
    let snapshot = ownership_mismatch_versioned();
    let resolver = OwnershipMismatchResolver;
    let read_calls = AtomicUsize::new(0);
    let read = control_read_checkpoint(
        &subject(),
        "GET",
        b"/_control/v1/tenants/acme/catalogs/cadastre",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}",
        &route_registry(),
        "",
        &snapshot,
        &resolver,
        |_| {
            read_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;
    assert_eq!(read, Err(ControlMiddlewareError::Resolution));
    assert_eq!(read_calls.load(Ordering::Relaxed), 0);

    let mutation_calls = AtomicUsize::new(0);
    let mutation = control_mutation_checkpoint(
        &subject(),
        "PUT",
        b"/_control/v1/tenants/acme/catalogs/cadastre",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}",
        &route_registry(),
        "",
        &snapshot,
        &mutation_changes(),
        &resolver,
        "ownership-mismatch",
        |_| {
            mutation_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;
    assert_eq!(mutation, Err(ControlMiddlewareError::Resolution));
    assert_eq!(mutation_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn matching_but_unregistered_route_templates_fail_closed() {
    let resolver = Resolver::default();
    let handler_calls = AtomicUsize::new(0);
    let result = control_read_checkpoint(
        &subject(),
        "GET",
        b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads/rogue/a",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}/rogue/{id}",
        &route_registry(),
        "",
        &platform_versioned(1),
        &resolver,
        |_| {
            handler_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;

    assert_eq!(result, Err(ControlMiddlewareError::UnmatchedRoute));
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn placeholders_cannot_replace_structural_route_segments() {
    let resolver = Resolver::default();
    let handler_calls = AtomicUsize::new(0);
    let result = control_read_checkpoint(
        &subject(),
        "GET",
        b"/_control/v1/tenants/acme/catalogs/cadastre/collections/roads/assets/a",
        "/_control/v1/tenants/{tenant}/catalogs/{catalog}/{kind}/{collection}/assets/{asset}",
        &route_registry(),
        "",
        &platform_versioned(1),
        &resolver,
        |_| {
            handler_calls.fetch_add(1, Ordering::Relaxed);
        },
    )
    .await;

    assert_eq!(result, Err(ControlMiddlewareError::UnmatchedRoute));
    assert_eq!(handler_calls.load(Ordering::Relaxed), 0);
}
