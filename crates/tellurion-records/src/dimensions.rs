//! Algorithmic OGC API — Dimensions (Records Profile) endpoints.
//!
//! Serves intensional, generator-backed dimensions (e.g. agro-meteorological
//! dekadal and pentadal calendars, uniform integer ranges) as OGC API — Records
//! collections with $O(1)$ per-page generation and standard `?datetime=` / CQL2 `filter` lookup.
//!
//! Decoupling the intensional dimension domain $D$ from extensional dataset
//! granule availability $S \subseteq D$ allows clients to navigate non-ISO
//! calendars and large/unbounded axes without conflating missing granules (`NaN`)
//! with non-existent domain coordinates.

use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use tellurion_core::filter::{
    self, CaseInsensitiveCompareOp, CompareOp, Filter, Literal, TemporalOp, TemporalValue,
};
use tellurion_core::query_params::percent_encode;
use tellurion_core::AppContext;

use crate::conformance::{GEOJSON_MEDIA_TYPE, ITEM_TYPE_RECORD, JSON_MEDIA_TYPE, REL_COLLECTION, REL_ITEMS};
use crate::model::Link;
use crate::problem::ApiError;

const DEFAULT_LIMIT: u32 = 36;
const MAX_LIMIT: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DimensionSpec {
    Dekadal { start_year: i32, end_year: i32 },
    Pentadal { start_year: i32, end_year: i32 },
    IntegerRange { start: i64, end: i64, step: i64 },
}

#[derive(Debug, Clone)]
struct DimensionCatalogEntry {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    variable_type: &'static str,
    unit: Option<&'static str>,
    spec: DimensionSpec,
}

const BUILTIN_DIMENSIONS: &[DimensionCatalogEntry] = &[
    DimensionCatalogEntry {
        id: "dekadal-2000-2026",
        title: "Dekadal Temporal Dimension (2000–2026)",
        description: "Agro-meteorological 10-day nominal periods (36/year: D1=1..10, D2=11..20, D3=21..EOM), aligned with FAO ASIS.",
        variable_type: "categoricalOrdinal",
        unit: None,
        spec: DimensionSpec::Dekadal {
            start_year: 2000,
            end_year: 2026,
        },
    },
    DimensionCatalogEntry {
        id: "dekadal-2021",
        title: "Dekadal Temporal Dimension (2021)",
        description: "Single-year 36-dekad calendar for 2021 demonstration GeoTIFF cubes.",
        variable_type: "categoricalOrdinal",
        unit: None,
        spec: DimensionSpec::Dekadal {
            start_year: 2021,
            end_year: 2021,
        },
    },
    DimensionCatalogEntry {
        id: "pentadal-2000-2026",
        title: "Pentadal Temporal Dimension (2000–2026)",
        description: "Agro-meteorological 5-day nominal periods (72/year: 6 pentads per month, P6=26..EOM).",
        variable_type: "categoricalOrdinal",
        unit: None,
        spec: DimensionSpec::Pentadal {
            start_year: 2000,
            end_year: 2026,
        },
    },
    DimensionCatalogEntry {
        id: "pentadal-2021",
        title: "Pentadal Temporal Dimension (2021)",
        description: "Single-year 72-pentad calendar (6 monthly pentads/month, P6=26..EOM) aligned with CHIRPS/FAO.",
        variable_type: "categoricalOrdinal",
        unit: None,
        spec: DimensionSpec::Pentadal {
            start_year: 2021,
            end_year: 2021,
        },
    },
    DimensionCatalogEntry {
        id: "elevation-bands-0-5000m",
        title: "Elevation Bands (0–5000 m, step 100 m)",
        description: "Discrete vertical elevation bins from 0 m to 5000 m at 100 m intervals.",
        variable_type: "numericalOrdinal",
        unit: Some("m"),
        spec: DimensionSpec::IntegerRange {
            start: 0,
            end: 5000,
            step: 100,
        },
    },
];

fn find_dimension(id: &str) -> Option<&'static DimensionCatalogEntry> {
    BUILTIN_DIMENSIONS.iter().find(|d| d.id == id)
}

const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

const fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

