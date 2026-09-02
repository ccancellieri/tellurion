//! Query parameter parsing for `/collections/{cid}/items` (`#36` slice B).
//! `ItemsQueryParams`/`parse_items_query` stay this crate's own: they build
//! this slice's own request shape, which genuinely differs from
//! `tellurion-features`' (no `filter`/`crs` here — CQL2 filtering for STAC
//! lands with `/search`, `#36` slice C, not here). The leaf-level
//! `bbox`/`datetime`/`limit`/percent-encoding parsing underneath that shape
//! is byte-identical to `tellurion-features`' own, so it lives once in
//! `tellurion_core::query_params` and both crates call into it rather than
//! keeping two copies in sync. What was already reused, verbatim, before
//! this: `tellurion_core::{ItemsQuery, DatetimeRange}`, `Router::
//! resolve_features`, and `FeatureSource::items`/`item` — this module only
//! ever turns query-string text into the same `ItemsQuery`
//! `tellurion-features` builds, never a parallel query type.

use serde::Deserialize;
use tellurion_core::query_params::parse_bounded_limit;
use tellurion_core::{Error, ItemsQuery, PageRequest, Result};

pub(crate) use tellurion_core::query_params::{parse_bbox, parse_datetime, percent_encode};

pub const DEFAULT_LIMIT: u32 = 10;
pub const MAX_LIMIT: u32 = 10_000;

/// Default page size for `GET /collections` (`#42`, `#59`) when no `limit`
/// is given — same value and rationale as
/// `tellurion_features::params::COLLECTIONS_DEFAULT_LIMIT` (kept as this
/// crate's own copy, per this module's own doc): large enough that any
/// realistic file-backed deployment's whole catalog still fits on the first
/// page, while still bounding a registry sized past that.
pub const COLLECTIONS_DEFAULT_LIMIT: u32 = 100;
pub const COLLECTIONS_MAX_LIMIT: u32 = 10_000;

/// `GET /collections/{cid}`'s own query parameters (`#50`): just `f`, the
/// standard OGC API format-selection parameter (same name
/// `tellurion-tiles::handlers::negotiate_format` already reads for its own
/// suffix-or-`f`-or-`Accept` negotiation) — used here to select the ISO
/// 19139 XML alternate representation instead of this endpoint's default
/// STAC Collection JSON.
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct GetCollectionQueryParams {
    pub f: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct ItemsQueryParams {
    pub limit: Option<u32>,
    pub bbox: Option<String>,
    pub datetime: Option<String>,
    pub token: Option<String>,
}

/// Parses every `/items` query parameter into a syntactically valid
/// `ItemsQuery`. Pure, I/O-free — same contract
/// `tellurion_features::params::parse_items_query` documents for itself.
pub fn parse_items_query(raw: &ItemsQueryParams) -> Result<ItemsQuery> {
    let limit = parse_limit(raw.limit)?;
    let bbox = raw.bbox.as_deref().map(parse_bbox).transpose()?;
    let datetime = raw.datetime.as_deref().map(parse_datetime).transpose()?;

    Ok(ItemsQuery {
        limit,
        bbox,
        datetime,
        token: raw.token.clone(),
        filter: None,
        ..ItemsQuery::default()
    })
}

/// `pub(crate)`: reused by `search.rs` (`#36` slice C) so `/search`'s
/// `limit` parameter parses identically to `/items`'s rather than a second,
/// possibly-drifting copy.
pub(crate) fn parse_limit(limit: Option<u32>) -> Result<u32> {
    parse_bounded_limit(limit, DEFAULT_LIMIT, MAX_LIMIT)
}

/// Builds an href for `path` echoing the parsed items-query params, with
/// `override_token` substituted for the page token when present (the `next`
/// link case) — same shape `tellurion_features::params::items_href` builds.
pub fn items_href(path: &str, params: &ItemsQueryParams, override_token: Option<&str>) -> String {
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

// -- /collections listing paging (`#42`, `#59`) -------------------------

/// `GET /collections`' own query parameters (`#59`) — deliberately just
/// `limit`/`token`, the same two names `tellurion_features::params::
/// CollectionsQueryParams` uses for the same concepts: both are cursor pages
/// over the same registry-seam paging discipline (never OFFSET).
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct CollectionsQueryParams {
    pub limit: Option<u32>,
    /// Opaque keyset paging cursor — the previous page's `next` link's
    /// token, echoed back verbatim. Same semantics as `ItemsQueryParams::
    /// token`; see `tellurion_core::registry`'s module doc for what the
    /// cursor actually encodes.
    pub token: Option<String>,
}

/// Parses `GET /collections`' query parameters straight into a
/// `PageRequest` (`tellurion_core::registry`) — the registry seam's own
/// page-request shape, so this crate never invents a second one just to
/// re-map it one call later. Same shape `tellurion_features::params::
/// parse_collections_query` builds.
pub fn parse_collections_query(raw: &CollectionsQueryParams) -> Result<PageRequest> {
    let limit = parse_collections_limit(raw.limit)?;
    Ok(PageRequest {
        limit,
        after: raw.token.clone(),
    })
}

fn parse_collections_limit(limit: Option<u32>) -> Result<u32> {
    match limit {
        None => Ok(COLLECTIONS_DEFAULT_LIMIT),
        Some(0) => Err(Error::Invalid("limit must be >= 1".to_string())),
        // Same spec behavior `parse_limit` applies to items: values above
        // the maximum are clamped, not rejected.
        Some(value) => Ok(value.min(COLLECTIONS_MAX_LIMIT)),
    }
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
    fn datetime_single_instant() {
        let range = parse_datetime("2020-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(range.end.as_deref(), Some("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_open_interval() {
        let range = parse_datetime("../2021-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start, None);
        assert_eq!(range.end.as_deref(), Some("2021-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_rejects_double_open() {
        assert!(matches!(parse_datetime("../.."), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_rejects_a_syntactically_invalid_single_instant() {
        assert!(matches!(parse_datetime("notadate"), Err(Error::Invalid(_))));
    }

    #[test]
    fn items_href_round_trips_token_override() {
        let params = ItemsQueryParams {
            limit: Some(5),
            token: Some("orig".to_string()),
            ..ItemsQueryParams::default()
        };
        let self_href = items_href("/collections/demo/items", &params, None);
        assert_eq!(self_href, "/collections/demo/items?limit=5&token=orig");

        let next_href = items_href("/collections/demo/items", &params, Some("next-token"));
        assert_eq!(
            next_href,
            "/collections/demo/items?limit=5&token=next-token"
        );
    }

    #[test]
    fn no_query_params_produces_a_bare_path() {
        let href = items_href(
            "/collections/demo/items",
            &ItemsQueryParams::default(),
            None,
        );
        assert_eq!(href, "/collections/demo/items");
    }

    // -- /collections listing paging (`#42`, `#59`) --------------------------

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
}
