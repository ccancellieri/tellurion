//! Handlers for the read-only OGC API — Records surface (`#192`).
//!
//! Every one of them resolves storage through `AppContext::current().router`
//! — this crate names no concrete driver, the same DB-free rule every other
//! protocol crate in this workspace follows. A record is read through the
//! collection's ordinary features lane (`Router::resolve_features`): that is
//! the whole point of the issue's framing. A record collection is not a
//! second catalog system with its own storage story; it is an ordinary
//! collection whose `kind` says its rows are records rather than features,
//! and it reuses the same drivers, the same keyset paging, the same
//! authorization, and the same canonical descriptor.
//!
//! Every request runs under a `/{tenant}/records/catalogs/{catalog}` mount;
//! `tenant`/`catalog` path parameters carry EXTERNAL ids exactly as the
//! client typed them and are resolved to internal ones through
//! `AppContext::current().resolver`, exactly as in `tellurion_features::
//! handlers`. Response bodies echo external ids only.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use tellurion_core::policy::{self, PolicyDecision, ResourceContext};
use tellurion_core::query_params::{parse_bounded_limit, parse_datetime, percent_encode};
use tellurion_core::{
    AppContext, CanonicalDescriptor, Credential, Error as CoreError, Filter, ItemsQuery,
    PageRequest, PolicyLane, RateCharge, RateCounter, RateVerdict, CRS84_URI,
};

use crate::conformance::{
    GEOJSON_MEDIA_TYPE, ITEM_TYPE_RECORD, JSON_MEDIA_TYPE, REL_COLLECTION, REL_ITEMS,
};
use crate::model::{Catalog, CatalogsResponse, Extent, Link, RecordsResponse, SpatialExtent};
use crate::problem::ApiError;

/// Mount-less fallbacks, so this crate's own tests can exercise a handler
/// without standing up the server's `/{tenant}/records/catalogs/{catalog}`
/// nesting — identical convention to `tellurion_features::handlers`.
pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";

/// Page sizes for `GET /collections` — same values and rationale as
/// `tellurion_features::params::COLLECTIONS_DEFAULT_LIMIT`/`COLLECTIONS_MAX_LIMIT`,
/// kept as this crate's own copy per the "each protocol crate owns its own
/// paging params" convention every sibling already follows.
const CATALOGS_DEFAULT_LIMIT: u32 = 100;
const CATALOGS_MAX_LIMIT: u32 = 10_000;

/// Page sizes for `GET /collections/{cid}/items`. OGC API — Records — Part 1:
/// Core Requirement 25 (`/req/record-core-query-parameters/limit`) defers
/// `limit` entirely to OGC API — Features — Part 1: Core, so these mirror
/// `tellurion_features::params::DEFAULT_LIMIT`/`MAX_LIMIT` exactly rather
/// than inventing a records-specific page size.
const RECORDS_DEFAULT_LIMIT: u32 = 10;
const RECORDS_MAX_LIMIT: u32 = 10_000;

#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct CatalogsQueryParams {
    pub limit: Option<u32>,
    pub token: Option<String>,
}

/// `GET /collections/{cid}/items`' query parameters.
///
/// Deliberately narrow: `limit`, `token` and `datetime` only. Every other
/// parameter Table 12 of the Standard lists (`bbox`, `q`, `type`, `ids`,
/// `externalIds`) is absent rather than accepted-and-ignored, which is why
/// this crate declares no Record Core Query Parameters conformance class —
/// see `crate::conformance`. `bbox` in particular is withheld on purpose: a
/// bounding-box predicate against a collection whose rows have no geometry
/// would filter everything out, and a client cannot tell that apart from an
/// empty catalog.
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct RecordsQueryParams {
    pub limit: Option<u32>,
    pub token: Option<String>,
    pub datetime: Option<String>,
}

fn tenant_of(params: &HashMap<String, String>) -> String {
    params
        .get("tenant")
        .cloned()
        .unwrap_or_else(|| DEFAULT_TENANT.to_string())
}

fn catalog_of(params: &HashMap<String, String>) -> String {
    params
        .get("catalog")
        .cloned()
        .unwrap_or_else(|| DEFAULT_CATALOG.to_string())
}

fn require_param(params: &HashMap<String, String>, name: &str) -> Result<String, ApiError> {
    params
        .get(name)
        .cloned()
        .ok_or(CoreError::NotFound)
        .map_err(ApiError::from)
}

