use std::collections::{btree_map::Entry, BTreeMap};
use std::ops::Bound;

use serde_json::{Map, Value};
use tellurion_core::{FeaturePage, ItemsQuery, Result as CoreResult};

use crate::MemoryDriverError;

/// One validated immutable GeoJSON FeatureCollection.
#[derive(Debug, Clone)]
pub struct MemoryDataset {
    name: String,
    features: BTreeMap<String, StoredFeature>,
    extent: Option<Envelope>,
    geometry_type: Option<String>,
    attributes: BTreeMap<String, PropertyKind>,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredFeature {
    pub(crate) value: Value,
    pub(crate) envelope: Option<Envelope>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Envelope([f64; 4]);

impl Envelope {
    fn point(x: f64, y: f64) -> Self {
        Self([x, y, x, y])
    }

    fn include(&mut self, other: Self) {
        self.0[0] = self.0[0].min(other.0[0]);
        self.0[1] = self.0[1].min(other.0[1]);
        self.0[2] = self.0[2].max(other.0[2]);
        self.0[3] = self.0[3].max(other.0[3]);
    }

    fn intersects(self, bbox: [f64; 4]) -> bool {
        self.0[0] <= bbox[2] && self.0[2] >= bbox[0] && self.0[1] <= bbox[3] && self.0[3] >= bbox[1]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropertyKind {
    Boolean,
    Bigint,
    Double,
    Text,
    Json,
    Null,
}

impl PropertyKind {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Boolean,
            Value::Number(number) if number.is_i64() || number.is_u64() => Self::Bigint,
            Value::Number(_) => Self::Double,
            Value::String(_) => Self::Text,
            Value::Array(_) | Value::Object(_) => Self::Json,
        }
    }

    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Null, kind) | (kind, Self::Null) => kind,
            (left, right) if left == right => left,
            _ => Self::Json,
        }
    }

    fn sql_type(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Bigint => "bigint",
            Self::Double => "double precision",
            Self::Text => "text",
            Self::Json | Self::Null => "json",
        }
    }
}

