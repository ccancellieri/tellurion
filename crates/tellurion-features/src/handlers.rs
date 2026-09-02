//! Handlers. Every one resolves storage through `AppContext::current().router`
//! — no handler here names a concrete driver. Every request runs under a
//! `/{tenant}/features/catalogs/{catalog}` mount (`#39`); `tenant`/`catalog`
//! path parameters carry EXTERNAL ids exactly as the client typed them —
//! `resolve_tenant_catalog` turns them (plus a collection's own external id,
//! when the route has one) into the internal ids `Router` expects, through
//! `AppContext::current().resolver`. Response bodies echo the external ids
//! straight back from the path (or a resolved `CollectionDecl::external_id`
//! for list responses) — an internal id is never serialized. A handler that
//! runs with no mount at all (this crate's own unit tests) falls back to
//! [`DEFAULT_TENANT`]/[`DEFAULT_CATALOG`] so those tests don't need a real
//! server mount.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use tellurion_core::policy::{self, PolicyDecision, ResourceContext};
use tellurion_core::{
    crs, locking, AppContext, CanonicalCapabilities, CanonicalDescriptor, Credential,
    Error as CoreError, Filter, GeometryProfile, Hints, LinkAnchor, PolicyLane, RateCharge,
    RateCounter, RateVerdict, RequestedCrs, ResourceRef, ServedSource, CRS84_URI,
};

use crate::model::{
    CollectionSummary, CollectionsResponse, Extent, FeatureCollectionResponse, FeatureSizeProfile,
    GeometryProfileSummary, Link, SpatialExtent, VertexProfile,
};
use crate::params::{
    build_queryable_filter, collections_href, items_href, parse_collections_query,
    parse_items_query, queryable_query_pairs, resolve_items_crs, CollectionsQueryParams,
    ItemsQueryParams,
};
use crate::problem::ApiError;
use crate::queryables::{self, SCHEMA_JSON_MEDIA_TYPE};

pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";
const GEOJSON_MEDIA_TYPE: &str = "application/geo+json";
const JSON_MEDIA_TYPE: &str = "application/json";
/// OGC API Features Part 2 Requirement 15/16: asserts the CRS used in a
/// response body's geometry coordinates. Not a registered `axum`/`http`
/// constant — every response this crate serves geometry in (`/items`,
/// `/items/{fid}`) sets it explicitly; see `set_content_crs`.
const CONTENT_CRS_HEADER: HeaderName = HeaderName::from_static("content-crs");
/// `X-Tellurion-Source` (`#183`): names the read chain entry (storage id)
/// that actually served an `/items`/`/items/{fid}` read — hinted or not —
/// so chain divergence (index vs main answering differently) is diagnosable
/// from the response alone. The name itself lives in `tellurion_core::hint`
/// so every protocol crate that grows a read lane emits the same header.
const READ_SOURCE_HEADER: HeaderName = HeaderName::from_static(tellurion_core::READ_SOURCE_HEADER);
/// Link relation type OGC API Features Part 3 Requirement 14 mandates for
/// the queryables link on every Collection resource.
const QUERYABLES_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/queryables";
/// OGC API Tiles Part 1's own relation types for linking a geospatial data
/// resource to its tilesets, verified against `opengeospatial/ogcapi-tiles`'s
/// "GeoData Tilesets" requirements class (clause 11), which shows this exact
/// pair of links — same href, two rels — on a Collection resource's `links`
/// array (`#49`).
pub const TILESETS_VECTOR_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/tilesets-vector";
pub const TILESETS_MAP_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/tilesets-map";
/// Not an OGC-registered relation type: 3D Tiles delivery (`places3d`/glb)
/// has no OGC API vocabulary yet, so this is Tellurion's own extension
/// member — an absolute URI (RFC 8288's required shape for an unregistered
/// relation type) that is never expected to be dereferenced, the same
/// non-fetchable-identifier convention every `opengis.net/def/rel/...`
/// value above already uses.
pub const PLACES3D_REL: &str = "https://tellurion.dev/rel/tileset-3d";

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

/// Resolves this request's `(tenant, catalog)` path segments — external ids
/// — to internal ones (`#39`). The one seam every handler in this crate
/// calls before touching `Router`; an unknown tenant or catalog external id
/// surfaces as the same 404 an unknown collection would.
async fn resolve_tenant_catalog(
    ctx: &AppContext,
    params: &HashMap<String, String>,
) -> Result<(String, String), ApiError> {
    let state = ctx.current();
    let tenant_ext = tenant_of(params);
    let catalog_ext = catalog_of(params);
    let tenant_id = state.resolver.resolve_tenant(&tenant_ext).await?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_ext)
        .await?;
    Ok((tenant_id, catalog_id))
}

/// Extracts a [`Credential`] from `Authorization: Bearer <token>` — mirrors
/// `tellurion-server::app`'s own `extract_credential` exactly (duplicated,
/// not shared: `tellurion-core` stays framework-free, so no crate this
/// workspace's protocol crates all depend on can host an `axum`-typed
/// helper — see `auth.rs`'s own module doc for that rule). Any other or
/// malformed `Authorization` header is `Credential::None`, same as no
/// header at all.
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