impl DimensionSpec {
    fn len(&self) -> u64 {
        match *self {
            DimensionSpec::Dekadal {
                start_year,
                end_year,
            } => ((end_year - start_year + 1) as u64) * 36,
            DimensionSpec::Pentadal {
                start_year,
                end_year,
            } => ((end_year - start_year + 1) as u64) * 72,
            DimensionSpec::IntegerRange { start, end, step } => {
                if end < start || step <= 0 {
                    0
                } else {
                    ((end - start) / step) as u64 + 1
                }
            }
        }
    }

    fn temporal_extent(&self) -> Option<[Option<String>; 2]> {
        match *self {
            DimensionSpec::Dekadal {
                start_year,
                end_year,
            }
            | DimensionSpec::Pentadal {
                start_year,
                end_year,
            } => Some([
                Some(format!("{start_year:04}-01-01T00:00:00Z")),
                Some(format!("{end_year:04}-12-31T23:59:59Z")),
            ]),
            DimensionSpec::IntegerRange { .. } => None,
        }
    }

    fn member_at(&self, index: u64) -> Option<Value> {
        if index >= self.len() {
            return None;
        }
        match *self {
            DimensionSpec::Dekadal { start_year, .. } => {
                let year = start_year + (index / 36) as i32;
                let rem = (index % 36) as u32;
                let month = rem / 3 + 1;
                let dekad = rem % 3 + 1;
                let (d_start, d_end) = match dekad {
                    1 => (1, 10),
                    2 => (11, 20),
                    _ => (21, days_in_month(year, month)),
                };
                let id = format!("{year:04}-{month:02}-D{dekad}");
                let start_iso = format!("{year:04}-{month:02}-{d_start:02}T00:00:00Z");
                let end_iso = format!("{year:04}-{month:02}-{d_end:02}T23:59:59Z");
                Some(json!({
                    "type": "Feature",
                    "id": id,
                    "geometry": null,
                    "time": { "interval": [start_iso, end_iso] },
                    "properties": {
                        "type": ITEM_TYPE_RECORD,
                        "title": format!("{year:04}-{month:02} Dekad {dekad}"),
                        "start_datetime": start_iso,
                        "end_datetime": end_iso,
                        "year": year,
                        "month": month,
                        "dekad": dekad,
                        "ordinal": index
                    }
                }))
            }
            DimensionSpec::Pentadal { start_year, .. } => {
                let year = start_year + (index / 72) as i32;
                let rem = (index % 72) as u32;
                let month = rem / 6 + 1;
                let pentad = rem % 6 + 1;
                let d_start = (pentad - 1) * 5 + 1;
                let d_end = if pentad == 6 {
                    days_in_month(year, month)
                } else {
                    pentad * 5
                };
                let id = format!("{year:04}-{month:02}-P{pentad}");
                let start_iso = format!("{year:04}-{month:02}-{d_start:02}T00:00:00Z");
                let end_iso = format!("{year:04}-{month:02}-{d_end:02}T23:59:59Z");
                Some(json!({
                    "type": "Feature",
                    "id": id,
                    "geometry": null,
                    "time": { "interval": [start_iso, end_iso] },
                    "properties": {
                        "type": ITEM_TYPE_RECORD,
                        "title": format!("{year:04}-{month:02} Pentad {pentad}"),
                        "start_datetime": start_iso,
                        "end_datetime": end_iso,
                        "year": year,
                        "month": month,
                        "pentad": pentad,
                        "ordinal": index
                    }
                }))
            }
            DimensionSpec::IntegerRange { start, step, .. } => {
                let val = start + (index as i64) * step;
                let id = val.to_string();
                Some(json!({
                    "type": "Feature",
                    "id": id,
                    "geometry": null,
                    "properties": {
                        "type": ITEM_TYPE_RECORD,
                        "title": format!("{val} m"),
                        "value": val,
                        "lower_bound": val,
                        "upper_bound": val + step,
                        "ordinal": index
                    }
                }))
            }
        }
    }