impl MemoryDataset {
    /// Validates and indexes a GeoJSON FeatureCollection under a physical name.
    pub fn from_feature_collection(
        name: impl Into<String>,
        document: Value,
    ) -> Result<Self, MemoryDriverError> {
        let object = document.as_object().ok_or_else(|| {
            MemoryDriverError::Configuration("GeoJSON document must be an object".into())
        })?;
        if object.get("type").and_then(Value::as_str) != Some("FeatureCollection") {
            return Err(MemoryDriverError::Configuration(
                "GeoJSON document must be a FeatureCollection".into(),
            ));
        }
        let input = object
            .get("features")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                MemoryDriverError::Configuration(
                    "GeoJSON FeatureCollection must contain a features array".into(),
                )
            })?;
        let mut features = BTreeMap::new();
        let mut extent: Option<Envelope> = None;
        let mut common_geometry_type: Option<String> = None;
        let mut mixed_geometry_types = false;
        let mut attributes = BTreeMap::new();
        for feature in input {
            let feature_object = feature.as_object().ok_or_else(|| {
                MemoryDriverError::Configuration("GeoJSON feature must be an object".into())
            })?;
            if feature_object.get("type").and_then(Value::as_str) != Some("Feature") {
                return Err(MemoryDriverError::Configuration(
                    "GeoJSON features must have type Feature".into(),
                ));
            }
            let id = match feature_object.get("id") {
                Some(Value::String(id)) => id.clone(),
                Some(Value::Number(id)) => id.to_string(),
                _ => {
                    return Err(MemoryDriverError::Configuration(
                        "GeoJSON feature id must be a string or number".into(),
                    ))
                }
            };
            let properties = feature_object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    MemoryDriverError::Configuration(format!(
                        "GeoJSON feature '{id}' properties must be an object"
                    ))
                })?;
            if properties.contains_key("geometry") {
                return Err(MemoryDriverError::Configuration(format!(
                    "GeoJSON feature '{id}' uses reserved property name 'geometry'"
                )));
            }
            merge_attribute_types(&mut attributes, properties);

            let (geometry_type, envelope) = match feature_object.get("geometry") {
                Some(Value::Null) => (None, None),
                Some(geometry) => {
                    let (geometry_type, envelope) = validate_geometry(geometry)?;
                    (Some(geometry_type), envelope)
                }
                None => {
                    return Err(MemoryDriverError::Configuration(format!(
                        "GeoJSON feature '{id}' must contain geometry"
                    )))
                }
            };
            if let Some(geometry_type) = geometry_type {
                let geometry_type = geometry_type.to_ascii_uppercase();
                match &common_geometry_type {
                    None => common_geometry_type = Some(geometry_type),
                    Some(common) if common == &geometry_type => {}
                    Some(_) => mixed_geometry_types = true,
                }
            }
            if let Some(envelope) = envelope {
                match &mut extent {
                    Some(current) => current.include(envelope),
                    None => extent = Some(envelope),
                }
            }

            if features
                .insert(
                    id.clone(),
                    StoredFeature {
                        value: feature.clone(),
                        envelope,
                    },
                )
                .is_some()
            {
                return Err(MemoryDriverError::Configuration(format!(
                    "duplicate GeoJSON feature id '{id}'"
                )));
            }
        }
        Ok(Self {
            name: name.into(),
            features,
            extent,
            geometry_type: (!mixed_geometry_types)
                .then_some(common_geometry_type)
                .flatten(),
            attributes,
        })
    }

    /// Returns the physical collection name reported by the catalog source.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the exact feature count.
    pub fn len(&self) -> usize {
        self.features.len()
    }

    /// Returns whether the collection contains no features.
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    pub(crate) fn extent(&self) -> Option<[f64; 4]> {
        self.extent.map(|envelope| envelope.0)
    }

    pub(crate) fn geometry_type(&self) -> Option<&str> {
        self.geometry_type.as_deref()
    }

    pub(crate) fn attribute_schema(&self) -> Vec<(&str, &str)> {
        self.attributes
            .iter()
            .map(|(name, kind)| (name.as_str(), kind.sql_type()))
            .collect()
    }

    pub(crate) fn item(&self, id: &str) -> Option<Value> {
        self.features.get(id).map(|feature| feature.value.clone())
    }

    pub(crate) fn items(&self, query: &ItemsQuery) -> CoreResult<FeaturePage> {
        if query.limit == 0 {
            return Err(MemoryDriverError::InvalidQuery(
                "page limit must be greater than zero".into(),
            )
            .into());
        }
        if query.datetime.is_some() {
            return Err(MemoryDriverError::InvalidQuery(
                "datetime filtering is not supported by the memory driver".into(),
            )
            .into());
        }
        if query.filter.is_some() {
            return Err(MemoryDriverError::InvalidQuery(
                "CQL2 filtering is not supported by the memory driver".into(),
            )
            .into());
        }
        if let Some(bbox) = query.bbox {
            validate_bbox(bbox)?;
        }

        let cursor = query.token.as_deref().map(decode_token).transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|id| !self.features.contains_key(id))
        {
            return Err(MemoryDriverError::InvalidQuery(
                "paging token does not name a feature in this dataset".into(),
            )
            .into());
        }

        let matches = |feature: &&StoredFeature| {
            query.bbox.is_none_or(|bbox| {
                feature
                    .envelope
                    .is_some_and(|envelope| envelope.intersects(bbox))
            })
        };
        let number_matched = self.features.values().filter(matches).count() as u64;
        let start = cursor.as_deref().map_or(Bound::Unbounded, Bound::Excluded);
        let mut selected: Vec<_> = self
            .features
            .range::<str, _>((start, Bound::Unbounded))
            .filter(|(_, feature)| matches(feature))
            .take(query.limit as usize + 1)
            .collect();
        let has_more = selected.len() > query.limit as usize;
        selected.truncate(query.limit as usize);
        let next_token = has_more
            .then(|| selected.last().map(|(id, _)| encode_token(id)))
            .flatten();
        let features_geojson = selected
            .into_iter()
            .map(|(_, feature)| feature.value.clone())
            .collect();

        Ok(FeaturePage {
            features_geojson,
            number_matched: Some(number_matched),
            next_token,
        })
    }
}

fn validate_bbox(bbox: [f64; 4]) -> CoreResult<()> {
    if bbox.iter().any(|coordinate| !coordinate.is_finite())
        || bbox[0] > bbox[2]
        || bbox[1] > bbox[3]
    {
        return Err(MemoryDriverError::InvalidQuery(
            "bbox must contain finite coordinates ordered minx,miny,maxx,maxy".into(),
        )
        .into());
    }
    Ok(())
}

