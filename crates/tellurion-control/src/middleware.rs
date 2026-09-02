use tellurion_core::{
    authorize_control_mutation, canonicalize_control_path, explain_control_canonical,
    AuthenticatedSubject, AuthorizedControlMutation, CanonicalControlPath, ControlChangeSet,
    ControlEvaluation, ControlScope, MutationControlDecision as ControlDecision,
    MutationControlRequestContext as ControlRequestContext, PrincipalIdentity, Resolver,
    ValidatedControlSnapshot, VersionedControlSnapshot,
};

pub use tellurion_core::{ControlMiddlewareError, ControlRouteDescriptor, ControlRouteRegistry};

pub type AuthorizedMutationContext = AuthorizedControlMutation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedReadContext {
    pub principal: PrincipalIdentity,
    pub effective_scope: ControlScope,
    pub decision_context: ControlEvaluation,
}

#[allow(clippy::too_many_arguments)]
pub async fn control_read_checkpoint<R, H, T>(
    subject: &AuthenticatedSubject,
    method: &str,
    raw_path: &[u8],
    route_template: &str,
    route_registry: &ControlRouteRegistry,
    application_root: &str,
    snapshot: &VersionedControlSnapshot,
    resolver: &R,
    handler: H,
) -> Result<T, ControlMiddlewareError>
where
    R: Resolver + ?Sized,
    H: FnOnce(AuthorizedReadContext) -> T,
{
    if !matches!(method, "GET" | "HEAD") {
        return Err(ControlMiddlewareError::WrongCheckpointKind);
    }
    let (scope, request, canonical) = resolve_request(
        method,
        raw_path,
        route_template,
        route_registry,
        application_root,
        snapshot,
        resolver,
    )
    .await?;
    let validated = snapshot
        .validated_snapshot()
        .map_err(|_| ControlMiddlewareError::InvalidSnapshot)?;
    let decision_context = explain_control_canonical(subject, &request, validated, &canonical);
    if decision_context.decision != ControlDecision::Allow {
        return Err(ControlMiddlewareError::Denied(Box::new(decision_context)));
    }
    Ok(handler(AuthorizedReadContext {
        principal: subject.principal.clone(),
        effective_scope: scope,
        decision_context,
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn control_mutation_checkpoint<R, H, T>(
    subject: &AuthenticatedSubject,
    method: &str,
    raw_path: &[u8],
    route_template: &str,
    route_registry: &ControlRouteRegistry,
    application_root: &str,
    snapshot: &VersionedControlSnapshot,
    changes: &ControlChangeSet,
    resolver: &R,
    correlation_id: &str,
    handler: H,
) -> Result<T, ControlMiddlewareError>
where
    R: Resolver + ?Sized,
    H: FnOnce(AuthorizedMutationContext) -> T,
{
    if matches!(method, "GET" | "HEAD") {
        return Err(ControlMiddlewareError::WrongCheckpointKind);
    }
    if correlation_id.trim().is_empty() {
        return Err(ControlMiddlewareError::InvalidCorrelationId);
    }
    resolve_request(
        method,
        raw_path,
        route_template,
        route_registry,
        application_root,
        snapshot,
        resolver,
    )
    .await?;
    let authorized = authorize_control_mutation(
        subject,
        method,
        raw_path,
        route_template,
        route_registry,
        application_root,
        snapshot,
        changes,
        correlation_id,
    )?;
    Ok(handler(authorized))
}

#[allow(clippy::too_many_arguments)]
async fn resolve_request<R: Resolver + ?Sized>(
    method: &str,
    raw_path: &[u8],
    route_template: &str,
    route_registry: &ControlRouteRegistry,
    application_root: &str,
    snapshot: &VersionedControlSnapshot,
    resolver: &R,
) -> Result<(ControlScope, ControlRequestContext, CanonicalControlPath), ControlMiddlewareError> {
    let canonical = canonicalize_control_path(raw_path, application_root)
        .map_err(ControlMiddlewareError::InvalidPath)?;
    verify_route_template(&canonical, route_template, route_registry)?;
    let validated = snapshot
        .validated_snapshot()
        .map_err(|_| ControlMiddlewareError::InvalidSnapshot)?;
    let scope = resolve_scope(&canonical, resolver, validated).await?;
    let request = ControlRequestContext {
        method: method.to_string(),
        canonical_path: canonical.as_str().to_string(),
        route_template: route_template.to_string(),
        scope: scope.clone(),
    };
    Ok((scope, request, canonical))
}

fn verify_route_template(
    canonical: &CanonicalControlPath,
    route_template: &str,
    route_registry: &ControlRouteRegistry,
) -> Result<(), ControlMiddlewareError> {
    route_registry.verify(canonical, route_template).map(|_| ())
}

async fn resolve_scope<R: Resolver + ?Sized>(
    canonical: &CanonicalControlPath,
    resolver: &R,
    snapshot: &ValidatedControlSnapshot,
) -> Result<ControlScope, ControlMiddlewareError> {
    let segments = canonical.segments().collect::<Vec<_>>();
    if segments[2] == "platform" || segments.len() == 3 {
        return Ok(ControlScope::Platform);
    }

    let tenant = resolver
        .resolve_tenant(segments[3])
        .await
        .map_err(|_| ControlMiddlewareError::Resolution)?;
    if resolver.tenant_external_id(&tenant) != Some(segments[3]) {
        return Err(ControlMiddlewareError::NonCanonicalIdentifier);
    }
    if !snapshot.owns_tenant_identifier(&tenant, segments[3]) {
        return Err(ControlMiddlewareError::Resolution);
    }
    if segments.len() <= 5 || segments.get(4) != Some(&"catalogs") {
        return Ok(ControlScope::Tenant { tenant_id: tenant });
    }

    let catalog = resolver
        .resolve_catalog(&tenant, segments[5])
        .await
        .map_err(|_| ControlMiddlewareError::Resolution)?;
    if resolver.catalog_external_id(&catalog) != Some(segments[5]) {
        return Err(ControlMiddlewareError::NonCanonicalIdentifier);
    }
    if !snapshot.owns_catalog_identifier(&tenant, &catalog, segments[5]) {
        return Err(ControlMiddlewareError::Resolution);
    }
    if segments.len() <= 7 || segments.get(6) != Some(&"collections") {
        return Ok(ControlScope::Catalog {
            tenant_id: tenant,
            catalog_id: catalog,
        });
    }

    let collection = resolver
        .resolve_collection(&catalog, segments[7])
        .await
        .map_err(|_| ControlMiddlewareError::Resolution)?;
    if resolver.collection_external_id(&collection) != Some(segments[7]) {
        return Err(ControlMiddlewareError::NonCanonicalIdentifier);
    }
    if !snapshot.owns_collection_identifier(&catalog, &collection, segments[7]) {
        return Err(ControlMiddlewareError::Resolution);
    }
    Ok(ControlScope::Collection {
        tenant_id: tenant,
        catalog_id: catalog,
        collection_id: collection,
    })
}