    fn member_by_id(&self, member_id: &str) -> Option<Value> {
        match *self {
            DimensionSpec::Dekadal {
                start_year,
                end_year,
            } => {
                let (ym, d_part) = member_id.split_once("-D")?;
                let (y_str, m_str) = ym.split_once('-')?;
                let year: i32 = y_str.parse().ok()?;
                let month: u32 = m_str.parse().ok()?;
                let dekad: u32 = d_part.parse().ok()?;
                if year < start_year || year > end_year || !(1..=12).contains(&month) || !(1..=3).contains(&dekad) {
                    return None;
                }
                let idx = ((year - start_year) as u64) * 36 + ((month - 1) as u64) * 3 + (dekad - 1) as u64;
                self.member_at(idx)
            }
            DimensionSpec::Pentadal {
                start_year,
                end_year,
            } => {
                let (ym, p_part) = member_id.split_once("-P")?;
                let (y_str, m_str) = ym.split_once('-')?;
                let year: i32 = y_str.parse().ok()?;
                let month: u32 = m_str.parse().ok()?;
                let pentad: u32 = p_part.parse().ok()?;
                if year < start_year || year > end_year || !(1..=12).contains(&month) || !(1..=6).contains(&pentad) {
                    return None;
                }
                let idx = ((year - start_year) as u64) * 72 + ((month - 1) as u64) * 6 + (pentad - 1) as u64;
                self.member_at(idx)
            }
            DimensionSpec::IntegerRange { start, end, step } => {
                let val: i64 = member_id.parse().ok()?;
                if val < start || val > end || (val - start) % step != 0 {
                    return None;
                }
                let idx = ((val - start) / step) as u64;
                self.member_at(idx)
            }
        }
    }

    fn index_of_date(&self, raw_value: &str) -> Result<u64, &'static str> {
        match *self {
            DimensionSpec::Dekadal {
                start_year,
                end_year,
            } => {
                let (year, month, day) =
                    parse_ymd_prefix(raw_value).ok_or("Expected ISO date prefix YYYY-MM-DD")?;
                if year < start_year || year > end_year {
                    return Err("Date outside configured dekadal domain");
                }
                let dekad = match day {
                    1..=10 => 1,
                    11..=20 => 2,
                    _ => 3,
                };
                Ok(((year - start_year) as u64) * 36 + ((month - 1) as u64) * 3 + (dekad - 1) as u64)
            }
            DimensionSpec::Pentadal {
                start_year,
                end_year,
            } => {
                let (year, month, day) =
                    parse_ymd_prefix(raw_value).ok_or("Expected ISO date prefix YYYY-MM-DD")?;
                if year < start_year || year > end_year {
                    return Err("Date outside configured pentadal domain");
                }
                let pentad = ((day - 1) / 5 + 1).min(6);
                Ok(((year - start_year) as u64) * 72 + ((month - 1) as u64) * 6 + (pentad - 1) as u64)
            }
            DimensionSpec::IntegerRange { start, end, step } => {
                let val: i64 = raw_value.trim().parse().map_err(|_| "Expected integer value")?;
                if val < start || val > end {
                    return Err("Value outside configured integer range");
                }
                Ok(((val - start) / step) as u64)
            }
        }
    }
}

fn parse_ymd_prefix(raw: &str) -> Option<(i32, u32, u32)> {
    let date_part = raw.trim().split('T').next()?;
    let mut parts = date_part.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    let max_d = days_in_month(year, month);
    if !(1..=12).contains(&month) || day == 0 || day > max_d {
        return None;
    }
    Some((year, month, day))
}

