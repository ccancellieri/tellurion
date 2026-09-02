//! `GET`/`POST /search` request parsing and paging-token codec (`#36` slice
//! C, STAC API - Item Search). Pure, I/O-free — same contract `params.rs`
//! documents for itself: property/capability validation against a resolved
//! collection needs data this module doesn't have and is `handlers.rs`'s
//! job.
//!
//! Verified 2026-07 against `stac-api-spec`'s `item-search/README.md` at the
//! `v1.0.0` tag, plus the STAC API Filter Extension's `README.md` at the
//! `stac-api-extensions/filter` repo's `v1.0.0-rc.4` tag (the filter
//! extension has not reached a non-prerelease release):
//!
//! - `collections`/`ids` are arrays: GET encodes them as a comma-separated
//!   string with no brackets and no whitespace ("query parameters must use
//!   comma-separated string values"); POST encodes them as a JSON array.
//! - `bbox`/`datetime`/`limit` are the same OGC API Features parameters
//!   `/items` already parses (`params.rs`, reused here — see
//!   [`crate::params::parse_bbox`]/[`crate::params::parse_datetime`]/
//!   [`crate::params::parse_limit`]).
//! - `intersects` is a GeoJSON Geometry. The spec leaves its GET encoding
//!   unspecified beyond noting POST is recommended "especially when using
//!   the `intersects` query parameter" (request-size concerns) — this
//!   implementation's own choice, undocumented upstream: a GET `intersects`
//!   value is the geometry's JSON text, percent-encoded like any other query
//!   value (axum's `Query` extractor already percent-decodes it).
//! - "Only one of either `intersects` or `bbox` may be specified. If both
//!   are specified, a 400 Bad Request status code must be returned" — quoted
//!   verbatim from `item-search/README.md`; enforced by
//!   [`reject_bbox_and_intersects_together`].
//! - `filter`/`filter-lang`/`filter-crs`: "three GET query parameters or POST
//!   JSON fields" — the extension names all three together, and spells each
//!   of them identically in both encodings, so `filter-crs` is the hyphenated
//!   spelling on a GET query string AND the hyphenated key of a POST body
//!   field, exactly as `filter-lang` already is. Default `filter-lang` is
//!   `cql2-text` on GET, `cql2-json` on POST — quoted from the filter
//!   extension README: "defaults to `cql2-text` for a GET request and
//!   `cql2-json` for a POST request." `filter-crs` is CRS84 on both, and its
//!   accepted value space is CRS84 alone — see
//!   [`resolve_search_filter_crs`] for the verbatim requirement and `#248`.
//! - `q` (`#181`): a free-text query, a plain string on GET and POST alike,
//!   served exclusively by the collection's derived search index (never
//!   approximated by the main chain — `handlers::run_q_search`'s doc
//!   carries the dispatch rules). This deliberately does NOT claim the STAC
//!   API Free Text Search extension's conformance class: that extension's
//!   own comma/quoting term syntax is not implemented — `q` travels
//!   verbatim into PostgreSQL's `websearch_to_tsquery` (space-separated
//!   terms AND, quoted phrases, `or`), and claiming the class while
//!   tokenizing differently would be conformance theater. [`validate_q`]
//!   refuses, by name, every parameter combination the index entry cannot
//!   express.
//!
//! `ids` is intentionally NOT composed into the same `Filter`/SQL query path
//! as `bbox`/`datetime`/`filter`/`intersects` — see `handlers.rs`'s own
//! `execute_search` doc for why (the primary key is not a filterable
//! property in this system's model) and how it's served instead (direct
//! per-id `FeatureSource::item` lookups, its own paging mode encoded in
//! [`SearchToken::Ids`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use tellurion_core::{crs, filter, DatetimeRange, Error, Filter, RequestedCrs, Result};

use crate::params::{parse_bbox, parse_datetime, parse_limit, percent_encode};

/// `GET /search` query parameters. Every array-valued STAC parameter
/// (`collections`, `ids`) stays a raw `String` here — [`split_csv`] does the
/// comma-splitting `parse_get` needs, matching how `bbox`/`datetime` already
/// stay raw strings in `crate::params::ItemsQueryParams` until their own
/// parse step.
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct SearchQueryParams {
    pub collections: Option<String>,
    pub ids: Option<String>,
    pub bbox: Option<String>,
    pub datetime: Option<String>,
    pub intersects: Option<String>,
    pub limit: Option<u32>,
    pub token: Option<String>,
    pub filter: Option<String>,
    #[serde(rename = "filter-lang")]
    pub filter_lang: Option<String>,
    /// The CRS every spatial literal inside `filter` is expressed in (`#248`,
    /// STAC API Filter Extension). Kept as a raw URI string here and resolved
    /// by [`resolve_search_filter_crs`], which is where the extension's own
    /// CRS84-only value space is enforced — spelled `filter-crs` on the query
    /// string, the same name [`SearchBody`] uses for the POST body field.
    #[serde(rename = "filter-crs")]
    pub filter_crs: Option<String>,
    /// Free-text query (`#181`), served exclusively by the collection's
    /// derived search index — see [`validate_q`] for the combinations this
    /// slice refuses by name rather than approximating.
    pub q: Option<String>,
}