fn encode_token(id: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(3 + id.len() * 2);
    encoded.push_str("v1.");
    for byte in id.bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_token(token: &str) -> CoreResult<String> {
    let encoded = token.strip_prefix("v1.").ok_or_else(|| {
        tellurion_core::Error::from(MemoryDriverError::InvalidQuery(
            "paging token has an unsupported version".into(),
        ))
    })?;
    if encoded.len() % 2 != 0
        || encoded
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(MemoryDriverError::InvalidQuery(
            "paging token must contain canonical lowercase hexadecimal bytes".into(),
        )
        .into());
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_value(pair[0]);
            let low = hex_value(pair[1]);
            (high << 4) | low
        })
        .collect::<Vec<_>>();
    String::from_utf8(bytes).map_err(|_| {
        MemoryDriverError::InvalidQuery("paging token is not valid UTF-8".into()).into()
    })
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("token characters are validated before decoding"),
    }
}

fn merge_attribute_types(
    attributes: &mut BTreeMap<String, PropertyKind>,
    properties: &Map<String, Value>,
) {
    for (name, value) in properties {
        let observed = PropertyKind::from_value(value);
        match attributes.entry(name.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(observed);
            }
            Entry::Occupied(mut entry) => {
                *entry.get_mut() = entry.get().merge(observed);
            }
        }
    }
}

fn validate_geometry(value: &Value) -> Result<(&str, Option<Envelope>), MemoryDriverError> {
    let geometry = value.as_object().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON geometry must be an object".into())
    })?;
    let geometry_type = geometry
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            MemoryDriverError::Configuration("GeoJSON geometry must have a string type".into())
        })?;

    let envelope = match geometry_type {
        "Point" => position(required_coordinates(geometry)?)?,
        "MultiPoint" => positions(required_coordinates(geometry)?, 1)?,
        "LineString" => positions(required_coordinates(geometry)?, 2)?,
        "MultiLineString" => nested_positions(required_coordinates(geometry)?, 1, 2, false)?,
        "Polygon" => polygon(required_coordinates(geometry)?)?,
        "MultiPolygon" => multi_polygon(required_coordinates(geometry)?)?,
        "GeometryCollection" => {
            let geometries = geometry
                .get("geometries")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    MemoryDriverError::Configuration(
                        "GeoJSON GeometryCollection must contain a geometries array".into(),
                    )
                })?;
            let mut envelope = None;
            for child in geometries {
                if let Some(child_envelope) = validate_geometry(child)?.1 {
                    merge_envelope(&mut envelope, child_envelope);
                }
            }
            envelope
        }
        other => {
            return Err(MemoryDriverError::Configuration(format!(
                "unsupported GeoJSON geometry type '{other}'"
            )))
        }
    };
    Ok((geometry_type, envelope))
}

fn required_coordinates(geometry: &Map<String, Value>) -> Result<&Value, MemoryDriverError> {
    geometry.get("coordinates").ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON geometry must contain coordinates".into())
    })
}

fn position(value: &Value) -> Result<Option<Envelope>, MemoryDriverError> {
    let coordinates = value.as_array().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON position must be an array".into())
    })?;
    if coordinates.len() < 2 || coordinates.iter().any(|coordinate| !coordinate.is_number()) {
        return Err(MemoryDriverError::Configuration(
            "GeoJSON position must contain at least two numbers".into(),
        ));
    }
    let x = coordinates[0].as_f64().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON longitude must be finite".into())
    })?;
    let y = coordinates[1].as_f64().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON latitude must be finite".into())
    })?;
    if !x.is_finite() || !y.is_finite() {
        return Err(MemoryDriverError::Configuration(
            "GeoJSON coordinates must be finite".into(),
        ));
    }
    Ok(Some(Envelope::point(x, y)))
}

fn positions(value: &Value, minimum: usize) -> Result<Option<Envelope>, MemoryDriverError> {
    let positions = value.as_array().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON coordinate sequence must be an array".into())
    })?;
    if positions.len() < minimum {
        return Err(MemoryDriverError::Configuration(format!(
            "GeoJSON coordinate sequence must contain at least {minimum} positions"
        )));
    }
    let mut envelope = None;
    for coordinate in positions {
        merge_envelope(
            &mut envelope,
            position(coordinate)?.expect("a valid position has bounds"),
        );
    }
    Ok(envelope)
}

