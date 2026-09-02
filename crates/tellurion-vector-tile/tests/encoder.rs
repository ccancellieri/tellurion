use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use geo_types::{
    Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Point,
    Polygon,
};
use geozero::mvt::{Message, Tile};
use tellurion_core::TileCoord;
use tellurion_vector_tile::{
    encode_tile, encode_tile_with_outcome, SourceCrs, TileEncodeError, TileFeature, TileRequest,
    TileScalar,
};

fn request(coord: TileCoord, source_crs: SourceCrs) -> TileRequest {
    TileRequest::new(coord, "demo", Vec::new(), 10, 10_000, 4096, source_crs)
}

fn feature(id: &str, geometry: impl Into<Geometry<f64>>) -> TileFeature {
    TileFeature::new(id, geometry.into(), Vec::new())
}

fn decode(bytes: &[u8]) -> Tile {
    Tile::decode(bytes).expect("encoder returns a valid MVT protobuf")
}

fn feature_ids(tile: &Tile) -> HashSet<String> {
    let layer = &tile.layers[0];
    layer
        .features
        .iter()
        .filter_map(|feature| {
            feature.tags.chunks(2).find_map(|pair| {
                (layer.keys[pair[0] as usize] == "id")
                    .then(|| layer.values[pair[1] as usize].string_value.clone())
                    .flatten()
            })
        })
        .collect()
}

fn polygon(exterior: Vec<(f64, f64)>, holes: Vec<Vec<(f64, f64)>>) -> Polygon<f64> {
    Polygon::new(
        LineString::from(exterior),
        holes.into_iter().map(LineString::from).collect(),
    )
}

#[test]
fn preserves_the_representative_geopackage_point_bytes() {
    let bytes = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        [Ok(feature("1", Point::new(1000.0, 2000.0)))],
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        bytes.as_ref(),
        &[
            26, 35, 10, 4, 100, 101, 109, 111, 18, 13, 18, 2, 0, 0, 24, 1, 34, 5, 9, 128, 32, 128,
            32, 26, 2, 105, 100, 34, 3, 10, 1, 49, 40, 128, 32, 120, 2,
        ]
    );
}

#[test]
fn legacy_policy_preserves_crossing_line_and_polygon_bytes() {
    let cases = [
        (
            feature(
                "1",
                LineString::from(vec![(-25_000_000.0, 0.0), (25_000_000.0, 0.0)]),
            ),
            &[
                26, 39, 10, 4, 100, 101, 109, 111, 18, 17, 18, 2, 0, 0, 24, 2, 34, 9, 9, 247, 7,
                130, 32, 10, 238, 79, 0, 26, 2, 105, 100, 34, 3, 10, 1, 49, 40, 128, 32, 120, 2,
            ][..],
        ),
        (
            feature(
                "1",
                polygon(
                    vec![
                        (15_000_000.0, -5_000_000.0),
                        (25_000_000.0, -5_000_000.0),
                        (25_000_000.0, 5_000_000.0),
                        (15_000_000.0, 5_000_000.0),
                        (15_000_000.0, -5_000_000.0),
                    ],
                    vec![vec![
                        (17_000_000.0, -1_000_000.0),
                        (17_000_000.0, 1_000_000.0),
                        (19_000_000.0, 1_000_000.0),
                        (19_000_000.0, -1_000_000.0),
                        (17_000_000.0, -1_000_000.0),
                    ]],
                ),
            ),
            &[
                26, 62, 10, 4, 100, 101, 109, 111, 18, 40, 18, 2, 0, 0, 24, 3, 34, 32, 9, 250, 55,
                128, 40, 26, 0, 253, 15, 252, 15, 0, 0, 254, 15, 15, 9, 227, 12, 177, 6, 26, 152,
                3, 0, 0, 153, 3, 151, 3, 0, 15, 26, 2, 105, 100, 34, 3, 10, 1, 49, 40, 128, 32,
                120, 2,
            ][..],
        ),
    ];

    for (input, expected) in cases {
        let bytes = encode_tile(
            request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator)
                .preserve_unclipped_geometry(),
            [Ok(input)],
        )
        .unwrap()
        .unwrap();
        assert_eq!(bytes.as_ref(), expected);
    }
}