/// `POST /search` JSON body. `intersects`/`filter` stay `serde_json::Value`:
/// `intersects` is always a GeoJSON geometry object, and `filter`'s shape
/// depends on `filter_lang` (a JSON string for `cql2-text`, a JSON object
/// for `cql2-json`) — see [`parse_post`].
///
/// `#248`: `filter-crs` is a real field now. Before this slice it was the one
/// parameter of the Filter Extension's "three GET query parameters or POST
/// JSON fields" with nowhere to land, so a client that declared one had it
/// dropped on the floor and its filter's geometries processed in CRS84
/// regardless — accepted, ignored, and answered `200` with rows selected in a
/// CRS the client never asked for. It is spelled `filter-crs` here, hyphenated
/// exactly as on the GET query string and exactly as `filter-lang` beside it:
/// the extension gives one name per parameter and does not re-spell any of
/// them for the JSON body.
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct SearchBody {
    pub collections: Option<Vec<String>>,
    pub ids: Option<Vec<String>>,
    pub bbox: Option<Vec<f64>>,
    pub datetime: Option<String>,
    pub intersects: Option<Value>,
    pub limit: Option<u32>,
    pub token: Option<String>,
    pub filter: Option<Value>,
    #[serde(rename = "filter-lang")]
    pub filter_lang: Option<String>,
    /// The CRS every spatial literal inside `filter` is expressed in (`#248`)
    /// — a plain JSON string, same raw-URI value space as the GET parameter,
    /// resolved by the same [`resolve_search_filter_crs`].
    #[serde(rename = "filter-crs")]
    pub filter_crs: Option<String>,
    /// Free-text query (`#181`) — a plain JSON string on POST, same value
    /// space as the GET parameter.
    pub q: Option<String>,
}

/// The two request shapes (`SearchQueryParams`/`SearchBody`) normalized to
/// one representation `handlers::execute_search` works from — same "parse
/// once, execute once" split `crate::params::parse_items_query` already
/// establishes for `/items`. `collections`/`ids` are external ids; empty
/// means "unrestricted" (every collection this catalog owns; no explicit id
/// narrowing), never `None` — an always-a-`Vec` shape is simpler for the
/// caller than an `Option<Vec<_>>` whose `None` and `Some(vec![])` would
/// otherwise mean the same thing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchRequest {
    pub collections: Vec<String>,
    pub ids: Vec<String>,
    pub bbox: Option<[f64; 4]>,
    pub datetime: Option<DatetimeRange>,
    pub intersects: Option<Value>,
    pub filter: Option<Filter>,
    /// The CRS `filter`'s (and `intersects`') own spatial literals are
    /// expressed in, already resolved against the extension's CRS84-only value
    /// space by [`resolve_search_filter_crs`] (`#248`). Only ever
    /// [`RequestedCrs::Omitted`] (no `filter-crs` on the wire — byte-for-byte
    /// this crate's pre-`#248` behaviour) or [`RequestedCrs::Crs84`]; a value
    /// naming anything else never becomes a `SearchRequest` at all.
    ///
    /// [`RequestedCrs::Storage`] is deliberately unreachable here, unlike on
    /// `tellurion-features`' `/items` lane where it is the interesting case:
    /// `/search` has no single collection whose storage CRS a URI could name
    /// (it fans out across every collection a catalog owns), and the extension
    /// pins the value space to CRS84 anyway. `handlers::run_cursor_search`
    /// copies this straight onto each collection's own
    /// `tellurion_core::ItemsQuery::filter_crs`.
    pub filter_crs: RequestedCrs,
    pub limit: u32,
    pub token: Option<String>,
    /// Free-text query (`#181`), kept verbatim — the PostGIS search plan
    /// binds it as a parameter through `websearch_to_tsquery`'s forgiving
    /// parser, so no tsquery syntax is interpreted (or interpretable) here.
    /// Always index-lane-only: [`validate_q`] has already refused, by name,
    /// every combination the search chain's index entry cannot express, so
    /// a `Some` here reaches `handlers::run_q_search` with `limit` and
    /// `collections` as its only companions.
    pub q: Option<String>,
}