/// The `#34` policy checkpoint every data-serving handler in this crate
/// calls right after resolving `(tenant_id, catalog_id, collection_id)` —
/// see `tellurion_core::policy`'s own module doc for the isolation/RBAC/ABAC
/// evaluation this wraps. `state.authorizer` being `None` (no `auth:`
/// configured) skips straight to unrestricted access with no extra work at
/// all, the same "byte-for-byte unchanged" rule `tellurion-server`'s own
/// `#17` tenant gate follows. On `Deny`, builds the same 401/403 problem+json
/// shape that gate uses, scoped to this resource.
///
/// `#188`: an allowed request then charges whatever rate ceilings its
/// matching grants declare, via `policy::enforce_rate_limits`. `charge` says
/// whether this particular checkpoint stands for a served request
/// ([`RateCharge::Charge`]) or is a visibility probe inside a listing
/// ([`RateCharge::Skip`]) — see that function's own doc for why one request
/// must charge exactly once. The counter comes from `AppContext` rather than
/// the reloadable `ContextState`, so a config reload never resets a window.
#[allow(clippy::too_many_arguments)]
async fn authorize_lane(
    state: &tellurion_core::ContextState,
    rate_counter: &dyn RateCounter,
    headers: &HeaderMap,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    lane: PolicyLane,
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
        lane,
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

/// Sets `Content-Crs` (Requirement 15/16: `"<" URI-reference ">"`) to the CRS
/// the response's coordinates are genuinely in (`crs::content_crs_uri`) —
/// called on every response this crate serves geometry coordinates in,
/// including the default (no `crs` parameter at all), since a response is
/// always expressed in *some* CRS whether or not the request named one
/// explicitly.
///
/// `crs_capable` is an input rather than a detail hidden inside the driver
/// because the CRS the bytes are in is not a function of what was *asked
/// for* (`#227`): an omitted `crs` transforms nothing on any driver, so a
/// projected collection's default response is in its storage CRS, and only a
/// driver that can reproject turns an explicit `crs=CRS84` into actual
/// CRS84. Stamping what the request wanted rather than what the driver did
/// is what made this header assert degrees over metres.
///
/// The URI is always plain ASCII (either `CRS84_URI` or an
/// `EPSG/0/<digits>` URI — see `crs::epsg_uri`), so `HeaderValue::from_str`
/// never actually fails; the fallback exists only so a future change to what
/// `content_crs_uri` can return can never panic a request over a response
/// header.
fn set_content_crs(
    response: &mut Response,
    resolved: RequestedCrs,
    storage_srid: Option<i32>,
    crs_capable: bool,
) {
    let uri = crs::content_crs_uri(resolved, storage_srid, crs_capable);
    let value = HeaderValue::from_str(&format!("<{uri}>")).unwrap_or_else(|_| {
        HeaderValue::from_static("<http://www.opengis.net/def/crs/OGC/1.3/CRS84>")
    });
    response.headers_mut().insert(CONTENT_CRS_HEADER, value);
}

/// Sets `X-Tellurion-Source` (`#183`) to the storage id the resolved read
/// chain actually served this response from — called only on success paths,
/// which is what makes a [`ServedSource`] label meaningful at all (see its
/// own doc: an errored read served nothing, and never reaches here). A
/// storage id that doesn't fit in a header value (config ids are plain
/// ASCII today, so this is purely defensive) omits the header rather than
/// failing the response, the same rule `set_etag` follows.
fn set_read_source(response: &mut Response, served: &ServedSource) {
    let Some(storage_id) = served.storage_id() else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(storage_id) {
        response.headers_mut().insert(READ_SOURCE_HEADER, value);
    }
}

/// Sets a strong `ETag` (OGC API Features — Part 4, 20-002r1 draft,
/// Optimistic Locking: ETags class, `#107`) on a single-feature `GET`
/// response — `tellurion_core::locking::compute_feature_etag`'s own quoted
/// wire form, unconditional on every collection whose features lane
/// resolves at all (the hash itself needs no per-driver declaration; see
/// that function's own module doc). `write_handlers::put_item`/
/// `delete_item` compare a caller's `If-Match` against this exact same
/// computation over a freshly-read CANONICAL representation — never a
/// CRS-reprojected one — so this must be built from `canonical_feature`,
/// not necessarily the (possibly-reprojected) `feature` this response
/// actually serves; see `get_item`'s own call site.
fn set_etag(response: &mut Response, canonical_feature: &Value) {
    let etag = locking::compute_feature_etag(canonical_feature);
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
}

/// Sets `Last-Modified` (OGC API Features — Part 4, Optimistic Locking:
/// Timestamps class, `#107`) on a single-feature `GET` response, ONLY when
/// `modified_column` names a real declared source AND `canonical_feature`'s
/// own `properties` actually carries a parseable value for it — never
/// fabricated (this collection's own honest "no Timestamps class" answer is
/// simply not setting the header at all, the same "absent, never invented"
/// rule every other optional metadata field in this crate follows).
fn set_last_modified(
    response: &mut Response,
    modified_column: Option<&str>,
    canonical_feature: &Value,
) {
    let Some(column) = modified_column else {
        return;
    };
    let Some(raw) = canonical_feature["properties"][column].as_str() else {
        return;
    };
    let Some(modified_at) = locking::parse_stored_timestamp(raw) else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(&locking::format_http_date(modified_at)) {
        response.headers_mut().insert(header::LAST_MODIFIED, value);
    }
}

/// Rewrites `collection_href`'s protocol-root segment — the path element
/// immediately before the fixed `catalogs` segment, per the
/// `/{tenant}/{protocol}/catalogs/{catalog}/...` mounting every protocol
/// crate's router shares (`#39`, see `tellurion-server::app`'s module docs)
/// — to `protocol`, then appends `suffix`. Used to link from a collection's
/// Features-root representation to that same collection's sibling resource
/// under another protocol root, without this crate depending on
/// `tellurion-server`'s route table (`#49`).
///
/// `None` when `collection_href` carries no `catalogs` segment to anchor
/// on — this crate's own unit tests mount `tellurion_features::router()`
/// with no server prefix in front of it (see `tests/handlers.rs`), so there
/// is nothing meaningful to rewrite; a caller that gets `None` here simply
/// omits the cross-protocol link rather than emitting a guessed one.
fn sibling_href(collection_href: &str, protocol: &str, suffix: &str) -> Option<String> {
    let mut segments: Vec<&str> = collection_href.split('/').collect();
    let catalogs_index = segments.iter().position(|segment| *segment == "catalogs")?;
    if catalogs_index == 0 {
        return None;
    }
    segments[catalogs_index - 1] = protocol;
    let mut href = segments.join("/");
    href.push_str(suffix);
    Some(href)
}

/// `capabilities.features` gates every member only a `FeatureSource` can
/// honour (`#287`): the `items` link (a collection whose only capability is
/// tiles — PMTiles, `#20`, no `FeatureSource` at all — has nothing at
/// `{href}/items` to link to, and that route 404s if hit), the `queryables`
/// link, `itemType`, the Part 2 `crs`/`storageCrs` pair, and the
/// per-collection `cql2ConformanceClasses`/`lockingConformanceClasses`
/// members (those two arrive pre-gated as `Option`s on
/// `CanonicalCapabilities` — see their docs there for the participates/
/// doesn't-participate distinction). Where the capability is absent the
/// member is absent — not empty, not null — so a client never reads a
/// promise (`itemType: "feature"`, a CRS to request, a queryables schema to
/// filter on) that the routed driver then refuses by name. The `queryables`
/// link was unconditional before `#287` (Requirement 14 reads "every
/// Collection resource", and `get_queryables` still serves every collection
/// — the request path is untouched); the advertisement is nonetheless
/// derived from the capability, because this document's job is to say what
/// the collection can DO, and a queryables schema for a collection that
/// accepts no `filter` and serves no `/items` describes nothing a request
/// can reach. `external_id` is always the collection's public id (`#39`) —
/// never `decl.id`, which is internal and must never serialize.
///
/// `capabilities.tiles`/`.tiles_vector`/`.places3d` advertise the render
/// lanes without client probing (`#49`), each gated on the capability that
/// actually serves it (`#287`): `tiles_vector` (the vector `TileSource`
/// lane alone, `Router::resolve_tiles`) gates the `tilesets-vector` link,
/// because that is the lane whose `.mvt` route a follower will hit — a
/// raster-only collection (COG/Zarr, `#37`) is `tiles: true` while that
/// route answers 400, which is exactly the over-advertisement `#287`
/// removed — while the coarse `tiles` (vector OR raster) still gates
/// `tilesets-map`, since the PNG lane rides either capability
/// (`tellurion-tiles::handlers::tile` renders PNG from the vector lane's
/// MVT, `tellurion-tiles`' raster path from a `RasterSource` window). This
/// mirrors `TilesLinkContributor`'s own independent vector/raster probes —
/// the two surfaces answer from the same capability signals, never from a
/// second ad-hoc check. Both tilesets links point at the same sibling
/// tiles-root tileset-list resource; a client that follows either finds
/// the full `TileSet` resource's own `mediaTypes`, `layers` (real
/// source-layer names), and `map`-rel links (one per applicable style) —
/// see `tellurion-tiles::handlers::tileset`. `capabilities.places3d` links
/// to the sibling 3D Tiles root's `tileset.json` resource via
/// `PLACES3D_REL`, an extension member (no OGC vocabulary covers 3D Tiles
/// delivery yet). `capabilities.write` feeds
/// `supports_non_autogenerated_resource_ids` alone — see that field's own
/// doc on `CollectionSummary`.
fn collection_summary(
    href: &str,
    external_id: &str,
    extent: Option<Extent>,
    storage_srid: Option<i32>,
    capabilities: CanonicalCapabilities,
    geometry_profile: Option<GeometryProfileSummary>,
) -> CollectionSummary {
    let CanonicalCapabilities {
        features: has_features,
        tiles: has_tiles,
        tiles_vector,
        places3d: has_places3d,
        crs_capable,
        cql2_conformance_classes,
        write: has_write,
        locking_conformance_classes,
    } = capabilities;
    let mut links = vec![Link::new(href.to_string(), "self", JSON_MEDIA_TYPE)];
    if has_features {
        links.push(Link::new(
            format!("{href}/queryables"),
            QUERYABLES_REL,
            SCHEMA_JSON_MEDIA_TYPE,
        ));
        links.push(Link::new(
            format!("{href}/items"),
            "items",
            GEOJSON_MEDIA_TYPE,
        ));
    }
    if has_tiles {
        if let Some(tiles_href) = sibling_href(href, "tiles", "/tiles") {
            if tiles_vector {
                links.push(Link::new(
                    tiles_href.clone(),
                    TILESETS_VECTOR_REL,
                    JSON_MEDIA_TYPE,
                ));
            }
            links.push(Link::new(tiles_href, TILESETS_MAP_REL, JSON_MEDIA_TYPE));
        }
    }
    if has_places3d {
        if let Some(places_href) = sibling_href(href, "3dtiles", "/3dtiles") {
            links.push(Link::new(places_href, PLACES3D_REL, JSON_MEDIA_TYPE));
        }
    }
    CollectionSummary {
        id: external_id.to_string(),
        title: external_id.to_string(),
        item_type: has_features.then(|| "feature".to_string()),
        extent,
        // Both CRS members exist only where a features lane exists to
        // honour them (`#287`): `crs`/`bbox-crs` are `/items` parameters,
        // so for a collection with no `FeatureSource` both members are
        // omitted entirely — the absent-not-null rule `itemType` follows.
        // Within a features-capable collection, both are further
        // capability-gated through the same `crs_capable` answer, because
        // Part 2 ties them together:
        // Requirement 2 (`/req/crs/fc-md-crs-list`) says `crs` lists what a
        // client may request, and `crs::advertised_crs` keeps only the
        // identifiers `crs::can_serve` — the very gate `list_items`/
        // `get_item` run — will actually honour, so a client is never
        // pointed at a `?crs=`/`?bbox-crs=` value that then earns a 400, nor
        // left unaware of one it could have had. Requirement 4 says
        // `storageCrs` SHALL be one of the identifiers in that very list, so
        // `crs::advertised_storage_crs` is a membership test against it
        // rather than a second rule: absent (`null`, the same "never
        // fabricated" shape `extent`/`geometryProfile` use) exactly when
        // there is nothing honest to name.
        //
        // Which one drops out depends on the storage. A 4326 collection on a
        // driver that cannot reproject loses its storage URI (`#217`):
        // `epsg_uri(4326)` is a different URI string from CRS84, and
        // axis-swapped from it. A *projected* one loses CRS84 instead
        // (`#227`) — it is served in metres and cannot transform, so CRS84
        // is the identifier it genuinely cannot honour.
        storage_crs: has_features.then(|| crs::advertised_storage_crs(storage_srid, crs_capable)),
        crs: has_features.then(|| crs::advertised_crs(storage_srid, crs_capable)),
        cql2_conformance_classes,
        supports_non_autogenerated_resource_ids: has_write.then_some(true),
        geometry_profile,
        locking_conformance_classes,
        links,
    }
}

/// Resolves `collection_id`'s (internal) `CanonicalDescriptor` for the
/// response (`#50`, convergence): the one merge of physical facts, declared
/// schema, STAC metadata, and live capabilities this crate's `/collections`
/// family now reads instead of separately calling `resolve_features`/
/// `resolve_tiles`/`collection_descriptor` per collection. Tolerates an
/// unresolvable `(tenant, catalog, collection)` triple by logging (naming
/// `external_id`, never the internal id — see the "internal id never on the
/// wire" rule this also follows for log readability) and returning `None` —
/// `Router::canonical_descriptor` already absorbs a mere descriptor-
/// derivation failure internally (physical facts simply come back absent),
/// so an `Err` reaching here means the collection itself couldn't be looked
/// up. Extent/capabilities are metadata, not something worth a 500 over —
/// same never-fail-the-request philosophy this crate always applied to
/// extent alone (`#27`), now covering the whole merge.
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
                "failed to resolve collection; serving collection metadata defaults"
            );
            None
        }
    }
}