fn nested_positions(
    value: &Value,
    minimum_groups: usize,
    minimum_positions: usize,
    closed: bool,
) -> Result<Option<Envelope>, MemoryDriverError> {
    let groups = value.as_array().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON coordinate groups must be an array".into())
    })?;
    if groups.len() < minimum_groups {
        return Err(MemoryDriverError::Configuration(format!(
            "GeoJSON coordinates must contain at least {minimum_groups} group"
        )));
    }
    let mut envelope = None;
    for group in groups {
        if closed {
            validate_closed_ring(group)?;
        }
        if let Some(group_envelope) = positions(group, minimum_positions)? {
            merge_envelope(&mut envelope, group_envelope);
        }
    }
    Ok(envelope)
}

fn validate_closed_ring(value: &Value) -> Result<(), MemoryDriverError> {
    let positions = value.as_array().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON linear ring must be an array".into())
    })?;
    if positions.len() < 4
        || !positions
            .first()
            .zip(positions.last())
            .is_some_and(|(first, last)| positions_are_equal(first, last))
    {
        return Err(MemoryDriverError::Configuration(
            "GeoJSON linear ring must contain four positions and be closed".into(),
        ));
    }
    Ok(())
}

fn positions_are_equal(left: &Value, right: &Value) -> bool {
    left.as_array()
        .zip(right.as_array())
        .is_some_and(|(left, right)| {
            left.len() == right.len()
                && left.iter().zip(right).all(|(left, right)| {
                    left.as_f64()
                        .zip(right.as_f64())
                        .is_some_and(|(left, right)| left == right)
                })
        })
}

fn polygon(value: &Value) -> Result<Option<Envelope>, MemoryDriverError> {
    nested_positions(value, 1, 4, true)
}

fn multi_polygon(value: &Value) -> Result<Option<Envelope>, MemoryDriverError> {
    let polygons = value.as_array().ok_or_else(|| {
        MemoryDriverError::Configuration("GeoJSON MultiPolygon coordinates must be an array".into())
    })?;
    if polygons.is_empty() {
        return Err(MemoryDriverError::Configuration(
            "GeoJSON MultiPolygon must contain at least one polygon".into(),
        ));
    }
    let mut envelope = None;
    for coordinates in polygons {
        if let Some(polygon_envelope) = polygon(coordinates)? {
            merge_envelope(&mut envelope, polygon_envelope);
        }
    }
    Ok(envelope)
}