/// Splits a comma-separated STAC array parameter per the item-search GET
/// encoding rule quoted in this module's doc comment: no brackets, no
/// whitespace between values. Trims incidental whitespace anyway (a lenient
/// superset of the spec, not a stricter one) and drops empty segments so a
/// trailing comma or an empty parameter both produce `[]`, not `[""]`.
fn split_csv(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

/// "Only one of either `intersects` or `bbox` may be specified. If both are
/// specified, a 400 Bad Request status code must be returned" — quoted
/// verbatim from `item-search/README.md` (`v1.0.0`).
fn reject_bbox_and_intersects_together(has_bbox: bool, has_intersects: bool) -> Result<()> {
    if has_bbox && has_intersects {
        return Err(Error::Invalid(
            "only one of 'bbox' or 'intersects' may be specified".to_string(),
        ));
    }
    Ok(())
}

/// `#181`'s first agreement gate, applied at parse time so GET and POST
/// refuse identically, before any routing runs: a free-text `q` is served
/// exclusively by the search chain's derived-index entry, and this slice's
/// `SearchQuery` expresses nothing beyond `q` + `limit` on that entry — so
/// a request combining `q` with any other predicate (`bbox`, `datetime`,
/// `intersects`, `filter`, `ids`) is expressible on NEITHER chain: the
/// index cannot apply the extra predicate, and the main chain cannot serve
/// free text. Dropping either half would make the feature set (and any
/// `numberMatched`) describe a different selection than the request named,
/// so the whole request is refused by name instead — never approximated,
/// never silently narrowed. `token` is refused too: `q`-mode is single-page
/// (the derived-index query has no cursor), so no genuine `q` continuation
/// token exists for a client to hold. A `q` that is empty after trimming is
/// its own named refusal rather than a silently-dropped predicate. A second
/// consequence, worth stating once: because `q` can never ride alongside
/// `filter`, there are no query properties to validate against queryables
/// in `q`-mode, so validation outcome is trivially identical no matter
/// which chain entry would have served (the issue's second gate).
fn validate_q(request: &SearchRequest) -> Result<()> {
    let Some(q) = request.q.as_deref() else {
        return Ok(());
    };
    if q.trim().is_empty() {
        return Err(Error::Invalid(
            "'q' must contain at least one non-whitespace character".to_string(),
        ));
    }
    let unexpressible = [
        (request.bbox.is_some(), "bbox"),
        (request.datetime.is_some(), "datetime"),
        (request.intersects.is_some(), "intersects"),
        (request.filter.is_some(), "filter"),
        (!request.ids.is_empty(), "ids"),
        (request.token.is_some(), "token"),
    ];
    if let Some((_, parameter)) = unexpressible.iter().find(|(present, _)| *present) {
        return Err(Error::Invalid(format!(
            "'q' cannot be combined with '{parameter}': free-text search is served only by the \
             derived search index, which cannot express the combined predicate set, and \
             dropping either predicate would misrepresent the result"
        )));
    }
    Ok(())
}

/// Resolves a raw `filter-crs` (`#248`) — the STAC `/search` lane's whole
/// value space, shared by GET and POST so the two can never diverge on which
/// CRSs a filter's geometries may be declared in.
///
/// The STAC API Filter Extension is far more restrictive about this parameter
/// than OGC API — Features Part 3 is about the identically-named one on
/// `/items`, and this is the seam where that difference lives. Verified 2026-08
/// against `stac-api-extensions/filter`'s `README.md` at the `v1.0.0-rc.4` tag
/// this module already pins, quoted verbatim:
///
/// - "filter-crs: recommended to not be passed, but server must only accept
///   `http://www.opengis.net/def/crs/OGC/1.3/CRS84` as a valid value, may
///   reject any others"
/// - "The parameter `filter-crs` always defaults to
///   `http://www.opengis.net/def/crs/OGC/1.3/CRS84` for a STAC API"
///
/// So: `None` — the parameter was not supplied — is [`RequestedCrs::Omitted`],
/// which every driver in this workspace compiles byte-for-byte the way it did
/// before `#217`/`#248` (and which is already CRS84 for every collection whose
/// storage is CRS84, i.e. the extension's stated default). CRS84 named
/// explicitly is [`RequestedCrs::Crs84`], genuinely honoured: on a collection
/// stored in a projected CRS that is a real `ST_Transform` of the filter's
/// spatial literals, not a no-op (`tellurion-postgis::sql::geometry_literal_expr`).
/// Every other value — including the URI of some collection's own storage CRS,
/// which is exactly what a Part 3 client would send to `/items` — is refused
/// **by name**, with a 400 naming both `filter-crs` and the one value this lane
/// accepts.
///
/// Refusing rather than honouring the storage-CRS case is not a shortcut, and
/// it is the half of `#248` that cannot be reused from `#217`. `/search` is
/// cross-collection: one `filter-crs` URI would have to be resolved against
/// every candidate collection's own supported CRS set
/// (`tellurion_core::crs::resolve`), and a URI naming one collection's storage
/// CRS is by construction *not* in another's unless the two happen to share an
/// SRID. The same request would then be honourable on one collection and
/// refusable on the next, and silently dropping the collections it did not fit
/// would be a fresh instance of exactly the silent degradation this issue was
/// opened for. There is no per-request answer to give, so the parameter's value
/// space stays where the extension put it.
///
/// Deliberately NOT refused: a `filter-crs` supplied without a `filter` or
/// `intersects`. It selects nothing differently (there are no geometries in a
/// filter expression to process), the extension attaches no such condition, and
/// inventing one would be inventing a rule.
fn resolve_search_filter_crs(raw: Option<&str>) -> Result<RequestedCrs> {
    match raw {
        None => Ok(RequestedCrs::Omitted),
        Some(uri) if uri == crs::CRS84_URI => Ok(RequestedCrs::Crs84),
        Some(uri) => Err(Error::Invalid(format!(
            "unsupported 'filter-crs' '{uri}': /search processes a filter's geometries in {} \
             only",
            crs::CRS84_URI
        ))),
    }
}

/// Structural check that `value` is shaped like a GeoJSON Geometry object —
/// same minimal rule `tellurion_core::filter`'s own CQL2-JSON `s_intersects`
/// parsing applies to a geometry literal (an object carrying a `type` key);
/// this crate does not parse or validate GeoJSON beyond that, matching
/// `tellurion_core::filter::GeometryLiteral::GeoJson`'s own "kept as opaque
/// JSON" contract.
fn validate_geometry_shape(value: Value) -> Result<Value> {
    let is_geometry_shaped = value
        .as_object()
        .is_some_and(|obj| obj.contains_key("type"));
    if is_geometry_shaped {
        Ok(value)
    } else {
        Err(Error::Invalid(
            "'intersects' must be a GeoJSON geometry object".to_string(),
        ))
    }
}

/// Parses a `GET /search` request. See this module's doc comment for the
/// per-parameter encoding rules.
pub fn parse_get(raw: &SearchQueryParams) -> Result<SearchRequest> {
    let collections = split_csv(raw.collections.as_deref());
    let ids = split_csv(raw.ids.as_deref());
    let bbox = raw.bbox.as_deref().map(parse_bbox).transpose()?;
    let datetime = raw.datetime.as_deref().map(parse_datetime).transpose()?;
    let intersects = raw
        .intersects
        .as_deref()
        .map(|text| {
            let value: Value = serde_json::from_str(text)
                .map_err(|e| Error::Invalid(format!("'intersects' is not valid JSON: {e}")))?;
            validate_geometry_shape(value)
        })
        .transpose()?;
    reject_bbox_and_intersects_together(bbox.is_some(), intersects.is_some())?;
    let limit = parse_limit(raw.limit)?;
    let filter = raw
        .filter
        .as_deref()
        .map(|text| {
            let lang = raw
                .filter_lang
                .as_deref()
                .unwrap_or(filter::FILTER_LANG_CQL2_TEXT);
            filter::parse(lang, text)
        })
        .transpose()?;
    let filter_crs = resolve_search_filter_crs(raw.filter_crs.as_deref())?;

    let request = SearchRequest {
        collections,
        ids,
        bbox,
        datetime,
        intersects,
        filter,
        filter_crs,
        limit,
        token: raw.token.clone(),
        q: raw.q.clone(),
    };
    validate_q(&request)?;
    Ok(request)
}

/// STAC bbox is 2D-or-3D ("length must be 2*n"); this codebase's bbox
/// concept is 2D-only everywhere it appears (`tellurion_core::ItemsQuery::bbox:
/// Option<[f64; 4]>`, unconditionally, since before this slice) — a 3D bbox
/// is a documented, pre-existing limitation, not a new one introduced here.
fn bbox_from_array(raw: &[f64]) -> Result<[f64; 4]> {
    <[f64; 4]>::try_from(raw).map_err(|_| {
        Error::Invalid(format!(
            "bbox must have exactly 4 numbers (2D); got {} (3D bbox is not supported)",
            raw.len()
        ))
    })
}

/// Parses a `POST /search` JSON body. See this module's doc comment for the
/// per-parameter encoding rules, including `filter`/`filter-lang`'s
/// GET-vs-POST default divergence.
pub fn parse_post(raw: &SearchBody) -> Result<SearchRequest> {
    let collections = raw.collections.clone().unwrap_or_default();
    let ids = raw.ids.clone().unwrap_or_default();
    let bbox = raw.bbox.as_deref().map(bbox_from_array).transpose()?;
    let datetime = raw.datetime.as_deref().map(parse_datetime).transpose()?;
    let intersects = raw
        .intersects
        .clone()
        .map(validate_geometry_shape)
        .transpose()?;
    reject_bbox_and_intersects_together(bbox.is_some(), intersects.is_some())?;
    let limit = parse_limit(raw.limit)?;
    let filter = raw
        .filter
        .clone()
        .map(|value| {
            let lang = raw
                .filter_lang
                .as_deref()
                .unwrap_or(filter::FILTER_LANG_CQL2_JSON);
            // `cql2-text` sends `filter` as a JSON string; `cql2-json` sends
            // it as a JSON object/array. `filter::parse` always wants text,
            // so a non-string value is re-serialized back to its JSON text
            // rather than requiring the caller to have picked the "right"
            // shape for `lang` up front — a `lang`/shape mismatch still
            // surfaces as a clean parse error from `filter::parse` itself
            // (e.g. `cql2-text` fed a JSON object's `{...}` text).
            let text = match &value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            filter::parse(lang, &text)
        })
        .transpose()?;
    let filter_crs = resolve_search_filter_crs(raw.filter_crs.as_deref())?;

    let request = SearchRequest {
        collections,
        ids,
        bbox,
        datetime,
        intersects,
        filter,
        filter_crs,
        limit,
        token: raw.token.clone(),
        q: raw.q.clone(),
    };
    validate_q(&request)?;
    Ok(request)
}

/// A GET-query-string-shaped view of a `/search` request, used only to
/// build `self`/`next` link hrefs — always in GET form, regardless of
/// whether the original request was itself GET or POST: a `next` link is
/// always followable via a plain `GET`, the same simplification
/// `crate::params::items_href` already makes for `/items`. Field values are
/// the literal text that would appear in a URL, not `SearchRequest`'s parsed
/// types — `filter` in particular is kept as its original text/JSON rather
/// than round-tripped through the parsed [`Filter`] tree, which this crate
/// has no serializer back to CQL2 text/JSON for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchHrefParams {
    pub limit: Option<u32>,
    pub bbox: Option<String>,
    pub datetime: Option<String>,
    pub intersects: Option<String>,
    pub ids: Option<String>,
    pub collections: Option<String>,
    pub filter: Option<String>,
    pub filter_lang: Option<String>,
    /// `#248`: echoed for the same reason `tellurion-features`' `items_href`
    /// echoes it — a `next` link that dropped `filter-crs` would evaluate page
    /// two's filter geometry in a different CRS than page one's.
    pub filter_crs: Option<String>,
    pub token: Option<String>,
    pub q: Option<String>,
}