/// `canonical.extent` mapped into this crate's own `Extent` DTO — `None`
/// when either `canonical` itself or its `extent` field is absent (an
/// unresolvable collection, or a backend that reported none), matching this
/// crate's pre-`#50` extent contract exactly: never fabricated, only ever
/// carried through.
fn extent_from_canonical(canonical: Option<&CanonicalDescriptor>) -> Option<Extent> {
    canonical.and_then(|c| c.extent).map(|extent| Extent {
        spatial: SpatialExtent {
            bbox: vec![extent.bbox],
            crs: CRS84_URI.to_string(),
        },
    })
}

/// Days-since-Unix-epoch -> proleptic Gregorian `(year, month, day)`. Howard
/// Hinnant's public-domain `civil_from_days` algorithm: small, exact, and
/// avoids pulling in a date/calendar crate (`chrono`/`time` are transitive-
/// only in this workspace, not a direct dependency of anything) just to
/// stamp one wall-clock field — the same tradeoff `tellurion-core::sigv4`
/// and `tellurion-stac::iso19139` each already make with their own private
/// copy of this exact algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `GeometryProfile::computed_at` (a `SystemTime`) rendered as an RFC 3339
/// UTC instant (`YYYY-MM-DDTHH:MM:SSZ`, second precision) for the
/// `geometryProfile.computedAt` response member — a pre-epoch clock (never
/// expected on a real deployment) clamps to the epoch itself rather than
/// producing a negative or panicking calculation.
fn format_rfc3339(time: SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    )
}

