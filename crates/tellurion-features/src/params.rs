//! Query parameter parsing for `/collections/{cid}/items`. Kept deliberately
//! light (no RFC 3339 dependency): datetime bounds are validated for shape
//! only, matching `tellurion_core::DatetimeRange` staying a raw-string type.
//! The leaf-level `bbox`/`datetime`/`limit`/percent-encoding parsing itself
//! lives in `tellurion_core::query_params` — byte-identical to
//! `tellurion-stac`'s own `/items` parsing, so it has one implementation
//! both crates call into rather than two copies to keep in sync.

use std::collections::BTreeMap;

use serde::Deserialize;
use tellurion_core::query_params::{
    parse_bbox, parse_bounded_limit, parse_datetime, percent_encode,
};
use tellurion_core::{
    crs, filter, CompareOp, Error, Filter, ItemsQuery, Literal, PageRequest, PropertyType,
    RequestedCrs, Result,
};

pub const DEFAULT_LIMIT: u32 = 10;
pub const MAX_LIMIT: u32 = 10_000;

/// Default page size for `GET /collections` (`#42`) when no `limit` is
/// given — large enough that any realistic file-backed deployment's whole
/// catalog still fits on the first page (today's exact single-page
/// response), while still bounding a registry sized past that. Deliberately
/// its own constant rather than reusing [`DEFAULT_LIMIT`]/[`MAX_LIMIT`]
/// above: those size an items *page* (naturally small, browsed by a human
/// or a map client), while listing "all my catalog's collections" is a
/// different, coarser-grained use case.
pub const COLLECTIONS_DEFAULT_LIMIT: u32 = 100;
pub const COLLECTIONS_MAX_LIMIT: u32 = 10_000;

/// `filter-lang` default when a `filter` is supplied without one (`#33`,
/// OGC API Features Part 3): CQL2-text.
const DEFAULT_FILTER_LANG: &str = filter::FILTER_LANG_CQL2_TEXT;

#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct ItemsQueryParams {
    pub limit: Option<u32>,
    pub bbox: Option<String>,
    pub datetime: Option<String>,
    pub token: Option<String>,
    /// A CQL2 filter expression, encoded per `filter_lang` (`#33`).
    pub filter: Option<String>,
    /// Which encoding `filter` is in: `cql2-text` (the default when
    /// omitted) or `cql2-json`. Ignored when `filter` itself is absent.
    #[serde(rename = "filter-lang")]
    pub filter_lang: Option<String>,
    /// Requested output CRS for geometry coordinates (OGC API Features
    /// Part 2 CRS by Reference). Kept as a raw URI string here — resolving
    /// it against a collection's supported CRS set needs the storage SRID,
    /// which this pure parse has no access to; see `handlers::list_items`/
    /// `::get_item`, which resolve it via `tellurion_core::crs::resolve`
    /// once the collection is known.
    pub crs: Option<String>,
    /// The CRS `bbox`'s four numbers are expressed in (Part 2). Same
    /// raw-string/resolved-by-the-handler split as `crs` above; meaningful
    /// only together with `bbox`.
    #[serde(rename = "bbox-crs")]
    pub bbox_crs: Option<String>,
    /// The CRS every spatial literal inside `filter` is expressed in (OGC
    /// API — Features Part 3: Filtering, 19-079r2 Requirement 8,
    /// `/req/filter/filter-crs-param`, `#217`). Same raw-string/
    /// resolved-by-the-handler split as `crs`/`bbox-crs` above, and
    /// meaningful only together with `filter`. `None` — the parameter was
    /// not supplied — is Requirement 7's own default
    /// (`/req/filter/filter-crs-wgs84`: process the filter's geometries in
    /// CRS84) and resolves to `RequestedCrs::Omitted`, which every driver
    /// compiles byte-for-byte the way it did before `#217`.
    #[serde(rename = "filter-crs")]
    pub filter_crs: Option<String>,
    /// Read-lane hints (`#183`), raw: the comma-separated `?hints=` value
    /// exactly as the client sent it. Kept as a raw string here for the
    /// same reason `crs` is — this is a pure parse, and the closed-
    /// vocabulary tokenization (`tellurion_core::Hints::parse`, unknown
    /// tokens dropped so a typo never 400s) belongs to the handler that
    /// hands the result to `Router::resolve_features_read`.
    pub hints: Option<String>,
}

