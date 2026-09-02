//! Exact item-response geometry budgeting.

use std::sync::Arc;

use serde_json::Value;

use crate::config::CollectionDecl;
use crate::crs::RequestedCrs;
use crate::error::{Error, Result};
use crate::filter::Filter;
use crate::settings::DEFAULT_ITEMS_VERTEX_BUDGET;
use crate::storage::{FeaturePage, FeatureSource, ItemsQuery};

/// Counts the source-geometry vertices carried by one GeoJSON Feature.
pub fn feature_vertex_count(feature: &Value) -> u64 {
    feature
        .get("geometry")
        .and_then(geometry_vertex_count)
        .unwrap_or(0)
}

/// Refuses a page when adding its first over-budget feature would cross
/// `limit`. Geometry is never simplified, dropped, or partially returned.
pub fn check_feature_page(collection: &str, features: &[Value], limit: u64) -> Result<()> {
    let mut cumulative_vertices = 0_u64;
    for feature in features {
        cumulative_vertices = cumulative_vertices.saturating_add(feature_vertex_count(feature));
        if cumulative_vertices > limit {
            return Err(Error::ItemsVertexBudgetExceeded {
                collection: collection.to_string(),
                feature_id: feature_id(feature),
                cumulative_vertices,
                limit,
            });
        }
    }
    Ok(())
}

/// Single-feature counterpart of [`check_feature_page`].
pub fn check_feature(collection: &str, feature: &Value, limit: u64) -> Result<()> {
    check_feature_page(collection, std::slice::from_ref(feature), limit)
}

/// Wraps a feature source with the mandatory driver-neutral exact-response
/// budget check. Drivers may enforce the same contract earlier to avoid
/// encoding work; this wrapper keeps every backend correct at the response
/// boundary.
pub(crate) fn budget_feature_source(inner: Arc<dyn FeatureSource>) -> Arc<dyn FeatureSource> {
    Arc::new(BudgetedFeatureSource { inner })
}

struct BudgetedFeatureSource {
    inner: Arc<dyn FeatureSource>,
}

#[async_trait::async_trait]
impl FeatureSource for BudgetedFeatureSource {
    async fn items(&self, collection: &CollectionDecl, query: &ItemsQuery) -> Result<FeaturePage> {
        let page = self.inner.items(collection, query).await?;
        let limit = effective_limit(collection);
        if let Err(error) =
            check_feature_page(collection.external_id(), &page.features_geojson, limit)
        {
            record_generic_refusal(&error);
            return Err(error);
        }
        Ok(page)
    }

    async fn item(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> Result<Option<Value>> {
        let item = self.inner.item(collection, id, filter).await?;
        check_optional_item(collection, item)
    }

    fn filter_capable(&self) -> bool {
        self.inner.filter_capable()
    }

    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        self.inner.cql2_conformance_classes()
    }

    fn crs_capable(&self) -> bool {
        self.inner.crs_capable()
    }

    fn filter_crs_capable(&self) -> bool {
        self.inner.filter_crs_capable()
    }

    async fn item_with_crs(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
        requested_crs: RequestedCrs,
    ) -> Result<Option<Value>> {
        let item = self
            .inner
            .item_with_crs(collection, id, filter, requested_crs)
            .await?;
        check_optional_item(collection, item)
    }
}

fn check_optional_item(collection: &CollectionDecl, item: Option<Value>) -> Result<Option<Value>> {
    if let Some(feature) = item.as_ref() {
        if let Err(error) = check_feature(
            collection.external_id(),
            feature,
            effective_limit(collection),
        ) {
            record_generic_refusal(&error);
            return Err(error);
        }
    }
    Ok(item)
}

fn effective_limit(collection: &CollectionDecl) -> u64 {
    collection
        .settings
        .items_vertex_budget
        .unwrap_or(DEFAULT_ITEMS_VERTEX_BUDGET)
}

fn record_generic_refusal(error: &Error) {
    if let Error::ItemsVertexBudgetExceeded {
        collection,
        feature_id,
        cumulative_vertices,
        limit,
    } = error
    {
        metrics::counter!("items_vertex_budget_exceeded_total", "backend" => "generic")
            .increment(1);
        tracing::warn!(
            collection,
            feature_id,
            cumulative_vertices,
            limit,
            backend = "generic",
            "exact item geometry exceeded the configured vertex budget"
        );
    }
}

fn geometry_vertex_count(geometry: &Value) -> Option<u64> {
    if geometry.is_null() {
        return Some(0);
    }
    let geometry_type = geometry.get("type")?.as_str()?;
    match geometry_type {
        "Point" => position(geometry.get("coordinates")?).map(|()| 1),
        "MultiPoint" | "LineString" => positions(geometry.get("coordinates")?),
        "MultiLineString" | "Polygon" => nested_positions(geometry.get("coordinates")?, 2),
        "MultiPolygon" => nested_positions(geometry.get("coordinates")?, 3),
        "GeometryCollection" => geometry
            .get("geometries")?
            .as_array()?
            .iter()
            .try_fold(0_u64, |total, child| {
                geometry_vertex_count(child).map(|count| total.saturating_add(count))
            }),
        _ => None,
    }
}

fn position(value: &Value) -> Option<()> {
    let coordinates = value.as_array()?;
    (coordinates.len() >= 2 && coordinates.iter().all(Value::is_number)).then_some(())
}

fn positions(value: &Value) -> Option<u64> {
    value
        .as_array()?
        .iter()
        .try_fold(0_u64, |total, coordinate| {
            position(coordinate).map(|()| total.saturating_add(1))
        })
}