/// `canonical.geometry_profile` mapped into this crate's own DTO (`#101`,
/// second half — HTTP exposure) — `None` when either `canonical` itself or
/// its `geometry_profile` field is absent, the same never-fabricated rule
/// `extent_from_canonical` above follows for extent.
fn geometry_profile_from_canonical(
    canonical: Option<&CanonicalDescriptor>,
) -> Option<GeometryProfileSummary> {
    let profile: GeometryProfile = canonical?.geometry_profile?;
    Some(GeometryProfileSummary {
        sample_size: profile.sample_size,
        computed_at: format_rfc3339(profile.computed_at),
        vertices: VertexProfile {
            mean: profile.vertices.mean,
            median: profile.vertices.median,
            p95: profile.vertices.p95,
            max: profile.vertices.max,
            total_estimated: profile.vertices.total_estimated,
        },
        vertex_density_per_area: profile.vertex_density_per_area,
        multi_part_fraction: profile.multi_part_fraction,
        mean_ring_count: profile.mean_ring_count,
        feature_size: FeatureSizeProfile {
            p50: profile.feature_size.p50,
            p95: profile.feature_size.p95,
            max: profile.feature_size.max,
        },
    })
}

/// Cross-protocol links contributed for `resource` (`#186`), narrowed to
/// `anchor` and converted into this crate's own `Link` DTO. An `AppContext`
/// with nothing registered — every deployment or test that never called
/// `AppContext::with_link_contributors`, including all of this crate's own
/// unit tests — returns an empty vec without touching the router at all, so
/// those responses stay byte-for-byte what they were before the seam
/// existed. This crate's own `#49` sibling links (`tilesets-vector`/
/// `tilesets-map`/places3d, built in `collection_summary`) are untouched by
/// this: the merge ([`extend_with_contributed`]) drops any contribution
/// that would restate one of them, and never replaces one.
async fn contributed_links(
    ctx: &AppContext,
    resource: &ResourceRef<'_>,
    anchor: LinkAnchor,
) -> Vec<Link> {
    if ctx.link_contributors.is_empty() {
        return Vec::new();
    }
    let state = ctx.current();
    ctx.link_contributors
        .contribute(&state.router, resource)
        .await
        .iter()
        .filter(|link| link.anchor == anchor)
        .map(Link::from)
        .collect()
}

/// Appends `contributed` to `links`, dropping any contribution the document
/// already carries under the same `rel` for the same `href` (`#220`).
///
/// The contribution seam is protocol-neutral by design: a contributor names
/// a sibling root's resource without knowing which root is serializing the
/// answer, and this crate's own `#49` sibling links name some of those very
/// resources under the same registered relation types. Merging blindly
/// would put two identical `(rel, href)` entries in one `links` array —
/// legal per RFC 8288, but a claim stated twice is not a claim stated
/// better. The document's own link wins, because it was built first and its
/// shape is this crate's own contract.
fn extend_with_contributed(links: &mut Vec<Link>, contributed: Vec<Link>) {
    for link in contributed {
        if links
            .iter()
            .any(|existing| existing.rel == link.rel && existing.href == link.href)
        {
            continue;
        }
        links.push(link);
    }
}