#[derive(Debug, Deserialize, Default)]
pub struct DimensionItemsParams {
    pub limit: Option<u32>,
    pub offset: Option<u64>,
    pub datetime: Option<String>,
    pub filter: Option<String>,
    #[serde(rename = "filter-lang")]
    pub filter_lang: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DimensionCollectionDto {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    #[serde(rename = "itemType")]
    pub item_type: &'static str,
    pub size: u64,
    #[serde(rename = "variableType")]
    pub variable_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extent: Option<Value>,
    pub links: Vec<Link>,
}

fn base_path(uri: &OriginalUri) -> String {
    uri.path().trim_end_matches('/').to_string()
}

fn json_response(media_type: &'static str, body: Value) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    (StatusCode::OK, headers, Json(body)).into_response()
}

fn entry_to_dto(entry: &DimensionCatalogEntry, self_href: &str) -> DimensionCollectionDto {
    let extent = entry.spec.temporal_extent().map(|interval| {
        json!({
            "temporal": { "interval": [interval] }
        })
    });
    DimensionCollectionDto {
        id: entry.id,
        title: entry.title,
        description: entry.description,
        item_type: ITEM_TYPE_RECORD,
        size: entry.spec.len(),
        variable_type: entry.variable_type,
        unit: entry.unit,
        extent,
        links: vec![
            Link::new(self_href, "self", JSON_MEDIA_TYPE),
            Link::new(format!("{self_href}/items"), REL_ITEMS, GEOJSON_MEDIA_TYPE),
        ],
    }
}

pub async fn list_dimensions(
    State(_ctx): State<Arc<AppContext>>,
    uri: OriginalUri,
) -> Response {
    let base = base_path(&uri);
    let collections: Vec<DimensionCollectionDto> = BUILTIN_DIMENSIONS
        .iter()
        .map(|entry| entry_to_dto(entry, &format!("{base}/{}", entry.id)))
        .collect();
    let body = json!({
        "collections": collections,
        "links": [
            Link::new(base, "self", JSON_MEDIA_TYPE)
        ]
    });
    json_response(JSON_MEDIA_TYPE, body)
}

pub async fn get_dimension(
    State(_ctx): State<Arc<AppContext>>,
    Path(params): Path<std::collections::HashMap<String, String>>,
    uri: OriginalUri,
) -> Result<Response, ApiError> {
    let did = params
        .get("did")
        .ok_or_else(|| ApiError::bad_request("Missing dimension id"))?;
    let entry = find_dimension(did)
        .ok_or_else(|| ApiError::not_found(format!("Dimension '{did}' not found")))?;
    let self_href = base_path(&uri);
    let dto = entry_to_dto(entry, &self_href);
    Ok(json_response(JSON_MEDIA_TYPE, serde_json::to_value(dto).unwrap_or_default()))
}

fn normalize_instant(raw: &str, end_of_day: bool) -> Option<String> {
    let s = raw.trim();
    if s.len() == 10 && s.as_bytes().get(4) == Some(&b'-') && s.as_bytes().get(7) == Some(&b'-') {
        return Some(if end_of_day {
            format!("{s}T23:59:59Z")
        } else {
            format!("{s}T00:00:00Z")
        });
    }
    if s.len() >= 19 && matches!(s.as_bytes().get(10), Some(b'T' | b't')) {
        return Some(format!("{}T{}Z", &s[..10], &s[11..19]));
    }
    None
}

fn temporal_value_bounds(val: &TemporalValue) -> Option<(String, String)> {
    match val {
        TemporalValue::Instant(s) => {
            if s.trim() == ".." {
                return None;
            }
            let start = normalize_instant(s, false)?;
            let end = normalize_instant(s, true)?;
            Some((start, end))
        }
        TemporalValue::Interval(s, e) => {
            let start = if s.trim() == ".." {
                "0000-01-01T00:00:00Z".to_string()
            } else {
                normalize_instant(s, false)?
            };
            let end = if e.trim() == ".." {
                "9999-12-31T23:59:59Z".to_string()
            } else {
                normalize_instant(e, true)?
            };
            Some((start, end))
        }
    }
}

fn member_time_bounds(member: &Value) -> Option<(&str, &str)> {
    let arr = member.get("time")?.get("interval")?.as_array()?;
    let s = arr.first()?.as_str()?;
    let e = arr.get(1)?.as_str()?;
    Some((s, e))
}

fn member_property<'a>(member: &'a Value, name: &str) -> Option<&'a Value> {
    if name == "id" {
        member.get("id")
    } else {
        member.get("properties").and_then(|p| p.get(name))
    }
}

fn compare_scalar(val: &Value, lit: &Literal) -> Option<std::cmp::Ordering> {
    match (val, lit) {
        (Value::Number(n), Literal::Number(rhs)) => n.as_f64().and_then(|lhs| lhs.partial_cmp(rhs)),
        (Value::String(lhs), Literal::Text(rhs)) => Some(lhs.as_str().cmp(rhs.as_str())),
        (Value::Bool(lhs), Literal::Bool(rhs)) => Some(lhs.cmp(rhs)),
        _ => None,
    }
}

fn eval_compare_op(ord: std::cmp::Ordering, op: CompareOp) -> bool {
    match op {
        CompareOp::Eq => ord == std::cmp::Ordering::Equal,
        CompareOp::Ne => ord != std::cmp::Ordering::Equal,
        CompareOp::Lt => ord == std::cmp::Ordering::Less,
        CompareOp::Gt => ord == std::cmp::Ordering::Greater,
        CompareOp::Le => ord != std::cmp::Ordering::Greater,
        CompareOp::Ge => ord != std::cmp::Ordering::Less,
    }
}