#[test]
fn default_policy_topology_clips_crossing_lines_and_polygons() {
    let inputs = [
        feature(
            "line",
            LineString::from(vec![(-25_000_000.0, 0.0), (25_000_000.0, 0.0)]),
        ),
        feature(
            "polygon",
            polygon(
                vec![
                    (15_000_000.0, -5_000_000.0),
                    (25_000_000.0, -5_000_000.0),
                    (25_000_000.0, 5_000_000.0),
                    (15_000_000.0, 5_000_000.0),
                    (15_000_000.0, -5_000_000.0),
                ],
                Vec::new(),
            ),
        ),
    ];
    let bytes = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        inputs.into_iter().map(Ok),
    )
    .unwrap()
    .unwrap();

    use geozero::ProcessToJson;
    let mut layer = decode(&bytes).layers.remove(0);
    let decoded: serde_json::Value = serde_json::from_str(&layer.to_json().unwrap()).unwrap();
    for feature in decoded["features"].as_array().unwrap() {
        fn assert_bounded(value: &serde_json::Value) {
            match value {
                serde_json::Value::Array(values) => values.iter().for_each(assert_bounded),
                serde_json::Value::Number(value) => {
                    let value = value.as_f64().unwrap();
                    assert!((0.0..=4096.0).contains(&value));
                }
                _ => {}
            }
        }
        assert_bounded(&feature["geometry"]["coordinates"]);
    }
}

#[test]
fn crs84_projection_uses_a_top_left_tile_origin_and_one_layer() {
    let features = [
        feature("ne", Point::new(45.0, 30.0)),
        feature("nw", Point::new(-45.0, 30.0)),
        feature("se", Point::new(45.0, -30.0)),
        feature("sw", Point::new(-45.0, -30.0)),
    ];
    let bytes = encode_tile(
        request(TileCoord { z: 1, x: 0, y: 0 }, SourceCrs::Crs84),
        features.into_iter().map(Ok),
    )
    .unwrap()
    .unwrap();

    let tile = decode(&bytes);
    assert_eq!(tile.layers.len(), 1);
    assert_eq!(feature_ids(&tile), HashSet::from(["nw".to_string()]));
}

#[test]
fn both_source_winding_directions_encode_the_same_polygon_commands() {
    let ccw_exterior = vec![
        (-2_000_000.0, -2_000_000.0),
        (2_000_000.0, -2_000_000.0),
        (2_000_000.0, 2_000_000.0),
        (-2_000_000.0, 2_000_000.0),
        (-2_000_000.0, -2_000_000.0),
    ];
    let cw_hole = vec![
        (-1_000_000.0, -1_000_000.0),
        (-1_000_000.0, 1_000_000.0),
        (1_000_000.0, 1_000_000.0),
        (1_000_000.0, -1_000_000.0),
        (-1_000_000.0, -1_000_000.0),
    ];
    let mut opposite_exterior = ccw_exterior.clone();
    opposite_exterior.reverse();
    let mut opposite_hole = cw_hole.clone();
    opposite_hole.reverse();

    let encode = |geometry| {
        encode_tile(
            request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
            [Ok(feature("1", geometry))],
        )
        .unwrap()
        .unwrap()
    };
    let conventional = decode(&encode(polygon(ccw_exterior, vec![cw_hole])));
    let opposite = decode(&encode(polygon(opposite_exterior, vec![opposite_hole])));

    assert_eq!(
        conventional.layers[0].features[0].geometry,
        opposite.layers[0].features[0].geometry
    );
}

