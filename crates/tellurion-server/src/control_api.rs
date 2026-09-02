use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::extract::{
    FromRequest, FromRequestParts, MatchedPath, OriginalUri, Path, Query, Request, State,
};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use tellurion_control::{
    control_mutation_checkpoint, control_read_checkpoint, ControlMiddlewareError,
    ControlRouteDescriptor, ControlRouteRegistry,
};
use tellurion_core::auth::Credential;
use tellurion_core::{
    canonicalize_control_path, preview_control_changes, role_binding_target_id, AppContext,
    AuthenticatedSubject, CollectionKind, ContextState, ControlAuditRecord, ControlChangeSet,
    ControlOperation, ControlPreview, ControlRevision, ControlScope, ControlSnapshot, ControlStore,
    Error, Problem, RoleBinding, SettingsDecl, StaticResolver, VersionedControlSnapshot,
    VisibilityDecl, PROBLEM_JSON,
};

use crate::app::{extract_credential, problem_response};
use crate::control_browser_auth::ControlBrowserAuth;

const CONTROL_ROUTES: [ControlRouteDescriptor; 21] = [
    ControlRouteDescriptor::PlatformOverview,
    ControlRouteDescriptor::PlatformEffectiveSettings,
    ControlRouteDescriptor::PlatformAudit,
    ControlRouteDescriptor::PlatformSettings,
    ControlRouteDescriptor::PlatformBatchImport,
    ControlRouteDescriptor::Tenants,
    ControlRouteDescriptor::Tenant,
    ControlRouteDescriptor::TenantPermanentDelete,
    ControlRouteDescriptor::TenantSettings,
    ControlRouteDescriptor::TenantCatalogs,
    ControlRouteDescriptor::TenantCollectionMove,
    ControlRouteDescriptor::Catalog,
    ControlRouteDescriptor::CatalogPermanentDelete,
    ControlRouteDescriptor::CatalogSettings,
    ControlRouteDescriptor::CatalogCollections,
    ControlRouteDescriptor::Collection,
    ControlRouteDescriptor::CollectionPermanentDelete,
    ControlRouteDescriptor::PlatformPathPolicy,
    ControlRouteDescriptor::CollectionPathPolicy,
    ControlRouteDescriptor::PlatformRoleBindings,
    ControlRouteDescriptor::PlatformRoleBinding,
];

#[cfg(test)]
pub(crate) fn router(ctx: &Arc<AppContext>) -> Router<Arc<AppContext>> {
    router_with_browser(ctx, None)
}

#[derive(Clone)]
struct ControlAuthMiddlewareState {
    browser: Option<Arc<ControlBrowserAuth>>,
}

#[derive(Clone)]
struct ControlRequestCredential(Arc<Credential>);

struct ControlMutationRequestParts {
    uri: axum::http::Uri,
    matched: MatchedPath,
    method: Method,
    headers: HeaderMap,
}

impl<S> FromRequestParts<S> for ControlMutationRequestParts
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let OriginalUri(uri) = OriginalUri::from_request_parts(parts, state)
            .await
            .expect("OriginalUri extraction is infallible");
        let matched = MatchedPath::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(Self {
            uri,
            matched,
            method: parts.method.clone(),
            headers: parts.headers.clone(),
        })
    }
}