fn like_matches(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == '%' {
            dp[i][0] = dp[i - 1][0];
        }
        for j in 1..=t.len() {
            dp[i][j] = match p[i - 1] {
                '%' => dp[i - 1][j] || dp[i][j - 1],
                '_' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == t[j - 1],
            };
        }
    }
    dp[p.len()][t.len()]
}

fn member_matches_cql2_filter(member: &Value, filter: &Filter) -> bool {
    match filter {
        Filter::And(items) => items.iter().all(|f| member_matches_cql2_filter(member, f)),
        Filter::Or(items) => items.iter().any(|f| member_matches_cql2_filter(member, f)),
        Filter::Not(inner) => !member_matches_cql2_filter(member, inner),
        Filter::Compare { property, op, value } => member_property(member, property)
            .and_then(|v| compare_scalar(v, value))
            .is_some_and(|ord| eval_compare_op(ord, *op)),
        Filter::IsNull { property, negated } => {
            let is_null = member_property(member, property).is_none_or(Value::is_null);
            if *negated { !is_null } else { is_null }
        }
        Filter::Like { property, pattern, negated } => {
            let matched = member_property(member, property)
                .and_then(Value::as_str)
                .is_some_and(|s| like_matches(s, pattern));
            if *negated { !matched } else { matched }
        }
        Filter::Between { property, low, high, negated } => {
            let matched = member_property(member, property)
                .and_then(|v| {
                    let lo = compare_scalar(v, low)?;
                    let hi = compare_scalar(v, high)?;
                    Some(lo != std::cmp::Ordering::Less && hi != std::cmp::Ordering::Greater)
                })
                .unwrap_or(false);
            if *negated { !matched } else { matched }
        }
        Filter::In { property, values, negated } => {
            let matched = member_property(member, property).is_some_and(|v| {
                values
                    .iter()
                    .any(|lit| compare_scalar(v, lit) == Some(std::cmp::Ordering::Equal))
            });
            if *negated { !matched } else { matched }
        }
        Filter::CaseInsensitiveCompare { property, op, value } => member_property(member, property)
            .and_then(Value::as_str)
            .is_some_and(|s| {
                let eq = s.to_ascii_lowercase() == value.to_ascii_lowercase();
                match op {
                    CaseInsensitiveCompareOp::Eq => eq,
                    CaseInsensitiveCompareOp::Ne => !eq,
                }
            }),
        Filter::After { instant, .. } => {
            let (Some((m_start, _)), Some(q)) = (member_time_bounds(member), normalize_instant(instant, true)) else {
                return false;
            };
            m_start > q.as_str()
        }
        Filter::Before { instant, .. } => {
            let (Some((_, m_end)), Some(q)) = (member_time_bounds(member), normalize_instant(instant, false)) else {
                return false;
            };
            m_end < q.as_str()
        }
        Filter::During { start, end, .. } => {
            let Some((m_start, m_end)) = member_time_bounds(member) else {
                return false;
            };
            let Some((q_start, q_end)) = temporal_value_bounds(&TemporalValue::Interval(start.clone(), end.clone())) else {
                return false;
            };
            m_start >= q_start.as_str() && m_end <= q_end.as_str()
        }
        Filter::Temporal { op, value, .. } => {
            let (Some((m_start, m_end)), Some((q_start, q_end))) = (member_time_bounds(member), temporal_value_bounds(value)) else {
                return false;
            };
            let qs = q_start.as_str();
            let qe = q_end.as_str();
            match op {
                TemporalOp::Intersects | TemporalOp::Overlaps | TemporalOp::OverlappedBy => {
                    m_start <= qe && m_end >= qs
                }
                TemporalOp::Disjoint => m_end < qs || m_start > qe,
                TemporalOp::Contains => m_start <= qs && m_end >= qe,
                TemporalOp::Equals => m_start == qs && m_end == qe,
                TemporalOp::Meets => m_end == qs,
                TemporalOp::MetBy => m_start == qe,
                TemporalOp::Starts | TemporalOp::StartedBy => m_start == qs,
                TemporalOp::Finishes | TemporalOp::FinishedBy => m_end == qe,
            }
        }
        Filter::Intersects { .. } | Filter::Spatial { .. } => false,
    }
}