#[test]
fn polygons_collapsed_by_tile_quantization_are_omitted() {
    let collapsed = polygon(
        vec![
            (1_000.0, 1_000.0),
            (1_100.0, 1_000.0),
            (1_100.0, 1_100.0),
            (1_000.0, 1_100.0),
            (1_000.0, 1_000.0),
        ],
        Vec::new(),
    );
    let visible = polygon(
        vec![
            (-1_000_000.0, -1_000_000.0),
            (1_000_000.0, -1_000_000.0),
            (1_000_000.0, 1_000_000.0),
            (-1_000_000.0, 1_000_000.0),
            (-1_000_000.0, -1_000_000.0),
        ],
        Vec::new(),
    );

    let bytes = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        [
            Ok(feature("collapsed", collapsed)),
            Ok(feature("visible", visible)),
        ],
    )
    .unwrap()
    .unwrap();
    let mut tile = decode(&bytes);

    assert_eq!(feature_ids(&tile), HashSet::from(["visible".to_string()]));
    use geozero::ProcessToJson;
    tile.layers[0]
        .to_json()
        .expect("the encoder must not emit malformed geometry commands");
}

#[test]
fn multipoint_clipped_multiline_and_multipolygon_holes_remain_valid() {
    let half_world = 20_037_508.342_789_244;
    let multipoint = MultiPoint(vec![
        Point::new(0.0, 0.0),
        Point::new(1_000_000.0, 1_000_000.0),
    ]);
    let multiline = MultiLineString(vec![
        LineString::from(vec![(-2.0 * half_world, 0.0), (2.0 * half_world, 0.0)]),
        LineString::from(vec![(0.0, -2.0 * half_world), (0.0, 2.0 * half_world)]),
    ]);
    let multipolygon = MultiPolygon(vec![
        polygon(
            vec![
                (-8_000_000.0, -2_000_000.0),
                (-4_000_000.0, -2_000_000.0),
                (-4_000_000.0, 2_000_000.0),
                (-8_000_000.0, 2_000_000.0),
                (-8_000_000.0, -2_000_000.0),
            ],
            Vec::new(),
        ),
        polygon(
            vec![
                (15_000_000.0, -4_000_000.0),
                (25_000_000.0, -4_000_000.0),
                (25_000_000.0, 4_000_000.0),
                (15_000_000.0, 4_000_000.0),
                (15_000_000.0, -4_000_000.0),
            ],
            vec![vec![
                (16_000_000.0, -1_000_000.0),
                (16_000_000.0, 1_000_000.0),
                (18_000_000.0, 1_000_000.0),
                (18_000_000.0, -1_000_000.0),
                (16_000_000.0, -1_000_000.0),
            ]],
        ),
    ]);

    let bytes = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        [
            Ok(feature("points", multipoint)),
            Ok(feature("lines", multiline)),
            Ok(feature("polygons", multipolygon)),
        ],
    )
    .unwrap()
    .unwrap();

    use geozero::ProcessToJson;
    let mut layer = decode(&bytes).layers.remove(0);
    let decoded: serde_json::Value = serde_json::from_str(&layer.to_json().unwrap()).unwrap();
    let types: HashMap<_, _> = decoded["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|feature| {
            (
                feature["properties"]["id"].as_str().unwrap(),
                feature["geometry"]["type"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(types["points"], "MultiPoint");
    assert_eq!(types["lines"], "MultiLineString");
    assert_eq!(types["polygons"], "MultiPolygon");
    let polygons = decoded["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|feature| feature["properties"]["id"] == "polygons")
        .unwrap();
    assert_eq!(
        polygons["geometry"]["coordinates"][1]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn homogeneous_geometry_collections_normalize_and_mixed_collections_refuse() {
    let homogeneous = GeometryCollection(vec![
        Geometry::Point(Point::new(0.0, 0.0)),
        Geometry::MultiPoint(MultiPoint(vec![Point::new(1.0, 1.0), Point::new(2.0, 2.0)])),
    ]);
    let bytes = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        [Ok(feature(
            "points",
            Geometry::GeometryCollection(homogeneous),
        ))],
    )
    .unwrap()
    .unwrap();
    assert_eq!(decode(&bytes).layers[0].features[0].r#type, Some(1));

    let mixed = GeometryCollection(vec![
        Geometry::Point(Point::new(0.0, 0.0)),
        Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 1.0)])),
    ]);
    let error = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        [Ok(feature("mixed", Geometry::GeometryCollection(mixed)))],
    )
    .unwrap_err();
    assert!(matches!(
        error,
        TileEncodeError::MixedGeometryCollection { feature_index: 0 }
    ));
}