impl From<&SearchQueryParams> for SearchHrefParams {
    fn from(raw: &SearchQueryParams) -> Self {
        Self {
            limit: raw.limit,
            bbox: raw.bbox.clone(),
            datetime: raw.datetime.clone(),
            intersects: raw.intersects.clone(),
            ids: raw.ids.clone(),
            collections: raw.collections.clone(),
            filter: raw.filter.clone(),
            filter_lang: raw.filter_lang.clone(),
            filter_crs: raw.filter_crs.clone(),
            token: raw.token.clone(),
            q: raw.q.clone(),
        }
    }
}

impl From<&SearchBody> for SearchHrefParams {
    fn from(raw: &SearchBody) -> Self {
        let join_csv = |values: &[String]| (!values.is_empty()).then(|| values.join(","));
        Self {
            limit: raw.limit,
            bbox: raw.bbox.as_deref().map(|values| {
                values
                    .iter()
                    .map(f64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            }),
            datetime: raw.datetime.clone(),
            intersects: raw.intersects.as_ref().map(Value::to_string),
            ids: raw.ids.as_deref().and_then(join_csv),
            collections: raw.collections.as_deref().and_then(join_csv),
            filter: raw.filter.as_ref().map(|value| match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
            filter_lang: raw.filter_lang.clone(),
            filter_crs: raw.filter_crs.clone(),
            token: raw.token.clone(),
            q: raw.q.clone(),
        }
    }
}

/// Builds an href for `path` echoing `params`, with `override_token`
/// substituted for `params.token` when present (the `next` link case) — same
/// shape `crate::params::items_href` builds for `/items`.
pub fn search_href(path: &str, params: &SearchHrefParams, override_token: Option<&str>) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(limit) = params.limit {
        pairs.push(("limit", limit.to_string()));
    }
    if let Some(bbox) = &params.bbox {
        pairs.push(("bbox", bbox.clone()));
    }
    if let Some(datetime) = &params.datetime {
        pairs.push(("datetime", datetime.clone()));
    }
    if let Some(intersects) = &params.intersects {
        pairs.push(("intersects", intersects.clone()));
    }
    if let Some(ids) = &params.ids {
        pairs.push(("ids", ids.clone()));
    }
    if let Some(collections) = &params.collections {
        pairs.push(("collections", collections.clone()));
    }
    if let Some(filter) = &params.filter {
        pairs.push(("filter", filter.clone()));
    }
    if let Some(filter_lang) = &params.filter_lang {
        pairs.push(("filter-lang", filter_lang.clone()));
    }
    if let Some(filter_crs) = &params.filter_crs {
        pairs.push(("filter-crs", filter_crs.clone()));
    }
    if let Some(q) = &params.q {
        pairs.push(("q", q.clone()));
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

/// Opaque `/search` continuation token (`#36` slice C): base64url-encoded
/// (no padding, hand-rolled — same "no external dependency for a small,
/// self-controlled encoding" convention `params.rs`'s own `percent_encode`
/// already follows) JSON of one of two paging modes.
///
/// `Cursor` is the ordinary keyset-paging mode (bbox/datetime/filter/
/// intersects search, or a bare listing): `collections` is the stable,
/// alphabetically-sorted list of collection external ids this search is
/// walking (fixed for the lifetime of one paging sequence, computed once on
/// the first, token-less request — see `handlers::execute_search`'s doc for
/// why re-deriving it on every page would be wrong under a concurrent config
/// change), `idx` is which collection is currently being read, and `cursor`
/// is that collection's own `FeaturePage::next_token`, or `None` when a
/// fresh collection is about to start from its beginning.
///
/// `Ids` is the `ids`-narrowed search's own paging mode (`handlers.rs`'s own
/// doc explains why it never shares `Cursor`'s query path): `collections` is
/// the same stable candidate list, `ids` is the full requested id list
/// (fixed for the sequence), and `start` is the index into the cross product
/// of `(collections, ids)` — see `handlers::next_ids_page` — to resume at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum SearchToken {
    Cursor {
        collections: Vec<String>,
        idx: usize,
        cursor: Option<String>,
    },
    Ids {
        collections: Vec<String>,
        ids: Vec<String>,
        start: usize,
    },
}

impl SearchToken {
    pub fn encode(&self) -> String {
        // `SearchToken` is a plain, non-recursive struct of strings/numbers
        // — serialization can only fail for a type serde itself rejects
        // (e.g. a non-string map key), none of which this type has.
        let json = serde_json::to_vec(self).expect("SearchToken always serializes");
        base64url_encode(&json)
    }

    /// `Err(Error::Invalid)` for anything that isn't a genuine, unmodified
    /// token this crate encoded itself — a tampered or hand-written token is
    /// a client error (400), never a panic.
    pub fn decode(token: &str) -> Result<Self> {
        let bytes = base64url_decode(token)
            .ok_or_else(|| Error::Invalid("malformed search continuation token".to_string()))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::Invalid("malformed search continuation token".to_string()))
    }
}

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url (RFC 4648 section 5) — hand-rolled rather than a new
/// workspace dependency, same rationale `params.rs`'s own `percent_encode`
/// already documents: this crate only ever encodes/decodes its own small,
/// self-controlled token bytes, never arbitrary external input.
fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied();
        let b2 = chunk.get(2).copied();