pub async fn list_dimension_items(
    State(_ctx): State<Arc<AppContext>>,
    Path(params): Path<std::collections::HashMap<String, String>>,
    Query(query): Query<DimensionItemsParams>,
    uri: OriginalUri,
) -> Result<Response, ApiError> {
    let did = params
        .get("did")
        .ok_or_else(|| ApiError::bad_request("Missing dimension id"))?;
    let entry = find_dimension(did)
        .ok_or_else(|| ApiError::not_found(format!("Dimension '{did}' not found")))?;

    let cql_filter = match query
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => {
            let lang = query.filter_lang.as_deref().unwrap_or("cql2-text");
            Some(filter::parse(lang, raw).map_err(|e| ApiError::bad_request(e.to_string()))?)
        }
        None => None,
    };

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as u64;
    let offset = query.offset.unwrap_or(0);
    let total = entry.spec.len();

    let (range_start, range_end) = match query
        .datetime
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(dt) => {
            if let Some((s_raw, e_raw)) = dt.split_once('/') {
                let s_trim = s_raw.trim();
                let e_trim = e_raw.trim();
                let s_idx = if s_trim.is_empty() || s_trim == ".." {
                    0
                } else {
                    entry.spec.index_of_date(s_trim).map_err(ApiError::bad_request)?
                };
                let e_idx_incl = if e_trim.is_empty() || e_trim == ".." {
                    total.saturating_sub(1)
                } else {
                    entry.spec.index_of_date(e_trim).map_err(ApiError::bad_request)?
                };
                if s_idx > e_idx_incl || total == 0 {
                    (0, 0)
                } else {
                    (s_idx, (e_idx_incl + 1).min(total))
                }
            } else {
                let idx = entry.spec.index_of_date(dt).map_err(ApiError::bad_request)?;
                (idx, (idx + 1).min(total))
            }
        }
        None => (0, total),
    };

    let (matched_total, features, has_more) = if let Some(parsed) = &cql_filter {
        let mut all_matched = Vec::new();
        for idx in range_start..range_end {
            if let Some(feat) = entry.spec.member_at(idx) {
                if member_matches_cql2_filter(&feat, parsed) {
                    all_matched.push(feat);
                }
            }
        }
        let matched = all_matched.len() as u64;
        let start = (offset as usize).min(all_matched.len());
        let end = start.saturating_add(limit as usize).min(all_matched.len());
        let page = all_matched[start..end].to_vec();
        (matched, page, end < all_matched.len())
    } else {
        let matched = range_end.saturating_sub(range_start);
        let slice_start = (range_start.saturating_add(offset)).min(range_end);
        let slice_end = (slice_start.saturating_add(limit)).min(range_end);
        let mut page = Vec::new();
        for idx in slice_start..slice_end {
            if let Some(feat) = entry.spec.member_at(idx) {
                page.push(feat);
            }
        }
        (matched, page, slice_end < range_end)
    };

    let items_base = base_path(&uri);
    let coll_base = items_base.strip_suffix("/items").unwrap_or(&items_base);
    let mut query_suffix = String::new();
    if let Some(dt) = query.datetime.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query_suffix.push_str(&format!("&datetime={}", percent_encode(dt)));
    }
    if let Some(f) = query.filter.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query_suffix.push_str(&format!("&filter={}", percent_encode(f)));
    }
    if let Some(fl) = query.filter_lang.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query_suffix.push_str(&format!("&filter-lang={}", percent_encode(fl)));
    }
    let mut links = vec![
        Link::new(
            format!("{items_base}?offset={offset}&limit={limit}{query_suffix}"),
            "self",
            GEOJSON_MEDIA_TYPE,
        ),
        Link::new(coll_base, REL_COLLECTION, JSON_MEDIA_TYPE),
    ];
    if has_more {
        let next_offset = offset.saturating_add(limit);
        links.push(Link::new(
            format!("{items_base}?offset={next_offset}&limit={limit}{query_suffix}"),
            "next",
            GEOJSON_MEDIA_TYPE,
        ));
    }
    if offset > 0 && matched_total > 0 {
        let prev_offset = offset.saturating_sub(limit);
        links.push(Link::new(
            format!("{items_base}?offset={prev_offset}&limit={limit}{query_suffix}"),
            "prev",
            GEOJSON_MEDIA_TYPE,
        ));
    }

    let body = json!({
        "type": "FeatureCollection",
        "features": features,
        "numberMatched": matched_total,
        "numberReturned": features.len(),
        "links": links
    });
    Ok(json_response(GEOJSON_MEDIA_TYPE, body))
}