fn merge_envelope(target: &mut Option<Envelope>, value: Envelope) {
    match target {
        Some(current) => current.include(value),
        None => *target = Some(value),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tellurion_core::{DatetimeRange, Error, Filter, ItemsQuery};

    use super::MemoryDataset;
    use crate::MemoryDriverError;

    #[test]
    fn derives_extent_geometry_type_and_attribute_schema() {
        let dataset = MemoryDataset::from_feature_collection(
            "roads",
            json!({"type": "FeatureCollection", "features": [
                {"type": "Feature", "id": "b",
                 "geometry": {"type": "LineString", "coordinates": [[3.0, 4.0], [5.0, 8.0]]},
                 "properties": {"active": true, "lanes": 2, "rating": 4.5, "name": "B", "mixed": 1, "empty": null}},
                {"type": "Feature", "id": "a",
                 "geometry": {"type": "LineString", "coordinates": [[-1.0, 2.0], [4.0, 6.0]]},
                 "properties": {"active": false, "lanes": 4, "rating": 3.0, "name": "A", "mixed": "one", "empty": null}},
                {"type": "Feature", "id": "c", "geometry": null,
                 "properties": {"structured": [1, 2, 3]}}
            ]}),
        )
        .unwrap();

        assert_eq!(dataset.extent(), Some([-1.0, 2.0, 5.0, 8.0]));
        assert_eq!(dataset.geometry_type(), Some("LINESTRING"));
        assert_eq!(
            dataset.attribute_schema(),
            vec![
                ("active", "boolean"),
                ("empty", "json"),
                ("lanes", "bigint"),
                ("mixed", "json"),
                ("name", "text"),
                ("rating", "double precision"),
                ("structured", "json"),
            ]
        );
    }

    #[test]
    fn mixed_geometry_types_do_not_claim_one_type() {
        let dataset = MemoryDataset::from_feature_collection(
            "mixed",
            json!({"type": "FeatureCollection", "features": [
                {"type": "Feature", "id": "a", "geometry": {"type": "Point", "coordinates": [1, 2]}, "properties": {}},
                {"type": "Feature", "id": "b", "geometry": {"type": "Polygon", "coordinates": [[[0, 0], [2, 0], [2, 2], [0, 0]]]}, "properties": {}}
            ]}),
        )
        .unwrap();

        assert_eq!(dataset.geometry_type(), None);
    }

    #[test]
    fn polygon_ring_closure_compares_numeric_coordinate_values() {
        let result = MemoryDataset::from_feature_collection(
            "polygon",
            json!({"type": "FeatureCollection", "features": [
                {"type": "Feature", "id": "a",
                 "geometry": {"type": "Polygon", "coordinates": [[[0, 0], [2, 0], [2, 2], [0.0, 0.0]]]},
                 "properties": {}}
            ]}),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_properties_that_collide_with_reserved_columns() {
        let result = MemoryDataset::from_feature_collection(
            "places",
            json!({"type": "FeatureCollection", "features": [
                {"type": "Feature", "id": "a",
                 "geometry": {"type": "Point", "coordinates": [0, 0]},
                 "properties": {"geometry": "shadow"}}
            ]}),
        );

        assert!(matches!(
            result,
            Err(MemoryDriverError::Configuration(message))
                if message.contains("reserved property name 'geometry'")
        ));
    }

    fn query_dataset() -> MemoryDataset {
        MemoryDataset::from_feature_collection(
            "places",
            json!({"type": "FeatureCollection", "features": [
                {"type": "Feature", "id": "c", "geometry": null, "properties": {"name": "Null Island"}},
                {"type": "Feature", "id": "a", "geometry": {"type": "Point", "coordinates": [0, 0]}, "properties": {"name": "Origin"}},
                {"type": "Feature", "id": "d", "geometry": {"type": "Point", "coordinates": [10, 10]}, "properties": {"name": "Far"}},
                {"type": "Feature", "id": "b", "geometry": {"type": "LineString", "coordinates": [[1, 1], [2, 2]]}, "properties": {"name": "Line"}}
            ]}),
        )
        .unwrap()
    }

    #[test]
    fn pages_all_features_once_in_stable_id_order() {
        let dataset = query_dataset();
        let mut token = None;
        let mut ids = Vec::new();

        loop {
            let page = dataset
                .items(&ItemsQuery {
                    limit: 2,
                    token,
                    ..ItemsQuery::default()
                })
                .unwrap();
            assert_eq!(page.number_matched, Some(4));
            ids.extend(page.features_geojson.iter().map(|feature| {
                feature
                    .get("id")
                    .and_then(|id| id.as_str())
                    .unwrap()
                    .to_string()
            }));
            match page.next_token {
                Some(next) => token = Some(next),
                None => break,
            }
        }

        assert_eq!(ids, ["a", "b", "c", "d"]);
    }

    #[test]
    fn bbox_counts_all_matches_and_includes_boundary_touches() {
        let dataset = query_dataset();
        let page = dataset
            .items(&ItemsQuery {
                limit: 1,
                bbox: Some([2.0, 2.0, 10.0, 10.0]),
                ..ItemsQuery::default()
            })
            .unwrap();

        assert_eq!(page.number_matched, Some(2));
        assert_eq!(page.features_geojson[0]["id"], "b");
        assert!(page.next_token.is_some());
    }

    #[test]
    fn item_lookup_is_exact() {
        let dataset = query_dataset();
        assert_eq!(dataset.item("a").unwrap()["properties"]["name"], "Origin");
        assert!(dataset.item("missing").is_none());
    }

    #[test]
    fn invalid_queries_are_refused() {
        let dataset = query_dataset();
        let invalid = [
            ItemsQuery {
                limit: 0,
                ..ItemsQuery::default()
            },
            ItemsQuery {
                bbox: Some([2.0, 0.0, 1.0, 1.0]),
                ..ItemsQuery::default()
            },
            ItemsQuery {
                token: Some("v2.61".into()),
                ..ItemsQuery::default()
            },
            ItemsQuery {
                token: Some("v1.6G".into()),
                ..ItemsQuery::default()
            },
            ItemsQuery {
                token: Some("v1.7A".into()),
                ..ItemsQuery::default()
            },
            ItemsQuery {
                token: Some("v1.7a".into()),
                ..ItemsQuery::default()
            },
            ItemsQuery {
                datetime: Some(DatetimeRange {
                    start: Some("2024-01-01T00:00:00Z".into()),
                    end: None,
                }),
                ..ItemsQuery::default()
            },
            ItemsQuery {
                filter: Some(Filter::IsNull {
                    property: "name".into(),
                    negated: false,
                }),
                ..ItemsQuery::default()
            },
        ];

        for query in invalid {
            assert!(matches!(dataset.items(&query), Err(Error::Invalid(_))));
        }
    }
}