async fn resolve_tenant_catalog(
    ctx: &AppContext,
    params: &HashMap<String, String>,
) -> Result<(String, String), ApiError> {
    let state = ctx.current();
    let tenant_id = state.resolver.resolve_tenant(&tenant_of(params)).await?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_of(params))
        .await?;
    Ok((tenant_id, catalog_id))
}

/// Mirrors `tellurion-server::app`'s own `extract_credential` (duplicated
/// per protocol crate, not shared — `tellurion-core` stays framework-free;
/// see `auth.rs`'s module doc).
fn extract_credential(headers: &HeaderMap) -> Credential {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Credential::None;
    };
    let Ok(value) = value.to_str() else {
        return Credential::None;
    };
    match value.strip_prefix("Bearer ") {
        Some(token) if !token.is_empty() => Credential::Bearer(token.to_string()),
        _ => Credential::None,
    }
}

/// The `#34` policy checkpoint, evaluated against
/// [`PolicyLane::Features`].
///
/// Reusing the Features lane rather than adding a `PolicyLane::Records`
/// variant is deliberate. `PolicyLane` is a closed vocabulary an operator
/// writes into `auth.grants[].lanes`; a new variant would silently narrow
/// every existing grant (a subject allowed to read a collection's rows would
/// suddenly be denied the same rows under a different URL) and would offer
/// nothing new to authorize — the records lane reads the *same rows of the
/// same table through the same `FeatureSource`* the features lane reads. A
/// grant that lets a subject read a collection's features is exactly the
/// grant that lets them read its records; anything else would be a
/// distinction without a resource behind it.
#[allow(clippy::too_many_arguments)]
async fn authorize_lane(
    state: &tellurion_core::ContextState,
    rate_counter: &dyn RateCounter,
    headers: &HeaderMap,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    lane_supports_filter: bool,
    charge: RateCharge,
) -> Result<Option<Filter>, ApiError> {
    let Some(authorizer) = state.authorizer.as_ref() else {
        return Ok(None);
    };
    let credential = extract_credential(headers);
    let subject = authorizer.subject(&credential).await;
    let visibility = state
        .router
        .effective_visibility(collection_id)
        .cloned()
        .unwrap_or_default();
    let resource = ResourceContext {
        tenant_id,
        catalog_id,
        collection_id,
        lane: PolicyLane::Features,
        visibility: &visibility,
    };
    let filter =
        match policy::authorize_resource(&state.config, &resource, &subject, lane_supports_filter)?
        {
            PolicyDecision::Allow { filter } => filter,
            PolicyDecision::Deny => return Err(crate::problem::policy_denied(&credential)),
        };
    if let RateVerdict::Refused(refusal) = policy::enforce_rate_limits(
        &state.config,
        &resource,
        &subject,
        Some(rate_counter),
        charge,
    )
    .await
    {
        return Err(crate::problem::policy_rate_limited(&refusal));
    }
    Ok(filter)
}

fn set_content_type(response: &mut Response, media_type: &'static str) {
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
}

/// Refuses, by name, a collection this root does not serve (`#192`).
///
/// The Records root serves record collections and nothing else — the mirror
/// image of the Features root's own listing filter, and what makes
/// Requirement 37 (`/req/records-api/catalogs-response`, "only collections
/// where the `itemType` property is a string with the value `record` SHALL
/// be considered to be catalogs") true of this root's `/collections` rather
/// than merely asserted by it. A vector or raster collection reached here is
/// `CapabilityUnsupported`, which `Problem::from_core_error` renders as a
/// `404` naming the capability — never a silent empty page, which an
/// operator could not tell apart from a correctly configured but empty
/// catalog.
///
/// A collection id this `Router` never indexed answers the same way, for the
/// same reason the resolver's own miss does: it is not a record collection
/// here.
fn require_record_collection(
    router: &tellurion_core::Router,
    collection_internal_id: &str,
    collection_external_id: &str,
) -> Result<(), ApiError> {
    let is_record = router
        .collection_kind(collection_internal_id)
        .is_some_and(|kind| kind.is_record());
    if is_record {
        return Ok(());
    }
    Err(ApiError::from(CoreError::CapabilityUnsupported {
        collection: collection_external_id.to_string(),
        capability: "records".to_string(),
    }))
}