pub(crate) fn router_with_browser(
    ctx: &Arc<AppContext>,
    browser: Option<Arc<ControlBrowserAuth>>,
) -> Router<Arc<AppContext>> {
    if ctx.control_store.is_none() || ctx.current().authorizer.is_none() {
        return Router::new();
    }
    let routes =
        Arc::new(ControlRouteRegistry::new(CONTROL_ROUTES).expect("fixed routes are valid"));
    Router::new()
        .route("/_control/v1/platform/overview", get(overview))
        .route(
            "/_control/v1/platform/effective-settings",
            get(effective_settings),
        )
        .route("/_control/v1/platform/audit", get(audit))
        .route("/_control/v1/platform/settings", put(mutate).patch(mutate))
        .route("/_control/v1/platform/import", post(mutate))
        .route("/_control/v1/tenants", get(list_tenants).post(mutate))
        .route(
            "/_control/v1/tenants/{tenant}",
            get(get_tenant).put(mutate).delete(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/permanent-delete",
            delete(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/settings",
            put(mutate).patch(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/catalogs",
            get(list_catalogs).post(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/collection-moves",
            post(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}",
            get(get_catalog).put(mutate).delete(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}/permanent-delete",
            delete(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}/settings",
            put(mutate).patch(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections",
            get(list_collections).post(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}",
            get(get_collection).put(mutate).delete(mutate),
        )
        .route(
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}/permanent-delete",
            delete(mutate),
        )
        .route("/_control/v1/platform/policies/{policy}", put(mutate).delete(mutate))
        .route(
            "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections/{collection}/policies/{policy}",
            put(mutate).delete(mutate),
        )
        .route("/_control/v1/platform/role-bindings", post(mutate))
        .route(
            "/_control/v1/platform/role-bindings/{binding}",
            delete(mutate),
        )
        .layer(Extension(routes))
        .route_layer(axum::middleware::from_fn_with_state(
            ControlAuthMiddlewareState { browser },
            resolve_control_request_credential,
        ))
}

async fn resolve_control_request_credential(
    State(state): State<ControlAuthMiddlewareState>,
    mut request: Request,
    next: Next,
) -> Response {
    let credential = if let Some(browser) = state.browser.as_ref() {
        match browser.resolve_request(request.headers()).await {
            Ok(Some(resolved)) => {
                if !matches!(
                    request.method(),
                    &Method::GET | &Method::HEAD | &Method::OPTIONS
                ) && !browser.cookie_mutation_is_valid(request.headers(), &resolved)
                {
                    return problem_response(
                        StatusCode::FORBIDDEN,
                        "Forbidden",
                        "the browser control request failed origin or CSRF validation",
                    );
                }
                let _verified_principal = &resolved.principal;
                resolved.credential
            }
            Ok(None) => Credential::None,
            Err(_) => {
                return problem_response(
                    StatusCode::UNAUTHORIZED,
                    "Unauthorized",
                    "the presented credentials could not be reconciled",
                )
            }
        }
    } else {
        extract_credential(request.headers())
    };
    request
        .extensions_mut()
        .insert(ControlRequestCredential(Arc::new(credential)));
    next.run(request).await
}

struct AuthorizedReadPrelude {
    store: Arc<dyn ControlStore>,
    state: Arc<ContextState>,
    snapshot: VersionedControlSnapshot,
}

async fn authorized_read_prelude(
    ctx: &Arc<AppContext>,
    routes: &ControlRouteRegistry,
    uri: &axum::http::Uri,
    matched: &MatchedPath,
    method: &Method,
    credential: &Credential,
) -> Result<AuthorizedReadPrelude, Response> {
    let Some(store) = ctx.control_store.clone() else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    let state = ctx.current();
    let Some(authorizer) = state.authorizer.clone() else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    let canonical = canonicalize_control_path(uri.path().as_bytes(), "")
        .map_err(|_| StatusCode::NOT_FOUND.into_response())?;
    routes
        .verify(&canonical, matched.as_str())
        .map_err(|_| StatusCode::NOT_FOUND.into_response())?;
    let subject = authorizer.subject(credential).await;
    let Some(principal) = subject.identity else {
        return Err(problem_response(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "the presented credential could not be verified",
        ));
    };
    let subject = AuthenticatedSubject {
        principal,
        claims: subject.claims,
    };
    let snapshot = store
        .load_snapshot()
        .await
        .map_err(render_read_store_error)?;
    let resolver = StaticResolver::build(&snapshot.snapshot.config);
    control_read_checkpoint(
        &subject,
        method.as_str(),
        uri.path().as_bytes(),
        matched.as_str(),
        routes,
        "",
        &snapshot,
        &resolver,
        |_| (),
    )
    .await
    .map_err(render_read_checkpoint_error)?;
    Ok(AuthorizedReadPrelude {
        store,
        state,
        snapshot,
    })
}

#[derive(Debug, Serialize)]
struct OverviewView {
    scope: &'static str,
    store_revision: ControlRevision,
    applied_revision: ControlRevision,
    lag: ControlRevision,
    last_successful_refresh_unix_ms: Option<u64>,
    poll_failures: u64,
    activation_failures: u64,
    config_version: String,
}

async fn overview(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    _headers: HeaderMap,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let runtime = ctx.control_runtime_status.snapshot();
    json_with_control_etag(
        OverviewView {
            scope: "self",
            store_revision: runtime.store_revision,
            applied_revision: runtime.applied_revision,
            lag: runtime.lag,
            last_successful_refresh_unix_ms: runtime.last_successful_refresh_unix_ms,
            poll_failures: runtime.poll_failures,
            activation_failures: runtime.activation_failures,
            config_version: read.state.config_version.to_string(),
        },
        read.snapshot.revision,
    )
}

#[derive(Debug, Serialize)]
struct EffectiveSettingsView {
    applied_revision: ControlRevision,
    effective: crate::config_view::EffectiveConfigView,
}

async fn effective_settings(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    _headers: HeaderMap,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let applied_revision = read.state.control_revision.unwrap_or(0);
    json_with_control_etag(
        EffectiveSettingsView {
            applied_revision,
            effective: crate::config_view::platform_effective_config_view(&read.state),
        },
        applied_revision,
    )
}

#[derive(Debug, Default, Deserialize)]
struct AuditQuery {
    after: Option<ControlRevision>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct AuditActorView {
    issuer: String,
    subject: String,
}

#[derive(Debug, Serialize)]
struct AuditItemView {
    revision: ControlRevision,
    actor: AuditActorView,
    method: String,
    canonical_path: String,
    correlation_id: String,
    changed_resources: Vec<String>,
    recorded_at_unix_ms: u64,
    applying_instance: String,
}

impl From<ControlAuditRecord> for AuditItemView {
    fn from(record: ControlAuditRecord) -> Self {
        Self {
            revision: record.revision,
            actor: AuditActorView {
                issuer: record.actor.issuer,
                subject: record.actor.subject,
            },
            method: record.request.method,
            canonical_path: record.request.canonical_path,
            correlation_id: record.request.correlation_id,
            changed_resources: record.changed_resources,
            recorded_at_unix_ms: record.recorded_at_unix_ms,
            applying_instance: record.applying_instance,
        }
    }
}

#[derive(Debug, Serialize)]
struct AuditPageView {
    revision: ControlRevision,
    items: Vec<AuditItemView>,
    next_after: Option<ControlRevision>,
}

async fn audit(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    _headers: HeaderMap,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let Query(query) = match Query::<AuditQuery>::try_from_uri(&uri) {
        Ok(query) => query,
        Err(_) => {
            return problem_response(
                StatusCode::BAD_REQUEST,
                "InvalidAuditQuery",
                "the audit query parameters are invalid",
            )
        }
    };
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "InvalidLimit",
            "limit must be between 1 and 100",
        );
    }
    let after = query.after.unwrap_or(0);
    let mut records = match read.store.audit_since(after, limit + 1).await {
        Ok(records) => records,
        Err(error) => return render_read_store_error(error),
    };
    records.retain(|record| record.revision <= read.snapshot.revision);
    records.sort_by_key(|record| record.revision);
    let has_more = records.len() > limit as usize;
    records.truncate(limit as usize);
    let next_after = if has_more {
        records.last().map(|record| record.revision)
    } else {
        None
    };
    json_with_control_etag(
        AuditPageView {
            revision: read.snapshot.revision,
            items: records.into_iter().map(AuditItemView::from).collect(),
            next_after,
        },
        read.snapshot.revision,
    )
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceListQuery {
    after: Option<String>,
    limit: Option<u32>,
}

enum ResourceListQueryError {
    InvalidQuery,
    InvalidLimit,
}

#[derive(Debug, Serialize)]
struct ResourceEnvelope<T> {
    control_revision: ControlRevision,
    entity_version: String,
    resource: T,
}

#[derive(Debug, Serialize)]
struct ResourcePage<T> {
    control_revision: ControlRevision,
    items: Vec<ResourceEnvelope<T>>,
    next_after: Option<String>,
}

#[derive(Debug, Serialize)]
struct TenantView {
    id: String,
    settings: SettingsDecl,
    tombstoned: bool,
}

#[derive(Debug, Serialize)]
struct VisibilityView {
    public: bool,
    shared_with: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CatalogView {
    id: String,
    tenant: String,
    settings: SettingsDecl,
    visibility: VisibilityView,
    tombstoned: bool,
}

#[derive(Debug, Serialize)]
struct CollectionView {
    id: String,
    catalog: String,
    kind: CollectionKind,
    settings: SettingsDecl,
    visibility: VisibilityView,
    tombstoned: bool,
}

fn resource_list_query(
    uri: &axum::http::Uri,
) -> Result<(usize, Option<String>), ResourceListQueryError> {
    let Query(query) = Query::<ResourceListQuery>::try_from_uri(uri)
        .map_err(|_| ResourceListQueryError::InvalidQuery)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ResourceListQueryError::InvalidLimit);
    }
    Ok((limit as usize, query.after))
}

fn render_resource_list_query_error(error: ResourceListQueryError) -> Response {
    match error {
        ResourceListQueryError::InvalidQuery => problem_response(
            StatusCode::BAD_REQUEST,
            "InvalidControlQuery",
            "the resource list query parameters are invalid",
        ),
        ResourceListQueryError::InvalidLimit => problem_response(
            StatusCode::BAD_REQUEST,
            "InvalidLimit",
            "limit must be between 1 and 100",
        ),
    }
}

fn truncate_page<T>(
    items: &mut Vec<T>,
    limit: usize,
    external_id: impl Fn(&T) -> &str,
) -> Option<String> {
    let has_more = items.len() > limit;
    items.truncate(limit);
    has_more
        .then(|| items.last().map(|item| external_id(item).to_string()))
        .flatten()
}

fn entity_version(snapshot: &VersionedControlSnapshot, scope: &ControlScope) -> String {
    snapshot
        .entity_versions
        .get(&scope.resource_key())
        .cloned()
        .unwrap_or_else(|| "0".to_string())
}

fn visibility_view(
    visibility: &VisibilityDecl,
    snapshot: &VersionedControlSnapshot,
) -> VisibilityView {
    let mut shared_with = visibility
        .shared_with
        .iter()
        .filter_map(|internal_id| {
            snapshot
                .snapshot
                .config
                .tenants
                .iter()
                .find(|tenant| tenant.id == *internal_id)
                .map(|tenant| tenant.external_id().to_string())
        })
        .collect::<Vec<_>>();
    shared_with.sort();
    shared_with.dedup();
    VisibilityView {
        public: visibility.public,
        shared_with,
    }
}

fn json_with_entity_etag<T: Serialize>(value: T, entity_version: &str) -> Response {
    let mut response = Json(value).into_response();
    let etag = format!("\"control-entity-{entity_version}\"");
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

async fn list_tenants(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    _headers: HeaderMap,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let (limit, after) = match resource_list_query(&uri) {
        Ok(query) => query,
        Err(error) => return render_resource_list_query_error(error),
    };
    let mut tenants = read
        .snapshot
        .snapshot
        .config
        .tenants
        .iter()
        .collect::<Vec<_>>();
    tenants.sort_by(|left, right| left.external_id().cmp(right.external_id()));
    tenants.retain(|tenant| {
        after
            .as_deref()
            .is_none_or(|cursor| tenant.external_id() > cursor)
    });
    let next_after = truncate_page(&mut tenants, limit, |tenant| tenant.external_id());
    let revision = read.snapshot.revision;
    let items = tenants
        .into_iter()
        .map(|tenant| {
            let scope = ControlScope::Tenant {
                tenant_id: tenant.id.clone(),
            };
            ResourceEnvelope {
                control_revision: revision,
                entity_version: entity_version(&read.snapshot, &scope),
                resource: TenantView {
                    id: tenant.external_id().to_string(),
                    settings: tenant.settings.clone(),
                    tombstoned: read.snapshot.snapshot.tombstoned_resources.contains(&scope),
                },
            }
        })
        .collect();
    json_with_control_etag(
        ResourcePage {
            control_revision: revision,
            items,
            next_after,
        },
        revision,
    )
}

async fn get_tenant(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    Path(tenant_id): Path<String>,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let Some(tenant) = read
        .snapshot
        .snapshot
        .config
        .tenants
        .iter()
        .find(|tenant| tenant.external_id() == tenant_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let scope = ControlScope::Tenant {
        tenant_id: tenant.id.clone(),
    };
    let version = entity_version(&read.snapshot, &scope);
    json_with_entity_etag(
        ResourceEnvelope {
            control_revision: read.snapshot.revision,
            entity_version: version.clone(),
            resource: TenantView {
                id: tenant.external_id().to_string(),
                settings: tenant.settings.clone(),
                tombstoned: read.snapshot.snapshot.tombstoned_resources.contains(&scope),
            },
        },
        &version,
    )
}

async fn list_catalogs(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    Path(tenant_id): Path<String>,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let (limit, after) = match resource_list_query(&uri) {
        Ok(query) => query,
        Err(error) => return render_resource_list_query_error(error),
    };
    let Some(tenant) = read
        .snapshot
        .snapshot
        .config
        .tenants
        .iter()
        .find(|tenant| tenant.external_id() == tenant_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut catalogs = read
        .snapshot
        .snapshot
        .config
        .catalogs
        .iter()
        .filter(|catalog| catalog.tenant == tenant.id)
        .collect::<Vec<_>>();
    catalogs.sort_by(|left, right| left.external_id().cmp(right.external_id()));
    catalogs.retain(|catalog| {
        after
            .as_deref()
            .is_none_or(|cursor| catalog.external_id() > cursor)
    });
    let next_after = truncate_page(&mut catalogs, limit, |catalog| catalog.external_id());
    let revision = read.snapshot.revision;
    let items = catalogs
        .into_iter()
        .map(|catalog| {
            let scope = ControlScope::Catalog {
                tenant_id: tenant.id.clone(),
                catalog_id: catalog.id.clone(),
            };
            ResourceEnvelope {
                control_revision: revision,
                entity_version: entity_version(&read.snapshot, &scope),
                resource: CatalogView {
                    id: catalog.external_id().to_string(),
                    tenant: tenant.external_id().to_string(),
                    settings: catalog.settings.clone(),
                    visibility: visibility_view(&catalog.visibility, &read.snapshot),
                    tombstoned: read.snapshot.snapshot.tombstoned_resources.contains(&scope),
                },
            }
        })
        .collect();
    json_with_control_etag(
        ResourcePage {
            control_revision: revision,
            items,
            next_after,
        },
        revision,
    )
}

async fn get_catalog(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    Path((tenant_id, catalog_id)): Path<(String, String)>,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let Some(tenant) = read
        .snapshot
        .snapshot
        .config
        .tenants
        .iter()
        .find(|tenant| tenant.external_id() == tenant_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(catalog) = read
        .snapshot
        .snapshot
        .config
        .catalogs
        .iter()
        .find(|catalog| catalog.tenant == tenant.id && catalog.external_id() == catalog_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let scope = ControlScope::Catalog {
        tenant_id: tenant.id.clone(),
        catalog_id: catalog.id.clone(),
    };
    let version = entity_version(&read.snapshot, &scope);
    json_with_entity_etag(
        ResourceEnvelope {
            control_revision: read.snapshot.revision,
            entity_version: version.clone(),
            resource: CatalogView {
                id: catalog.external_id().to_string(),
                tenant: tenant.external_id().to_string(),
                settings: catalog.settings.clone(),
                visibility: visibility_view(&catalog.visibility, &read.snapshot),
                tombstoned: read.snapshot.snapshot.tombstoned_resources.contains(&scope),
            },
        },
        &version,
    )
}

async fn list_collections(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    Path((tenant_id, catalog_id)): Path<(String, String)>,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let (limit, after) = match resource_list_query(&uri) {
        Ok(query) => query,
        Err(error) => return render_resource_list_query_error(error),
    };
    let Some(tenant) = read
        .snapshot
        .snapshot
        .config
        .tenants
        .iter()
        .find(|tenant| tenant.external_id() == tenant_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(catalog) = read
        .snapshot
        .snapshot
        .config
        .catalogs
        .iter()
        .find(|catalog| catalog.tenant == tenant.id && catalog.external_id() == catalog_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut collections = read
        .snapshot
        .snapshot
        .config
        .collections
        .iter()
        .filter(|collection| collection.catalog == catalog.id)
        .collect::<Vec<_>>();
    collections.sort_by(|left, right| left.external_id().cmp(right.external_id()));
    collections.retain(|collection| {
        after
            .as_deref()
            .is_none_or(|cursor| collection.external_id() > cursor)
    });
    let next_after = truncate_page(&mut collections, limit, |collection| {
        collection.external_id()
    });
    let revision = read.snapshot.revision;
    let items = collections
        .into_iter()
        .map(|collection| {
            let scope = ControlScope::Collection {
                tenant_id: tenant.id.clone(),
                catalog_id: catalog.id.clone(),
                collection_id: collection.id.clone(),
            };
            ResourceEnvelope {
                control_revision: revision,
                entity_version: entity_version(&read.snapshot, &scope),
                resource: CollectionView {
                    id: collection.external_id().to_string(),
                    catalog: catalog.external_id().to_string(),
                    kind: collection.kind,
                    settings: collection.settings.clone(),
                    visibility: visibility_view(&collection.visibility, &read.snapshot),
                    tombstoned: read.snapshot.snapshot.tombstoned_resources.contains(&scope),
                },
            }
        })
        .collect();
    json_with_control_etag(
        ResourcePage {
            control_revision: revision,
            items,
            next_after,
        },
        revision,
    )
}

async fn get_collection(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    OriginalUri(uri): OriginalUri,
    matched: MatchedPath,
    method: Method,
    Path((tenant_id, catalog_id, collection_id)): Path<(String, String, String)>,
) -> Response {
    let read =
        match authorized_read_prelude(&ctx, &routes, &uri, &matched, &method, &credential).await {
            Ok(read) => read,
            Err(response) => return response,
        };
    let Some(tenant) = read
        .snapshot
        .snapshot
        .config
        .tenants
        .iter()
        .find(|tenant| tenant.external_id() == tenant_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(catalog) = read
        .snapshot
        .snapshot
        .config
        .catalogs
        .iter()
        .find(|catalog| catalog.tenant == tenant.id && catalog.external_id() == catalog_id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(collection) = read
        .snapshot
        .snapshot
        .config
        .collections
        .iter()
        .find(|collection| {
            collection.catalog == catalog.id && collection.external_id() == collection_id
        })
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let scope = ControlScope::Collection {
        tenant_id: tenant.id.clone(),
        catalog_id: catalog.id.clone(),
        collection_id: collection.id.clone(),
    };
    let version = entity_version(&read.snapshot, &scope);
    json_with_entity_etag(
        ResourceEnvelope {
            control_revision: read.snapshot.revision,
            entity_version: version.clone(),
            resource: CollectionView {
                id: collection.external_id().to_string(),
                catalog: catalog.external_id().to_string(),
                kind: collection.kind,
                settings: collection.settings.clone(),
                visibility: visibility_view(&collection.visibility, &read.snapshot),
                tombstoned: read.snapshot.snapshot.tombstoned_resources.contains(&scope),
            },
        },
        &version,
    )
}

fn json_with_control_etag<T: Serialize>(value: T, revision: ControlRevision) -> Response {
    let mut response = Json(value).into_response();
    let etag = format!("\"control-revision-{revision}\"");
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MutationQuery {
    dry_run: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ControlPreviewView {
    base_revision: ControlRevision,
    prospective_revision: ControlRevision,
    changed_resources: Vec<String>,
    entity_versions: BTreeMap<String, String>,
}

fn role_binding_preview_keys(changes: &ControlChangeSet) -> BTreeMap<String, String> {
    changes
        .operations
        .iter()
        .filter_map(|versioned| {
            let binding = match &versioned.operation {
                ControlOperation::PutRoleBinding(binding) => binding.clone(),
                ControlOperation::DeleteRoleBinding {
                    principal,
                    scope,
                    role,
                } => RoleBinding {
                    principal: principal.clone(),
                    role: role.clone(),
                    scope: scope.clone(),
                },
                _ => return None,
            };
            Some((
                format!(
                    "role-binding/{}/{}/{}/{}",
                    binding.scope.resource_key(),
                    binding.role,
                    binding.principal.issuer,
                    binding.principal.subject
                ),
                format!("role-binding/{}", role_binding_target_id(&binding)),
            ))
        })
        .collect()
}

fn external_scope_key(scope: &ControlScope, snapshot: &ControlSnapshot) -> Option<String> {
    match scope {
        ControlScope::Platform => Some("platform".to_string()),
        ControlScope::Tenant { tenant_id } => snapshot
            .config
            .tenants
            .iter()
            .find(|tenant| tenant.id == *tenant_id)
            .map(|tenant| format!("tenant/{}", tenant.external_id())),
        ControlScope::Catalog {
            tenant_id,
            catalog_id,
        } => {
            let tenant = snapshot
                .config
                .tenants
                .iter()
                .find(|tenant| tenant.id == *tenant_id)?;
            let catalog = snapshot
                .config
                .catalogs
                .iter()
                .find(|catalog| catalog.id == *catalog_id && catalog.tenant == *tenant_id)?;
            Some(format!(
                "tenant/{}/catalog/{}",
                tenant.external_id(),
                catalog.external_id()
            ))
        }
        ControlScope::Collection {
            tenant_id,
            catalog_id,
            collection_id,
        } => {
            let tenant = snapshot
                .config
                .tenants
                .iter()
                .find(|tenant| tenant.id == *tenant_id)?;
            let catalog = snapshot
                .config
                .catalogs
                .iter()
                .find(|catalog| catalog.id == *catalog_id && catalog.tenant == *tenant_id)?;
            let collection = snapshot.config.collections.iter().find(|collection| {
                collection.id == *collection_id && collection.catalog == *catalog_id
            })?;
            Some(format!(
                "tenant/{}/catalog/{}/collection/{}",
                tenant.external_id(),
                catalog.external_id(),
                collection.external_id()
            ))
        }
    }
}

fn externalize_preview_key(
    key: &str,
    authoritative: &ControlSnapshot,
    prospective: &ControlSnapshot,
    binding_keys: &BTreeMap<String, String>,
) -> Option<String> {
    if let Some(external) = binding_keys.get(key) {
        return Some(external.clone());
    }
    if key == "platform" {
        return Some(key.to_string());
    }
    if key.starts_with("path-policy/") {
        return Some(key.to_string());
    }
    let segments = key.split('/').collect::<Vec<_>>();
    let scope = match segments.as_slice() {
        ["tenant", tenant_id] => ControlScope::Tenant {
            tenant_id: (*tenant_id).to_string(),
        },
        ["tenant", tenant_id, "catalog", catalog_id] => ControlScope::Catalog {
            tenant_id: (*tenant_id).to_string(),
            catalog_id: (*catalog_id).to_string(),
        },
        ["tenant", tenant_id, "catalog", catalog_id, "collection", collection_id] => {
            ControlScope::Collection {
                tenant_id: (*tenant_id).to_string(),
                catalog_id: (*catalog_id).to_string(),
                collection_id: (*collection_id).to_string(),
            }
        }
        _ => return None,
    };
    external_scope_key(&scope, prospective).or_else(|| external_scope_key(&scope, authoritative))
}

fn control_preview_view(
    preview: &ControlPreview,
    authoritative: &ControlSnapshot,
    changes: &ControlChangeSet,
) -> Option<ControlPreviewView> {
    let binding_keys = role_binding_preview_keys(changes);
    let mut changed_resources = BTreeSet::new();
    let mut entity_versions = BTreeMap::new();
    for internal in &preview.changed_resources {
        let external = externalize_preview_key(
            internal,
            authoritative,
            preview.prospective_snapshot(),
            &binding_keys,
        )?;
        if let Some(version) = preview.entity_versions.get(internal) {
            entity_versions.insert(external.clone(), version.clone());
        }
        changed_resources.insert(external);
    }
    Some(ControlPreviewView {
        base_revision: preview.base_revision,
        prospective_revision: preview.prospective_revision,
        changed_resources: changed_resources.into_iter().collect(),
        entity_versions,
    })
}

async fn mutate(
    State(ctx): State<Arc<AppContext>>,
    Extension(routes): Extension<Arc<ControlRouteRegistry>>,
    Extension(ControlRequestCredential(credential)): Extension<ControlRequestCredential>,
    ControlMutationRequestParts {
        uri,
        matched,
        method,
        headers,
    }: ControlMutationRequestParts,
    request: Request,
) -> Response {
    let Some(store) = ctx.control_store.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let state = ctx.current();
    let Some(authorizer) = state.authorizer.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let subject = authorizer.subject(&credential).await;
    let Some(principal) = subject.identity else {
        return problem_response(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "the presented credential could not be verified",
        );
    };
    let subject = AuthenticatedSubject {
        principal,
        claims: subject.claims,
    };
    let Json(changes) = match Json::<ControlChangeSet>::from_request(request, &ctx).await {
        Ok(changes) => changes,
        Err(_) => return invalid_control_mutation_response(),
    };
    let snapshot = match store.load_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => return render_core_error(error),
    };
    let resolver = StaticResolver::build(&snapshot.snapshot.config);
    let correlation_id = crate::request_id::current_id(&headers);
    let authorization = match control_mutation_checkpoint(
        &subject,
        method.as_str(),
        uri.path().as_bytes(),
        matched.as_str(),
        &routes,
        "",
        &snapshot,
        &changes,
        &resolver,
        &correlation_id,
        |authorization| authorization,
    )
    .await
    {
        Ok(authorization) => authorization,
        Err(error) => return render_checkpoint_error(error),
    };
    let Query(query) = match Query::<MutationQuery>::try_from_uri(&uri) {
        Ok(query) => query,
        Err(_) => return invalid_control_mutation_response(),
    };
    if query.dry_run.unwrap_or(false) {
        return match preview_control_changes(&snapshot, &authorization, &changes) {
            Ok(preview) => match control_preview_view(&preview, &snapshot.snapshot, &changes) {
                Some(view) => json_with_control_etag(view, snapshot.revision),
                None => {
                    tracing::error!("control preview produced an unknown internal resource key");
                    problem_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "InternalError",
                        "an internal control preview error occurred",
                    )
                }
            },
            Err(error) => render_core_error(error),
        };
    }
    match store.transact(&authorization, &changes).await {
        Ok(commit) => Json(commit).into_response(),
        Err(error) => render_core_error(error),
    }
}

fn invalid_control_mutation_response() -> Response {
    problem_response(
        StatusCode::BAD_REQUEST,
        "InvalidControlMutation",
        "the durable control mutation request is invalid",
    )
}

fn render_read_checkpoint_error(error: ControlMiddlewareError) -> Response {
    match error {
        ControlMiddlewareError::Denied(evaluation) => match evaluation.effective_scope {
            ControlScope::Platform | ControlScope::Tenant { .. } => problem_response(
                StatusCode::FORBIDDEN,
                "Forbidden",
                "the presented credential does not authorize this control read",
            ),
            ControlScope::Catalog { .. } | ControlScope::Collection { .. } => {
                StatusCode::NOT_FOUND.into_response()
            }
        },
        ControlMiddlewareError::InvalidPath(_)
        | ControlMiddlewareError::Resolution
        | ControlMiddlewareError::NonCanonicalIdentifier
        | ControlMiddlewareError::UnmatchedRoute
        | ControlMiddlewareError::WrongCheckpointKind
        | ControlMiddlewareError::InvalidCorrelationId
        | ControlMiddlewareError::MutationIntentMismatch => StatusCode::NOT_FOUND.into_response(),
        ControlMiddlewareError::InvalidSnapshot => {
            tracing::error!("durable control read used an invalid control snapshot");
            problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "an internal control read error occurred",
            )
        }
    }
}

fn render_read_store_error(error: Error) -> Response {
    tracing::error!(%error, "durable control read failed");
    problem_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "InternalError",
        "an internal control read error occurred",
    )
}

fn render_checkpoint_error(error: ControlMiddlewareError) -> Response {
    match error {
        ControlMiddlewareError::Denied(evaluation) => match evaluation.effective_scope {
            ControlScope::Platform | ControlScope::Tenant { .. } => problem_response(
                StatusCode::FORBIDDEN,
                "Forbidden",
                "the presented credential does not authorize this control mutation",
            ),
            ControlScope::Catalog { .. } | ControlScope::Collection { .. } => {
                StatusCode::NOT_FOUND.into_response()
            }
        },
        ControlMiddlewareError::Resolution
        | ControlMiddlewareError::NonCanonicalIdentifier
        | ControlMiddlewareError::UnmatchedRoute => StatusCode::NOT_FOUND.into_response(),
        ControlMiddlewareError::InvalidPath(_)
        | ControlMiddlewareError::WrongCheckpointKind
        | ControlMiddlewareError::InvalidCorrelationId
        | ControlMiddlewareError::MutationIntentMismatch => invalid_control_mutation_response(),
        ControlMiddlewareError::InvalidSnapshot => {
            tracing::error!("durable control mutation used an invalid control snapshot");
            problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "an internal control mutation error occurred",
            )
        }
    }
}

fn render_core_error(error: Error) -> Response {
    let problem = Problem::from_core_error(&error, "control");
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, Json(problem)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
    response
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use axum::body::{to_bytes, Body, Bytes, HttpBody};
    use axum::http::{Request, StatusCode};
    use http_body::Frame;
    use tellurion_core::auth::{AuthDecision, Credential, PlatformAdminDecision, TenantAuthorizer};
    use tellurion_core::catalog::{CatalogSource, PhysicalCollection};
    use tellurion_core::router::{DriverFactory, Registry, Router as CoreRouter, StorageDriver};
    use tellurion_core::{
        AppConfig, AppContext, AuthenticatedSubject, AuthorizedControlMutation, BootstrapOutcome,
        ControlAuditRecord, ControlBootstrapMode, ControlBrowserAuthConfig, ControlChangeSet,
        ControlCommit, ControlEvent, ControlEventCursor, ControlOperation, ControlRevision,
        ControlScope, ControlSnapshot, ControlStore, Error, FileStyleStore, InMemoryControlStore,
        MokaTileCache, PathPolicy, PolicyEffect, PrincipalIdentity, Resolver, RoleBinding,
        StaticResolver, StyleStore, TileCache, VersionedControlOperation, VersionedControlSnapshot,
    };
    use tokio::sync::Notify;
    use tower::ServiceExt;

    use crate::control_session::ControlSessionStore as _;

    struct FixtureDriver;

    #[async_trait::async_trait]
    impl CatalogSource for FixtureDriver {
        async fn collections(&self) -> tellurion_core::Result<Vec<PhysicalCollection>> {
            Ok(Vec::new())
        }
    }

    impl StorageDriver for FixtureDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FixtureDriver)
        }
    }

    struct FixtureFactory;

    impl DriverFactory for FixtureFactory {
        fn name(&self) -> &str {
            "fixture"
        }

        fn build(
            &self,
            _: &tellurion_core::StorageDecl,
        ) -> tellurion_core::Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FixtureDriver))
        }
    }

    #[derive(Clone)]
    struct VerifiedAuthorizer {
        identity: PrincipalIdentity,
    }

    struct DistinctAuthorizer;

    #[async_trait::async_trait]
    impl TenantAuthorizer for DistinctAuthorizer {
        async fn authorize(&self, _: &Credential, _: &str) -> AuthDecision {
            AuthDecision::Allow
        }

        async fn subject(&self, credential: &Credential) -> tellurion_core::Subject {
            let identity = match credential {
                Credential::Bearer(token) if token == "verified" => Some(principal()),
                Credential::Bearer(token) if token == "other-identity" => Some(PrincipalIdentity {
                    issuer: "https://issuer.example".to_string(),
                    subject: "operator-2".to_string(),
                }),
                _ => None,
            };
            match identity {
                Some(identity) => tellurion_core::Subject {
                    memberships: HashMap::new(),
                    claims: HashMap::new(),
                    principal: Some(format!("{}#{}", identity.issuer, identity.subject)),
                    identity: Some(identity),
                },
                None => tellurion_core::Subject::anonymous(),
            }
        }

        async fn authorize_platform_admin(&self, _: &Credential) -> PlatformAdminDecision {
            PlatformAdminDecision::Deny(tellurion_core::DenyReason::NoCredential)
        }
    }

    struct UnusedBrowserOidc;

    #[async_trait::async_trait]
    impl crate::control_browser_auth::OidcTransport for UnusedBrowserOidc {
        async fn discover(
            &self,
            _: &str,
        ) -> Result<crate::control_browser_auth::OidcEndpoints, ()> {
            Err(())
        }

        async fn exchange(
            &self,
            _: &url::Url,
            _: crate::control_browser_auth::TokenExchange,
        ) -> Result<crate::control_browser_auth::OidcTokens, ()> {
            Err(())
        }
    }

    struct UnusedBrowserIdentity;

    #[async_trait::async_trait]
    impl crate::control_browser_auth::BrowserIdentityVerifier for UnusedBrowserIdentity {
        async fn verify(&self, _: &str, _: &str) -> Result<PrincipalIdentity, ()> {
            Err(())
        }
    }

    struct ContextCredentialAuthorizer {
        context: Arc<AppContext>,
    }

    #[async_trait::async_trait]
    impl crate::control_browser_auth::ControlCredentialAuthorizer for ContextCredentialAuthorizer {
        async fn subject(&self, credential: &Credential) -> Option<AuthenticatedSubject> {
            let authorizer = self.context.current().authorizer.clone()?;
            let subject = authorizer.subject(credential).await;
            Some(AuthenticatedSubject {
                principal: subject.identity?,
                claims: subject.claims,
            })
        }

        async fn authorize_platform_admin(&self, _: &Credential) -> Option<String> {
            None
        }
    }

    fn browser_config() -> ControlBrowserAuthConfig {
        ControlBrowserAuthConfig {
            issuer: "https://issuer.example".to_string(),
            client_id: "control-ui".to_string(),
            client_secret_env: None,
            public_origin: "https://console.example.com".to_string(),
            scopes: vec!["openid".to_string()],
            session_ttl_s: 3600,
            login_ttl_s: 300,
            max_sessions: 32,
        }
    }

    fn browser_auth(
        context: &Arc<AppContext>,
        sessions: Arc<dyn crate::control_session::ControlSessionStore>,
    ) -> Arc<crate::control_browser_auth::ControlBrowserAuth> {
        crate::control_browser_auth::ControlBrowserAuth::new_with_dependencies(
            browser_config(),
            None,
            sessions,
            Arc::new(UnusedBrowserOidc),
            Arc::new(UnusedBrowserIdentity),
            Arc::new(ContextCredentialAuthorizer {
                context: Arc::clone(context),
            }),
        )
    }

    async fn browser_session(
        context: &Arc<AppContext>,
        access_token: &str,
        identity: PrincipalIdentity,
    ) -> (
        Arc<crate::control_browser_auth::ControlBrowserAuth>,
        String,
        String,
    ) {
        let sessions = Arc::new(crate::control_session::InMemoryControlSessionStore::new(32));
        let active = crate::control_session::ControlBrowserSession::new(
            identity,
            access_token.to_string(),
            tokio::time::Instant::now() + std::time::Duration::from_secs(600),
        );
        let csrf = active.csrf_token.clone();
        let session_id = sessions.create(active).await.unwrap();
        (
            browser_auth(
                context,
                Arc::clone(&sessions) as Arc<dyn crate::control_session::ControlSessionStore>,
            ),
            format!("tellurion_control_session={session_id}"),
            csrf,
        )
    }

    #[async_trait::async_trait]
    impl TenantAuthorizer for VerifiedAuthorizer {
        async fn authorize(&self, _: &Credential, _: &str) -> AuthDecision {
            AuthDecision::Allow
        }

        async fn subject(&self, credential: &Credential) -> tellurion_core::Subject {
            match credential {
                Credential::Bearer(token)
                    if matches!(
                        token.as_str(),
                        "verified" | "sentinel-credential" | "postgres://sentinel-dsn"
                    ) =>
                {
                    tellurion_core::Subject {
                        memberships: HashMap::new(),
                        claims: HashMap::new(),
                        principal: Some(format!(
                            "{}#{}",
                            self.identity.issuer, self.identity.subject
                        )),
                        identity: Some(self.identity.clone()),
                    }
                }
                _ => tellurion_core::Subject::anonymous(),
            }
        }

        async fn authorize_platform_admin(&self, _: &Credential) -> PlatformAdminDecision {
            PlatformAdminDecision::Deny(tellurion_core::DenyReason::NoCredential)
        }
    }

    fn principal() -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "operator-1".to_string(),
        }
    }

    fn fixture_config() -> AppConfig {
        serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fixture, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-internal, external_id: acme } ]
catalogs: [ { id: catalog-internal, external_id: cadastre, tenant: tenant-internal } ]
collections:
  - id: collection-internal
    external_id: roads
    catalog: catalog-internal
    storage: main
auth:
  trusted_issuers:
    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }
"#,
        )
        .unwrap()
    }

    fn snapshot() -> ControlSnapshot {
        ControlSnapshot {
            config: fixture_config(),
            role_bindings: vec![RoleBinding {
                principal: principal(),
                role: "sysadmin".to_string(),
                scope: ControlScope::Platform,
            }],
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        }
    }

    async fn fixture_context(with_store: bool) -> (Arc<AppContext>, Arc<InMemoryControlStore>) {
        fixture_context_with(
            with_store,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            snapshot(),
        )
        .await
    }

    async fn fixture_context_with(
        with_store: bool,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        seed: ControlSnapshot,
    ) -> (Arc<AppContext>, Arc<InMemoryControlStore>) {
        let config = fixture_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(FixtureFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let store = Arc::new(InMemoryControlStore::new());
        store
            .bootstrap_if_empty(
                &seed,
                &principal(),
                ControlBootstrapMode::AllowEmptyPlatform,
            )
            .await
            .unwrap();
        let context = AppContext::new(config, router, resolver, authorizer, cache, style_store);
        let context = if with_store {
            context.with_control_store(Arc::clone(&store) as Arc<dyn ControlStore>)
        } else {
            context
        };
        (Arc::new(context), store)
    }

    struct RevisionConflictStore {
        inner: Arc<InMemoryControlStore>,
    }

    struct LoadCountingStore {
        inner: Arc<InMemoryControlStore>,
        snapshot_reads: Arc<AtomicUsize>,
    }

    struct BlockingLoadStore {
        inner: Arc<InMemoryControlStore>,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    struct FailingLoadStore;

    #[async_trait::async_trait]
    impl ControlStore for FailingLoadStore {
        async fn bootstrap_if_empty(
            &self,
            _: &ControlSnapshot,
            _: &PrincipalIdentity,
            _: ControlBootstrapMode,
        ) -> tellurion_core::Result<BootstrapOutcome> {
            Err(raw_store_error())
        }

        async fn current_revision(&self) -> tellurion_core::Result<Option<ControlRevision>> {
            Err(raw_store_error())
        }

        async fn load_snapshot(&self) -> tellurion_core::Result<VersionedControlSnapshot> {
            Err(raw_store_error())
        }

        async fn transact(
            &self,
            _: &AuthorizedControlMutation,
            _: &ControlChangeSet,
        ) -> tellurion_core::Result<ControlCommit> {
            Err(raw_store_error())
        }

        async fn changes_since(
            &self,
            _: Option<ControlEventCursor>,
            _: u32,
        ) -> tellurion_core::Result<Vec<ControlEvent>> {
            Err(raw_store_error())
        }

        async fn audit_since(
            &self,
            _: ControlRevision,
            _: u32,
        ) -> tellurion_core::Result<Vec<ControlAuditRecord>> {
            Err(raw_store_error())
        }
    }

    fn raw_store_error() -> Error {
        Error::ControlValidation("sentinel-raw-store-error".to_string())
    }

    #[async_trait::async_trait]
    impl ControlStore for BlockingLoadStore {
        async fn bootstrap_if_empty(
            &self,
            seed: &ControlSnapshot,
            actor: &PrincipalIdentity,
            mode: ControlBootstrapMode,
        ) -> tellurion_core::Result<BootstrapOutcome> {
            self.inner.bootstrap_if_empty(seed, actor, mode).await
        }

        async fn current_revision(&self) -> tellurion_core::Result<Option<ControlRevision>> {
            self.inner.current_revision().await
        }

        async fn load_snapshot(&self) -> tellurion_core::Result<VersionedControlSnapshot> {
            self.entered.notify_one();
            self.release.notified().await;
            self.inner.load_snapshot().await
        }

        async fn transact(
            &self,
            authorization: &AuthorizedControlMutation,
            changes: &ControlChangeSet,
        ) -> tellurion_core::Result<ControlCommit> {
            self.inner.transact(authorization, changes).await
        }

        async fn changes_since(
            &self,
            after: Option<ControlEventCursor>,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlEvent>> {
            self.inner.changes_since(after, limit).await
        }

        async fn audit_since(
            &self,
            after: ControlRevision,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlAuditRecord>> {
            self.inner.audit_since(after, limit).await
        }
    }

    #[async_trait::async_trait]
    impl ControlStore for LoadCountingStore {
        async fn bootstrap_if_empty(
            &self,
            seed: &ControlSnapshot,
            actor: &PrincipalIdentity,
            mode: ControlBootstrapMode,
        ) -> tellurion_core::Result<BootstrapOutcome> {
            self.inner.bootstrap_if_empty(seed, actor, mode).await
        }

        async fn current_revision(&self) -> tellurion_core::Result<Option<ControlRevision>> {
            self.inner.current_revision().await
        }

        async fn load_snapshot(&self) -> tellurion_core::Result<VersionedControlSnapshot> {
            self.snapshot_reads.fetch_add(1, Ordering::SeqCst);
            self.inner.load_snapshot().await
        }

        async fn transact(
            &self,
            authorization: &AuthorizedControlMutation,
            changes: &ControlChangeSet,
        ) -> tellurion_core::Result<ControlCommit> {
            self.inner.transact(authorization, changes).await
        }

        async fn changes_since(
            &self,
            after: Option<ControlEventCursor>,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlEvent>> {
            self.inner.changes_since(after, limit).await
        }

        async fn audit_since(
            &self,
            after: ControlRevision,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlAuditRecord>> {
            self.inner.audit_since(after, limit).await
        }
    }

    #[async_trait::async_trait]
    impl ControlStore for RevisionConflictStore {
        async fn bootstrap_if_empty(
            &self,
            seed: &ControlSnapshot,
            actor: &PrincipalIdentity,
            mode: ControlBootstrapMode,
        ) -> tellurion_core::Result<BootstrapOutcome> {
            self.inner.bootstrap_if_empty(seed, actor, mode).await
        }

        async fn current_revision(&self) -> tellurion_core::Result<Option<ControlRevision>> {
            self.inner.current_revision().await
        }

        async fn load_snapshot(&self) -> tellurion_core::Result<VersionedControlSnapshot> {
            self.inner.load_snapshot().await
        }

        async fn transact(
            &self,
            _: &AuthorizedControlMutation,
            _: &ControlChangeSet,
        ) -> tellurion_core::Result<ControlCommit> {
            Err(tellurion_core::Error::ControlRevisionConflict {
                expected: 1,
                current: 2,
            })
        }

        async fn changes_since(
            &self,
            after: Option<ControlEventCursor>,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlEvent>> {
            self.inner.changes_since(after, limit).await
        }

        async fn audit_since(
            &self,
            after: ControlRevision,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlAuditRecord>> {
            self.inner.audit_since(after, limit).await
        }
    }

    async fn revision_conflict_context() -> Arc<AppContext> {
        let config = fixture_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(FixtureFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer: Arc<dyn TenantAuthorizer> = Arc::new(VerifiedAuthorizer {
            identity: principal(),
        });
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let inner = Arc::new(InMemoryControlStore::new());
        inner
            .bootstrap_if_empty(
                &snapshot(),
                &principal(),
                ControlBootstrapMode::AllowEmptyPlatform,
            )
            .await
            .unwrap();
        Arc::new(
            AppContext::new(
                config,
                router,
                resolver,
                Some(authorizer),
                cache,
                style_store,
            )
            .with_control_store(Arc::new(RevisionConflictStore { inner }) as Arc<dyn ControlStore>),
        )
    }

    fn replace_platform_settings() -> ControlChangeSet {
        ControlChangeSet {
            idempotency_key: None,
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(fixture_config()),
            }],
        }
    }

    fn malformed_request(bearer: Option<&str>, content_type: Option<&str>) -> Request<Body> {
        let mut request = Request::builder()
            .method("PUT")
            .uri("/_control/v1/platform/settings");
        if let Some(bearer) = bearer {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        request.body(Body::from("{")).unwrap()
    }

    struct PollRecordingBody {
        polled: Arc<AtomicBool>,
        emitted: bool,
    }

    impl HttpBody for PollRecordingBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            self.polled.store(true, Ordering::SeqCst);
            if self.emitted {
                Poll::Ready(None)
            } else {
                self.emitted = true;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"{")))))
            }
        }
    }

    fn poll_recording_request(bearer: Option<&str>) -> (Request<Body>, Arc<AtomicBool>) {
        let polled = Arc::new(AtomicBool::new(false));
        let mut request = Request::builder()
            .method("PUT")
            .uri("/_control/v1/platform/settings")
            .header("content-type", "application/json");
        if let Some(bearer) = bearer {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        let body = Body::new(PollRecordingBody {
            polled: Arc::clone(&polled),
            emitted: false,
        });
        (request.body(body).unwrap(), polled)
    }

    const READ_ROUTES: [&str; 3] = [
        "/_control/v1/platform/overview",
        "/_control/v1/platform/effective-settings",
        "/_control/v1/platform/audit",
    ];

    fn read_request(uri: &str, bearer: Option<&str>) -> Request<Body> {
        let mut request = Request::builder().method("GET").uri(uri);
        if let Some(bearer) = bearer {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        request.body(Body::empty()).unwrap()
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn commit_platform_settings(ctx: &Arc<AppContext>, idempotency_key: Option<&str>) {
        let current = ctx
            .control_store
            .as_ref()
            .unwrap()
            .load_snapshot()
            .await
            .unwrap();
        let changes = ControlChangeSet {
            idempotency_key: idempotency_key.map(str::to_string),
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(current.snapshot.config),
            }],
        };
        let response = super::router(ctx)
            .with_state(Arc::clone(ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn control_reads_are_absent_without_a_store_or_authorizer() {
        let (without_store, _) = fixture_context(false).await;
        let (without_authorizer, _) = fixture_context_with(true, None, snapshot()).await;

        for ctx in [without_store, without_authorizer] {
            for path in READ_ROUTES {
                let response = super::router(&ctx)
                    .with_state(Arc::clone(&ctx))
                    .oneshot(read_request(path, None))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
                assert!(to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .is_empty());
            }
        }
    }

    #[tokio::test]
    async fn control_reads_require_verified_platform_policy_authority() {
        let (sysadmin, _) = fixture_context(true).await;
        for bearer in [None, Some("unverifiable")] {
            for path in READ_ROUTES {
                let response = super::router(&sysadmin)
                    .with_state(Arc::clone(&sysadmin))
                    .oneshot(read_request(path, bearer))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
                assert_eq!(
                    response.headers()["content-type"],
                    "application/problem+json"
                );
            }
        }

        let mut denied_snapshot = snapshot();
        denied_snapshot.role_bindings.clear();
        let (unauthorized, _) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            denied_snapshot,
        )
        .await;
        for path in READ_ROUTES {
            let response = super::router(&unauthorized)
                .with_state(Arc::clone(&unauthorized))
                .oneshot(read_request(path, Some("verified")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        }

        for path in READ_ROUTES {
            let response = super::router(&sysadmin)
                .with_state(Arc::clone(&sysadmin))
                .oneshot(read_request(path, Some("verified")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn scoped_resource_reads_are_external_id_only_and_keyset_paginated() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fixture, url_env: DATABASE_URL } ]
tenants:
  - { id: tenant-internal, external_id: acme }
  - { id: tenant-bravo-internal, external_id: bravo }
catalogs:
  - { id: catalog-internal, external_id: cadastre, tenant: tenant-internal, visibility: { shared_with: [tenant-bravo-internal] } }
  - { id: catalog-surveys-internal, external_id: surveys, tenant: tenant-internal }
collections:
  - id: collection-internal
    external_id: roads
    catalog: catalog-internal
    storage: main
    visibility: { shared_with: [tenant-bravo-internal] }
  - id: collection-water-internal
    external_id: waterways
    catalog: catalog-internal
    storage: main
auth:
  trusted_issuers:
    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }
"#,
        )
        .unwrap();
        let seed = ControlSnapshot {
            config,
            role_bindings: snapshot().role_bindings,
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        };
        let (ctx, _) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            seed,
        )
        .await;

        for invalid_limit in [0, 101] {
            let response = super::router(&ctx)
                .with_state(Arc::clone(&ctx))
                .oneshot(read_request(
                    &format!("/_control/v1/tenants?limit={invalid_limit}"),
                    Some("verified"),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(response_json(response).await["code"], "InvalidLimit");
        }
        let unknown_query = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants?limti=1",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(unknown_query.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(unknown_query).await["code"],
            "InvalidControlQuery"
        );

        let first = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants?limit=1",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()["etag"], "\"control-revision-1\"");
        let first = response_json(first).await;
        assert_eq!(first["control_revision"], 1);
        assert_eq!(first["items"].as_array().unwrap().len(), 1);
        assert_eq!(first["items"][0]["entity_version"], "0");
        assert_eq!(first["items"][0]["resource"]["id"], "acme");
        assert_eq!(first["next_after"], "acme");
        assert!(!first.to_string().contains("tenant-internal"));

        let second = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants?limit=1&after=acme",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second = response_json(second).await;
        assert_eq!(second["items"][0]["resource"]["id"], "bravo");
        assert!(second["next_after"].is_null());
        assert!(!second.to_string().contains("tenant-bravo-internal"));

        let durable_tenant = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request("/_control/v1/tenants/bravo", Some("verified")))
            .await
            .unwrap();
        assert_eq!(durable_tenant.status(), StatusCode::OK);
        let durable_tenant = response_json(durable_tenant).await;
        assert_eq!(durable_tenant["resource"]["id"], "bravo");
        assert!(!durable_tenant.to_string().contains("tenant-bravo-internal"));

        let tenant = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request("/_control/v1/tenants/acme", Some("verified")))
            .await
            .unwrap();
        assert_eq!(tenant.status(), StatusCode::OK);
        assert_eq!(tenant.headers()["etag"], "\"control-entity-0\"");
        let tenant = response_json(tenant).await;
        assert_eq!(tenant["resource"]["id"], "acme");
        assert!(!tenant.to_string().contains("tenant-internal"));

        let catalogs = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs?limit=1",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(catalogs.status(), StatusCode::OK);
        let catalogs = response_json(catalogs).await;
        assert_eq!(catalogs["items"][0]["resource"]["id"], "cadastre");
        assert_eq!(catalogs["items"][0]["resource"]["tenant"], "acme");
        assert_eq!(catalogs["next_after"], "cadastre");
        assert!(!catalogs.to_string().contains("catalog-internal"));

        let next_catalogs = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs?limit=1&after=cadastre",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(next_catalogs.status(), StatusCode::OK);
        let next_catalogs = response_json(next_catalogs).await;
        assert_eq!(next_catalogs["items"][0]["resource"]["id"], "surveys");
        assert!(next_catalogs["next_after"].is_null());

        let catalog = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs/cadastre",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(catalog.status(), StatusCode::OK);
        assert_eq!(catalog.headers()["etag"], "\"control-entity-0\"");
        let catalog = response_json(catalog).await;
        assert_eq!(catalog["resource"]["id"], "cadastre");
        assert_eq!(catalog["resource"]["tenant"], "acme");
        assert_eq!(
            catalog["resource"]["visibility"]["shared_with"],
            serde_json::json!(["bravo"])
        );
        assert!(!catalog.to_string().contains("catalog-internal"));

        let collections = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs/cadastre/collections?limit=1",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(collections.status(), StatusCode::OK);
        let collections = response_json(collections).await;
        assert_eq!(collections["items"][0]["resource"]["id"], "roads");
        assert_eq!(collections["items"][0]["resource"]["catalog"], "cadastre");
        assert_eq!(collections["next_after"], "roads");
        assert!(!collections.to_string().contains("collection-internal"));

        let next_collections = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs/cadastre/collections?limit=1&after=roads",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(next_collections.status(), StatusCode::OK);
        let next_collections = response_json(next_collections).await;
        assert_eq!(next_collections["items"][0]["resource"]["id"], "waterways");
        assert!(next_collections["next_after"].is_null());

        let collection = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(collection.status(), StatusCode::OK);
        assert_eq!(collection.headers()["etag"], "\"control-entity-0\"");
        let collection = response_json(collection).await;
        assert_eq!(collection["resource"]["id"], "roads");
        assert_eq!(collection["resource"]["catalog"], "cadastre");
        assert_eq!(
            collection["resource"]["visibility"]["shared_with"],
            serde_json::json!(["bravo"])
        );
        assert!(!collection.to_string().contains("collection-internal"));
    }

    #[tokio::test]
    async fn explicit_tenant_policy_can_authorize_scoped_reads_without_a_sysadmin_role() {
        let mut seed = snapshot();
        seed.role_bindings = vec![RoleBinding {
            principal: principal(),
            role: "tenant-inspector".to_string(),
            scope: ControlScope::Tenant {
                tenant_id: "tenant-internal".to_string(),
            },
        }];
        seed.path_policies = vec![PathPolicy::new(
            "inspect-acme",
            "tenant-inspector",
            ControlScope::Tenant {
                tenant_id: "tenant-internal".to_string(),
            },
            PolicyEffect::Allow,
            ["GET"],
            ["/_control/v1/tenants/acme/**"],
        )];
        let (ctx, _) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            seed,
        )
        .await;

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs",
                Some("verified"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn denied_catalog_reads_are_indistinguishable_from_missing_resources() {
        let mut seed = snapshot();
        seed.role_bindings.clear();
        let (ctx, _) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            seed,
        )
        .await;

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/tenants/acme/catalogs/cadastre",
                Some("verified"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn read_policies_are_the_only_role_gate_for_platform_reads() {
        let viewer_snapshot = ControlSnapshot {
            role_bindings: vec![RoleBinding {
                principal: principal(),
                role: "viewer".to_string(),
                scope: ControlScope::Platform,
            }],
            ..snapshot()
        };
        let mut custom_snapshot = snapshot();
        custom_snapshot.role_bindings = vec![RoleBinding {
            principal: principal(),
            role: "custom-platform-reader".to_string(),
            scope: ControlScope::Platform,
        }];
        custom_snapshot.path_policies = vec![PathPolicy::new(
            "custom-platform-read",
            "custom-platform-reader",
            ControlScope::Platform,
            PolicyEffect::Allow,
            ["GET"],
            ["/_control/v1/platform/**"],
        )];
        let denied_snapshot = ControlSnapshot {
            role_bindings: vec![RoleBinding {
                principal: principal(),
                role: "unprivileged".to_string(),
                scope: ControlScope::Platform,
            }],
            ..snapshot()
        };

        for allowed_snapshot in [viewer_snapshot, custom_snapshot] {
            let (ctx, _) = fixture_context_with(
                true,
                Some(Arc::new(VerifiedAuthorizer {
                    identity: principal(),
                })),
                allowed_snapshot,
            )
            .await;
            for path in READ_ROUTES {
                let response = super::router(&ctx)
                    .with_state(Arc::clone(&ctx))
                    .oneshot(read_request(path, Some("verified")))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{path}");
            }
        }

        let (denied_ctx, _) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            denied_snapshot,
        )
        .await;
        for path in READ_ROUTES {
            let response = super::router(&denied_ctx)
                .with_state(Arc::clone(&denied_ctx))
                .oneshot(read_request(path, Some("verified")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json"
            );
        }
    }

    #[tokio::test]
    async fn control_read_store_failures_never_echo_raw_error_text() {
        let config = fixture_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(FixtureFactory));
        let core_router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer: Arc<dyn TenantAuthorizer> = Arc::new(VerifiedAuthorizer {
            identity: principal(),
        });
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(
            AppContext::new(
                config,
                core_router,
                resolver,
                Some(authorizer),
                cache,
                style_store,
            )
            .with_control_store(Arc::new(FailingLoadStore) as Arc<dyn ControlStore>),
        );

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(READ_ROUTES[0], Some("verified")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let headers = format!("{:?}", response.headers());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!headers.contains("sentinel-raw-store-error"));
        assert!(!body
            .windows(b"sentinel-raw-store-error".len())
            .any(|window| window == b"sentinel-raw-store-error"));
    }

    #[tokio::test]
    async fn overview_reports_local_runtime_state_with_the_durable_revision_etag() {
        let (ctx, _) = fixture_context(true).await;
        ctx.control_runtime_status.observe_store_revision(8);
        ctx.control_runtime_status.observe_applied_revision(5);
        ctx.control_runtime_status.record_poll_failure();
        ctx.control_runtime_status.record_activation_failure();
        ctx.control_runtime_status.record_refresh_success();

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(READ_ROUTES[0], Some("verified")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["etag"], "\"control-revision-1\"");
        let body = response_json(response).await;
        assert_eq!(body["scope"], "self");
        assert_eq!(body["store_revision"], 8);
        assert_eq!(body["applied_revision"], 5);
        assert_eq!(body["lag"], 3);
        assert!(body["last_successful_refresh_unix_ms"].as_u64().is_some());
        assert_eq!(body["poll_failures"], 1);
        assert_eq!(body["activation_failures"], 1);
        assert_eq!(
            body["config_version"],
            ctx.current().config_version.to_string()
        );
    }

    #[tokio::test]
    async fn effective_settings_wrap_the_legacy_platform_view_at_the_applied_revision() {
        let (ctx, _) = fixture_context(true).await;
        let legacy = crate::config_view::effective_config_view(
            axum::extract::State(Arc::clone(&ctx)),
            axum::extract::Path(HashMap::new()),
        )
        .await;
        assert_eq!(legacy.status(), StatusCode::OK);
        let legacy = response_json(legacy).await;

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(READ_ROUTES[1], Some("verified")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["etag"], "\"control-revision-0\"");
        let body = response_json(response).await;
        assert_eq!(body["applied_revision"], 0);
        assert_eq!(body["effective"], legacy);
    }

    #[tokio::test]
    async fn effective_settings_and_revision_come_from_one_active_generation() {
        let mut initial_config = fixture_config();
        initial_config.settings.cache_ttl_s = Some(11);
        let initial_snapshot = ControlSnapshot {
            config: initial_config.clone(),
            role_bindings: vec![RoleBinding {
                principal: principal(),
                role: "sysadmin".to_string(),
                scope: ControlScope::Platform,
            }],
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        };
        let mut registry = Registry::new();
        registry.register(Arc::new(FixtureFactory));
        let initial_router = CoreRouter::build(&initial_config, &registry).unwrap();
        let initial_resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&initial_config));
        let authorizer: Arc<dyn TenantAuthorizer> = Arc::new(VerifiedAuthorizer {
            identity: principal(),
        });
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let inner = Arc::new(InMemoryControlStore::new());
        inner
            .bootstrap_if_empty(
                &initial_snapshot,
                &principal(),
                ControlBootstrapMode::AllowEmptyPlatform,
            )
            .await
            .unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let ctx = Arc::new(
            AppContext::new(
                initial_config,
                initial_router,
                initial_resolver,
                Some(Arc::clone(&authorizer)),
                cache,
                style_store,
            )
            .with_control_store(Arc::new(BlockingLoadStore {
                inner,
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }) as Arc<dyn ControlStore>),
        );

        let request_ctx = Arc::clone(&ctx);
        let request = tokio::spawn(async move {
            super::router(&request_ctx)
                .with_state(Arc::clone(&request_ctx))
                .oneshot(read_request(READ_ROUTES[1], Some("verified")))
                .await
                .unwrap()
        });
        entered.notified().await;

        let mut replacement = fixture_config();
        replacement.settings.cache_ttl_s = Some(22);
        let replacement_router = CoreRouter::build(&replacement, &registry).unwrap();
        let replacement_resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&replacement));
        ctx.reload(
            replacement,
            replacement_router,
            replacement_resolver,
            Some(authorizer),
        );
        ctx.control_runtime_status.observe_applied_revision(2);
        release.notify_one();

        let response = request.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["etag"], "\"control-revision-0\"");
        let body = response_json(response).await;
        assert_eq!(body["applied_revision"], 0);
        assert_eq!(body["effective"]["settings"]["cache_ttl_s"]["value"], 11);
    }

    #[tokio::test]
    async fn noncanonical_and_unregistered_read_paths_do_not_reach_store_reads() {
        let config = fixture_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(FixtureFactory));
        let core_router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer: Arc<dyn TenantAuthorizer> = Arc::new(VerifiedAuthorizer {
            identity: principal(),
        });
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let inner = Arc::new(InMemoryControlStore::new());
        inner
            .bootstrap_if_empty(
                &snapshot(),
                &principal(),
                ControlBootstrapMode::AllowEmptyPlatform,
            )
            .await
            .unwrap();
        let snapshot_reads = Arc::new(AtomicUsize::new(0));
        let ctx = Arc::new(
            AppContext::new(
                config,
                core_router,
                resolver,
                Some(authorizer),
                cache,
                style_store,
            )
            .with_control_store(Arc::new(LoadCountingStore {
                inner,
                snapshot_reads: Arc::clone(&snapshot_reads),
            }) as Arc<dyn ControlStore>),
        );

        for path in [
            "/_control/v1/platform/%6fverview",
            "/_control/v1/platform/overview/unregistered",
        ] {
            let response = super::router(&ctx)
                .with_state(Arc::clone(&ctx))
                .oneshot(read_request(path, Some("verified")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty());
        }
        assert_eq!(snapshot_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn audit_read_is_bounded_ordered_cursor_paginated_and_revision_tagged() {
        let (ctx, store) = fixture_context(true).await;
        for index in 0..51 {
            let key = (index == 0).then_some("sentinel-idempotency-key");
            commit_platform_settings(&ctx, key).await;
        }
        assert_eq!(store.current_revision().await.unwrap(), Some(52));

        let default_page = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(READ_ROUTES[2], Some("verified")))
            .await
            .unwrap();
        assert_eq!(default_page.status(), StatusCode::OK);
        assert_eq!(default_page.headers()["etag"], "\"control-revision-52\"");
        let default_page = response_json(default_page).await;
        assert_eq!(default_page["revision"], 52);
        assert_eq!(default_page["items"].as_array().unwrap().len(), 50);
        assert_eq!(default_page["next_after"], 50);
        let revisions = default_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["revision"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert!(revisions.windows(2).all(|pair| pair[0] < pair[1]));
        let first = &default_page["items"][0];
        assert_eq!(first["actor"]["issuer"], principal().issuer);
        assert_eq!(first["actor"]["subject"], principal().subject);
        assert_eq!(first["method"], "BOOTSTRAP");
        assert_eq!(first["canonical_path"], "/_control/v1/platform");
        assert_eq!(first["correlation_id"], "bootstrap");
        assert!(first["changed_resources"].is_array());
        assert!(first["recorded_at_unix_ms"].as_u64().is_some());
        assert_eq!(first["applying_instance"], "in-memory-control-store");
        assert!(first.get("idempotency_key").is_none());

        let continuation = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/platform/audit?after=50&limit=2",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(continuation.status(), StatusCode::OK);
        let continuation = response_json(continuation).await;
        assert_eq!(continuation["items"][0]["revision"], 51);
        assert_eq!(continuation["items"][1]["revision"], 52);
        assert!(continuation["next_after"].is_null());

        let empty = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(read_request(
                "/_control/v1/platform/audit?after=52",
                Some("verified"),
            ))
            .await
            .unwrap();
        assert_eq!(empty.status(), StatusCode::OK);
        let empty = response_json(empty).await;
        assert_eq!(empty["items"], serde_json::json!([]));
        assert!(empty["next_after"].is_null());

        for limit in [0, 101] {
            let response = super::router(&ctx)
                .with_state(Arc::clone(&ctx))
                .oneshot(read_request(
                    &format!("/_control/v1/platform/audit?limit={limit}"),
                    Some("verified"),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json"
            );
            let problem = response_json(response).await;
            assert_eq!(problem["code"], "InvalidLimit");
        }
    }

    #[tokio::test]
    async fn control_read_bytes_exclude_credentials_environment_dsns_and_idempotency_keys() {
        let mut seed = snapshot();
        seed.config.storages[0].url_env = "postgres://sentinel-dsn".to_string();
        seed.config.auth.bearer_tokens.push(
            serde_yaml::from_str("token_env: SENTINEL_ENV_VALUE\ntenants: [tenant-internal]\n")
                .unwrap(),
        );
        let (ctx, _) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            seed,
        )
        .await;
        commit_platform_settings(&ctx, Some("sentinel-idempotency-key")).await;

        let mut bytes = Vec::new();
        for (path, bearer) in [
            (READ_ROUTES[0], "sentinel-credential"),
            (READ_ROUTES[1], "postgres://sentinel-dsn"),
            (READ_ROUTES[2], "sentinel-credential"),
        ] {
            let response = super::router(&ctx)
                .with_state(Arc::clone(&ctx))
                .oneshot(read_request(path, Some(bearer)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            bytes.extend_from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap());
        }
        let bytes = String::from_utf8(bytes).unwrap();
        for secret in [
            "sentinel-credential",
            "SENTINEL_ENV_VALUE",
            "postgres://sentinel-dsn",
            "sentinel-idempotency-key",
        ] {
            assert!(!bytes.contains(secret), "response leaked {secret}");
        }
    }

    #[tokio::test]
    async fn verified_sysadmin_commits_through_the_durable_store() {
        let (ctx, store) = fixture_context(true).await;
        let request = Request::builder()
            .method("PUT")
            .uri("/_control/v1/platform/settings")
            .header("content-type", "application/json")
            .header("authorization", "Bearer verified")
            .header("x-request-id", "control-http-1")
            .body(Body::from(
                serde_json::to_vec(&replace_platform_settings()).unwrap(),
            ))
            .unwrap();

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let commit: tellurion_core::ControlCommit = serde_json::from_slice(&body).unwrap();
        assert_eq!(commit.revision, 2);
        assert!(!commit.replayed);
        assert_eq!(store.current_revision().await.unwrap(), Some(2));
        let audit = store.audit_since(0, 10).await.unwrap();
        assert_eq!(
            audit.last().unwrap().request.correlation_id,
            "control-http-1"
        );
    }

    #[tokio::test]
    async fn durable_routes_are_absent_without_a_store() {
        let (ctx, _) = fixture_context(false).await;
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn missing_authorizer_returns_without_polling_the_request_body() {
        let (ctx, _) = fixture_context_with(true, None, snapshot()).await;
        let (request, polled) = poll_recording_request(None);
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
        assert!(!polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn invalid_credential_returns_without_polling_the_request_body() {
        let (ctx, _) = fixture_context(true).await;
        let (request, polled) = poll_recording_request(Some("unverifiable"));
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "Unauthorized"
        );
        assert!(!polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn malformed_json_does_not_reveal_a_surface_without_an_authorizer() {
        let (ctx, _) = fixture_context_with(true, None, snapshot()).await;
        for content_type in [Some("application/json"), None, Some("text/plain")] {
            let response = super::router(&ctx)
                .with_state(Arc::clone(&ctx))
                .oneshot(malformed_request(None, content_type))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert!(to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty());
        }
    }

    #[tokio::test]
    async fn malformed_json_does_not_distinguish_missing_or_invalid_bearers() {
        let (ctx, _) = fixture_context(true).await;
        for bearer in [None, Some("unverifiable")] {
            for content_type in [Some("application/json"), None, Some("text/plain")] {
                let response = super::router(&ctx)
                    .with_state(Arc::clone(&ctx))
                    .oneshot(malformed_request(bearer, content_type))
                    .await
                    .unwrap();

                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                assert_eq!(
                    response.headers()["content-type"],
                    "application/problem+json"
                );
                let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
                    "Unauthorized"
                );
            }
        }
    }

    #[tokio::test]
    async fn verified_malformed_json_is_a_safe_problem() {
        let (ctx, store) = fixture_context(true).await;
        let revision = store.current_revision().await.unwrap();
        for content_type in [Some("application/json"), None, Some("text/plain")] {
            let response = super::router(&ctx)
                .with_state(Arc::clone(&ctx))
                .oneshot(malformed_request(Some("verified"), content_type))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json"
            );
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(problem["code"], "InvalidControlMutation");
            assert_eq!(
                problem["detail"],
                "the durable control mutation request is invalid"
            );
        }
        assert_eq!(store.current_revision().await.unwrap(), revision);
    }

    #[tokio::test]
    async fn missing_authorizer_hides_the_durable_surface() {
        let (ctx, _) = fixture_context_with(true, None, snapshot()).await;
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&replace_platform_settings()).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unverified_bearer_is_unauthorized() {
        let (ctx, _) = fixture_context(true).await;
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("authorization", "Bearer unverifiable")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&replace_platform_settings()).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "Unauthorized"
        );
    }

    #[tokio::test]
    async fn catalog_denial_is_a_bare_not_found() {
        let mut denied = snapshot();
        denied.role_bindings.clear();
        let (ctx, store) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            denied,
        )
        .await;
        let revision = store.current_revision().await.unwrap();
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/tenants/acme/catalogs/cadastre")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ControlChangeSet {
                            idempotency_key: None,
                            operations: vec![VersionedControlOperation {
                                expected_entity_version: None,
                                operation: ControlOperation::PutCatalog(
                                    fixture_config().catalogs[0].clone(),
                                ),
                            }],
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(store.current_revision().await.unwrap(), revision);
    }

    #[tokio::test]
    async fn route_operation_mismatch_is_rejected_before_preview_or_commit() {
        let (ctx, store) = fixture_context(true).await;
        let revision = store.current_revision().await.unwrap();
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings?dry_run=true")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ControlChangeSet {
                            idempotency_key: None,
                            operations: vec![VersionedControlOperation {
                                expected_entity_version: None,
                                operation: ControlOperation::PutTenant(
                                    fixture_config().tenants[0].clone(),
                                ),
                            }],
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "InvalidControlMutation"
        );
        assert_eq!(store.current_revision().await.unwrap(), revision);
    }

    #[tokio::test]
    async fn encoded_separator_never_reaches_the_store() {
        let (ctx, store) = fixture_context(true).await;
        let revision = store.current_revision().await.unwrap();
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/tenants/acme/catalogs/cadastre%2Fhidden")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ControlChangeSet {
                            idempotency_key: None,
                            operations: vec![VersionedControlOperation {
                                expected_entity_version: None,
                                operation: ControlOperation::PutCatalog(
                                    fixture_config().catalogs[0].clone(),
                                ),
                            }],
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(store.current_revision().await.unwrap(), revision);
    }

    #[tokio::test]
    async fn platform_import_is_the_only_batch_capable_route() {
        let (ctx, store) = fixture_context(true).await;
        let batch = ControlChangeSet {
            idempotency_key: None,
            operations: vec![
                VersionedControlOperation {
                    expected_entity_version: None,
                    operation: ControlOperation::ReplacePlatformSettings(fixture_config()),
                },
                VersionedControlOperation {
                    expected_entity_version: None,
                    operation: ControlOperation::ReplacePlatformSettings(fixture_config()),
                },
            ],
        };
        let import = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_control/v1/platform/import")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&batch).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(import.status(), StatusCode::OK);

        let revision = store.current_revision().await.unwrap();
        let settings = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&batch).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(settings.status(), StatusCode::BAD_REQUEST);
        assert_eq!(store.current_revision().await.unwrap(), revision);
    }

    #[tokio::test]
    async fn dry_run_previews_the_change_without_mutating_durable_state() {
        let (ctx, store) = fixture_context(true).await;
        let before_snapshot = store.load_snapshot().await.unwrap();
        let before_audit = store.audit_since(0, 100).await.unwrap();
        let before_events = store.changes_since(None, 100).await.unwrap();
        let mut candidate = before_snapshot.snapshot.config.clone();
        candidate.settings.cache_ttl_s = Some(60);
        let changes = ControlChangeSet {
            idempotency_key: Some("preview-only".to_string()),
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(candidate),
            }],
        };

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings?dry_run=true")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["etag"], "\"control-revision-1\"");
        let body = response_json(response).await;
        assert_eq!(body["base_revision"], 1);
        assert_eq!(body["prospective_revision"], 2);
        assert_eq!(body["changed_resources"], serde_json::json!(["platform"]));
        assert_eq!(body["entity_versions"]["platform"], "2");
        assert_eq!(store.load_snapshot().await.unwrap(), before_snapshot);
        assert_eq!(store.audit_since(0, 100).await.unwrap(), before_audit);
        assert_eq!(store.changes_since(None, 100).await.unwrap(), before_events);

        let commit = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(commit.status(), StatusCode::OK);
        let commit: tellurion_core::ControlCommit =
            serde_json::from_value(response_json(commit).await).unwrap();
        assert_eq!(commit.revision, 2);
        assert!(!commit.replayed);
    }

    #[tokio::test]
    async fn dry_run_accepts_only_explicit_boolean_values() {
        let (ctx, store) = fixture_context(true).await;
        let revision = store.current_revision().await.unwrap();

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings?dry_run=yes")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&replace_platform_settings()).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["code"], "InvalidControlMutation");
        assert_eq!(store.current_revision().await.unwrap(), revision);
    }

    #[tokio::test]
    async fn dry_run_false_retains_the_normal_commit_contract() {
        let (ctx, store) = fixture_context(true).await;

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings?dry_run=false")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&replace_platform_settings()).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("etag").is_none());
        let commit: tellurion_core::ControlCommit =
            serde_json::from_value(response_json(response).await).unwrap();
        assert_eq!(commit.revision, 2);
        assert!(!commit.replayed);
        assert_eq!(store.current_revision().await.unwrap(), Some(2));
    }

    #[tokio::test]
    async fn dry_run_reports_entity_version_conflicts_without_writing() {
        let (ctx, store) = fixture_context(true).await;
        let before = store.load_snapshot().await.unwrap();
        let mut changes = replace_platform_settings();
        changes.operations[0].expected_entity_version = Some("stale".to_string());

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings?dry_run=true")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        let body = response_json(response).await;
        assert_eq!(body["code"], "ControlEntityVersionConflict");
        assert_eq!(store.load_snapshot().await.unwrap(), before);
    }

    #[tokio::test]
    async fn dry_run_resolves_scopes_from_the_authoritative_durable_snapshot() {
        let mut seed = snapshot();
        seed.config.tenants.push(tellurion_core::TenantDecl {
            id: "tenant-bravo-internal".to_string(),
            external_id: Some("bravo".to_string()),
            settings: Default::default(),
        });
        let mut catalog = seed.config.catalogs[0].clone();
        catalog.id = "catalog-new-internal".to_string();
        catalog.external_id = Some("new-catalog".to_string());
        catalog.tenant = "tenant-bravo-internal".to_string();
        let (ctx, store) = fixture_context_with(
            true,
            Some(Arc::new(VerifiedAuthorizer {
                identity: principal(),
            })),
            seed,
        )
        .await;
        let before = store.load_snapshot().await.unwrap();
        let changes = ControlChangeSet {
            idempotency_key: None,
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutCatalog(catalog),
            }],
        };

        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_control/v1/tenants/bravo/catalogs?dry_run=true")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(store.load_snapshot().await.unwrap(), before);
    }

    #[tokio::test]
    async fn dry_run_responses_hide_internal_ids_and_principal_identity() {
        let (ctx, store) = fixture_context(true).await;
        let catalog_changes = ControlChangeSet {
            idempotency_key: None,
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutCatalog(fixture_config().catalogs[0].clone()),
            }],
        };
        let catalog_response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/tenants/acme/catalogs/cadastre?dry_run=true")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&catalog_changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(catalog_response.status(), StatusCode::OK);
        let catalog_body = response_json(catalog_response).await.to_string();
        assert!(catalog_body.contains("tenant/acme/catalog/cadastre"));
        assert!(!catalog_body.contains("tenant-internal"));
        assert!(!catalog_body.contains("catalog-internal"));

        let binding_changes = ControlChangeSet {
            idempotency_key: None,
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::PutRoleBinding(RoleBinding {
                    principal: PrincipalIdentity {
                        issuer: "https://identity.example".to_string(),
                        subject: "sensitive-principal".to_string(),
                    },
                    role: "viewer".to_string(),
                    scope: ControlScope::Platform,
                }),
            }],
        };
        let binding_response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_control/v1/platform/role-bindings?dry_run=true")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&binding_changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(binding_response.status(), StatusCode::OK);
        let binding_body = response_json(binding_response).await.to_string();
        assert!(binding_body.contains("role-binding/"));
        assert!(!binding_body.contains("identity.example"));
        assert!(!binding_body.contains("sensitive-principal"));
        assert_eq!(store.current_revision().await.unwrap(), Some(1));
    }

    #[tokio::test]
    async fn same_idempotent_request_replays_the_original_commit() {
        let (ctx, store) = fixture_context(true).await;
        let mut changes = replace_platform_settings();
        changes.idempotency_key = Some("repeat-control-http".to_string());
        let request = || {
            Request::builder()
                .method("PUT")
                .uri("/_control/v1/platform/settings")
                .header("authorization", "Bearer verified")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&changes).unwrap()))
                .unwrap()
        };
        let first = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first: tellurion_core::ControlCommit =
            serde_json::from_slice(&to_bytes(first.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(first.revision, 2);
        assert!(!first.replayed);

        let replay = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay: tellurion_core::ControlCommit =
            serde_json::from_slice(&to_bytes(replay.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(replay.revision, 2);
        assert!(replay.replayed);
        assert_eq!(store.current_revision().await.unwrap(), Some(2));
    }

    #[tokio::test]
    async fn changed_body_reusing_an_idempotency_key_is_a_named_conflict() {
        let (ctx, store) = fixture_context(true).await;
        let mut first = replace_platform_settings();
        first.idempotency_key = Some("conflicting-control-http".to_string());
        let first_response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&first).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first_response.status(), StatusCode::OK);

        let mut changed_config = fixture_config();
        changed_config.settings.cache_ttl_s = Some(60);
        let changed = ControlChangeSet {
            idempotency_key: first.idempotency_key.clone(),
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(changed_config),
            }],
        };
        let revision = store.current_revision().await.unwrap();
        let conflict = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&changed).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let body = to_bytes(conflict.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "ControlIdempotencyConflict"
        );
        assert_eq!(store.current_revision().await.unwrap(), revision);
    }

    #[tokio::test]
    async fn durable_store_revision_conflict_is_a_named_problem() {
        let ctx = revision_conflict_context().await;
        let response = super::router(&ctx)
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header("authorization", "Bearer verified")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&replace_platform_settings()).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "ControlRevisionConflict"
        );
    }

    #[tokio::test]
    async fn every_cookie_mutation_rejects_origin_or_csrf_before_body_and_store_access() {
        let config = fixture_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(FixtureFactory));
        let core_router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer: Arc<dyn TenantAuthorizer> = Arc::new(VerifiedAuthorizer {
            identity: principal(),
        });
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let inner = Arc::new(InMemoryControlStore::new());
        inner
            .bootstrap_if_empty(
                &snapshot(),
                &principal(),
                ControlBootstrapMode::AllowEmptyPlatform,
            )
            .await
            .unwrap();
        let snapshot_reads = Arc::new(AtomicUsize::new(0));
        let ctx = Arc::new(
            AppContext::new(
                config,
                core_router,
                resolver,
                Some(authorizer),
                cache,
                style_store,
            )
            .with_control_store(Arc::new(LoadCountingStore {
                inner,
                snapshot_reads: Arc::clone(&snapshot_reads),
            }) as Arc<dyn ControlStore>),
        );
        let (browser, cookie, csrf) = browser_session(&ctx, "verified", principal()).await;
        let app = super::router_with_browser(&ctx, Some(browser)).with_state(Arc::clone(&ctx));
        let mutations = [
            ("PUT", "/_control/v1/platform/settings"),
            ("PATCH", "/_control/v1/platform/settings"),
            ("POST", "/_control/v1/platform/import"),
            ("POST", "/_control/v1/tenants"),
            ("PUT", "/_control/v1/tenants/acme"),
            ("DELETE", "/_control/v1/tenants/acme"),
            ("DELETE", "/_control/v1/tenants/acme/permanent-delete"),
            ("PUT", "/_control/v1/tenants/acme/settings"),
            ("PATCH", "/_control/v1/tenants/acme/settings"),
            ("POST", "/_control/v1/tenants/acme/catalogs"),
            ("POST", "/_control/v1/tenants/acme/collection-moves"),
            ("PUT", "/_control/v1/tenants/acme/catalogs/cadastre"),
            ("DELETE", "/_control/v1/tenants/acme/catalogs/cadastre"),
            (
                "DELETE",
                "/_control/v1/tenants/acme/catalogs/cadastre/permanent-delete",
            ),
            (
                "PUT",
                "/_control/v1/tenants/acme/catalogs/cadastre/settings",
            ),
            (
                "PATCH",
                "/_control/v1/tenants/acme/catalogs/cadastre/settings",
            ),
            (
                "POST",
                "/_control/v1/tenants/acme/catalogs/cadastre/collections",
            ),
            (
                "PUT",
                "/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
            ),
            (
                "DELETE",
                "/_control/v1/tenants/acme/catalogs/cadastre/collections/roads",
            ),
            (
                "DELETE",
                "/_control/v1/tenants/acme/catalogs/cadastre/collections/roads/permanent-delete",
            ),
            ("PUT", "/_control/v1/platform/policies/policy-1"),
            ("DELETE", "/_control/v1/platform/policies/policy-1"),
            (
                "PUT",
                "/_control/v1/tenants/acme/catalogs/cadastre/collections/roads/policies/policy-1",
            ),
            (
                "DELETE",
                "/_control/v1/tenants/acme/catalogs/cadastre/collections/roads/policies/policy-1",
            ),
            ("POST", "/_control/v1/platform/role-bindings"),
            ("DELETE", "/_control/v1/platform/role-bindings/binding-1"),
        ];
        let invalid_headers = [
            (None, Some(csrf.as_str())),
            (Some("https://foreign.example"), Some(csrf.as_str())),
            (Some("https://console.example.com/"), Some(csrf.as_str())),
            (Some("https://console.example.com"), None),
            (Some("https://console.example.com"), Some("wrong-csrf")),
        ];

        for (method, uri) in mutations {
            for (origin, presented_csrf) in invalid_headers {
                let polled = Arc::new(AtomicBool::new(false));
                let body = Body::new(PollRecordingBody {
                    polled: Arc::clone(&polled),
                    emitted: false,
                });
                let mut request = Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .header("cookie", &cookie);
                if let Some(origin) = origin {
                    request = request.header("origin", origin);
                }
                if let Some(presented_csrf) = presented_csrf {
                    request = request.header("x-tellurion-csrf", presented_csrf);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(body).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
                assert!(!polled.load(Ordering::SeqCst), "{method} {uri}");
                assert_eq!(snapshot_reads.load(Ordering::SeqCst), 0, "{method} {uri}");
            }
        }
    }

    #[tokio::test]
    async fn cookie_reads_reauthorize_and_observe_role_removal_on_the_next_request() {
        let (ctx, store) = fixture_context(true).await;
        let (browser, cookie, csrf) = browser_session(&ctx, "verified", principal()).await;
        let app = super::router_with_browser(&ctx, Some(browser)).with_state(Arc::clone(&ctx));
        let read = || {
            Request::builder()
                .uri("/_control/v1/platform/overview")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            app.clone().oneshot(read()).await.unwrap().status(),
            StatusCode::OK
        );

        let binding = snapshot().role_bindings[0].clone();
        let binding_id = tellurion_core::role_binding_target_id(&binding);
        let changes = ControlChangeSet {
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
        let mutation = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/_control/v1/platform/role-bindings/{binding_id}"))
                    .header("cookie", &cookie)
                    .header("origin", "https://console.example.com")
                    .header("x-tellurion-csrf", &csrf)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&changes).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mutation.status(), StatusCode::OK);
        assert!(store
            .load_snapshot()
            .await
            .unwrap()
            .snapshot
            .role_bindings
            .is_empty());

        assert_eq!(
            app.oneshot(read()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn conflicting_bearer_and_cookie_identities_are_rejected() {
        let authorizer: Arc<dyn TenantAuthorizer> = Arc::new(DistinctAuthorizer);
        let (ctx, _) = fixture_context_with(true, Some(authorizer), snapshot()).await;
        let (browser, cookie, _) = browser_session(&ctx, "verified", principal()).await;
        let response = super::router_with_browser(&ctx, Some(browser))
            .with_state(Arc::clone(&ctx))
            .oneshot(
                Request::builder()
                    .uri("/_control/v1/platform/overview")
                    .header("authorization", "Bearer other-identity")
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["code"], "Unauthorized");
        assert!(!body.to_string().contains("operator-1"));
        assert!(!body.to_string().contains("operator-2"));
    }
}
