use serde_json::json;
use tellurion_memory::MemoryDataset;

#[test]
fn dataset_accepts_a_valid_feature_collection() {
    let dataset = MemoryDataset::from_feature_collection(
        "roads",
        json!({
            "type": "FeatureCollection",
            "features": [
                {
                    "type": "Feature",
                    "id": "b",
                    "geometry": {"type": "Point", "coordinates": [3.0, 4.0]},
                    "properties": {"name": "Broadway"}
                },
                {
                    "type": "Feature",
                    "id": 1,
                    "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
                    "properties": {"name": "First"}
                }
            ]
        }),
    )
    .expect("valid GeoJSON should preload");

    assert_eq!(dataset.name(), "roads");
    assert_eq!(dataset.len(), 2);
}

#[test]
fn dataset_rejects_invalid_feature_collections() {
    let invalid_documents = [
        json!({"type": "Point", "coordinates": [1, 2]}),
        json!({"type": "FeatureCollection", "features": [{
            "type": "Feature", "geometry": null, "properties": {}
        }]}),
        json!({"type": "FeatureCollection", "features": [{
            "type": "Feature", "id": true, "geometry": null, "properties": {}
        }]}),
        json!({"type": "FeatureCollection", "features": [{
            "type": "Feature", "id": "a", "geometry": null, "properties": []
        }]}),
        json!({"type": "FeatureCollection", "features": [{
            "type": "Feature", "id": "a",
            "geometry": {"type": "Point", "coordinates": [1]}, "properties": {}
        }]}),
        json!({"type": "FeatureCollection", "features": [{
            "type": "Feature", "id": "a",
            "geometry": {"type": "Unknown", "coordinates": [1, 2]}, "properties": {}
        }]}),
    ];

    for document in invalid_documents {
        assert!(MemoryDataset::from_feature_collection("invalid", document).is_err());
    }
}

#[test]
fn dataset_rejects_duplicate_stringified_ids() {
    let result = MemoryDataset::from_feature_collection(
        "duplicate",
        json!({"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": 1, "geometry": null, "properties": {}},
            {"type": "Feature", "id": "1", "geometry": null, "properties": {}}
        ]}),
    );

    assert!(result.is_err());
}