/// `canonical`'s spatial extent, carried through verbatim. A record
/// collection commonly has none — that is `None` here and an omitted
/// `extent` member on the wire, never a fabricated whole-Earth bbox.
fn extent_from_canonical(canonical: Option<&CanonicalDescriptor>) -> Option<Extent> {
    canonical.and_then(|c| c.extent).map(|extent| Extent {
        spatial: SpatialExtent {
            bbox: vec![extent.bbox],
            crs: CRS84_URI.to_string(),
        },
    })
}

/// The one merge every projection reads (`#50`). Tolerates an unresolvable
/// triple by logging and returning `None`, exactly as
/// `tellurion_features::handlers::resolved_canonical` and its STAC twin do:
/// collection metadata is never worth a 500.
async fn resolved_canonical(
    ctx: &AppContext,
    tenant: &str,
    catalog: &str,
    collection_id: &str,
    external_id: &str,
) -> Option<CanonicalDescriptor> {
    let state = ctx.current();
    match state
        .router
        .canonical_descriptor(tenant, catalog, collection_id)
        .await
    {
        Ok(canonical) => Some(canonical),
        Err(error) => {
            tracing::warn!(
                %error,
                tenant,
                catalog,
                collection = external_id,
                "failed to resolve record collection; serving catalog metadata defaults"
            );
            None
        }
    }
}

/// Builds one catalog object. `self_href` is this catalog's own URL; the
/// `items` link is derived from it, which keeps Requirement 16's endpoint
/// and the route that actually answers it in one expression.
fn catalog_object(
    self_href: &str,
    external_id: &str,
    canonical: Option<&CanonicalDescriptor>,
) -> Catalog {
    let stac = canonical.and_then(|c| c.stac.as_ref());
    Catalog {
        id: external_id.to_string(),
        type_: "Collection",
        item_type: ITEM_TYPE_RECORD,
        // No declared-title source exists in this workspace's descriptor
        // model yet, so this stays absent rather than echoing the id back as
        // a "title" nobody wrote. Recommendation 17
        // (`/rec/record-core/catalog-title`) is a SHOULD, and a fabricated
        // title would satisfy it only in form.
        title: None,
        extent: extent_from_canonical(canonical),
        license: stac.and_then(|s| s.license.clone()),
        keywords: stac.map(|s| s.keywords.clone()).unwrap_or_default(),
        links: vec![
            Link::new(self_href, "self", JSON_MEDIA_TYPE),
            Link::new(format!("{self_href}/items"), REL_ITEMS, GEOJSON_MEDIA_TYPE),
        ],
    }
}