        out.push(BASE64URL_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(
            BASE64URL_ALPHABET[(((b0 & 0x03) << 4) | (b1.unwrap_or(0) >> 4)) as usize] as char,
        );
        if let Some(b1) = b1 {
            out.push(
                BASE64URL_ALPHABET[(((b1 & 0x0f) << 2) | (b2.unwrap_or(0) >> 6)) as usize] as char,
            );
        }
        if let Some(b2) = b2 {
            out.push(BASE64URL_ALPHABET[(b2 & 0x3f) as usize] as char);
        }
    }
    out
}

fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }

    let chars: Vec<u8> = text.bytes().collect();
    let mut out = Vec::with_capacity(chars.len() * 3 / 4);
    for chunk in chars.chunks(4) {
        let mut values = [0u8; 4];
        let mut n = 0;
        for (i, &c) in chunk.iter().enumerate() {
            values[i] = value(c)?;
            n += 1;
        }
        out.push((values[0] << 2) | (values[1] >> 4));
        if n > 2 {
            out.push((values[1] << 4) | (values[2] >> 2));
        }
        if n > 3 {
            out.push((values[2] << 6) | values[3]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- split_csv ------------------------------------------------------

    #[test]
    fn split_csv_splits_on_commas_and_drops_empty_segments() {
        assert_eq!(
            split_csv(Some("a,b,c")),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(split_csv(Some("")), Vec::<String>::new());
        assert_eq!(
            split_csv(Some("a,,b")),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(split_csv(None), Vec::<String>::new());
    }

    // -- parse_get --------------------------------------------------------

    #[test]
    fn parse_get_defaults_to_unrestricted_collections_and_ids() {
        let req = parse_get(&SearchQueryParams::default()).unwrap();
        assert!(req.collections.is_empty());
        assert!(req.ids.is_empty());
        assert!(req.bbox.is_none());
        assert!(req.intersects.is_none());
        assert!(req.filter.is_none());
        assert_eq!(req.limit, crate::params::DEFAULT_LIMIT);
    }

    #[test]
    fn parse_get_splits_collections_and_ids() {
        let raw = SearchQueryParams {
            collections: Some("a,b".to_string()),
            ids: Some("x,y,z".to_string()),
            ..SearchQueryParams::default()
        };
        let req = parse_get(&raw).unwrap();
        assert_eq!(req.collections, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            req.ids,
            vec!["x".to_string(), "y".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn parse_get_parses_a_json_encoded_intersects_geometry() {
        let raw = SearchQueryParams {
            intersects: Some(r#"{"type":"Point","coordinates":[1.0,2.0]}"#.to_string()),
            ..SearchQueryParams::default()
        };
        let req = parse_get(&raw).unwrap();
        assert_eq!(req.intersects.unwrap()["type"], "Point");
    }

    #[test]
    fn parse_get_rejects_a_non_geometry_intersects_value() {
        let raw = SearchQueryParams {
            intersects: Some(r#"{"not":"a geometry"}"#.to_string()),
            ..SearchQueryParams::default()
        };
        assert!(matches!(parse_get(&raw), Err(Error::Invalid(_))));
    }

    #[test]
    fn parse_get_rejects_malformed_intersects_json() {
        let raw = SearchQueryParams {
            intersects: Some("not json".to_string()),
            ..SearchQueryParams::default()
        };
        assert!(matches!(parse_get(&raw), Err(Error::Invalid(_))));
    }

    #[test]
    fn parse_get_rejects_bbox_and_intersects_together() {
        let raw = SearchQueryParams {
            bbox: Some("1,2,3,4".to_string()),
            intersects: Some(r#"{"type":"Point","coordinates":[1.0,2.0]}"#.to_string()),
            ..SearchQueryParams::default()
        };
        assert!(matches!(parse_get(&raw), Err(Error::Invalid(_))));
    }

    #[test]
    fn parse_get_filter_defaults_to_cql2_text() {
        let raw = SearchQueryParams {
            filter: Some("name = 'a'".to_string()),
            ..SearchQueryParams::default()
        };
        let req = parse_get(&raw).unwrap();
        assert!(req.filter.is_some());
    }

    #[test]
    fn parse_get_filter_lang_cql2_json_is_accepted_explicitly() {
        let raw = SearchQueryParams {
            filter: Some(r#"{"op":"=","args":[{"property":"name"},"a"]}"#.to_string()),
            filter_lang: Some("cql2-json".to_string()),
            ..SearchQueryParams::default()
        };
        let req = parse_get(&raw).unwrap();
        assert!(req.filter.is_some());
    }

    // -- q (`#181`) -----------------------------------------------------

    #[test]
    fn parse_get_keeps_q_verbatim_alongside_limit_and_collections() {
        let raw = SearchQueryParams {
            q: Some("acme \"deep harbour\" or beta".to_string()),
            collections: Some("demo".to_string()),
            limit: Some(5),
            ..SearchQueryParams::default()
        };
        let req = parse_get(&raw).unwrap();
        assert_eq!(req.q.as_deref(), Some("acme \"deep harbour\" or beta"));
        assert_eq!(req.collections, vec!["demo".to_string()]);
        assert_eq!(req.limit, 5);
    }

    #[test]
    fn parse_post_keeps_q_verbatim() {
        let raw = SearchBody {
            q: Some("acme".to_string()),
            ..SearchBody::default()
        };
        let req = parse_post(&raw).unwrap();
        assert_eq!(req.q.as_deref(), Some("acme"));
    }

    /// Gate 1 (`#181`): every predicate the index entry cannot express is a
    /// named refusal when combined with `q` — on GET and POST alike, and
    /// with the offending parameter named in the message.
    #[test]
    fn q_combined_with_an_unexpressible_predicate_is_refused_by_name() {
        let cases: Vec<(&str, SearchQueryParams)> = vec![
            (
                "bbox",
                SearchQueryParams {
                    q: Some("acme".to_string()),
                    bbox: Some("1,2,3,4".to_string()),
                    ..SearchQueryParams::default()
                },
            ),
            (
                "datetime",
                SearchQueryParams {
                    q: Some("acme".to_string()),
                    datetime: Some("2020-01-01T00:00:00Z".to_string()),
                    ..SearchQueryParams::default()
                },
            ),
            (
                "intersects",
                SearchQueryParams {
                    q: Some("acme".to_string()),
                    intersects: Some(r#"{"type":"Point","coordinates":[1.0,2.0]}"#.to_string()),
                    ..SearchQueryParams::default()
                },
            ),
            (
                "filter",
                SearchQueryParams {
                    q: Some("acme".to_string()),
                    filter: Some("name = 'a'".to_string()),
                    ..SearchQueryParams::default()
                },
            ),
            (
                "ids",
                SearchQueryParams {
                    q: Some("acme".to_string()),
                    ids: Some("a,b".to_string()),
                    ..SearchQueryParams::default()
                },
            ),
            (
                "token",
                SearchQueryParams {
                    q: Some("acme".to_string()),
                    token: Some("tok".to_string()),
                    ..SearchQueryParams::default()
                },
            ),
        ];
        for (parameter, raw) in cases {
            match parse_get(&raw) {
                Err(Error::Invalid(message)) => assert!(
                    message.contains(&format!("'{parameter}'")),
                    "expected the message to name '{parameter}', got: {message}"
                ),
                other => {
                    panic!("expected a named Invalid refusal for '{parameter}', got {other:?}")
                }
            }
        }

        let body = SearchBody {
            q: Some("acme".to_string()),
            bbox: Some(vec![1.0, 2.0, 3.0, 4.0]),
            ..SearchBody::default()
        };
        assert!(matches!(parse_post(&body), Err(Error::Invalid(_))));
    }

    /// An empty (or whitespace-only) `q` is refused rather than silently
    /// dropped — a dropped predicate is exactly what `#181`'s gates forbid.
    #[test]
    fn an_empty_q_is_refused_not_ignored() {
        for raw_q in ["", "   "] {
            let raw = SearchQueryParams {
                q: Some(raw_q.to_string()),
                ..SearchQueryParams::default()
            };
            assert!(
                matches!(parse_get(&raw), Err(Error::Invalid(_))),
                "q = {raw_q:?} must be refused"
            );
        }
    }

    // -- filter-crs (`#248`, STAC API Filter Extension) --------------------

    /// Campaign rule 1: no `filter-crs` on the wire is `RequestedCrs::Omitted`
    /// on both encodings — the value every driver compiles byte-for-byte the
    /// way it did before `#248`.
    #[test]
    fn an_absent_filter_crs_resolves_to_omitted_on_get_and_post() {
        assert_eq!(
            parse_get(&SearchQueryParams::default()).unwrap().filter_crs,
            RequestedCrs::Omitted
        );
        assert_eq!(
            parse_post(&SearchBody::default()).unwrap().filter_crs,
            RequestedCrs::Omitted
        );
    }

    /// The one value the extension requires a STAC server to accept, spelled
    /// identically as a GET query parameter and as a POST body field — the
    /// extension names "three GET query parameters or POST JSON fields" and
    /// re-spells none of them, so `filter-crs` is `filter-crs` either way.
    #[test]
    fn an_explicit_crs84_resolves_to_crs84_on_get_and_post() {
        let raw = SearchQueryParams {
            filter_crs: Some(crs::CRS84_URI.to_string()),
            ..SearchQueryParams::default()
        };
        assert_eq!(parse_get(&raw).unwrap().filter_crs, RequestedCrs::Crs84);

        let body: SearchBody =
            serde_json::from_value(json!({ "filter-crs": crs::CRS84_URI })).unwrap();
        assert_eq!(body.filter_crs.as_deref(), Some(crs::CRS84_URI));
        assert_eq!(parse_post(&body).unwrap().filter_crs, RequestedCrs::Crs84);
    }

    /// Every other value is refused **by name**, GET and POST alike. EPSG:4326
    /// by authority is the case that matters: datum-identical to CRS84,
    /// opposite axis order, so silently reading it as CRS84 (what this lane
    /// did before `#248`) returns different rows under a `200`.
    #[test]
    fn any_other_filter_crs_is_refused_by_name_on_get_and_post() {
        for uri in [crs::epsg_uri(4326), crs::epsg_uri(3857), "nonsense".into()] {
            let raw = SearchQueryParams {
                filter_crs: Some(uri.clone()),
                ..SearchQueryParams::default()
            };
            match parse_get(&raw) {
                Err(Error::Invalid(message)) => {
                    assert!(
                        message.contains("filter-crs"),
                        "the refusal must name the parameter, got: {message}"
                    );
                    assert!(
                        message.contains(crs::CRS84_URI),
                        "the refusal must name the one accepted value, got: {message}"
                    );
                }
                other => panic!("expected a named refusal for {uri}, got {other:?}"),
            }

            let body = SearchBody {
                filter_crs: Some(uri.clone()),
                ..SearchBody::default()
            };
            assert!(
                matches!(parse_post(&body), Err(Error::Invalid(_))),
                "POST must refuse {uri} exactly as GET does"
            );
        }
    }

    /// A `next` link that dropped `filter-crs` would evaluate page two's
    /// filter geometry in a different CRS than page one's — the same reason
    /// `tellurion-features`' `items_href` echoes it (`#217`). Echoed from a
    /// POST body too, since `/search`'s links are always followable GETs.
    #[test]
    fn search_href_echoes_filter_crs_from_both_request_shapes() {
        let from_get = SearchHrefParams::from(&SearchQueryParams {
            filter_crs: Some(crs::CRS84_URI.to_string()),
            ..SearchQueryParams::default()
        });
        let from_post = SearchHrefParams::from(&SearchBody {
            filter_crs: Some(crs::CRS84_URI.to_string()),
            ..SearchBody::default()
        });
        for params in [from_get, from_post] {
            let href = search_href("/search", &params, Some("next"));
            assert!(
                href.contains(
                    "filter-crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84"
                ),
                "href was: {href}"
            );
            assert!(href.contains("token=next"), "href was: {href}");
        }
    }

    #[test]
    fn search_href_echoes_q() {
        let params = SearchHrefParams {
            q: Some("deep harbour".to_string()),
            ..SearchHrefParams::default()
        };
        assert_eq!(
            search_href("/search", &params, None),
            "/search?q=deep%20harbour"
        );
    }

    // -- parse_post ---------------------------------------------------------

    #[test]
    fn parse_post_reads_json_arrays_directly() {
        let raw = SearchBody {
            collections: Some(vec!["a".to_string(), "b".to_string()]),
            ids: Some(vec!["x".to_string()]),
            bbox: Some(vec![1.0, 2.0, 3.0, 4.0]),
            ..SearchBody::default()
        };
        let req = parse_post(&raw).unwrap();
        assert_eq!(req.collections, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(req.ids, vec!["x".to_string()]);
        assert_eq!(req.bbox, Some([1.0, 2.0, 3.0, 4.0]));
    }

    #[test]
    fn parse_post_rejects_a_3d_bbox() {
        let raw = SearchBody {
            bbox: Some(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            ..SearchBody::default()
        };
        assert!(matches!(parse_post(&raw), Err(Error::Invalid(_))));
    }

    #[test]
    fn parse_post_filter_defaults_to_cql2_json_and_accepts_a_json_object() {
        let raw = SearchBody {
            filter: Some(json!({"op": "=", "args": [{"property": "name"}, "a"]})),
            ..SearchBody::default()
        };
        let req = parse_post(&raw).unwrap();
        assert!(req.filter.is_some());
    }

    #[test]
    fn parse_post_filter_lang_cql2_text_accepts_a_json_string() {
        let raw = SearchBody {
            filter: Some(json!("name = 'a'")),
            filter_lang: Some("cql2-text".to_string()),
            ..SearchBody::default()
        };
        let req = parse_post(&raw).unwrap();
        assert!(req.filter.is_some());
    }

    #[test]
    fn parse_post_intersects_accepts_a_geojson_object() {
        let raw = SearchBody {
            intersects: Some(json!({"type": "Point", "coordinates": [1.0, 2.0]})),
            ..SearchBody::default()
        };
        let req = parse_post(&raw).unwrap();
        assert_eq!(req.intersects.unwrap()["type"], "Point");
    }

    #[test]
    fn parse_post_rejects_bbox_and_intersects_together() {
        let raw = SearchBody {
            bbox: Some(vec![1.0, 2.0, 3.0, 4.0]),
            intersects: Some(json!({"type": "Point", "coordinates": [1.0, 2.0]})),
            ..SearchBody::default()
        };
        assert!(matches!(parse_post(&raw), Err(Error::Invalid(_))));
    }

    // -- SearchHrefParams / search_href ----------------------------------

    #[test]
    fn href_params_from_post_body_join_arrays_with_commas() {
        let body = SearchBody {
            collections: Some(vec!["a".to_string(), "b".to_string()]),
            ids: Some(vec!["x".to_string()]),
            bbox: Some(vec![1.0, 2.0, 3.0, 4.0]),
            ..SearchBody::default()
        };
        let href_params = SearchHrefParams::from(&body);
        assert_eq!(href_params.collections.as_deref(), Some("a,b"));
        assert_eq!(href_params.ids.as_deref(), Some("x"));
        assert_eq!(href_params.bbox.as_deref(), Some("1,2,3,4"));
    }

    #[test]
    fn href_params_from_post_body_stringifies_a_cql2_json_filter() {
        let body = SearchBody {
            filter: Some(json!({"op": "=", "args": [{"property": "a"}, 1]})),
            filter_lang: Some("cql2-json".to_string()),
            ..SearchBody::default()
        };
        let href_params = SearchHrefParams::from(&body);
        assert!(href_params.filter.unwrap().contains("\"op\":\"=\""));
    }

    #[test]
    fn search_href_round_trips_a_token_override() {
        let params = SearchHrefParams {
            limit: Some(5),
            token: Some("orig".to_string()),
            ..SearchHrefParams::default()
        };
        let self_href = search_href("/search", &params, None);
        assert_eq!(self_href, "/search?limit=5&token=orig");

        let next_href = search_href("/search", &params, Some("next-token"));
        assert_eq!(next_href, "/search?limit=5&token=next-token");
    }

    #[test]
    fn search_href_with_no_params_is_a_bare_path() {
        assert_eq!(
            search_href("/search", &SearchHrefParams::default(), None),
            "/search"
        );
    }

    // -- SearchToken ----------------------------------------------------

    #[test]
    fn cursor_token_round_trips_through_encode_decode() {
        let token = SearchToken::Cursor {
            collections: vec!["a".to_string(), "b".to_string()],
            idx: 1,
            cursor: Some("42".to_string()),
        };
        let encoded = token.encode();
        // Opaque: the JSON structure/field names don't leak through
        // verbatim (unlike, say, plain percent-encoded JSON would).
        assert!(!encoded.contains("collections"));
        assert!(!encoded.contains('{'));
        let decoded = SearchToken::decode(&encoded).unwrap();
        assert_eq!(decoded, token);
    }

    #[test]
    fn ids_token_round_trips_through_encode_decode() {
        let token = SearchToken::Ids {
            collections: vec!["demo".to_string()],
            ids: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            start: 2,
        };
        let decoded = SearchToken::decode(&token.encode()).unwrap();
        assert_eq!(decoded, token);
    }

    #[test]
    fn decode_rejects_a_garbage_token() {
        assert!(matches!(
            SearchToken::decode("not-a-real-token!!!"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn decode_rejects_a_well_formed_base64_string_that_isnt_a_token() {
        let encoded = base64url_encode(b"just some bytes, not json");
        assert!(matches!(
            SearchToken::decode(&encoded),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn base64url_round_trips_arbitrary_byte_lengths() {
        for input in [
            &b""[..],
            &b"f"[..],
            &b"fo"[..],
            &b"foo"[..],
            &b"foob"[..],
            &b"fooba"[..],
            &b"foobar"[..],
        ] {
            let encoded = base64url_encode(input);
            assert_eq!(base64url_decode(&encoded).unwrap(), input);
        }
    }
}