/// Parses every `/items` query parameter except `crs`/`bbox-crs` into a
/// syntactically valid `ItemsQuery` — `filter`/`filter-lang` (`#33`) into a
/// `Filter` tree, `bbox` into a plain `[f64; 4]` in the axis order the
/// request supplied it. This is a pure, I/O-free parse: property-name
/// validation against a collection's derived attribute schema, the driver-
/// capability checks (`filter_capable`/`crs_capable`), and CRS resolution
/// (needs the collection's storage SRID) all need data this function doesn't
/// have and are the caller's job — see `handlers::list_items`, which resolves
/// `crs`/`bbox-crs` (and axis-swaps `bbox` when `bbox-crs` demands it)
/// immediately after parsing this.
pub fn parse_items_query(raw: &ItemsQueryParams) -> Result<ItemsQuery> {
    let limit = parse_limit(raw.limit)?;
    let bbox = raw.bbox.as_deref().map(parse_bbox).transpose()?;
    let datetime = raw.datetime.as_deref().map(parse_datetime).transpose()?;
    let filter = raw
        .filter
        .as_deref()
        .map(|text| {
            let lang = raw.filter_lang.as_deref().unwrap_or(DEFAULT_FILTER_LANG);
            filter::parse(lang, text)
        })
        .transpose()?;

    Ok(ItemsQuery {
        limit,
        bbox,
        datetime,
        token: raw.token.clone(),
        filter,
        crs: RequestedCrs::Omitted,
        bbox_crs: RequestedCrs::Omitted,
        filter_crs: RequestedCrs::Omitted,
    })
}

/// Every CRS-bearing `/items` query parameter, resolved through the one
/// `tellurion_core::crs::resolve` seam: `crs` and `bbox-crs` (Part 2 CRS by
/// Reference) and `filter-crs` (Part 3 Filtering Requirement 8,
/// `/req/filter/filter-crs-param`, `#217`). All three are resolved here
/// rather than two here and one in the handler, so a collection can never be
/// handed a CRS its own descriptor didn't advertise on one parameter but not
/// another.
///
/// When `bbox-crs` named this collection's own storage CRS and that storage
/// SRID is exactly `4326`, `query.bbox` is axis-swapped in place
/// (`crs::swap_bbox_axes`) — the classic Part 2 axis-order trap (`crs`
/// module doc): a `bbox-crs` naming a latitude-before-longitude CRS supplies
/// its four numbers `[minLat, minLon, maxLat, maxLon]`, and every SQL
/// envelope builder in `tellurion-postgis::sql` assumes the longitude-first
/// `[minx, miny, maxx, maxy]` shape. `filter-crs` gets no equivalent swap
/// here: a filter's spatial literals are a whole WKT/GeoJSON geometry tree,
/// not four numbers, so the same axis correction is applied by the driver
/// that compiles them (`ST_FlipCoordinates`, `tellurion-postgis::sql::
/// geometry_literal_expr`) rather than rewritten into the AST — one place
/// that already has to reason about the storage CRS anyway, instead of a
/// second geometry-rewriting pass here.
///
/// Mutates `query.crs`/`::bbox_crs`/`::filter_crs` to the resolved values
/// and returns them so the caller can also build the `Content-Crs` response
/// header and validate each parameter against the resolved driver's own
/// capability, without re-deriving any of them from the raw strings again.
pub fn resolve_items_crs(
    raw: &ItemsQueryParams,
    query: &mut ItemsQuery,
    storage_srid: Option<i32>,
) -> Result<ResolvedItemsCrs> {
    let resolved_crs = crs::resolve(raw.crs.as_deref(), storage_srid)?;
    let resolved_bbox_crs = crs::resolve(raw.bbox_crs.as_deref(), storage_srid)?;
    let resolved_filter_crs = crs::resolve(raw.filter_crs.as_deref(), storage_srid)?;
    if resolved_bbox_crs == RequestedCrs::Storage && storage_srid.is_some_and(crs::is_lat_lon_order)
    {
        if let Some(bbox) = query.bbox.as_mut() {
            *bbox = crs::swap_bbox_axes(*bbox);
        }
    }
    query.crs = resolved_crs;
    query.bbox_crs = resolved_bbox_crs;
    query.filter_crs = resolved_filter_crs;
    Ok(ResolvedItemsCrs {
        crs: resolved_crs,
        bbox_crs: resolved_bbox_crs,
        filter_crs: resolved_filter_crs,
    })
}