/// Echoes `params` back into an href, substituting `override_token` for the
/// page token when present — the `next`-link builder every cursor-paginated
/// listing in this workspace has its own copy of.
fn paged_href(path: &str, pairs: Vec<(&str, String)>, override_token: Option<&str>) -> String {
    let mut pairs = pairs;
    if let Some(token) = override_token {
        pairs.retain(|(key, _)| *key != "token");
        pairs.push(("token", token.to_string()));
    }
    if pairs.is_empty() {
        return path.to_string();
    }
    let query = pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

fn catalogs_query_pairs(params: &CatalogsQueryParams) -> Vec<(&'static str, String)> {
    let mut pairs = Vec::new();
    if let Some(limit) = params.limit {
        pairs.push(("limit", limit.to_string()));
    }
    if let Some(token) = &params.token {
        pairs.push(("token", token.clone()));
    }
    pairs
}

fn records_query_pairs(params: &RecordsQueryParams) -> Vec<(&'static str, String)> {
    let mut pairs = Vec::new();
    if let Some(limit) = params.limit {
        pairs.push(("limit", limit.to_string()));
    }
    if let Some(datetime) = &params.datetime {
        pairs.push(("datetime", datetime.clone()));
    }
    if let Some(token) = &params.token {
        pairs.push(("token", token.clone()));
    }
    pairs
}

/// `GET /collections` — every record collection in this catalog.
///
/// Filtered to `CollectionKind::Record` off each declaration, before any
/// descriptor is resolved: the kind is declared, not derived, so there is
/// nothing to look up. This is the mirror image of the filter
/// `tellurion_features::handlers::list_collections` applies, and together
/// the two make the Features and Records roots partition the catalog rather
/// than both claiming all of it.
///
/// Cursor-paginated through the same `RegistryReader::list_collections` seam
/// every other `/collections` listing uses. A collection filtered out here
/// can leave a page shorter than the requested `limit` even when the
/// registry has more beyond it — the `next` link, not the page's length, is
/// what a client follows.
pub async fn list_catalogs(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<CatalogsQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let self_path = uri.path().trim_end_matches('/').to_string();
    let state = ctx.current();
    let page_request = PageRequest {
        limit: parse_bounded_limit(raw_query.limit, CATALOGS_DEFAULT_LIMIT, CATALOGS_MAX_LIMIT)?,
        after: raw_query.token.clone(),
    };

    let page = state
        .registry
        .list_collections(&catalog_id, page_request)
        .await?;

    let mut collections = Vec::new();
    for decl in &page.items {
        if !decl.kind.is_record() {
            continue;
        }
        // `#34`: a collection the subject isn't authorized to see is omitted
        // from the listing entirely rather than refused — the same rule the
        // Features and STAC listings follow, so a private record collection
        // is not advertised, only refused on direct access below.
        if authorize_lane(
            &state,
            ctx.rate_counter.as_ref(),
            &headers,
            &tenant_id,
            &catalog_id,
            &decl.id,
            true,
            RateCharge::Skip,
        )
        .await
        .is_err()
        {
            continue;
        }
        let canonical =
            resolved_canonical(&ctx, &tenant_id, &catalog_id, &decl.id, decl.external_id()).await;
        let href = format!("{self_path}/{}", decl.external_id());
        collections.push(catalog_object(
            &href,
            decl.external_id(),
            canonical.as_ref(),
        ));
    }

    let pairs = catalogs_query_pairs(&raw_query);
    let mut links = vec![Link::new(
        paged_href(&self_path, pairs.clone(), None),
        "self",
        JSON_MEDIA_TYPE,
    )];
    if let Some(next_token) = page.next.as_deref() {
        links.push(Link::new(
            paged_href(&self_path, pairs, Some(next_token)),
            "next",
            JSON_MEDIA_TYPE,
        ));
    }

    let mut response = (
        StatusCode::OK,
        Json(CatalogsResponse { links, collections }),
    )
        .into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

/// `GET /collections/{cid}` — one catalog.
pub async fn get_catalog(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    require_record_collection(&state.router, &collection_id, &cid)?;
    authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        true,
        RateCharge::Charge,
    )
    .await?;

    let canonical = resolved_canonical(&ctx, &tenant_id, &catalog_id, &collection_id, &cid).await;
    let self_href = uri.path().trim_end_matches('/').to_string();
    let mut response = (
        StatusCode::OK,
        Json(catalog_object(&self_href, &cid, canonical.as_ref())),
    )
        .into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

/// `GET /collections/{cid}/items` — a page of records.
///
/// Reads through `Router::resolve_features`, the collection's ordinary
/// features lane: a record collection is an ordinary collection whose rows
/// happen to be records. Everything a driver already gives that lane —
/// keyset paging, `datetime` filtering, the `#34` grant filter — applies
/// here unchanged, which is exactly the reuse `#192` asks for.
pub async fn list_records(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<RecordsQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    require_record_collection(&state.router, &collection_id, &cid)?;

    let (decl, source) = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await?;
    let policy_filter = authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        source.filter_capable(),
        RateCharge::Charge,
    )
    .await?;

    let query = ItemsQuery {
        limit: parse_bounded_limit(raw_query.limit, RECORDS_DEFAULT_LIMIT, RECORDS_MAX_LIMIT)?,
        datetime: raw_query
            .datetime
            .as_deref()
            .map(parse_datetime)
            .transpose()?,
        token: raw_query.token.clone(),
        filter: policy_filter,
        ..ItemsQuery::default()
    };
    let mut page = source.items(&decl, &query).await?;
    // `#184`: the same post-source byte budget the Features lane applies.
    // `None` (no level declared `page_max_bytes`) leaves the page untouched.
    if let Some(budget) = decl.settings.page_max_bytes {
        tellurion_core::page_bytes::truncate_page_to_byte_budget(
            decl.external_id(),
            &mut page,
            budget,
        );
    }

    let self_path = uri.path().to_string();
    // Requirement 8 (`/req/record-core/links`, clause A): each record links
    // back to the catalog it belongs to, and clause B allows exactly one
    // such link. The href is this `/items` path with its last segment
    // dropped, so it is always the catalog that actually served the page.
    let catalog_href = self_path
        .strip_suffix("/items")
        .unwrap_or(&self_path)
        .to_string();
    let collection_link = serde_json::json!({
        "href": catalog_href,
        "rel": REL_COLLECTION,
        "type": JSON_MEDIA_TYPE,
    });
    let features = page
        .features_geojson
        .into_iter()
        .map(|record| with_collection_link(record, &collection_link))
        .collect::<Vec<_>>();

    let pairs = records_query_pairs(&raw_query);
    let mut links = vec![Link::new(
        paged_href(&self_path, pairs.clone(), None),
        "self",
        GEOJSON_MEDIA_TYPE,
    )];
    if let Some(next_token) = page.next_token.as_deref() {
        links.push(Link::new(
            paged_href(&self_path, pairs, Some(next_token)),
            "next",
            GEOJSON_MEDIA_TYPE,
        ));
    }

    let body = RecordsResponse {
        type_: "FeatureCollection",
        number_returned: features.len() as u64,
        number_matched: page.number_matched,
        features,
        links,
    };
    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
    Ok(response)
}

/// `GET /collections/{cid}/items/{recordId}` — one record.
pub async fn get_record(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let record_id = require_param(&params, "recordId")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    require_record_collection(&state.router, &collection_id, &cid)?;

    let (decl, source) = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await?;
    let policy_filter = authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        source.filter_capable(),
        RateCharge::Charge,
    )
    .await?;

    let record = source
        .item(&decl, &record_id, policy_filter.as_ref())
        .await?
        .ok_or(CoreError::NotFound)?;

    let self_path = uri.path().to_string();
    // `.../items/{recordId}` -> `...`: two segments up is the catalog.
    let catalog_href = self_path
        .rsplitn(3, '/')
        .nth(2)
        .unwrap_or(&self_path)
        .to_string();
    let collection_link = serde_json::json!({
        "href": catalog_href,
        "rel": REL_COLLECTION,
        "type": JSON_MEDIA_TYPE,
    });
    let mut response = (
        StatusCode::OK,
        Json(with_collection_link(record, &collection_link)),
    )
        .into_response();
    set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
    Ok(response)
}