#[test]
fn selected_typed_attributes_preserve_finite_types_and_ignore_unselected_values() {
    let selected = vec![
        "name".to_string(),
        "signed".to_string(),
        "unsigned".to_string(),
        "ratio".to_string(),
        "active".to_string(),
        "missing".to_string(),
    ];
    let input = TileFeature::new(
        "7",
        Geometry::Point(Point::new(0.0, 0.0)),
        vec![
            ("name".to_string(), TileScalar::String("acme".to_string())),
            ("signed".to_string(), TileScalar::Signed(-42)),
            ("unsigned".to_string(), TileScalar::Unsigned(u64::MAX)),
            ("ratio".to_string(), TileScalar::Float(1.5)),
            ("active".to_string(), TileScalar::Bool(true)),
            ("missing".to_string(), TileScalar::Null),
            (
                "unselected".to_string(),
                TileScalar::String("ignored".to_string()),
            ),
        ],
    );
    let mut request = request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator);
    request.selected_properties = selected;
    let bytes = encode_tile(request, [Ok(input)]).unwrap().unwrap();
    let tile = decode(&bytes);
    let layer = &tile.layers[0];
    let attrs: HashMap<_, _> = layer.features[0]
        .tags
        .chunks(2)
        .map(|pair| {
            (
                layer.keys[pair[0] as usize].as_str(),
                &layer.values[pair[1] as usize],
            )
        })
        .collect();
    assert_eq!(attrs["name"].string_value.as_deref(), Some("acme"));
    assert_eq!(attrs["signed"].sint_value, Some(-42));
    assert_eq!(attrs["unsigned"].uint_value, Some(u64::MAX));
    assert_eq!(attrs["ratio"].double_value, Some(1.5));
    assert_eq!(attrs["active"].bool_value, Some(true));
    assert!(!attrs.contains_key("missing"));
    assert!(!attrs.contains_key("unselected"));
}

#[test]
fn non_finite_typed_properties_are_rejected_by_property_name() {
    let input = TileFeature::new(
        "1",
        Geometry::Point(Point::new(0.0, 0.0)),
        vec![("ratio".to_string(), TileScalar::Float(f64::INFINITY))],
    );
    let mut request = request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator);
    request.selected_properties = vec!["ratio".to_string()];
    let error = encode_tile(request, [Ok(input)]).unwrap_err();
    assert!(matches!(
        error,
        TileEncodeError::NonFiniteProperty {
            feature_index: 0,
            ref property,
        } if property == "ratio"
    ));
}

#[test]
fn feature_and_vertex_caps_stop_pulling_at_the_bounded_prefix() {
    let pulls = Cell::new(0usize);
    let mut feature_zero_request = request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator);
    feature_zero_request.feature_cap = 0;
    let input = std::iter::from_fn(|| {
        pulls.set(pulls.get() + 1);
        Some(Ok(feature("unexpected", Point::new(0.0, 0.0))))
    });
    assert!(encode_tile(feature_zero_request, input).unwrap().is_none());
    assert_eq!(pulls.get(), 0);

    pulls.set(0);
    let mut feature_two_request = request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator);
    feature_two_request.feature_cap = 2;
    let mut next_id = 0usize;
    let input = std::iter::from_fn(|| {
        pulls.set(pulls.get() + 1);
        let id = next_id;
        next_id += 1;
        Some(Ok(feature(&id.to_string(), Point::new(id as f64, 0.0))))
    });
    let bytes = encode_tile(feature_two_request, input).unwrap().unwrap();
    assert_eq!(pulls.get(), 2);
    assert_eq!(feature_ids(&decode(&bytes)).len(), 2);

    pulls.set(0);
    let features = [
        feature("first", Point::new(0.0, 0.0)),
        feature(
            "over",
            LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]),
        ),
        feature("later", Point::new(3.0, 0.0)),
    ];
    let mut features = features.into_iter();
    let input = std::iter::from_fn(|| {
        let next = features.next()?;
        pulls.set(pulls.get() + 1);
        Some(Ok::<_, TileEncodeError>(next))
    });
    let mut vertex_request = request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator);
    vertex_request.vertex_cap = 1;
    let outcome = encode_tile_with_outcome(vertex_request, input).unwrap();
    assert_eq!(
        pulls.get(),
        2,
        "the feature after the first overage is never pulled"
    );
    assert!(outcome.vertex_limit_exceeded);
    assert_eq!(outcome.vertices_used, 1);
    assert_eq!(
        feature_ids(&decode(outcome.tile.as_ref().unwrap())).len(),
        1
    );
}