/// What [`resolve_items_crs`] answers: each CRS-bearing `/items` query
/// parameter as it resolved against this collection's supported CRS set. A
/// named struct rather than a widening tuple (`#217` added the third member)
/// so a caller reads `resolved.filter_crs` instead of counting positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedItemsCrs {
    /// `crs` — the requested output CRS (Part 2).
    pub crs: RequestedCrs,
    /// `bbox-crs` — the CRS `bbox`'s four numbers came in (Part 2).
    pub bbox_crs: RequestedCrs,
    /// `filter-crs` — the CRS a `filter`'s spatial literals came in
    /// (Part 3 Requirement 8).
    pub filter_crs: RequestedCrs,
}

// -- queryables as query parameters (OGC API Features Part 3, `#52`) --------

/// Query parameter names `/items` already gives a fixed meaning to, at Core
/// (`limit`), Part 2 CRS (`crs`, `bbox-crs`), or Part 3 Filtering
/// (`bbox`/`datetime` inherited from Core, `token` — this crate's own
/// cursor-paging name, the OGC vocabulary's closest equivalent being
/// `offset`, which this crate deliberately never implements: paging here is
/// keyset-only, never `OFFSET` — `filter`, `filter-lang`, `filter-crs`).
/// Requirement 4 (`/req/queryables-query-parameters/parameters`) says
/// nothing about a name collision with one of these; this crate's own
/// choice, undocumented upstream so written down here, is that a reserved
/// name never becomes an implicit equality predicate even when a collection
/// happens to declare a queryable of the same name — the protocol-level
/// parameter always wins. `offset` is reserved even though this crate
/// deliberately never implements it (offset-based paging), so that adding it
/// later is never a silent behavior change for an existing collection's
/// queryables; `filter-crs` was reserved on the same forward-looking grounds
/// and is now genuinely implemented (`#217`, Part 3 Requirement 8
/// `/req/filter/filter-crs-param`) — the reservation is what let it become
/// real without any collection's queryables changing meaning. `hints` is Tellurion's own read-lane
/// hint parameter (`#183`) — reserving it is what keeps a hinted request
/// from being misread as a queryable-equality predicate (and 400ing on a
/// collection that declares no queryable of that name), which would defeat
/// the hint vocabulary's "a typo never 400s" rule.
const RESERVED_ITEMS_PARAM_NAMES: &[&str] = &[
    "limit",
    "bbox",
    "datetime",
    "token",
    "filter",
    "filter-lang",
    "offset",
    "crs",
    "bbox-crs",
    "filter-crs",
    "hints",
];

