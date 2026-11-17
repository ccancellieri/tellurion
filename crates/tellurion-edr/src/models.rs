//! Serde models for OGC API - EDR Part 1 v1.1 and CoverageJSON (OGC 21-069).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub href: String,
    pub rel: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Link {
    pub fn new(href: impl Into<String>, rel: impl Into<String>, media_type: &str) -> Self {
        Self {
            href: href.into(),
            rel: rel.into(),
            media_type: Some(media_type.to_string()),
            title: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrLandingPage {
    pub title: String,
    pub description: String,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrConformance {
    #[serde(rename = "conformsTo")]
    pub conforms_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrExtent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spatial: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrDataQueryLink {
    pub link: Link,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrParameter {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<Value>,
    #[serde(rename = "observedProperty")]
    pub observed_property: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrCollection {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extent: Option<EdrExtent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_queries: Option<BTreeMap<String, EdrDataQueryLink>>,
    pub crs: Vec<String>,
    pub output_formats: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter_names: Option<BTreeMap<String, EdrParameter>>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrCollections {
    pub collections: Vec<EdrCollection>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdrInstances {
    pub instances: Vec<EdrCollection>,
    pub links: Vec<Link>,
}

/// CoverageJSON (OGC 21-069) Coverage document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coverage {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub domain: Domain,
    pub parameters: BTreeMap<String, CovJsonParameter>,
    pub ranges: BTreeMap<String, NdArray>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Domain {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(rename = "domainType")]
    pub domain_type: &'static str,
    pub axes: BTreeMap<String, Axis>,
    pub referencing: Vec<Referencing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Axis {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num: Option<usize>,
    /// Explicit `[min, max]` bounds pairs for irregular intervals (e.g. dekadal/pentadal).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Referencing {
    pub coordinates: Vec<String>,
    pub system: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CovJsonParameter {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<Value>,
    #[serde(rename = "observedProperty")]
    pub observed_property: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NdArray {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(rename = "dataType")]
    pub data_type: &'static str,
    #[serde(rename = "axisNames")]
    pub axis_names: Vec<String>,
    pub shape: Vec<usize>,
    /// Row-major values; `None` encodes JSON `null` for nodata, masked polygons, or sparse gaps.
    pub values: Vec<Option<f64>>,
}