pub async fn get_dimension_item(
    State(_ctx): State<Arc<AppContext>>,
    Path(params): Path<std::collections::HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let did = params
        .get("did")
        .ok_or_else(|| ApiError::bad_request("Missing dimension id"))?;
    let member_id = params
        .get("memberId")
        .ok_or_else(|| ApiError::bad_request("Missing member id"))?;
    let entry = find_dimension(did)
        .ok_or_else(|| ApiError::not_found(format!("Dimension '{did}' not found")))?;
    let feat = entry
        .spec
        .member_by_id(member_id)
        .ok_or_else(|| ApiError::not_found(format!("Member '{member_id}' not found in '{did}'")))?;
    Ok(json_response(GEOJSON_MEDIA_TYPE, feat))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dekadal_leap_year_and_date_lookup_are_bijective() {
        let spec = DimensionSpec::Dekadal {
            start_year: 2020,
            end_year: 2024,
        };
        assert_eq!(spec.len(), 5 * 36);

        let feb24 = spec.member_by_id("2024-02-D3").expect("2024-02-D3 exists");
        assert_eq!(
            feb24["properties"]["end_datetime"],
            "2024-02-29T23:59:59Z"
        );
        let feb21 = spec.member_by_id("2021-02-D3").expect("2021-02-D3 exists");
        assert_eq!(
            feb21["properties"]["end_datetime"],
            "2021-02-28T23:59:59Z"
        );

        let idx = spec.index_of_date("2024-02-29T14:30:00Z").expect("date resolves");
        assert_eq!(spec.member_at(idx).unwrap()["id"], "2024-02-D3");
        assert!(spec.index_of_date("2019-12-31").is_err());
    }

    #[test]
    fn pentadal_and_integer_range_work() {
        let pent = DimensionSpec::Pentadal {
            start_year: 2021,
            end_year: 2021,
        };
        assert_eq!(pent.len(), 72);
        let p3_idx = pent.index_of_date("2021-06-13").expect("pentad resolves");
        assert_eq!(pent.member_at(p3_idx).unwrap()["id"], "2021-06-P3");

        let elev = DimensionSpec::IntegerRange {
            start: 0,
            end: 5000,
            step: 100,
        };
        assert_eq!(elev.len(), 51);
        let band_idx = elev.index_of_date("1550").expect("elevation band resolves");
        assert_eq!(elev.member_at(band_idx).unwrap()["id"], "1500");
    }

    #[tokio::test]
    async fn dimensions_router_serves_algorithmic_collections_items_and_cql2_filter() {
        use axum::body::{to_bytes, Body};
        use axum::http::{Request, StatusCode};
        use tellurion_core::{
            AppConfig, FileStyleStore, MokaTileCache, Registry, Resolver, Router as CoreRouter,
            StaticResolver, StyleStore, TileCache,
        };
        use tower::ServiceExt;

        let config: AppConfig = serde_yaml::from_str("version: 1\n").unwrap();
        let core_router = CoreRouter::build(&config, &Registry::new()).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let ctx = Arc::new(AppContext::new(
            config,
            core_router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = crate::router().with_state(ctx);

        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/dimensions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["collections"].as_array().unwrap().len() >= 3);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/dimensions/dekadal-2021/items?limit=12&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["numberMatched"], 36);
        assert_eq!(json["numberReturned"], 12);
        assert_eq!(json["features"][0]["id"], "2021-01-D1");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/dimensions/dekadal-2021/items/2021-02-D3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["properties"]["end_datetime"], "2021-02-28T23:59:59Z");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/dimensions/dekadal-2021/items?datetime=2021-06-15")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["numberMatched"], 1);
        assert_eq!(json["features"][0]["id"], "2021-06-D2");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/dimensions/dekadal-2021/items?filter=T_INTERSECTS(time,TIMESTAMP(%272021-06-15T00:00:00Z%27))&filter-lang=cql2-text")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["numberMatched"], 1);
        assert_eq!(json["features"][0]["id"], "2021-06-D2");

        let resp_removed = app
            .oneshot(
                Request::builder()
                    .uri("/dimensions/dekadal-2021/inverse?value=2021-06-15")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_removed.status(), StatusCode::NOT_FOUND);
    }
}