/// Every `raw` query pair whose name is not one of [`RESERVED_ITEMS_PARAM_NAMES`]
/// — those keep their existing fixed meaning (or, for a reserved name this
/// crate doesn't implement, stay silently inert exactly as before `#52`) and
/// are never considered for the queryable-equality mechanism below.
/// Borrowed, not owned: `raw` (built from the request's full query string,
/// `handlers::list_items`) already outlives the call.
pub fn queryable_query_pairs(raw: &BTreeMap<String, String>) -> Vec<(&str, &str)> {
    raw.iter()
        .filter(|(name, _)| !RESERVED_ITEMS_PARAM_NAMES.contains(&name.as_str()))
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Builds the implicit equality [`Filter`] Requirement 4D describes ("the
/// response SHALL only include resources that match the provided value for
/// the queryable") for every `(name, value)` pair [`queryable_query_pairs`]
/// returned. `queryable_types` is `queryables::queryable_property_types`'s
/// output — the same source of truth the queryables document itself is
/// built from, so a name only reaches here at all when it is a real,
/// currently-declared queryable for this collection (closed-schema
/// narrowing included, since `queryable_types` already applies it).
///
/// A `name` absent from `queryable_types` is a query parameter "that is not
/// specified in the API definition" — OGC API Features Part 1 Core
/// Requirement 8 (`/req/core/query-param-unknown`) mandates a 400 for
/// exactly that case, and Part 3's own requirements class leaves the
/// question open, so this crate follows Core's rule rather than silently
/// ignoring the parameter. Every value is coerced against its queryable's
/// declared type ([`coerce_queryable_value`]); a value that doesn't fit also
/// fails with [`Error::Invalid`], naming `name`.
///
/// Multiple pairs AND together (`Filter::And`) in the order `pairs` lists
/// them; a single pair returns a bare `Filter::Compare`, never a
/// one-element `And`, matching the CQL2 parser's own flattening
/// (`filter::parse_text`'s `parse_and`).
pub fn build_queryable_filter(
    pairs: &[(&str, &str)],
    queryable_types: &BTreeMap<String, PropertyType>,
) -> Result<Option<Filter>> {
    let mut predicates = Vec::with_capacity(pairs.len());
    for (name, value) in pairs {
        let type_ = queryable_types.get(*name).copied().ok_or_else(|| {
            Error::Invalid(format!(
                "query parameter '{name}' is not a declared queryable for this collection"
            ))
        })?;
        predicates.push(Filter::Compare {
            property: (*name).to_string(),
            op: CompareOp::Eq,
            value: coerce_queryable_value(name, value, type_)?,
        });
    }
    Ok(match predicates.len() {
        0 => None,
        1 => predicates.pop(),
        _ => Some(Filter::And(predicates)),
    })
}

/// Coerces `raw` (a query parameter's string value, always text on the
/// wire) to `type_`'s `Filter` [`Literal`] shape, or `Error::Invalid` naming
/// `name` when it doesn't fit — Requirement 4C's "the collection SHALL
/// support a query parameter ... with the same schema as the schema of the
/// queryable" read as a runtime type check. `PropertyType::Integer` requires
/// a whole number (`i64`); `PropertyType::Number` accepts any finite
/// decimal. `PropertyType::Boolean` accepts exactly `true`/`false` (no
/// `1`/`0`/`yes`/`no` aliasing — the same literal spelling CQL2-text's own
/// `TRUE`/`FALSE` keywords lowercase to). `PropertyType::String`/`::Date`/
/// `::DateTime` all pass through as `Literal::Text`: the latter two are
/// JSON-Schema `string`s with a `format` (`PropertyType::json_schema_shape`),
/// and `Filter::Compare` against this crate's datetime column already
/// accepts a plain text literal the same way an ordinary `filter=observed_at
/// = '...'` expression does — no separate date-parsing/validation is added
/// here.
fn coerce_queryable_value(name: &str, raw: &str, type_: PropertyType) -> Result<Literal> {
    match type_ {
        PropertyType::String | PropertyType::Date | PropertyType::DateTime => {
            Ok(Literal::Text(raw.to_string()))
        }
        PropertyType::Integer => raw
            .parse::<i64>()
            .map(|n| Literal::Number(n as f64))
            .map_err(|_| {
                Error::Invalid(format!(
                    "query parameter '{name}' must be an integer, got '{raw}'"
                ))
            }),
        PropertyType::Number => raw.parse::<f64>().map(Literal::Number).map_err(|_| {
            Error::Invalid(format!(
                "query parameter '{name}' must be a number, got '{raw}'"
            ))
        }),
        PropertyType::Boolean => match raw {
            "true" => Ok(Literal::Bool(true)),
            "false" => Ok(Literal::Bool(false)),
            _ => Err(Error::Invalid(format!(
                "query parameter '{name}' must be 'true' or 'false', got '{raw}'"
            ))),
        },
    }
}

fn parse_limit(limit: Option<u32>) -> Result<u32> {
    parse_bounded_limit(limit, DEFAULT_LIMIT, MAX_LIMIT)
}

/// Builds an href for `path` echoing the parsed items-query params plus
/// `queryable_pairs` (the bare `?propertyName=value` equality parameters
/// `queryable_query_pairs` returned for this same request, `#52`) — omitting
/// those would silently drop the queryable-equality narrowing from a `next`
/// link, serving a later page that doesn't match what the first page's
/// caller actually asked for. `override_token` substitutes for the page
/// token when present (the `next` link case). Absolute scheme/host are the
/// server crate's concern (proxy headers); this crate only ever emits path +
/// query.
pub fn items_href(
    path: &str,
    params: &ItemsQueryParams,
    queryable_pairs: &[(&str, &str)],
    override_token: Option<&str>,
) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(limit) = params.limit {
        pairs.push(("limit", limit.to_string()));
    }
    if let Some(bbox) = &params.bbox {
        pairs.push(("bbox", bbox.clone()));
    }
    if let Some(bbox_crs) = &params.bbox_crs {
        pairs.push(("bbox-crs", bbox_crs.clone()));
    }
    if let Some(datetime) = &params.datetime {
        pairs.push(("datetime", datetime.clone()));
    }
    if let Some(crs) = &params.crs {
        pairs.push(("crs", crs.clone()));
    }
    if let Some(filter) = &params.filter {
        pairs.push(("filter", filter.clone()));
    }
    if let Some(filter_lang) = &params.filter_lang {
        pairs.push(("filter-lang", filter_lang.clone()));
    }
    if let Some(filter_crs) = &params.filter_crs {
        // `#217`: a `next` link that dropped `filter-crs` would evaluate
        // page two's filter geometry in a different CRS than page one's —
        // the same silent wrong-CRS evaluation this parameter exists to
        // prevent, reintroduced one page later.
        pairs.push(("filter-crs", filter_crs.clone()));
    }
    if let Some(hints) = &params.hints {
        // Echoed raw (`#183`), unrecognized tokens included: a `next` link
        // that silently dropped the hint would reorder the read chain out
        // from under a paging client mid-scroll.
        pairs.push(("hints", hints.clone()));
    }
    for (name, value) in queryable_pairs {
        pairs.push((name, (*value).to_string()));
    }
    let token = override_token
        .map(str::to_string)
        .or_else(|| params.token.clone());
    if let Some(token) = token {
        pairs.push(("token", token));
    }

    if pairs.is_empty() {
        return path.to_string();
    }

    let query = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

// -- /collections listing paging (`#42`) -------------------------------

/// `GET /collections`' own query parameters — deliberately just `limit`/
/// `token`, the same two names `ItemsQueryParams` uses for the same
/// concepts, since both are cursor pages over the same kind of paging
/// discipline (never OFFSET).
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct CollectionsQueryParams {
    pub limit: Option<u32>,
    /// Opaque keyset paging cursor — the previous page's `next` link's
    /// token, echoed back verbatim. Same semantics as
    /// `ItemsQueryParams::token`; see `tellurion_core::registry`'s module
    /// doc for what the cursor actually encodes.
    pub token: Option<String>,
}

/// Parses `GET /collections`' query parameters straight into a
/// `PageRequest` (`tellurion_core::registry`) — the registry seam's own
/// page-request shape, so this crate never invents a second one just to
/// re-map it one call later.
pub fn parse_collections_query(raw: &CollectionsQueryParams) -> Result<PageRequest> {
    let limit = parse_collections_limit(raw.limit)?;
    Ok(PageRequest {
        limit,
        after: raw.token.clone(),
    })
}

fn parse_collections_limit(limit: Option<u32>) -> Result<u32> {
    parse_bounded_limit(limit, COLLECTIONS_DEFAULT_LIMIT, COLLECTIONS_MAX_LIMIT)
}

/// Builds an href for `path` echoing `params`, with `override_token`
/// substituted for the page token when present (the `next` link case) — the
/// `/collections`-listing counterpart of [`items_href`].
pub fn collections_href(
    path: &str,
    params: &CollectionsQueryParams,
    override_token: Option<&str>,
) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(limit) = params.limit {
        pairs.push(("limit", limit.to_string()));
    }
    let token = override_token
        .map(str::to_string)
        .or_else(|| params.token.clone());
    if let Some(token) = token {
        pairs.push(("token", token));
    }

    if pairs.is_empty() {
        return path.to_string();
    }

    let query = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_is_ten() {
        assert_eq!(parse_limit(None).unwrap(), DEFAULT_LIMIT);
    }

    #[test]
    fn zero_limit_is_invalid() {
        assert!(matches!(parse_limit(Some(0)), Err(Error::Invalid(_))));
    }

    #[test]
    fn limit_above_max_is_clamped() {
        assert_eq!(parse_limit(Some(50_000)).unwrap(), MAX_LIMIT);
    }

    #[test]
    fn bbox_parses_four_numbers() {
        assert_eq!(parse_bbox("1,2,3,4").unwrap(), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn bbox_rejects_wrong_count() {
        assert!(matches!(parse_bbox("1,2,3"), Err(Error::Invalid(_))));
    }

    #[test]
    fn bbox_rejects_non_numeric() {
        assert!(matches!(parse_bbox("a,2,3,4"), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_single_instant() {
        let range = parse_datetime("2020-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(range.end.as_deref(), Some("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_closed_interval() {
        let range = parse_datetime("2020-01-01T00:00:00Z/2021-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(range.end.as_deref(), Some("2021-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_open_start() {
        let range = parse_datetime("../2021-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start, None);
        assert_eq!(range.end.as_deref(), Some("2021-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_open_end() {
        let range = parse_datetime("2020-01-01T00:00:00Z/..").unwrap();
        assert_eq!(range.start.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(range.end, None);
    }

    #[test]
    fn datetime_rejects_double_open() {
        assert!(matches!(parse_datetime("../.."), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_rejects_extra_slashes() {
        assert!(matches!(parse_datetime("a/b/c"), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_rejects_a_syntactically_invalid_single_instant() {
        assert!(matches!(parse_datetime("notadate"), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_accepts_fractional_seconds_and_a_numeric_offset() {
        assert!(parse_datetime("2020-01-01T00:00:00.123Z").is_ok());
        assert!(parse_datetime("2020-01-01T00:00:00+02:00").is_ok());
    }

    #[test]
    fn datetime_rejects_out_of_range_month() {
        assert!(matches!(
            parse_datetime("2020-13-01T00:00:00Z"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn items_href_round_trips_token_override() {
        let params = ItemsQueryParams {
            limit: Some(5),
            token: Some("orig".to_string()),
            ..ItemsQueryParams::default()
        };
        let self_href = items_href("/collections/demo/items", &params, &[], None);
        assert_eq!(self_href, "/collections/demo/items?limit=5&token=orig");

        let next_href = items_href("/collections/demo/items", &params, &[], Some("next-token"));
        assert_eq!(
            next_href,
            "/collections/demo/items?limit=5&token=next-token"
        );
    }

    // -- filter / filter-lang (`#33`) ----------------------------------------

    #[test]
    fn filter_defaults_to_cql2_text_when_filter_lang_is_omitted() {
        let raw = ItemsQueryParams {
            filter: Some("name = 'a'".to_string()),
            ..ItemsQueryParams::default()
        };
        let query = parse_items_query(&raw).unwrap();
        assert!(query.filter.is_some());
    }

    #[test]
    fn filter_lang_cql2_json_is_accepted_explicitly() {
        let raw = ItemsQueryParams {
            filter: Some(r#"{"op":"=","args":[{"property":"name"},"a"]}"#.to_string()),
            filter_lang: Some("cql2-json".to_string()),
            ..ItemsQueryParams::default()
        };
        let query = parse_items_query(&raw).unwrap();
        assert!(query.filter.is_some());
    }

    #[test]
    fn unsupported_filter_lang_is_rejected() {
        let raw = ItemsQueryParams {
            filter: Some("name = 'a'".to_string()),
            filter_lang: Some("cql2-xml".to_string()),
            ..ItemsQueryParams::default()
        };
        assert!(matches!(parse_items_query(&raw), Err(Error::Invalid(_))));
    }

    #[test]
    fn a_syntactically_invalid_filter_is_rejected() {
        let raw = ItemsQueryParams {
            filter: Some("name = ".to_string()),
            ..ItemsQueryParams::default()
        };
        assert!(matches!(parse_items_query(&raw), Err(Error::Invalid(_))));
    }

    #[test]
    fn no_filter_parameter_leaves_the_query_filter_none() {
        let raw = ItemsQueryParams::default();
        let query = parse_items_query(&raw).unwrap();
        assert!(query.filter.is_none());
    }

    // -- /collections listing paging (`#42`) ---------------------------------

    #[test]
    fn collections_default_limit_is_used_when_omitted() {
        let query = parse_collections_query(&CollectionsQueryParams::default()).unwrap();
        assert_eq!(query.limit, COLLECTIONS_DEFAULT_LIMIT);
        assert_eq!(query.after, None);
    }

    #[test]
    fn collections_zero_limit_is_invalid() {
        let raw = CollectionsQueryParams {
            limit: Some(0),
            ..CollectionsQueryParams::default()
        };
        assert!(matches!(
            parse_collections_query(&raw),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn collections_limit_above_max_is_clamped() {
        let raw = CollectionsQueryParams {
            limit: Some(50_000),
            ..CollectionsQueryParams::default()
        };
        let query = parse_collections_query(&raw).unwrap();
        assert_eq!(query.limit, COLLECTIONS_MAX_LIMIT);
    }

    #[test]
    fn collections_query_carries_the_token_through_as_the_cursor() {
        let raw = CollectionsQueryParams {
            token: Some("bravo".to_string()),
            ..CollectionsQueryParams::default()
        };
        let query = parse_collections_query(&raw).unwrap();
        assert_eq!(query.after.as_deref(), Some("bravo"));
    }

    #[test]
    fn collections_href_round_trips_token_override() {
        let params = CollectionsQueryParams {
            limit: Some(5),
            token: Some("orig".to_string()),
        };
        let self_href = collections_href("/collections", &params, None);
        assert_eq!(self_href, "/collections?limit=5&token=orig");

        let next_href = collections_href("/collections", &params, Some("next-token"));
        assert_eq!(next_href, "/collections?limit=5&token=next-token");
    }

    #[test]
    fn collections_href_is_bare_path_with_no_params() {
        let href = collections_href("/collections", &CollectionsQueryParams::default(), None);
        assert_eq!(href, "/collections");
    }

    #[test]
    fn items_href_echoes_filter_and_filter_lang() {
        let params = ItemsQueryParams {
            filter: Some("name = 'a'".to_string()),
            filter_lang: Some("cql2-text".to_string()),
            ..ItemsQueryParams::default()
        };
        let href = items_href("/collections/demo/items", &params, &[], None);
        assert_eq!(
            href,
            "/collections/demo/items?filter=name%20%3D%20%27a%27&filter-lang=cql2-text"
        );
    }

    /// `#183`: a `next` link must carry the request's `?hints=` through
    /// verbatim, or page two would silently resolve a different chain order
    /// than page one.
    #[test]
    fn items_href_echoes_hints_verbatim() {
        let params = ItemsQueryParams {
            hints: Some("prefer:alt,unknown-token".to_string()),
            token: Some("tok".to_string()),
            ..ItemsQueryParams::default()
        };
        let href = items_href("/collections/demo/items", &params, &[], Some("next-token"));
        assert_eq!(
            href,
            "/collections/demo/items?hints=prefer%3Aalt%2Cunknown-token&token=next-token"
        );
    }

    // -- filter-crs (Part 3 Filtering Req 7/Req 8, `#217`) -----------------

    /// Campaign rule 1: no `filter-crs` on the wire means
    /// `RequestedCrs::Omitted` on the query — the value every driver
    /// compiles exactly the way it did before `#217` — whatever the storage
    /// SRID is.
    #[test]
    fn an_absent_filter_crs_resolves_to_omitted_for_every_storage_srid() {
        for storage_srid in [None, Some(4326), Some(3857)] {
            let raw = ItemsQueryParams::default();
            let mut query = parse_items_query(&raw).unwrap();
            let resolved = resolve_items_crs(&raw, &mut query, storage_srid).unwrap();
            assert_eq!(resolved.filter_crs, RequestedCrs::Omitted);
            assert_eq!(query.filter_crs, RequestedCrs::Omitted);
        }
    }

    #[test]
    fn filter_crs_resolves_through_the_same_seam_as_crs_and_bbox_crs() {
        let raw = ItemsQueryParams {
            filter_crs: Some(crs::epsg_uri(3857)),
            ..ItemsQueryParams::default()
        };
        let mut query = parse_items_query(&raw).unwrap();
        let resolved = resolve_items_crs(&raw, &mut query, Some(3857)).unwrap();
        assert_eq!(resolved.filter_crs, RequestedCrs::Storage);
        assert_eq!(query.filter_crs, RequestedCrs::Storage);

        // Same URI, a collection that doesn't store in it: refused here, so
        // a driver can never be handed a CRS the collection never advertised.
        let mut query = parse_items_query(&raw).unwrap();
        assert!(matches!(
            resolve_items_crs(&raw, &mut query, Some(4326)),
            Err(Error::Invalid(_))
        ));
    }

    /// `filter-crs` resolving to the storage CRS must not drag `bbox`'s axis
    /// swap along with it — the swap belongs to `bbox-crs` alone, and a
    /// request that named only `filter-crs` never said anything about how
    /// its `bbox`'s four numbers are ordered.
    #[test]
    fn a_storage_filter_crs_does_not_axis_swap_the_bbox() {
        let raw = ItemsQueryParams {
            bbox: Some("9,44,10.5,45.5".to_string()),
            filter_crs: Some(crs::epsg_uri(4326)),
            ..ItemsQueryParams::default()
        };
        let mut query = parse_items_query(&raw).unwrap();
        resolve_items_crs(&raw, &mut query, Some(4326)).unwrap();
        assert_eq!(query.bbox, Some([9.0, 44.0, 10.5, 45.5]));
        assert_eq!(query.bbox_crs, RequestedCrs::Omitted);
    }

    #[test]
    fn items_href_echoes_filter_crs() {
        let params = ItemsQueryParams {
            filter: Some("S_INTERSECTS(geom,BBOX(1,2,3,4))".to_string()),
            filter_crs: Some(crs::epsg_uri(4326)),
            ..ItemsQueryParams::default()
        };
        let href = items_href("/collections/demo/items", &params, &[], Some("next"));
        assert!(
            href.contains("filter-crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326"),
            "href was: {href}"
        );
        assert!(href.contains("token=next"), "href was: {href}");
    }

    /// `filter-crs` stays a reserved `/items` parameter name (`#52`): now
    /// that it is genuinely implemented it must still never be offered to
    /// the queryable-equality mechanism, or a collection declaring a
    /// queryable of that name would shadow the protocol parameter.
    #[test]
    fn filter_crs_is_not_offered_to_the_queryable_equality_mechanism() {
        let mut raw = BTreeMap::new();
        raw.insert("filter-crs".to_string(), crs::epsg_uri(4326));
        raw.insert("population".to_string(), "20".to_string());
        assert_eq!(queryable_query_pairs(&raw), vec![("population", "20")]);
    }

    #[test]
    fn items_href_echoes_queryable_pairs_between_filter_lang_and_token() {
        let params = ItemsQueryParams {
            filter: Some("name = 'a'".to_string()),
            token: Some("tok".to_string()),
            ..ItemsQueryParams::default()
        };
        let href = items_href(
            "/collections/demo/items",
            &params,
            &[("population", "20")],
            None,
        );
        assert_eq!(
            href,
            "/collections/demo/items?filter=name%20%3D%20%27a%27&population=20&token=tok"
        );
    }
}