/// GET /collections — a collection is listed when its driver resolves
/// *either* the features or the tiles capability for this (tenant, catalog)
/// (reuses the Router's capability-refusal logic instead of duplicating it
/// here). A collection routed entirely to a tiles-only archive driver
/// (PMTiles, `#20`) has no `FeatureSource` at all but is still real,
/// servable data — excluding it here would make `/collections` a
/// features-only listing despite `CatalogSource`/`TileSource` being
/// independent capabilities.
///
/// Cursor-paginated (`#42`, per the OGC API Common/Features paging model):
/// reads through `AppContext`'s registry seam
/// (`RegistryReader::list_collections`) instead of scanning every
/// collection this catalog owns on every request. A small registry (fewer
/// collections than `COLLECTIONS_DEFAULT_LIMIT`) still gets exactly today's
/// single-page response back — a `next` link only ever appears once the
/// registry actually has more to serve. A collection filtered out below
/// (neither lane resolves, or the subject can't see it — `#34`) can leave a
/// page shorter than the requested `limit` even when the registry has more
/// beyond it; the `next` link, not the page's length, is what a client must
/// follow to know whether more remain.
pub async fn list_collections(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<CollectionsQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let self_path = uri.path().to_string();
    let state = ctx.current();
    let page_request = parse_collections_query(&raw_query)?;

    let page = state
        .registry
        .list_collections(&catalog_id, page_request)
        .await?;

    let mut collections = Vec::with_capacity(page.items.len());
    for decl in &page.items {
        // `#192`: a geometry-less record collection is not a feature
        // collection, and this root's `/collections` is a listing of feature
        // collections. Checked off the declaration itself, before the
        // canonical descriptor is resolved, because the kind is declared
        // rather than derived — there is nothing to look up. The Records
        // root's own listing (`tellurion_records`) applies the mirror-image
        // filter, and STAC's applies none: a STAC Collection describes
        // metadata regardless of whether the described thing has geometry.
        if !decl.kind.has_geometry() {
            continue;
        }
        let canonical =
            resolved_canonical(&ctx, &tenant_id, &catalog_id, &decl.id, decl.external_id()).await;
        let capabilities = canonical
            .as_ref()
            .map(|c| c.capabilities.clone())
            .unwrap_or_default();
        if !capabilities.features && !capabilities.tiles {
            continue;
        }
        // `#34`: a collection the subject isn't authorized to see is
        // omitted from the listing entirely, mirroring
        // `tellurion_stac::handlers::list_collections`'s own rule — a
        // private collection should not be advertised, only refused on
        // direct access (`get_collection`, below). `PolicyLane::Features`
        // with `lane_supports_filter: true`: this handler serves metadata,
        // never row data, so a filtered-only grant is enough to see the
        // collection listed (the filter itself is never applied to
        // anything here).
        if authorize_lane(
            &state,
            ctx.rate_counter.as_ref(),
            &headers,
            &tenant_id,
            &catalog_id,
            &decl.id,
            PolicyLane::Features,
            true,
            // `#188`: a listing asks this checkpoint once per candidate
            // collection to decide what to advertise; the one request it
            // serves is charged by whichever handler actually serves data.
            RateCharge::Skip,
        )
        .await
        .is_err()
        {
            continue;
        }
        let href = format!("{}/{}", self_path.trim_end_matches('/'), decl.external_id());
        let extent = extent_from_canonical(canonical.as_ref());
        let storage_srid = canonical.as_ref().and_then(|c| c.srid);
        let geometry_profile = geometry_profile_from_canonical(canonical.as_ref());
        let mut summary = collection_summary(
            &href,
            decl.external_id(),
            extent,
            storage_srid,
            capabilities,
            geometry_profile,
        );
        // `#186`: capability-derived cross-protocol links, appended after
        // this crate's own links so existing consumers' link order is
        // untouched. External ids only in the `ResourceRef`'s wire-facing
        // fields — an internal id never serializes (`#39`).
        let contributed = contributed_links(
            &ctx,
            &ResourceRef {
                tenant: &tenant_of(&params),
                catalog: &catalog_of(&params),
                collection: decl.external_id(),
                item_id: None,
                base_url: "",
                tenant_id: &tenant_id,
                catalog_id: &catalog_id,
                collection_id: &decl.id,
            },
            LinkAnchor::Collection,
        )
        .await;
        extend_with_contributed(&mut summary.links, contributed);
        collections.push(summary);
    }

    let mut links = vec![Link::new(
        collections_href(&self_path, &raw_query, None),
        "self",
        JSON_MEDIA_TYPE,
    )];
    if let Some(next_token) = page.next.as_deref() {
        links.push(Link::new(
            collections_href(&self_path, &raw_query, Some(next_token)),
            "next",
            JSON_MEDIA_TYPE,
        ));
    }

    let body = CollectionsResponse { links, collections };

    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

/// GET /collections/{cid} — same features-or-tiles reasoning as
/// `list_collections`: a collection reachable only through its tiles lane
/// (PMTiles, `#20`) must still resolve here, not 404 just because it has no
/// `FeatureSource`. Tries the features lane first (the common case); a
/// tiles-only collection falls through to the tiles lane, whose error (if
/// that lane is unrouted too) is what a genuinely unknown collection id
/// still surfaces as.
pub async fn get_collection(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;

    let features = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await;
    let has_features = features.is_ok();
    // Resolved unconditionally (not just as a features-lane fallback) so
    // `has_tiles` is known either way — same double-resolve `list_collections`
    // already does per collection (`#49`).
    let tiles = state
        .router
        .resolve_tiles(&tenant_id, &catalog_id, &collection_id)
        .await;
    let has_tiles = tiles.is_ok();
    if features.is_err() {
        // Existence gate, unchanged from before `#50`: a collection
        // resolving neither lane still surfaces the tiles-lane error
        // (same detail text) rather than serving empty metadata for an
        // unroutable collection id.
        tiles?;
    }

    let canonical = resolved_canonical(&ctx, &tenant_id, &catalog_id, &collection_id, &cid).await;
    // `features`/`tiles` come from this handler's own live resolution above,
    // not from the canonical descriptor's copy — that existence gate is what
    // decides whether this request is served at all, so the summary must
    // report the same answer it was gated on. Only the capabilities this
    // handler doesn't resolve itself are read off the descriptor.
    let capabilities = CanonicalCapabilities {
        features: has_features,
        tiles: has_tiles,
        // `tiles` above IS this handler's own `resolve_tiles` — the vector
        // `TileSource` lane — so it doubles as `tiles_vector` (`#287`). A
        // raster-only collection never reaches this point at all: the
        // existence gate above surfaced its tiles-lane error, exactly as it
        // did before `#287` (its document lives in the listing).
        tiles_vector: has_tiles,
        places3d: canonical
            .as_ref()
            .map(|c| c.capabilities.places3d)
            .unwrap_or(false),
        crs_capable: canonical
            .as_ref()
            .map(|c| c.capabilities.crs_capable)
            .unwrap_or(false),
        // Read off this handler's own `features` resolution above, not the
        // canonical descriptor's copy — same "resolved here, not re-read
        // from the descriptor" rule `features`/`tiles` above already follow,
        // and for the same reason: this handler already paid for the real
        // `FeatureSource`, so there is no reason to trust a second copy of
        // the same fact over it.
        cql2_conformance_classes: features
            .as_ref()
            .ok()
            .map(|(_, source)| source.cql2_conformance_classes()),
        write: canonical
            .as_ref()
            .map(|c| c.capabilities.write)
            .unwrap_or(false),
        // Gated on this handler's own `features` resolution, like `cql2_`
        // above (`#287`): a features-capable collection always carries the
        // member (empty when the canonical descriptor couldn't be resolved
        // — the same served-with-defaults tolerance `resolved_canonical`
        // documents), a featureless one never does.
        locking_conformance_classes: has_features.then(|| {
            canonical
                .as_ref()
                .and_then(|c| c.capabilities.locking_conformance_classes.clone())
                .unwrap_or_default()
        }),
    };
    let extent = extent_from_canonical(canonical.as_ref());
    let storage_srid = canonical.as_ref().and_then(|c| c.srid);
    let geometry_profile = geometry_profile_from_canonical(canonical.as_ref());

    let mut body = collection_summary(
        uri.path(),
        &cid,
        extent,
        storage_srid,
        capabilities,
        geometry_profile,
    );
    // `#186`: same capability-derived cross-protocol links the listing
    // appends per collection — see `contributed_links`'s own doc.
    let contributed = contributed_links(
        &ctx,
        &ResourceRef {
            tenant: &tenant_of(&params),
            catalog: &catalog_of(&params),
            collection: &cid,
            item_id: None,
            base_url: "",
            tenant_id: &tenant_id,
            catalog_id: &catalog_id,
            collection_id: &collection_id,
        },
        LinkAnchor::Collection,
    )
    .await;
    extend_with_contributed(&mut body.links, contributed);
    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

/// GET /collections/{cid}/queryables (OGC API Features Part 3, "Queryables"
/// requirements class, `#33` follow-up): resolved the same tolerant
/// features-or-tiles way `get_collection` is, so a tiles-only collection
/// (no `FeatureSource`, PMTiles `#20`) still returns a document — possibly
/// with an empty `properties` object if the driver can't introspect columns
/// at all — rather than 404ing just because it has no items lane. An
/// unrouted collection id still 404s, the same way `get_collection` does.
///
/// Builds its `CanonicalDescriptor` (`#50`) directly from
/// `Router::collection_descriptor` rather than going through
/// `Router::canonical_descriptor`/`resolved_canonical` the way
/// `list_collections`/`get_collection` do: this handler has never tolerated
/// a descriptor-derivation failure the way extent/capability metadata does
/// elsewhere (a `collection_descriptor` error still propagates as a 500 via
/// `?`, unchanged from before `#50`), and `canonical_descriptor` is
/// deliberately tolerant of exactly that failure (see its own doc) — using
/// it here would silently turn this handler's existing hard failure into a
/// degraded-but-200 response. The merge logic itself is still the single
/// `descriptor::canonical::build` every other canonical-descriptor consumer
/// shares; only the I/O-tolerance policy around it differs, by design, for
/// this one endpoint.
pub async fn get_queryables(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    // `#34`: queryables describe schema, not row data — a filtered grant
    // still permits viewing them (`lane_supports_filter: true` here just
    // means "a filtered grant match is fine," the returned filter itself is
    // never applied to anything).
    authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        PolicyLane::Features,
        true,
        RateCharge::Charge,
    )
    .await?;

    let features = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await;
    let decl = match features {
        Ok((decl, _source)) => decl,
        Err(_) => {
            state
                .router
                .resolve_tiles(&tenant_id, &catalog_id, &collection_id)
                .await?
                .0
        }
    };

    let descriptor = state
        .router
        .collection_descriptor(&tenant_id, &catalog_id, &collection_id)
        .await?;
    let canonical = tellurion_core::descriptor::canonical::build(
        Some(&descriptor),
        &decl,
        decl.schema.as_ref(),
        None,
        tellurion_core::CanonicalCapabilities::default(),
        None,
    );

    let body = queryables::build_document(&canonical, &cid, uri.path().to_string());
    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, SCHEMA_JSON_MEDIA_TYPE);
    Ok(response)
}

