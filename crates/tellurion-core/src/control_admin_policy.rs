//! Default-deny hierarchical policy evaluation for control-plane requests.

use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::control_admin_path::{
    canonicalize_control_path, CanonicalControlPath, CompiledPathPattern, ControlPathError,
};
use crate::control_model::{
    AuditRequestContext, ControlChangeSet, ControlOperation, ControlRevision, ControlScope,
    ControlSnapshot, PathPolicy, PolicyEffect, PrincipalIdentity, RoleBinding,
    VersionedControlSnapshot,
};
use crate::error::Result as CoreResult;
use crate::identity::AuthenticatedSubject;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlRequestContext {
    pub method: String,
    pub canonical_path: String,
    pub route_template: String,
    pub scope: ControlScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMiddlewareError {
    InvalidPath(ControlPathError),
    UnmatchedRoute,
    Resolution,
    NonCanonicalIdentifier,
    WrongCheckpointKind,
    InvalidCorrelationId,
    InvalidSnapshot,
    MutationIntentMismatch,
    Denied(Box<ControlEvaluation>),
}

/// A closed Task-7 route contract. Variants carry no caller-controlled
/// fields; each one owns its canonical template and mutation intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ControlRouteDescriptor {
    PlatformOverview,
    PlatformEffectiveSettings,
    PlatformAudit,
    PlatformSettings,
    PlatformBatchImport,
    Tenants,
    Tenant,
    TenantPermanentDelete,
    TenantSettings,
    TenantCatalogs,
    TenantCollectionMove,
    Catalog,
    CatalogPermanentDelete,
    CatalogSettings,
    CatalogCollections,
    Collection,
    CollectionPermanentDelete,
    CollectionMetadata,
    CollectionAsset,
    PlatformPathPolicy,
    CollectionPathPolicy,
    PlatformRoleBindings,
    PlatformRoleBinding,
}

impl ControlRouteDescriptor {
    pub const fn template(self) -> &'static str {
        match self {
            Self::PlatformOverview => "/_control/v1/platform/overview",
            Self::PlatformEffectiveSettings => "/_control/v1/platform/effective-settings",
            Self::PlatformAudit => "/_control/v1/platform/audit",
            Self::PlatformSettings => "/_control/v1/platform/settings",
            Self::PlatformBatchImport => "/_control/v1/platform/import",
            Self::Tenants => "/_control/v1/tenants",
            Self::Tenant => "/_control/v1/tenants/{tenant}",
            Self::TenantPermanentDelete => {
                "/_control/v1/tenants/{tenant}/permanent-delete"
            }
            Self::TenantSettings => "/_control/v1/tenants/{tenant}/settings",
            Self::TenantCatalogs => "/_control/v1/tenants/{tenant}/catalogs",
            Self::TenantCollectionMove => "/_control/v1/tenants/{tenant}/collection-moves",
            Self::Catalog => "/_control/v1/tenants/{tenant}/catalogs/{catalog}",
            Self::CatalogPermanentDelete => {
                "/_control/v1/tenants/{tenant}/catalogs/{catalog}/permanent-delete"
            }
            Self::CatalogSettings => "/_control/v1/tenants/{tenant}/catalogs/{catalog}/settings",
            Self::CatalogCollections => {
                "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections"
            }
            Self::Collection => {
                "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}"
            }
            Self::CollectionPermanentDelete => "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}/permanent-delete",
            Self::CollectionMetadata => "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}/metadata",
            Self::CollectionAsset => "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}/assets/{asset}",
            Self::PlatformPathPolicy => "/_control/v1/platform/policies/{policy}",
            Self::CollectionPathPolicy => "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}/policies/{policy}",
            Self::PlatformRoleBindings => "/_control/v1/platform/role-bindings",
            Self::PlatformRoleBinding => "/_control/v1/platform/role-bindings/{binding}",
        }
    }
}

/// Stable opaque URL target for one exact role binding. Length-prefixing
/// keeps adjacent caller-controlled fields unambiguous, while hashing keeps
/// issuer, subject, role, and scope values out of URLs and logs.
pub fn role_binding_target_id(binding: &RoleBinding) -> String {
    let mut bytes = b"tellurion-role-binding-v1".to_vec();
    let mut frame = |component: &str| {
        bytes.extend_from_slice(&(component.len() as u64).to_be_bytes());
        bytes.extend_from_slice(component.as_bytes());
    };
    for component in [
        binding.principal.issuer.as_str(),
        binding.principal.subject.as_str(),
        binding.role.as_str(),
    ] {
        frame(component);
    }
    match &binding.scope {
        ControlScope::Platform => frame("platform"),
        ControlScope::Tenant { tenant_id } => {
            frame("tenant");
            frame(tenant_id);
        }
        ControlScope::Catalog {
            tenant_id,
            catalog_id,
        } => {
            frame("catalog");
            frame(tenant_id);
            frame(catalog_id);
        }
        ControlScope::Collection {
            tenant_id,
            catalog_id,
            collection_id,
        } => {
            frame("collection");
            frame(tenant_id);
            frame(catalog_id);
            frame(collection_id);
        }
    }
    crate::sigv4::sha256_hex(&bytes)
}

/// Immutable catalog of typed control routes accepted by an authorization
/// checkpoint. Callers may select known descriptors but cannot register a
/// custom template or mutation contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlRouteRegistry {
    templates: std::collections::BTreeMap<String, (CanonicalControlPath, ControlRouteDescriptor)>,
}

impl ControlRouteRegistry {
    pub fn new<I>(descriptors: I) -> Result<Self, ControlMiddlewareError>
    where
        I: IntoIterator<Item = ControlRouteDescriptor>,
    {
        let mut registered = std::collections::BTreeMap::new();
        for descriptor in descriptors {
            let template = descriptor.template();
            let canonical = canonicalize_control_path(template.as_bytes(), "")
                .map_err(|_| ControlMiddlewareError::UnmatchedRoute)?;
            validate_template_structure(&canonical)?;
            registered.insert(template.to_string(), (canonical, descriptor));
        }
        Ok(Self {
            templates: registered,
        })
    }