fn nested_positions(value: &Value, depth: usize) -> Option<u64> {
    if depth == 1 {
        return positions(value);
    }
    value.as_array()?.iter().try_fold(0_u64, |total, child| {
        nested_positions(child, depth - 1).map(|count| total.saturating_add(count))
    })
}

fn feature_id(feature: &Value) -> String {
    match feature.get("id") {
        Some(Value::String(id)) => id.clone(),
        Some(Value::Number(id)) => id.to_string(),
        Some(Value::Bool(id)) => id.to_string(),
        _ => "<unknown>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::{
        CollectionDecl, Error, FeaturePage, FeatureSource, Filter, ItemsQuery, RequestedCrs,
    };

    struct StaticSource {
        page: FeaturePage,
        item: Option<Value>,
    }

    #[async_trait::async_trait]
    impl FeatureSource for StaticSource {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> Result<FeaturePage> {
            Ok(self.page.clone())
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> Result<Option<Value>> {
            Ok(self.item.clone())
        }

        fn filter_capable(&self) -> bool {
            true
        }

        fn crs_capable(&self) -> bool {
            true
        }

        async fn item_with_crs(
            &self,
            collection: &CollectionDecl,
            id: &str,
            filter: Option<&Filter>,
            _requested_crs: RequestedCrs,
        ) -> Result<Option<Value>> {
            self.item(collection, id, filter).await
        }
    }

    #[test]
    fn counts_every_geojson_geometry_family_and_recursive_collection() {
        let cases = [
            (json!({"geometry":{"type":"Point","coordinates":[1,2]}}), 1),
            (
                json!({"geometry":{"type":"MultiPoint","coordinates":[[1,2],[3,4]]}}),
                2,
            ),
            (
                json!({"geometry":{"type":"LineString","coordinates":[[1,2],[3,4],[5,6]]}}),
                3,
            ),
            (
                json!({"geometry":{"type":"MultiLineString","coordinates":[[[1,2],[3,4]],[[5,6]]]}}),
                3,
            ),
            (
                json!({"geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]],[[0.2,0.2],[0.3,0.2],[0.2,0.2]]]}}),
                7,
            ),
            (
                json!({"geometry":{
                    "type":"MultiPolygon",
                    "coordinates":[
                        [[[0,0],[1,0],[0,0]]],
                        [[[2,2],[3,2],[2,2]]]
                    ]
                }}),
                6,
            ),
            (
                json!({"geometry":{"type":"GeometryCollection","geometries":[
                    {"type":"Point","coordinates":[1,2]},
                    {"type":"GeometryCollection","geometries":[
                        {"type":"LineString","coordinates":[[0,0],[1,1]]}
                    ]}
                ]}}),
                3,
            ),
        ];

        for (feature, expected) in cases {
            assert_eq!(feature_vertex_count(&feature), expected, "{feature}");
        }
    }

    #[test]
    fn absent_null_and_malformed_geometry_contribute_zero() {
        for feature in [
            json!({}),
            json!({"geometry":null}),
            json!({"geometry":{"type":"Point","coordinates":"not-an-array"}}),
            json!({"geometry":{"type":"Unknown","coordinates":[[1,2]]}}),
        ] {
            assert_eq!(feature_vertex_count(&feature), 0, "{feature}");
        }
    }

    #[test]
    fn exact_budget_is_accepted_and_first_crossing_feature_is_named() {
        let features = vec![
            json!({"type":"Feature","id":"a","geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]}}),
            json!({"type":"Feature","id":2,"geometry":{"type":"MultiPoint","coordinates":[[2,2],[3,3]]}}),
        ];

        check_feature_page("places", &features, 4).unwrap();
        let error = check_feature_page("places", &features, 3).unwrap_err();
        assert!(matches!(
            error,
            Error::ItemsVertexBudgetExceeded {
                collection,
                feature_id,
                cumulative_vertices: 4,
                limit: 3,
            } if collection == "places" && feature_id == "2"
        ));
    }

    #[test]
    fn single_item_uses_the_same_contract() {
        let feature = json!({
            "type":"Feature",
            "id":"large",
            "geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]}
        });

        check_feature("places", &feature, 2).unwrap();
        assert!(matches!(
            check_feature("places", &feature, 1),
            Err(Error::ItemsVertexBudgetExceeded {
                cumulative_vertices: 2,
                limit: 1,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn decorator_preserves_under_budget_results_and_refuses_whole_over_budget_results() {
        let features = vec![
            json!({"type":"Feature","id":"a","geometry":{"type":"Point","coordinates":[0,0]}}),
            json!({"type":"Feature","id":"b","geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]}}),
        ];
        let page = FeaturePage {
            features_geojson: features.clone(),
            number_matched: Some(2),
            next_token: Some("b".to_string()),
        };
        let source = budget_feature_source(Arc::new(StaticSource {
            page: page.clone(),
            item: Some(features[1].clone()),
        }));
        let mut collection: CollectionDecl =
            serde_yaml::from_str("id: places\ncatalog: default\nstorage: main").unwrap();
        collection.settings.items_vertex_budget = Some(3);

        assert_eq!(
            source
                .items(&collection, &ItemsQuery::default())
                .await
                .unwrap(),
            page
        );
        assert_eq!(
            source
                .item_with_crs(&collection, "b", None, RequestedCrs::Crs84,)
                .await
                .unwrap(),
            Some(features[1].clone())
        );
        assert!(source.filter_capable());
        assert!(source.crs_capable());

        collection.settings.items_vertex_budget = Some(2);
        assert!(matches!(
            source.items(&collection, &ItemsQuery::default()).await,
            Err(Error::ItemsVertexBudgetExceeded {
                feature_id,
                cumulative_vertices: 3,
                limit: 2,
                ..
            }) if feature_id == "b"
        ));
    }
}