/// Appends the single `collection` link Requirement 8 mandates to one
/// record's `links` array, creating the array when the driver's projection
/// carried none.
///
/// A record that already carries a `collection` link (no driver in this
/// workspace produces one, but a future one could) is left exactly as it is
/// rather than gaining a second: clause B of that requirement says only a
/// single such link SHALL be included, and a record that already names its
/// catalog is not improved by this crate naming it again.
///
/// A record whose top level is not a JSON object — never produced by any
/// `FeatureSource` in this workspace, all of which emit GeoJSON Features —
/// passes through untouched rather than being coerced into one.
fn with_collection_link(
    mut record: serde_json::Value,
    collection_link: &serde_json::Value,
) -> serde_json::Value {
    let Some(object) = record.as_object_mut() else {
        return record;
    };
    let links = object
        .entry("links")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(array) = links.as_array_mut() else {
        return record;
    };
    let already_linked = array
        .iter()
        .any(|link| link.get("rel").and_then(|rel| rel.as_str()) == Some(REL_COLLECTION));
    if !already_linked {
        array.push(collection_link.clone());
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn collection_link(href: &str) -> serde_json::Value {
        json!({ "href": href, "rel": REL_COLLECTION, "type": JSON_MEDIA_TYPE })
    }

    /// OGC API — Records — Part 1: Core Requirement 8
    /// (`/req/record-core/links`, clause A): a record that is a member of a
    /// catalog SHALL link back to it. A driver's GeoJSON projection carries
    /// no `links` at all, so the array is created.
    #[test]
    fn a_record_with_no_links_gains_the_collection_link() {
        let record = json!({
            "type": "Feature",
            "id": "1",
            "geometry": serde_json::Value::Null,
            "properties": { "title": "Hydrography thesaurus" },
        });
        let linked = with_collection_link(record, &collection_link("/records/collections/t"));
        let links = linked["links"].as_array().expect("a links array");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0]["rel"], REL_COLLECTION);
        assert_eq!(links[0]["href"], "/records/collections/t");
        // Nothing else about the record is touched — the properties are the
        // backing table's own columns and stay exactly as the driver emitted
        // them.
        assert_eq!(linked["properties"]["title"], "Hydrography thesaurus");
        assert!(linked["geometry"].is_null());
    }

    /// Clause B of the same requirement: "Only a single link (relation:
    /// `collection`) SHALL be included in a record." A record that already
    /// names its catalog is left alone rather than gaining a second.
    #[test]
    fn a_record_that_already_names_its_catalog_does_not_gain_a_second_link() {
        let record = json!({
            "type": "Feature",
            "id": "1",
            "geometry": serde_json::Value::Null,
            "properties": {},
            "links": [ { "href": "/elsewhere", "rel": "collection", "type": "application/json" } ],
        });
        let linked = with_collection_link(record, &collection_link("/records/collections/t"));
        let links = linked["links"].as_array().expect("a links array");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0]["href"], "/elsewhere");
    }

    /// A record that carries unrelated links keeps every one of them.
    #[test]
    fn unrelated_links_on_a_record_are_preserved() {
        let record = json!({
            "type": "Feature",
            "id": "1",
            "geometry": serde_json::Value::Null,
            "properties": {},
            "links": [ { "href": "/doc.pdf", "rel": "describes", "type": "application/pdf" } ],
        });
        let linked = with_collection_link(record, &collection_link("/records/collections/t"));
        let links = linked["links"].as_array().expect("a links array");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0]["rel"], "describes");
        assert_eq!(links[1]["rel"], REL_COLLECTION);
    }

    /// Defensive, not reachable through any `FeatureSource` in this
    /// workspace: a non-object passes through rather than being coerced.
    #[test]
    fn a_record_that_is_not_an_object_passes_through_untouched() {
        let record = json!("not a feature");
        let linked = with_collection_link(record.clone(), &collection_link("/x"));
        assert_eq!(linked, record);
    }

    /// The `next`-link builder replaces the page token rather than appending
    /// a second one — a `next` href carrying two `token` parameters would
    /// page unpredictably depending on which the server read.
    #[test]
    fn a_next_href_replaces_the_page_token_rather_than_appending_one() {
        let params = RecordsQueryParams {
            limit: Some(5),
            token: Some("first".to_string()),
            datetime: None,
        };
        let href = paged_href(
            "/collections/t/items",
            records_query_pairs(&params),
            Some("second"),
        );
        assert_eq!(href, "/collections/t/items?limit=5&token=second");
    }

    #[test]
    fn a_self_href_echoes_exactly_the_parameters_the_request_carried() {
        let params = RecordsQueryParams {
            limit: None,
            token: None,
            datetime: Some("2024-01-01T00:00:00Z".to_string()),
        };
        let href = paged_href("/collections/t/items", records_query_pairs(&params), None);
        assert_eq!(
            href,
            "/collections/t/items?datetime=2024-01-01T00%3A00%3A00Z"
        );
    }

    #[test]
    fn a_parameterless_request_gets_a_bare_self_href() {
        let href = paged_href(
            "/collections",
            catalogs_query_pairs(&CatalogsQueryParams::default()),
            None,
        );
        assert_eq!(href, "/collections");
    }

    /// The catalog object's own shape: `type`, `itemType` (Requirement 12)
    /// and the two links Requirements 22 and 16 mandate.
    #[test]
    fn a_catalog_carries_its_item_type_and_the_two_mandated_links() {
        let catalog = catalog_object("/records/collections/thesaurus", "thesaurus", None);
        assert_eq!(catalog.id, "thesaurus");
        assert_eq!(catalog.type_, "Collection");
        assert_eq!(catalog.item_type, ITEM_TYPE_RECORD);
        let rels: Vec<&str> = catalog.links.iter().map(|l| l.rel.as_str()).collect();
        assert_eq!(rels, vec!["self", REL_ITEMS]);
        assert_eq!(
            catalog.links[1].href,
            "/records/collections/thesaurus/items"
        );
        // Absent stays absent: no descriptor means no extent, no license, no
        // keywords — never a fabricated whole-Earth bbox or a title echoed
        // back from the id.
        assert!(catalog.title.is_none());
        assert!(catalog.extent.is_none());
        assert!(catalog.license.is_none());
        assert!(catalog.keywords.is_empty());
    }
}