    pub fn verify(
        &self,
        canonical: &CanonicalControlPath,
        route_template: &str,
    ) -> Result<ControlRouteDescriptor, ControlMiddlewareError> {
        let (template, descriptor) = self
            .templates
            .get(route_template)
            .ok_or(ControlMiddlewareError::UnmatchedRoute)?;
        if canonical.segments().len() != template.segments().len() {
            return Err(ControlMiddlewareError::UnmatchedRoute);
        }
        let matched = canonical
            .segments()
            .zip(template.segments())
            .all(|(actual, expected)| {
                let placeholder = expected
                    .strip_prefix('{')
                    .and_then(|value| value.strip_suffix('}'))
                    .is_some_and(|name| !name.is_empty() && !name.contains(['{', '}']));
                placeholder || actual == expected
            });
        if matched {
            Ok(*descriptor)
        } else {
            Err(ControlMiddlewareError::UnmatchedRoute)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlEvaluation {
    pub decision: ControlDecision,
    pub effective_scope: ControlScope,
    pub evaluated_roles: Vec<String>,
    pub matched_allows: Vec<String>,
    pub matched_denies: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationError {
    LegacyStatement,
    InvalidStatement,
    OutsideAllowEnvelope,
    IntersectsExplicitDeny,
}

#[derive(Debug, Clone, PartialEq)]
struct CompiledPolicyStatement {
    id: String,
    role: String,
    scope: ControlScope,
    effect: PolicyEffect,
    methods: Vec<String>,
    patterns: Vec<CompiledPathPattern>,
    has_conditions: bool,
}

impl CompiledPolicyStatement {
    fn matches(&self, path: &crate::control_admin_path::CanonicalControlPath) -> bool {
        self.patterns.iter().any(|pattern| pattern.matches(path))
    }

    fn covers(&self, candidate: &CompiledPathPattern) -> bool {
        self.patterns
            .iter()
            .any(|pattern| pattern.covers(candidate))
    }
}

/// An immutable authorization view whose policy patterns were compiled only
/// after the complete persisted snapshot passed validation.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedControlSnapshot {
    source_snapshot: ControlSnapshot,
    config: AppConfig,
    role_bindings: Vec<RoleBinding>,
    policies: Vec<CompiledPolicyStatement>,
}

impl TryFrom<&ControlSnapshot> for ValidatedControlSnapshot {
    type Error = crate::Error;

    fn try_from(snapshot: &ControlSnapshot) -> CoreResult<Self> {
        snapshot.validate()?;
        let mut policies = compile_builtin_policy_templates()?;
        for policy in snapshot
            .path_policies
            .iter()
            .filter(|policy| policy.role.is_some())
        {
            policies.push(compile_policy_statement(policy)?);
        }
        Ok(Self {
            source_snapshot: snapshot.clone(),
            config: snapshot.config.clone(),
            role_bindings: snapshot.role_bindings.clone(),
            policies,
        })
    }
}

impl ControlSnapshot {
    pub fn validated(&self) -> CoreResult<ValidatedControlSnapshot> {
        ValidatedControlSnapshot::try_from(self)
    }
}

impl ValidatedControlSnapshot {
    pub fn owns_tenant_identifier(&self, tenant_id: &str, external_id: &str) -> bool {
        self.config
            .tenants
            .iter()
            .any(|tenant| tenant.id == tenant_id && tenant.external_id() == external_id)
    }

    pub fn owns_catalog_identifier(
        &self,
        tenant_id: &str,
        catalog_id: &str,
        external_id: &str,
    ) -> bool {
        self.config.catalogs.iter().any(|catalog| {
            catalog.id == catalog_id
                && catalog.tenant == tenant_id
                && catalog.external_id() == external_id
        })
    }

    pub fn owns_collection_identifier(
        &self,
        catalog_id: &str,
        collection_id: &str,
        external_id: &str,
    ) -> bool {
        self.config.collections.iter().any(|collection| {
            collection.id == collection_id
                && collection.catalog == catalog_id
                && collection.external_id() == external_id
        })
    }
}

/// Proof that one authenticated principal was authorized against one exact
/// versioned snapshot, canonical mutation request, and changeset.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizedControlMutation {
    request_fingerprint: String,
    replay_only: bool,
    principal: PrincipalIdentity,
    effective_scope: ControlScope,
    decision_context: ControlEvaluation,
    audit_request: AuditRequestContext,
    snapshot_revision: ControlRevision,
    authoritative_snapshot: ControlSnapshot,
    authoritative_entity_versions: std::collections::BTreeMap<String, String>,
    changes: ControlChangeSet,
}

impl AuthorizedControlMutation {
    pub fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    pub fn is_replay_only(&self) -> bool {
        self.replay_only
    }

    pub fn principal(&self) -> &PrincipalIdentity {
        &self.principal
    }

    pub fn effective_scope(&self) -> &ControlScope {
        &self.effective_scope
    }

    pub fn decision_context(&self) -> &ControlEvaluation {
        &self.decision_context
    }

    pub fn audit_request(&self) -> &AuditRequestContext {
        &self.audit_request
    }

    pub fn snapshot_revision(&self) -> ControlRevision {
        self.snapshot_revision
    }

    pub fn validate_intent(&self, changes: &ControlChangeSet) -> CoreResult<()> {
        if self.changes != *changes {
            return Err(crate::Error::ControlValidation(
                "control mutation authorization does not match the submitted changeset".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_authoritative_state(
        &self,
        snapshot: &ControlSnapshot,
        entity_versions: &std::collections::BTreeMap<String, String>,
    ) -> CoreResult<()> {
        if self.authoritative_snapshot != *snapshot
            || self.authoritative_entity_versions != *entity_versions
        {
            return Err(crate::Error::ControlValidation(
                "control mutation authorization does not match authoritative store state"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn authorize_control_mutation(
    subject: &AuthenticatedSubject,
    method: &str,
    raw_path: &[u8],
    route_template: &str,
    route_registry: &ControlRouteRegistry,
    application_root: &str,
    snapshot: &VersionedControlSnapshot,
    changes: &ControlChangeSet,
    correlation_id: &str,
) -> Result<AuthorizedControlMutation, ControlMiddlewareError> {
    if matches!(method, "GET" | "HEAD") {
        return Err(ControlMiddlewareError::WrongCheckpointKind);
    }
    if correlation_id.trim().is_empty() {
        return Err(ControlMiddlewareError::InvalidCorrelationId);
    }
    let path = canonicalize_control_path(raw_path, application_root)
        .map_err(ControlMiddlewareError::InvalidPath)?;
    let descriptor = route_registry.verify(&path, route_template)?;
    let validated = snapshot
        .validated_snapshot()
        .map_err(|_| ControlMiddlewareError::InvalidSnapshot)?;
    let scope = resolve_snapshot_scope(&path, validated)?;
    let replay_only =
        validate_mutation_intent(descriptor, method, &path, &scope, validated, changes)?;
    let request = ControlRequestContext {
        method: method.to_string(),
        canonical_path: path.as_str().to_string(),
        route_template: route_template.to_string(),
        scope,
    };
    let audit_request = AuditRequestContext {
        method: method.to_string(),
        canonical_path: request.canonical_path.clone(),
        correlation_id: correlation_id.to_string(),
    };
    authorize_control_mutation_canonical(
        subject,
        &request,
        audit_request,
        snapshot,
        changes,
        &path,
        replay_only,
    )
    .map_err(ControlMiddlewareError::Denied)
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn authorize_control_mutation_from_context(
    subject: &AuthenticatedSubject,
    request: &ControlRequestContext,
    audit_request: AuditRequestContext,
    snapshot: &VersionedControlSnapshot,
    changes: &ControlChangeSet,
) -> Result<AuthorizedControlMutation, Box<ControlEvaluation>> {
    let Some(path) = canonical_request_path(request) else {
        return Err(Box::new(denied(request.scope.clone())));
    };
    authorize_control_mutation_canonical(
        subject,
        request,
        audit_request,
        snapshot,
        changes,
        &path,
        false,
    )
}

pub(crate) fn authorize_control_mutation_canonical(
    subject: &AuthenticatedSubject,
    request: &ControlRequestContext,
    audit_request: AuditRequestContext,
    snapshot: &VersionedControlSnapshot,
    changes: &ControlChangeSet,
    path: &CanonicalControlPath,
    replay_only: bool,
) -> Result<AuthorizedControlMutation, Box<ControlEvaluation>> {
    let Ok((validated, snapshot_revision, entity_versions)) = snapshot.validated_state() else {
        return Err(Box::new(denied(request.scope.clone())));
    };
    if changes.validate().is_err() {
        return Err(Box::new(denied(request.scope.clone())));
    };
    if audit_request.method != request.method
        || audit_request.canonical_path != request.canonical_path
        || audit_request.correlation_id.trim().is_empty()
    {
        return Err(Box::new(denied(request.scope.clone())));
    }
    if path.as_str() != request.canonical_path {
        return Err(Box::new(denied(request.scope.clone())));
    }
    let decision_context = explain_control_canonical(subject, request, validated, path);
    if decision_context.decision != ControlDecision::Allow {
        return Err(Box::new(decision_context));
    }
    let Some(request_fingerprint) = control_request_fingerprint(subject, request, changes) else {
        return Err(Box::new(denied(request.scope.clone())));
    };
    Ok(AuthorizedControlMutation {
        request_fingerprint,
        replay_only,
        principal: subject.principal.clone(),
        effective_scope: request.scope.clone(),
        decision_context,
        audit_request,
        snapshot_revision,
        authoritative_snapshot: validated.source_snapshot.clone(),
        authoritative_entity_versions: entity_versions.clone(),
        changes: changes.clone(),
    })
}

fn resolve_snapshot_scope(
    canonical: &CanonicalControlPath,
    snapshot: &ValidatedControlSnapshot,
) -> Result<ControlScope, ControlMiddlewareError> {
    let segments = canonical.segments().collect::<Vec<_>>();
    if segments[2] == "platform" || segments.len() == 3 {
        return Ok(ControlScope::Platform);
    }

    let tenant = snapshot
        .config
        .tenants
        .iter()
        .find(|tenant| tenant.external_id() == segments[3])
        .ok_or(ControlMiddlewareError::Resolution)?;
    if segments.len() <= 5 || segments.get(4) != Some(&"catalogs") {
        return Ok(ControlScope::Tenant {
            tenant_id: tenant.id.clone(),
        });
    }

    let catalog = snapshot
        .config
        .catalogs
        .iter()
        .find(|catalog| catalog.tenant == tenant.id && catalog.external_id() == segments[5])
        .ok_or(ControlMiddlewareError::Resolution)?;
    if segments.len() <= 7 || segments.get(6) != Some(&"collections") {
        return Ok(ControlScope::Catalog {
            tenant_id: tenant.id.clone(),
            catalog_id: catalog.id.clone(),
        });
    }

    let collection = snapshot
        .config
        .collections
        .iter()
        .find(|collection| {
            collection.catalog == catalog.id && collection.external_id() == segments[7]
        })
        .ok_or(ControlMiddlewareError::Resolution)?;
    Ok(ControlScope::Collection {
        tenant_id: tenant.id.clone(),
        catalog_id: catalog.id.clone(),
        collection_id: collection.id.clone(),
    })
}

fn validate_mutation_intent(
    descriptor: ControlRouteDescriptor,
    method: &str,
    canonical: &CanonicalControlPath,
    resolved_scope: &ControlScope,
    snapshot: &ValidatedControlSnapshot,
    changes: &ControlChangeSet,
) -> Result<bool, ControlMiddlewareError> {
    if !matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") {
        return Err(ControlMiddlewareError::MutationIntentMismatch);
    }
    if descriptor == ControlRouteDescriptor::PlatformBatchImport {
        return (method == "POST"
            && *resolved_scope == ControlScope::Platform
            && changes.validate().is_ok())
        .then_some(false)
        .ok_or(ControlMiddlewareError::MutationIntentMismatch);
    }
    let [operation] = changes.operations.as_slice() else {
        return Err(ControlMiddlewareError::MutationIntentMismatch);
    };
    match (descriptor, method, &operation.operation) {
        (ControlRouteDescriptor::Tenants, "POST", ControlOperation::PutTenant(candidate)) => {
            if *resolved_scope != ControlScope::Platform {
                return Err(ControlMiddlewareError::MutationIntentMismatch);
            }
            return create_or_replay(
                snapshot
                    .config
                    .tenants
                    .iter()
                    .find(|current| current.id == candidate.id),
                candidate,
                changes,
            );
        }
        (
            ControlRouteDescriptor::TenantCatalogs,
            "POST",
            ControlOperation::PutCatalog(candidate),
        ) => {
            if !matches!(
                resolved_scope,
                ControlScope::Tenant { tenant_id } if candidate.tenant == *tenant_id
            ) {
                return Err(ControlMiddlewareError::MutationIntentMismatch);
            }
            return create_or_replay(
                snapshot
                    .config
                    .catalogs
                    .iter()
                    .find(|current| current.id == candidate.id),
                candidate,
                changes,
            );
        }
        (
            ControlRouteDescriptor::CatalogCollections,
            "POST",
            ControlOperation::PutCollection(candidate),
        ) => {
            if !matches!(
                resolved_scope,
                ControlScope::Catalog { catalog_id, .. } if candidate.catalog == *catalog_id
            ) {
                return Err(ControlMiddlewareError::MutationIntentMismatch);
            }
            return create_or_replay(
                snapshot
                    .config
                    .collections
                    .iter()
                    .find(|current| current.id == candidate.id),
                candidate,
                changes,
            );
        }
        (
            ControlRouteDescriptor::TenantCollectionMove,
            "POST",
            ControlOperation::PutCollection(candidate),
        ) => {
            return tenant_collection_move_or_replay(resolved_scope, candidate, snapshot, changes);
        }
        _ => {}
    }
    let matches = match (descriptor, method, &operation.operation) {
        (
            ControlRouteDescriptor::PlatformSettings,
            "PUT" | "PATCH",
            ControlOperation::ReplacePlatformSettings(candidate),
        ) => replaces_only_platform_settings(candidate, &snapshot.config),
        (ControlRouteDescriptor::Tenant, "PUT", ControlOperation::PutTenant(tenant)) => {
            matches!(
                resolved_scope,
                ControlScope::Tenant { tenant_id } if tenant.id == *tenant_id
            )
        }
        (
            ControlRouteDescriptor::TenantSettings,
            "PUT" | "PATCH",
            ControlOperation::PutTenant(tenant),
        ) => snapshot
            .config
            .tenants
            .iter()
            .find(|current| {
                matches!(resolved_scope, ControlScope::Tenant { tenant_id } if current.id == *tenant_id)
            })
            .is_some_and(|current| replaces_only_tenant_settings(tenant, current)),
        (ControlRouteDescriptor::Catalog, "PUT", ControlOperation::PutCatalog(catalog)) => {
            matches!(
                resolved_scope,
                ControlScope::Catalog {
                    tenant_id,
                    catalog_id,
                } if catalog.id == *catalog_id && catalog.tenant == *tenant_id
            )
        }
        (
            ControlRouteDescriptor::CatalogSettings,
            "PUT" | "PATCH",
            ControlOperation::PutCatalog(catalog),
        ) => snapshot
            .config
            .catalogs
            .iter()
            .find(|current| {
                matches!(
                    resolved_scope,
                    ControlScope::Catalog { tenant_id, catalog_id }
                        if current.id == *catalog_id && current.tenant == *tenant_id
                )
            })
            .is_some_and(|current| replaces_only_catalog_settings(catalog, current)),
        (
            ControlRouteDescriptor::Collection,
            "PUT",
            ControlOperation::PutCollection(collection),
        ) => {
            matches!(
                resolved_scope,
                ControlScope::Collection {
                    catalog_id,
                    collection_id,
                    ..
                } if collection.id == *collection_id && collection.catalog == *catalog_id
            )
        }
        (
            ControlRouteDescriptor::Tenant
            | ControlRouteDescriptor::Catalog
            | ControlRouteDescriptor::Collection,
            "DELETE",
            ControlOperation::TombstoneResource { scope },
        ) => scope == resolved_scope,
        (
            ControlRouteDescriptor::TenantPermanentDelete
            | ControlRouteDescriptor::CatalogPermanentDelete
            | ControlRouteDescriptor::CollectionPermanentDelete,
            "DELETE",
            ControlOperation::PermanentlyDeleteResource { scope },
        ) => {
            scope == resolved_scope
                && operation.expected_entity_version.is_some()
                && snapshot.source_snapshot.tombstoned_resources.contains(scope)
        }
        (
            ControlRouteDescriptor::PlatformPathPolicy
            | ControlRouteDescriptor::CollectionPathPolicy,
            "PUT",
            ControlOperation::PutPathPolicy(policy),
        ) => {
            let target = canonical.segments().last();
            policy.scope.as_ref() == Some(resolved_scope) && target == Some(policy.id.as_str())
        }
        (
            ControlRouteDescriptor::PlatformPathPolicy
            | ControlRouteDescriptor::CollectionPathPolicy,
            "DELETE",
            ControlOperation::DeletePathPolicy { id },
        ) => {
            let target = canonical.segments().last();
            target == Some(id.as_str())
                && snapshot
                    .source_snapshot
                    .path_policies
                    .iter()
                    .any(|policy| policy.id == *id && policy.scope.as_ref() == Some(resolved_scope))
        }
        (
            ControlRouteDescriptor::PlatformRoleBindings,
            "POST",
            ControlOperation::PutRoleBinding(binding),
        ) => binding.scope == *resolved_scope,
        (
            ControlRouteDescriptor::PlatformRoleBinding,
            "DELETE",
            ControlOperation::DeleteRoleBinding {
                principal,
                scope,
                role,
            },
        ) => {
            let binding = RoleBinding {
                principal: principal.clone(),
                role: role.clone(),
                scope: scope.clone(),
            };
            let target = role_binding_target_id(&binding);
            scope == resolved_scope && canonical.segments().last() == Some(target.as_str())
        }
        _ => false,
    };
    matches
        .then_some(false)
        .ok_or(ControlMiddlewareError::MutationIntentMismatch)
}

fn create_or_replay<T: PartialEq>(
    current: Option<&T>,
    candidate: &T,
    changes: &ControlChangeSet,
) -> Result<bool, ControlMiddlewareError> {
    match current {
        None => Ok(false),
        Some(current) if changes.idempotency_key.is_some() && current == candidate => Ok(true),
        Some(_) => Err(ControlMiddlewareError::MutationIntentMismatch),
    }
}

fn control_request_fingerprint(
    subject: &AuthenticatedSubject,
    request: &ControlRequestContext,
    changes: &ControlChangeSet,
) -> Option<String> {
    let mut bytes = b"tellurion-control-request-v1".to_vec();
    let mut frame = |component: &[u8]| {
        bytes.extend_from_slice(&(component.len() as u64).to_be_bytes());
        bytes.extend_from_slice(component);
    };
    for component in [
        subject.principal.issuer.as_bytes(),
        subject.principal.subject.as_bytes(),
        request.route_template.as_bytes(),
        request.method.as_bytes(),
        request.canonical_path.as_bytes(),
    ] {
        frame(component);
    }
    match &request.scope {
        ControlScope::Platform => frame(b"platform"),
        ControlScope::Tenant { tenant_id } => {
            frame(b"tenant");
            frame(tenant_id.as_bytes());
        }
        ControlScope::Catalog {
            tenant_id,
            catalog_id,
        } => {
            frame(b"catalog");
            frame(tenant_id.as_bytes());
            frame(catalog_id.as_bytes());
        }
        ControlScope::Collection {
            tenant_id,
            catalog_id,
            collection_id,
        } => {
            frame(b"collection");
            frame(tenant_id.as_bytes());
            frame(catalog_id.as_bytes());
            frame(collection_id.as_bytes());
        }
    }
    let value = serde_json::to_value(changes).ok()?;
    let mut canonical = Vec::new();
    write_canonical_json(&value, &mut canonical)?;
    frame(&canonical);
    Some(crate::sigv4::sha256_hex(&bytes))
}

fn write_canonical_json(value: &serde_json::Value, output: &mut Vec<u8>) -> Option<()> {
    match value {
        serde_json::Value::Null => output.extend_from_slice(b"null"),
        serde_json::Value::Bool(value) => {
            output.extend_from_slice(if *value { b"true" } else { b"false" })
        }
        serde_json::Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        serde_json::Value::String(value) => {
            serde_json::to_writer(&mut *output, value).ok()?;
        }
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        serde_json::Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key).ok()?;
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Some(())
}

fn replaces_only_platform_settings(candidate: &AppConfig, current: &AppConfig) -> bool {
    let mut expected = current.clone();
    expected.settings = candidate.settings.clone();
    *candidate == expected
}

fn replaces_only_tenant_settings(
    candidate: &crate::config::TenantDecl,
    current: &crate::config::TenantDecl,
) -> bool {
    let mut expected = current.clone();
    expected.settings = candidate.settings.clone();
    *candidate == expected
}

fn replaces_only_catalog_settings(
    candidate: &crate::config::CatalogDecl,
    current: &crate::config::CatalogDecl,
) -> bool {
    let mut expected = current.clone();
    expected.settings = candidate.settings.clone();
    *candidate == expected
}

fn tenant_collection_move_or_replay(
    resolved_scope: &ControlScope,
    candidate: &crate::config::CollectionDecl,
    snapshot: &ValidatedControlSnapshot,
    changes: &ControlChangeSet,
) -> Result<bool, ControlMiddlewareError> {
    let ControlScope::Tenant { tenant_id } = resolved_scope else {
        return Err(ControlMiddlewareError::MutationIntentMismatch);
    };
    let Some(current) = snapshot
        .config
        .collections
        .iter()
        .find(|current| current.id == candidate.id)
    else {
        return Err(ControlMiddlewareError::MutationIntentMismatch);
    };
    let catalog_belongs_to_route_tenant = |catalog_id: &str| {
        snapshot
            .config
            .catalogs
            .iter()
            .any(|catalog| catalog.id == catalog_id && catalog.tenant == *tenant_id)
    };
    if !catalog_belongs_to_route_tenant(&current.catalog)
        || !catalog_belongs_to_route_tenant(&candidate.catalog)
    {
        return Err(ControlMiddlewareError::MutationIntentMismatch);
    }
    if current == candidate {
        return changes
            .idempotency_key
            .is_some()
            .then_some(true)
            .ok_or(ControlMiddlewareError::MutationIntentMismatch);
    }
    if current.catalog == candidate.catalog {
        return Err(ControlMiddlewareError::MutationIntentMismatch);
    }
    let mut expected = current.clone();
    expected.catalog.clone_from(&candidate.catalog);
    (*candidate == expected)
        .then_some(false)
        .ok_or(ControlMiddlewareError::MutationIntentMismatch)
}

fn validate_template_structure(
    template: &CanonicalControlPath,
) -> Result<(), ControlMiddlewareError> {
    let segments = template.segments().collect::<Vec<_>>();
    let placeholder = |segment: &str| {
        segment
            .strip_prefix('{')
            .and_then(|value| value.strip_suffix('}'))
            .is_some_and(|name| !name.is_empty() && !name.contains(['{', '}']))
    };
    if segments
        .iter()
        .any(|segment| (segment.contains('{') || segment.contains('}')) && !placeholder(segment))
    {
        return Err(ControlMiddlewareError::UnmatchedRoute);
    }
    if segments[2] == "tenants" {
        if segments.get(4).is_some_and(|segment| placeholder(segment)) {
            return Err(ControlMiddlewareError::UnmatchedRoute);
        }
        if segments.get(4) == Some(&"catalogs")
            && segments.get(6).is_some_and(|segment| placeholder(segment))
        {
            return Err(ControlMiddlewareError::UnmatchedRoute);
        }
    }
    Ok(())
}

pub fn authorize_control(
    subject: &AuthenticatedSubject,
    request: &ControlRequestContext,
    snapshot: &ValidatedControlSnapshot,
) -> ControlDecision {
    let Some(path) = canonical_request_path(request) else {
        return ControlDecision::Deny;
    };
    authorize_control_canonical(subject, request, snapshot, &path)
}

pub fn explain_control(
    subject: &AuthenticatedSubject,
    request: &ControlRequestContext,
    snapshot: &ValidatedControlSnapshot,
) -> ControlEvaluation {
    let Some(path) = canonical_request_path(request) else {
        return denied(request.scope.clone());
    };
    explain_control_canonical(subject, request, snapshot, &path)
}

pub fn authorize_control_canonical(
    subject: &AuthenticatedSubject,
    request: &ControlRequestContext,
    snapshot: &ValidatedControlSnapshot,
    path: &CanonicalControlPath,
) -> ControlDecision {
    if path.as_str() != request.canonical_path {
        return ControlDecision::Deny;
    }
    let mut allowed = false;
    for binding in snapshot
        .role_bindings
        .iter()
        .filter(|binding| binding.principal == subject.principal)
        .filter(|binding| binding.scope.contains(&request.scope))
    {
        for policy in snapshot
            .policies
            .iter()
            .filter(|policy| policy.role == binding.role)
            .filter(|policy| policy.scope.contains(&request.scope))
            .filter(|policy| {
                policy
                    .methods
                    .iter()
                    .any(|method| method == &request.method)
            })
            .filter(|policy| policy.matches(path))
        {
            match policy.effect {
                PolicyEffect::Deny => return ControlDecision::Deny,
                PolicyEffect::Allow if !policy.has_conditions => allowed = true,
                PolicyEffect::Allow => {}
            }
        }
    }
    if allowed {
        ControlDecision::Allow
    } else {
        ControlDecision::Deny
    }
}

pub fn explain_control_canonical(
    subject: &AuthenticatedSubject,
    request: &ControlRequestContext,
    snapshot: &ValidatedControlSnapshot,
    path: &CanonicalControlPath,
) -> ControlEvaluation {
    if path.as_str() != request.canonical_path {
        return denied(request.scope.clone());
    }
    let mut matched_allows = Vec::new();
    let mut matched_denies = Vec::new();
    let mut evaluated_roles = Vec::new();
    for binding in snapshot
        .role_bindings
        .iter()
        .filter(|binding| binding.principal == subject.principal)
        .filter(|binding| binding.scope.contains(&request.scope))
    {
        evaluated_roles.push(binding.role.clone());
        for policy in snapshot
            .policies
            .iter()
            .filter(|policy| policy.role == binding.role)
            .filter(|policy| policy.scope.contains(&request.scope))
            .filter(|policy| {
                policy
                    .methods
                    .iter()
                    .any(|method| method == &request.method)
            })
            .filter(|policy| policy.matches(path))
        {
            match policy.effect {
                PolicyEffect::Allow if !policy.has_conditions => {
                    matched_allows.push(policy.id.clone())
                }
                PolicyEffect::Allow => {}
                PolicyEffect::Deny => matched_denies.push(policy.id.clone()),
            }
        }
    }
    matched_allows.sort();
    matched_allows.dedup();
    matched_denies.sort();
    matched_denies.dedup();
    evaluated_roles.sort();
    evaluated_roles.dedup();
    let decision = if matched_denies.is_empty() && !matched_allows.is_empty() {
        ControlDecision::Allow
    } else {
        ControlDecision::Deny
    };
    ControlEvaluation {
        decision,
        effective_scope: request.scope.clone(),
        evaluated_roles,
        matched_allows,
        matched_denies,
    }
}

fn canonical_request_path(request: &ControlRequestContext) -> Option<CanonicalControlPath> {
    let path = canonicalize_control_path(request.canonical_path.as_bytes(), "").ok()?;
    (path.as_str() == request.canonical_path).then_some(path)
}

fn denied(scope: ControlScope) -> ControlEvaluation {
    ControlEvaluation {
        decision: ControlDecision::Deny,
        effective_scope: scope,
        evaluated_roles: Vec::new(),
        matched_allows: Vec::new(),
        matched_denies: Vec::new(),
    }
}

pub fn validate_delegated_policy(
    actor: &AuthenticatedSubject,
    delegated: &PathPolicy,
    snapshot: &ControlSnapshot,
) -> Result<(), DelegationError> {
    let validated = snapshot
        .validated()
        .map_err(|_| DelegationError::InvalidStatement)?;
    validate_delegated_policy_against(actor, delegated, &validated)
}

fn validate_delegated_policy_against(
    actor: &AuthenticatedSubject,
    delegated: &PathPolicy,
    snapshot: &ValidatedControlSnapshot,
) -> Result<(), DelegationError> {
    let delegated_scope = delegated
        .scope
        .as_ref()
        .ok_or(DelegationError::LegacyStatement)?;
    delegated_scope
        .validate_against(&snapshot.config)
        .map_err(|_| DelegationError::InvalidStatement)?;
    if delegated.role.is_none() || delegated.methods.is_empty() || delegated.patterns.is_empty() {
        return Err(DelegationError::InvalidStatement);
    }
    let delegated_patterns = delegated
        .patterns
        .iter()
        .map(|pattern| CompiledPathPattern::compile(pattern))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DelegationError::InvalidStatement)?;
    let scope_envelope = scope_envelope(delegated_scope, &snapshot.config)
        .and_then(|pattern| CompiledPathPattern::compile(&pattern).ok())
        .ok_or(DelegationError::InvalidStatement)?;
    if delegated_patterns
        .iter()
        .any(|pattern| !scope_envelope.covers(pattern))
    {
        return Err(DelegationError::OutsideAllowEnvelope);
    }

    let allow_bindings = snapshot
        .role_bindings
        .iter()
        .filter(|binding| binding.principal == actor.principal)
        .filter(|binding| binding.scope.contains(delegated_scope))
        .collect::<Vec<_>>();
    if allow_bindings.is_empty() {
        return Err(DelegationError::OutsideAllowEnvelope);
    }

    for method in &delegated.methods {
        for delegated_pattern in &delegated_patterns {
            let covered = allow_bindings.iter().any(|binding| {
                snapshot.policies.iter().any(|policy| {
                    policy.effect == PolicyEffect::Allow
                        && !policy.has_conditions
                        && policy.role == binding.role
                        && policy.scope.contains(delegated_scope)
                        && policy.methods.iter().any(|allowed| allowed == method)
                        && policy.covers(delegated_pattern)
                })
            });
            if !covered {
                return Err(DelegationError::OutsideAllowEnvelope);
            }

            let denied = actor_has_overlapping_deny(
                &actor.principal,
                delegated_scope,
                method,
                delegated_pattern,
                snapshot,
            );
            if denied {
                return Err(DelegationError::IntersectsExplicitDeny);
            }
        }
    }
    Ok(())
}

fn actor_has_overlapping_deny(
    actor: &PrincipalIdentity,
    delegated_scope: &ControlScope,
    method: &str,
    delegated_pattern: &CompiledPathPattern,
    snapshot: &ValidatedControlSnapshot,
) -> bool {
    snapshot
        .role_bindings
        .iter()
        .filter(|binding| binding.principal == *actor)
        .any(|binding| {
            snapshot
                .policies
                .iter()
                .filter(|policy| policy.role == binding.role)
                .filter(|policy| policy.effect == PolicyEffect::Deny)
                .filter(|policy| policy.methods.iter().any(|denied| denied == method))
                .filter_map(|policy| {
                    let effective_scope = narrower_scope(&binding.scope, &policy.scope)?;
                    if !scopes_overlap(effective_scope, delegated_scope) {
                        return None;
                    }
                    let scope_pattern = scope_envelope(effective_scope, &snapshot.config)
                        .and_then(|pattern| CompiledPathPattern::compile(&pattern).ok())?;
                    Some(policy.patterns.iter().any(|deny_pattern| {
                        deny_pattern
                            .intersection(&scope_pattern)
                            .is_some_and(|deny| deny.overlaps(delegated_pattern))
                    }))
                })
                .any(|overlaps| overlaps)
        })
}

fn narrower_scope<'a>(left: &'a ControlScope, right: &'a ControlScope) -> Option<&'a ControlScope> {
    if left.contains(right) {
        Some(right)
    } else if right.contains(left) {
        Some(left)
    } else {
        None
    }
}

fn scopes_overlap(left: &ControlScope, right: &ControlScope) -> bool {
    left.contains(right) || right.contains(left)
}

pub fn validate_delegated_role_binding(
    actor: &PrincipalIdentity,
    delegated: &RoleBinding,
    snapshot: &ControlSnapshot,
) -> Result<(), DelegationError> {
    validate_role_binding_delegation(actor, delegated, snapshot, snapshot)
}

pub(crate) fn validate_role_binding_delegation(
    actor: &PrincipalIdentity,
    delegated: &RoleBinding,
    authority_snapshot: &ControlSnapshot,
    target_snapshot: &ControlSnapshot,
) -> Result<(), DelegationError> {
    let authority_snapshot = authority_snapshot
        .validated()
        .map_err(|_| DelegationError::InvalidStatement)?;
    let target_snapshot = target_snapshot
        .validated()
        .map_err(|_| DelegationError::InvalidStatement)?;
    delegated
        .scope
        .validate_against(&target_snapshot.config)
        .map_err(|_| DelegationError::InvalidStatement)?;
    if delegated.role.trim().is_empty() {
        return Err(DelegationError::InvalidStatement);
    }
    let effective_scope_pattern = scope_envelope(&delegated.scope, &target_snapshot.config)
        .and_then(|pattern| CompiledPathPattern::compile(&pattern).ok())
        .ok_or(DelegationError::InvalidStatement)?;
    let actor_bindings = authority_snapshot
        .role_bindings
        .iter()
        .filter(|binding| binding.principal == *actor)
        .filter(|binding| binding.scope.contains(&delegated.scope))
        .collect::<Vec<_>>();
    if actor_bindings.is_empty() {
        return Err(DelegationError::OutsideAllowEnvelope);
    }

    for target in target_snapshot
        .policies
        .iter()
        .filter(|policy| policy.role == delegated.role)
        .filter(|policy| policy.effect == PolicyEffect::Allow && !policy.has_conditions)
    {
        let target_scope = &target.scope;
        let effective_scope = if delegated.scope.contains(target_scope) {
            target_scope
        } else if target_scope.contains(&delegated.scope) {
            &delegated.scope
        } else {
            continue;
        };
        let effective_scope_pattern = if effective_scope == &delegated.scope {
            effective_scope_pattern.clone()
        } else {
            scope_envelope(effective_scope, &target_snapshot.config)
                .and_then(|pattern| CompiledPathPattern::compile(&pattern).ok())
                .ok_or(DelegationError::InvalidStatement)?
        };

        for method in &target.methods {
            for target_pattern in &target.patterns {
                let Some(effective_pattern) = target_pattern.intersection(&effective_scope_pattern)
                else {
                    continue;
                };
                let covered = actor_bindings.iter().any(|binding| {
                    authority_snapshot
                        .policies
                        .iter()
                        .filter(|policy| {
                            policy.role == binding.role
                                && policy.effect == PolicyEffect::Allow
                                && !policy.has_conditions
                                && policy.methods.iter().any(|allowed| allowed == method)
                                && policy.scope.contains(effective_scope)
                        })
                        .any(|policy| policy.covers(&effective_pattern))
                });
                if !covered {
                    return Err(DelegationError::OutsideAllowEnvelope);
                }
                let denied = actor_has_overlapping_deny(
                    actor,
                    effective_scope,
                    method,
                    &effective_pattern,
                    &authority_snapshot,
                );
                if denied {
                    return Err(DelegationError::IntersectsExplicitDeny);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_role_binding_removal(
    actor: &PrincipalIdentity,
    removed: &RoleBinding,
    authority_snapshot: &ControlSnapshot,
) -> Result<(), DelegationError> {
    let authority_snapshot = authority_snapshot
        .validated()
        .map_err(|_| DelegationError::InvalidStatement)?;
    removed
        .scope
        .validate_against(&authority_snapshot.config)
        .map_err(|_| DelegationError::InvalidStatement)?;
    if removed.role.trim().is_empty() {
        return Err(DelegationError::InvalidStatement);
    }

    let actor_bindings = authority_snapshot
        .role_bindings
        .iter()
        .filter(|binding| binding.principal == *actor)
        .collect::<Vec<_>>();
    for denied_policy in authority_snapshot
        .policies
        .iter()
        .filter(|policy| policy.role == removed.role)
        .filter(|policy| policy.effect == PolicyEffect::Deny)
    {
        let Some(effective_scope) = narrower_scope(&removed.scope, &denied_policy.scope) else {
            continue;
        };
        let scope_pattern = scope_envelope(effective_scope, &authority_snapshot.config)
            .and_then(|pattern| CompiledPathPattern::compile(&pattern).ok())
            .ok_or(DelegationError::InvalidStatement)?;
        for method in &denied_policy.methods {
            for denied_pattern in &denied_policy.patterns {
                let Some(effective_pattern) = denied_pattern.intersection(&scope_pattern) else {
                    continue;
                };
                let covered = actor_bindings.iter().any(|binding| {
                    authority_snapshot
                        .policies
                        .iter()
                        .filter(|policy| {
                            policy.role == binding.role
                                && policy.effect == PolicyEffect::Allow
                                && !policy.has_conditions
                                && policy.methods.iter().any(|allowed| allowed == method)
                        })
                        .any(|policy| {
                            let Some(actor_scope) = narrower_scope(&binding.scope, &policy.scope)
                            else {
                                return false;
                            };
                            if !actor_scope.contains(effective_scope) {
                                return false;
                            }
                            let Some(actor_scope_pattern) =
                                scope_envelope(actor_scope, &authority_snapshot.config).and_then(
                                    |pattern| CompiledPathPattern::compile(&pattern).ok(),
                                )
                            else {
                                return false;
                            };
                            policy.patterns.iter().any(|actor_pattern| {
                                actor_pattern
                                    .intersection(&actor_scope_pattern)
                                    .is_some_and(|covered| covered.covers(&effective_pattern))
                            })
                        })
                });
                if !covered {
                    return Err(DelegationError::OutsideAllowEnvelope);
                }
                if actor_has_overlapping_deny(
                    actor,
                    effective_scope,
                    method,
                    &effective_pattern,
                    &authority_snapshot,
                ) {
                    return Err(DelegationError::IntersectsExplicitDeny);
                }
            }
        }
    }
    Ok(())
}

struct BuiltInPolicyTemplate {
    id: &'static str,
    role: &'static str,
    methods: &'static [&'static str],
    pattern: &'static str,
}

const ALL_METHODS: &[&str] = &["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];
const READ_METHODS: &[&str] = &["GET", "HEAD"];
const WRITE_METHODS: &[&str] = &["POST", "PUT", "PATCH", "DELETE"];
const READ_UPDATE_METHODS: &[&str] = &["GET", "HEAD", "PUT", "PATCH"];
const RESOURCE_METHODS: &[&str] = &["GET", "HEAD", "PUT", "PATCH", "DELETE"];
const LIST_CREATE_METHODS: &[&str] = &["GET", "HEAD", "POST"];
const POST_METHOD: &[&str] = &["POST"];

const BUILT_IN_POLICY_TEMPLATES: &[BuiltInPolicyTemplate] = &[
    BuiltInPolicyTemplate {
        id: "builtin:sysadmin:all",
        role: "sysadmin",
        methods: ALL_METHODS,
        pattern: "/_control/v1/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:settings",
        role: "tenant_admin",
        methods: READ_UPDATE_METHODS,
        pattern: "/_control/v1/tenants/*/settings",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:effective-settings",
        role: "tenant_admin",
        methods: READ_METHODS,
        pattern: "/_control/v1/tenants/*/effective-settings",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:catalog-list",
        role: "tenant_admin",
        methods: LIST_CREATE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:collection-moves",
        role: "tenant_admin",
        methods: POST_METHOD,
        pattern: "/_control/v1/tenants/*/collection-moves",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:catalog-lifecycle",
        role: "tenant_admin",
        methods: RESOURCE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:principals",
        role: "tenant_admin",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/principals/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:role-bindings",
        role: "tenant_admin",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/role-bindings/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:tenant-admin:policies",
        role: "tenant_admin",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/policies/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:settings",
        role: "catalog_admin",
        methods: READ_UPDATE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/settings",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:metadata",
        role: "catalog_admin",
        methods: READ_UPDATE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/metadata",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:visibility",
        role: "catalog_admin",
        methods: READ_UPDATE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/visibility",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:styles",
        role: "catalog_admin",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/styles/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:collection-list",
        role: "catalog_admin",
        methods: LIST_CREATE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:collection-lifecycle",
        role: "catalog_admin",
        methods: RESOURCE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:role-bindings",
        role: "catalog_admin",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/role-bindings/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:catalog-admin:policies",
        role: "catalog_admin",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/policies/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:collection-editor:assets",
        role: "collection_editor",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*/assets/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:collection-editor:data",
        role: "collection_editor",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*/data/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:collection-editor:items",
        role: "collection_editor",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*/items/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:collection-editor:metadata",
        role: "collection_editor",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*/metadata/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:collection-editor:styles",
        role: "collection_editor",
        methods: ALL_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*/styles/**",
    },
    BuiltInPolicyTemplate {
        id: "builtin:publisher:catalog-visibility",
        role: "publisher",
        methods: WRITE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/visibility",
    },
    BuiltInPolicyTemplate {
        id: "builtin:publisher:collection-visibility",
        role: "publisher",
        methods: WRITE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*/visibility",
    },
    BuiltInPolicyTemplate {
        id: "builtin:publisher:publication",
        role: "publisher",
        methods: WRITE_METHODS,
        pattern: "/_control/v1/tenants/*/catalogs/*/collections/*/publication",
    },
    BuiltInPolicyTemplate {
        id: "builtin:viewer:read",
        role: "viewer",
        methods: READ_METHODS,
        pattern: "/_control/v1/**",
    },
];

fn compile_policy_statement(policy: &PathPolicy) -> CoreResult<CompiledPolicyStatement> {
    let patterns = policy.validate()?;
    Ok(CompiledPolicyStatement {
        id: policy.id.clone(),
        role: policy.role.clone().ok_or_else(|| {
            crate::Error::ControlValidation("legacy policy is not executable".to_string())
        })?,
        scope: policy.scope.clone().ok_or_else(|| {
            crate::Error::ControlValidation("legacy policy is not executable".to_string())
        })?,
        effect: policy.effect,
        methods: policy.methods.clone(),
        patterns,
        has_conditions: !policy.conditions.is_empty(),
    })
}

fn compile_builtin_policy_templates() -> CoreResult<Vec<CompiledPolicyStatement>> {
    BUILT_IN_POLICY_TEMPLATES
        .iter()
        .map(|template| {
            let policy = PathPolicy::new(
                template.id,
                template.role,
                ControlScope::Platform,
                PolicyEffect::Allow,
                template.methods.iter().copied(),
                [template.pattern],
            );
            compile_policy_statement(&policy)
        })
        .collect()
}

pub(crate) fn validate_builtin_policy_templates() -> CoreResult<()> {
    compile_builtin_policy_templates().map(|_| ())
}

pub(crate) fn policy_within_scope(policy: &PathPolicy, config: &AppConfig) -> bool {
    let Some(scope) = policy.scope.as_ref() else {
        return true;
    };
    let Some(envelope) = scope_envelope(scope, config)
        .and_then(|pattern| CompiledPathPattern::compile(&pattern).ok())
    else {
        return false;
    };
    policy.patterns.iter().all(|pattern| {
        CompiledPathPattern::compile(pattern).is_ok_and(|pattern| envelope.covers(&pattern))
    })
}

fn scope_envelope(scope: &ControlScope, config: &AppConfig) -> Option<String> {
    let tenant_external = |tenant_id: &str| {
        config
            .tenants
            .iter()
            .find(|tenant| tenant.id == tenant_id)
            .map(|tenant| tenant.external_id().to_string())
    };
    let catalog_external = |catalog_id: &str| {
        config
            .catalogs
            .iter()
            .find(|catalog| catalog.id == catalog_id)
            .map(|catalog| catalog.external_id().to_string())
    };
    let collection_external = |collection_id: &str| {
        config
            .collections
            .iter()
            .find(|collection| collection.id == collection_id)
            .map(|collection| collection.external_id().to_string())
    };

    match scope {
        ControlScope::Platform => Some("/_control/v1/**".to_string()),
        ControlScope::Tenant { tenant_id } => Some(format!(
            "/_control/v1/tenants/{}/**",
            tenant_external(tenant_id)?
        )),
        ControlScope::Catalog {
            tenant_id,
            catalog_id,
        } => Some(format!(
            "/_control/v1/tenants/{}/catalogs/{}/**",
            tenant_external(tenant_id)?,
            catalog_external(catalog_id)?
        )),
        ControlScope::Collection {
            tenant_id,
            catalog_id,
            collection_id,
        } => Some(format!(
            "/_control/v1/tenants/{}/catalogs/{}/collections/{}/**",
            tenant_external(tenant_id)?,
            catalog_external(catalog_id)?,
            collection_external(collection_id)?
        )),
    }
}