/// GET /collections/{cid}/items. `all_params` is a second, independent
/// parse of the same query string as `raw_query` — axum's `Query` extractor
/// only ever reads `parts.uri.query()`, never the body, so having both in
/// one handler signature is safe — into an unstructured map, the only way to
/// see a query parameter name `ItemsQueryParams` has no field for: the
/// per-collection queryable names Requirement 4 (`/req/queryables-query-
/// parameters/parameters`, OGC API Features Part 3, `#52`) binds are dynamic
/// and unknown at compile time.
pub async fn list_items(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<ItemsQueryParams>,
    Query(all_params): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;

    // `#183`: read-lane hints. `prefer:` only reorders the resolved chain
    // (never extends it — `Router::resolve_features_read`'s own doc), so the
    // `authorize_lane` checkpoint below evaluates exactly what it would for
    // the unhinted request: the policy decision keys on ids and lane, and
    // `source.filter_capable()` is an order-independent intersection over
    // the same entry set. Unknown tokens were already dropped by the parse —
    // a hint can influence which chain entry answers, never whether this
    // request is allowed to ask.
    let hints = Hints::parse(raw_query.hints.as_deref());
    let (decl, source, served) = state
        .router
        .resolve_features_read(&tenant_id, &catalog_id, &collection_id, &hints)
        .await?;
    // `#34`: this lane pushes an ABAC grant filter all the way to PostGIS
    // (AND-merged below with any user-supplied `filter`) — but only when
    // the resolved driver can actually compile one; `source.filter_capable()`
    // (not a blanket `true`) is what `authorize_resource` uses to decide
    // whether a filtered-only grant match may proceed or must deny, so a
    // FlatGeobuf-backed collection under a filter-requiring grant denies
    // (403) rather than surfacing the generic "filter not supported" 400 a
    // user-supplied filter against the same driver already gets.
    let policy_filter = authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        PolicyLane::Features,
        source.filter_capable(),
        RateCharge::Charge,
    )
    .await?;
    let mut query = parse_items_query(&raw_query)?;
    // `crs`/`bbox-crs` (OGC API Features Part 2 CRS by Reference) resolve
    // against `decl.srid` — already known here at no extra I/O cost, since
    // `resolve_features` above derives it as part of the effective decl it
    // hands back (`Router::effective_decl`). A request naming a CRS this
    // driver cannot actually put out refuses here, the same
    // 400-naming-the-unsupported-capability shape `filter_capable` already
    // uses for `filter`/queryable params.
    //
    // `crs::can_serve` decides, rather than a `crs_capable` check spelled
    // out here (`#227`), because it is the same rule the collection's own
    // `crs` list is filtered by — so a client following
    // `/req/crs/fc-md-crs-list` straight into a `?crs=` can never be refused
    // for asking for something the metadata offered, and can never be served
    // something the metadata withheld. It is not simply "`Storage` needs
    // `crs_capable`": on a **projected** collection under a driver that
    // never reprojects, the storage CRS is the free one (its rows already
    // come out in it) and CRS84 is the expensive one, so it is `crs=CRS84`
    // that must be refused by name — the alternative being metres under a
    // header naming degrees, which is exactly the defect `#227` closed. On a
    // CRS84-equivalent storage — every live deployment — the verdict is
    // byte-for-byte what it was before.
    let resolved = resolve_items_crs(&raw_query, &mut query, decl.srid).map_err(ApiError::from)?;
    let resolved_crs = resolved.crs;
    let crs_capable = source.crs_capable();
    if !crs::can_serve(resolved_crs, decl.srid, crs_capable)
        || !crs::can_serve(resolved.bbox_crs, decl.srid, crs_capable)
    {
        return Err(ApiError::from(CoreError::Invalid(format!(
            "collection '{cid}' does not support the 'crs'/'bbox-crs' parameters: it is \
             served in {}",
            crs::advertised_crs(decl.srid, crs_capable).join(", ")
        ))));
    }
    // `#255`: a `bbox` with no `bbox-crs` names no CRS, but it is not
    // CRS-less. Part 1 Requirement 23 (`/req/core/fc-bbox-definition`) clause
    // C — "the coordinate reference system of the values SHALL be interpreted
    // as WGS 84 longitude/latitude ... unless a different coordinate reference
    // system is specified in a parameter `bbox-crs`" — and Part 2 Requirement
    // 8 (`/req/crs/fc-bbox-crs-valid-default-value`) both say the omitted
    // parameter *is* CRS84. So the servability question for it is the very
    // question `can_serve` already answers for an explicit `bbox-crs=CRS84`,
    // asked here about `RequestedCrs::Crs84` rather than about the variant on
    // the wire — one predicate (`#227`), two spellings of one request.
    //
    // `can_serve`'s own `Omitted` arm is `true` for every driver and stays
    // that way: it answers for the OUTPUT `crs` parameter, where omitting it
    // asks for nothing in particular and `content_crs_uri` names whatever came
    // back. An omitted `bbox-crs` is the opposite — it fixes the meaning of
    // four numbers the client already sent.
    //
    // A driver that cannot transform them has no third option. PostGIS used to
    // hand PostgreSQL a CRS84 envelope beside a projected column, and because
    // `&&` (unlike `ST_Intersects`) does not raise on mixed SRIDs it answered
    // `200` with degrees compared against metres — wrong rows, violating Part 1
    // Requirement 24 (`/req/core/fc-bbox-response`) clause A, undetectable by
    // any client. A driver comparing raw coordinates in memory (GeoPackage)
    // does exactly the same thing with no database to object. So it refuses BY
    // NAME instead, and names what the collection *is* served in — which for
    // such a collection is its own storage CRS, a `bbox-crs` value its `crs`
    // list advertises and this lane serves without any transform at all.
    //
    // Every CRS84-stored collection — every live demo — answers `true` to
    // `can_serve(Crs84, ..)` whatever the driver, and never reaches this
    // branch.
    if query.bbox.is_some()
        && resolved.bbox_crs == RequestedCrs::Omitted
        && !crs::can_serve(RequestedCrs::Crs84, decl.srid, crs_capable)
    {
        return Err(ApiError::from(CoreError::Invalid(format!(
            "collection '{cid}' cannot evaluate a 'bbox' with no 'bbox-crs': such a bbox is \
             CRS84, its storage is not, and this driver cannot transform the bbox into it — it \
             is served in {}",
            crs::advertised_crs(decl.srid, crs_capable).join(", ")
        ))));
    }
    // `filter-crs` (Part 3 Filtering, 19-079r2 Requirement 8,
    // `/req/filter/filter-crs-param`) rides its own driver capability, NOT
    // `crs_capable` above (`#217`): reprojecting output geometry and
    // transforming a filter's input literals are different work, and this
    // issue exists because PostGIS had the first without the second.
    // Requirement 8's own closing sentence — "The server SHALL return an
    // error, if it does not support the CRS identified in `filter-crs` for
    // the resource" — is this refusal; CRS84 is always accepted (it is
    // Requirement 7's default, a no-op for literals already read in it), so
    // only a `filter-crs` naming the collection's own storage CRS can reach
    // here. Refused BY NAME rather than accepted and ignored: silently
    // evaluating the filter in the other CRS returns the wrong features
    // under a 200, which is exactly what this parameter was reserved to
    // prevent.
    if resolved.filter_crs == RequestedCrs::Storage && !source.filter_crs_capable() {
        return Err(ApiError::from(CoreError::Invalid(format!(
            "collection '{cid}' does not support the 'filter-crs' parameter"
        ))));
    }
    let queryable_pairs = queryable_query_pairs(&all_params);
    if let Some(grant_filter) = policy_filter {
        // AND-merge the policy grant's filter with whatever the request
        // itself supplied — a grant narrows what a subject may see
        // regardless of what they additionally chose to filter by.
        query.filter = Some(match query.filter.take() {
            None => grant_filter,
            Some(existing) => Filter::And(vec![existing, grant_filter]),
        });
    }

    if query.filter.is_some() || !queryable_pairs.is_empty() {
        // Both filtering surfaces ride the same driver capability (`#52`
        // reuses `#33`'s gate rather than inventing a second one) — refused
        // before either is ever compiled or reaches `items`, not silently
        // ignored or partially evaluated.
        if !source.filter_capable() {
            let detail = if query.filter.is_some() {
                format!("collection '{cid}' does not support the 'filter' parameter")
            } else {
                format!(
                    "collection '{cid}' does not support queryable query parameters (e.g. '{}')",
                    queryable_pairs[0].0
                )
            };
            return Err(ApiError::from(CoreError::Invalid(detail)));
        }
        let descriptor = state
            .router
            .collection_descriptor(&tenant_id, &catalog_id, &collection_id)
            .await?;
        if let Some(filter) = &query.filter {
            tellurion_core::filter::validate(filter, &descriptor, decl.schema.as_ref())
                .map_err(ApiError::from)?;
        }
        if !queryable_pairs.is_empty() {
            // The same `CanonicalDescriptor` merge `get_queryables` builds
            // its document from (`#50`) — `queryable_property_types` is that
            // document's own source of truth for which names are declared
            // queryables, so a name accepted here is guaranteed to be one
            // `/queryables` would also list, closed-schema narrowing
            // included.
            let canonical = tellurion_core::descriptor::canonical::build(
                Some(&descriptor),
                &decl,
                decl.schema.as_ref(),
                None,
                tellurion_core::CanonicalCapabilities::default(),
                None,
            );
            let queryable_types = queryables::queryable_property_types(&canonical);
            if let Some(predicate) = build_queryable_filter(&queryable_pairs, &queryable_types)
                .map_err(ApiError::from)?
            {
                query.filter = Some(match query.filter.take() {
                    None => predicate,
                    Some(Filter::And(mut items)) => {
                        items.push(predicate);
                        Filter::And(items)
                    }
                    Some(existing) => Filter::And(vec![existing, predicate]),
                });
            }
        }
    }

    // `#247`, Requirement 7 (`/req/filter/filter-crs-wgs84`): with no
    // `filter-crs` on the wire the server SHALL process the filter's
    // geometries in CRS84. Against a collection stored in a projected CRS
    // that is a real coordinate transform of every spatial literal — the same
    // work an explicit `filter-crs=CRS84` asks for, because the two say the
    // same thing about the same numbers (`crs::crs84_literals_need_transform`)
    // — and only a `filter_crs_capable` driver can perform it.
    //
    // A driver that cannot has exactly two other options and both are
    // forbidden: PostGIS used to hand PostgreSQL a CRS84 literal beside a
    // projected column and return the mixed-SRID `500` this issue was opened
    // for, and a driver comparing raw coordinates in memory (GeoPackage)
    // answers `200` with rows selected in a CRS the client never named. So it
    // refuses BY NAME instead, the same shape `#248` already gives the
    // explicit-CRS84 case one lane to the left.
    //
    // Narrowed to a filter that actually carries a geometry: an attribute-only
    // `population > 10` has nothing to process in any CRS, so refusing it
    // would name a transform the request never asked for. Every CRS84-stored
    // collection — every live demo — answers `false` to
    // `crs84_literals_need_transform` and never reaches this branch at all.
    if query.filter_crs == RequestedCrs::Omitted
        && query
            .filter
            .as_ref()
            .is_some_and(tellurion_core::Filter::has_spatial_literal)
        && tellurion_core::crs::crs84_literals_need_transform(decl.srid)
        && !source.filter_crs_capable()
    {
        return Err(ApiError::from(CoreError::Invalid(format!(
            "collection '{cid}' cannot evaluate a spatial filter: its storage is not CRS84, and \
             this driver cannot transform a filter's spatial literals into it"
        ))));
    }

    let mut page = source.items(&decl, &query).await?;
    // `#184`: byte-budget the page AFTER the source returned it — trimming
    // is response-shaping policy, deliberately not part of the
    // `FeatureSource` contract (contrast the vertex budget, which refuses
    // via a source decorator — `tellurion_core::items_budget`). The
    // effective value rode the settings chain onto this decl through
    // `Router::apply_inherited_settings`; `None` (no level ever declared
    // `page_max_bytes`) means the lane is off and the page passes through
    // exactly as before. The `next` link below reads `page.next_token`, so
    // a re-minted token needs no link restructuring.
    if let Some(budget) = decl.settings.page_max_bytes {
        tellurion_core::page_bytes::truncate_page_to_byte_budget(
            decl.external_id(),
            &mut page,
            budget,
        );
    }

    let path = uri.path().to_string();
    let mut links = vec![Link::new(
        items_href(&path, &raw_query, &queryable_pairs, None),
        "self",
        GEOJSON_MEDIA_TYPE,
    )];
    if let Some(next_token) = page.next_token.as_deref() {
        links.push(Link::new(
            items_href(&path, &raw_query, &queryable_pairs, Some(next_token)),
            "next",
            GEOJSON_MEDIA_TYPE,
        ));
    }

    let body = FeatureCollectionResponse {
        type_: "FeatureCollection",
        number_returned: page.features_geojson.len() as u64,
        number_matched: page.number_matched,
        features: page.features_geojson,
        links,
    };

    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
    set_content_crs(&mut response, resolved_crs, decl.srid, crs_capable);
    set_read_source(&mut response, &served);
    Ok(response)
}