#[test]
fn exact_and_zero_vertex_caps_report_outcomes_without_off_by_one_errors() {
    let mut exact = request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator);
    exact.vertex_cap = 2;
    let outcome = encode_tile_with_outcome(
        exact,
        [
            Ok::<_, TileEncodeError>(feature("1", Point::new(0.0, 0.0))),
            Ok::<_, TileEncodeError>(feature("2", Point::new(1.0, 0.0))),
        ],
    )
    .unwrap();
    assert!(!outcome.vertex_limit_exceeded);
    assert_eq!(outcome.vertices_used, 2);
    assert_eq!(
        feature_ids(&decode(outcome.tile.as_ref().unwrap())).len(),
        2
    );

    let pulls = Cell::new(0usize);
    let mut zero = request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator);
    zero.vertex_cap = 0;
    let input = std::iter::from_fn(|| {
        pulls.set(pulls.get() + 1);
        Some(Ok::<_, TileEncodeError>(feature("1", Point::new(0.0, 0.0))))
    });
    let outcome = encode_tile_with_outcome(zero, input).unwrap();
    assert!(outcome.tile.is_none());
    assert!(outcome.vertex_limit_exceeded);
    assert_eq!(outcome.vertices_used, 0);
    assert_eq!(pulls.get(), 1);
}

#[test]
fn fallible_input_stops_at_the_first_error() {
    let pulls = Cell::new(0usize);
    let input = std::iter::from_fn(|| {
        pulls.set(pulls.get() + 1);
        Some(if pulls.get() == 1 {
            Err(TileEncodeError::Source("broken row".to_string()))
        } else {
            Ok(feature("later", Point::new(0.0, 0.0)))
        })
    });
    let error = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        input,
    )
    .unwrap_err();
    assert!(matches!(error, TileEncodeError::Source(ref message) if message == "broken row"));
    assert_eq!(pulls.get(), 1);
}

#[test]
fn zoom_and_coordinate_validation_is_checked_and_fallible() {
    let empty = || std::iter::empty::<Result<TileFeature, TileEncodeError>>();
    assert!(encode_tile(
        request(
            TileCoord {
                z: 24,
                x: (1 << 24) - 1,
                y: (1 << 24) - 1,
            },
            SourceCrs::WebMercator,
        ),
        empty(),
    )
    .unwrap()
    .is_none());
    for zoom in [25, 64] {
        let error = encode_tile(
            request(
                TileCoord {
                    z: zoom,
                    x: 0,
                    y: 0,
                },
                SourceCrs::WebMercator,
            ),
            empty(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            TileEncodeError::UnsupportedZoom { found, max: 24 } if found == zoom
        ));
    }

    let error = encode_tile(
        request(
            TileCoord {
                z: 24,
                x: 1 << 24,
                y: 0,
            },
            SourceCrs::WebMercator,
        ),
        empty(),
    )
    .unwrap_err();
    assert!(matches!(error, TileEncodeError::InvalidCoordinate { .. }));
}

#[test]
fn source_and_projected_non_finite_coordinates_are_rejected() {
    let source_error = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::WebMercator),
        [Ok(feature("1", Point::new(f64::INFINITY, 0.0)))],
    )
    .unwrap_err();
    assert!(matches!(
        source_error,
        TileEncodeError::NonFiniteSourceCoordinate { feature_index: 0 }
    ));

    let projection_error = encode_tile(
        request(TileCoord { z: 0, x: 0, y: 0 }, SourceCrs::Crs84),
        [Ok(feature("1", Point::new(f64::MAX, 0.0)))],
    )
    .unwrap_err();
    assert!(matches!(
        projection_error,
        TileEncodeError::NonFiniteProjectedCoordinate { feature_index: 0 }
    ));
}