/// GET /collections/{cid}/items/{fid}. `raw_query`'s only meaningful keys
/// are `crs` (Part 2) and `hints` (`#183`) — a single-feature fetch has no
/// `bbox` to interpret a `bbox-crs` against, and `filter`/queryable params
/// don't apply here either (the only row-level filter on this lane is the
/// `#34` grant filter pushed through `item_with_crs` below). Read as an
/// untyped map, the same `#52` pattern `list_items`' own `all_params` uses,
/// rather than a dedicated struct for two fields.
pub async fn get_item(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let fid = require_param(&params, "fid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    // `#183`: same hint discipline as `list_items` (see the comment there) —
    // reorder-only, so the `authorize_lane` checkpoint below is unaffected.
    let hints = Hints::parse(raw_query.get("hints").map(String::as_str));
    let (decl, source, served) = state
        .router
        .resolve_features_read(&tenant_id, &catalog_id, &collection_id, &hints)
        .await?;
    // `#34`: a single-item fetch pushes the grant filter into the same
    // single-row `WHERE` clause `FeatureSource::item` now compiles it into
    // (`tellurion-postgis::sql::build_item_plan`), when the resolved driver
    // can compile one at all — `source.filter_capable()` decides, exactly
    // as `list_items` already does for the items-list lane. An item this
    // filter excludes comes back `Ok(None)` from `item` itself,
    // indistinguishable from a genuinely absent id — this handler never
    // learns the difference, so it can never leak one.
    let policy_filter = authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        PolicyLane::Features,
        source.filter_capable(),
        RateCharge::Charge,
    )
    .await?;

    // The single-feature twin of `list_items`' own gate — same
    // `crs::can_serve` rule, so the two lanes cannot disagree about what a
    // collection can be served in. See that gate's comment for why the
    // refusal is not simply "`Storage` needs `crs_capable`" (`#227`).
    let resolved_crs = crs::resolve(raw_query.get("crs").map(String::as_str), decl.srid)
        .map_err(ApiError::from)?;
    let crs_capable = source.crs_capable();
    if !crs::can_serve(resolved_crs, decl.srid, crs_capable) {
        return Err(ApiError::from(CoreError::Invalid(format!(
            "collection '{cid}' does not support the 'crs' parameter: it is served in {}",
            crs::advertised_crs(decl.srid, crs_capable).join(", ")
        ))));
    }
    let feature = source
        .item_with_crs(&decl, &fid, policy_filter.as_ref(), resolved_crs)
        .await?
        .ok_or(CoreError::NotFound)?;

    // OGC API Features — Part 4 Optimistic Locking (`#107`): both classes
    // read a CANONICAL representation, independent of this request's own
    // `?crs=` choice (`tellurion_core::locking`'s own module doc explains
    // why — the write side has no `?crs=` of its own to be consistent
    // with). `item_with_crs`'s own contract makes `RequestedCrs::Omitted`
    // (no `?crs=` at all, the overwhelming common case) identical to a
    // plain `item()` call, so only a request that asked for genuine
    // reprojection pays for a second, canonical read here.
    let canonical_feature = if resolved_crs == RequestedCrs::Omitted {
        feature.clone()
    } else {
        source
            .item(&decl, &fid, policy_filter.as_ref())
            .await?
            .unwrap_or_else(|| feature.clone())
    };

    let path = uri.path().to_string();
    let collection_href = path
        .rsplit_once("/items/")
        .map(|(base, _)| base.to_string())
        .unwrap_or_else(|| path.clone());

    let feature = attach_links(
        feature,
        &[
            Link::new(path, "self", GEOJSON_MEDIA_TYPE),
            Link::new(collection_href, "collection", JSON_MEDIA_TYPE),
        ],
    );

    let mut response = (StatusCode::OK, Json(feature)).into_response();
    set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
    set_content_crs(&mut response, resolved_crs, decl.srid, crs_capable);
    set_read_source(&mut response, &served);
    set_etag(&mut response, &canonical_feature);
    set_last_modified(
        &mut response,
        decl.modified_column.as_deref(),
        &canonical_feature,
    );
    Ok(response)
}

fn attach_links(mut feature: Value, links: &[Link]) -> Value {
    if let Value::Object(map) = &mut feature {
        map.insert("links".to_string(), json!(links));
    }
    feature
}

#[cfg(test)]
mod tests {
    use super::*;

    // `format_rfc3339`'s own civil-calendar round-trips — the same
    // known-date vectors `tellurion-core::sigv4`'s `civil_from_days` tests
    // use (independently verified against Python's standard-library
    // `datetime`), reformatted from amz-date punctuation to RFC 3339.
    #[test]
    fn format_rfc3339_formats_known_calendar_dates() {
        let cases = [
            (0u64, "1970-01-01T00:00:00Z"),
            (1_440_938_160, "2015-08-30T12:36:00Z"),
            (1_369_353_600, "2013-05-24T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"), // a leap day
            (1_735_689_599, "2024-12-31T23:59:59Z"),
            (946_684_799, "1999-12-31T23:59:59Z"),
        ];
        for (secs, expected) in cases {
            let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
            assert_eq!(format_rfc3339(time), expected, "unix seconds {secs}");
        }
    }
}
