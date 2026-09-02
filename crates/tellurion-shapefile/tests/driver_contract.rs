use std::{
    io::{Cursor, Write},
    ops::Range,
    sync::Arc,
};

use async_trait::async_trait;
use bytes::Bytes;
use geozero::mvt::{tile::GeomType, Message, Tile};
use tellurion_core::{
    CatalogSource, DriverFactory, FeatureSource, ItemsQuery, PhysicalCollection, StorageDecl,
    TileCoord, TileSource,
};
use tellurion_http_source::{ContentIdentity, RangeObject, SourceError, SourceHandle};
use tellurion_shapefile::{
    ArchiveLimits, ArchiveSpool, ScanLimits, ShapefileBackend, ShapefileDriverFactory,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

#[derive(Clone)]
struct FixtureObject {
    bytes: Arc<Vec<u8>>,
    handle: SourceHandle,
    identity: ContentIdentity,
}

impl FixtureObject {
    fn new(bytes: Vec<u8>) -> Self {
        let length = bytes.len() as u64;
        Self {
            bytes: Arc::new(bytes),
            handle: SourceHandle::new("driver-fixture"),
            identity: ContentIdentity::StrongEtag {
                source_key: [7; 32],
                revision_key: [8; 32],
                length,
            },
        }
    }
}

#[async_trait]
impl RangeObject for FixtureObject {
    fn handle(&self) -> &SourceHandle {
        &self.handle
    }
    fn identity(&self) -> &ContentIdentity {
        &self.identity
    }
    fn length(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn display_name(&self) -> &str {
        "driver-fixture.zip"
    }
    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
        Ok(Bytes::copy_from_slice(
            &self.bytes[range.start as usize..range.end as usize],
        ))
    }
}

async fn backend(records: &[Record], cpg: Option<&str>) -> ShapefileBackend {
    ShapefileBackend::new(validated(records, cpg).await)
}

async fn validated(
    records: &[Record],
    cpg: Option<&str>,
) -> tellurion_shapefile::ValidatedShapefile {
    let archive = archive(records, cpg);
    let root = tempfile::tempdir().unwrap();
    // Leak only the test root: the validated bundle owns the extracted directory.
    let root = root.keep();
    let validated = ArchiveSpool::new(&root, ArchiveLimits::default())
        .unwrap()
        .materialize(Arc::new(FixtureObject::new(archive)))
        .await
        .unwrap();
    validated
}

async fn materialize(archive: Vec<u8>) -> tellurion_shapefile::ValidatedShapefile {
    let root = tempfile::tempdir().unwrap().keep();
    ArchiveSpool::new(&root, ArchiveLimits::default())
        .unwrap()
        .materialize(Arc::new(FixtureObject::new(archive)))
        .await
        .unwrap()
}

async fn assert_rejected_everywhere(archive: Vec<u8>) {
    let source = || async { ShapefileBackend::new(materialize(archive.clone()).await) };
    assert!(source().await.collections().await.is_err());
    let physical = PhysicalCollection {
        name: "dataset".into(),
        geometry_column: None,
        primary_key: None,
        srid: None,
        geometry_type: None,
    };
    assert!(source().await.row_estimate(&physical).await.is_err());
    assert!(source().await.attribute_schema(&physical).await.is_err());
    assert!(source()
        .await
        .items(&collection(), &ItemsQuery::default())
        .await
        .is_err());
    assert!(source()
        .await
        .items(
            &collection(),
            &ItemsQuery {
                token: Some("1".into()),
                ..Default::default()
            },
        )
        .await
        .is_err());
    assert!(source().await.item(&collection(), "1", None).await.is_err());
}

fn bbox(bbox: [f64; 4]) -> ItemsQuery {
    ItemsQuery {
        bbox: Some(bbox),
        ..Default::default()
    }
}

#[tokio::test]
async fn catalog_attributes_extent_and_cpg_decoding_are_real_file_facts() {
    let source = backend(
        &[
            Record::point(2.0, 3.0, "cafe"),
            Record::bytes(5.0, 7.0, b"caf\xe9"),
            Record::null(""),
        ],
        Some("ISO-8859-1"),
    )
    .await;

    let physical = source.collections().await.unwrap().pop().unwrap();
    assert_eq!(physical.name, "dataset");
    assert_eq!(physical.geometry_column.as_deref(), Some("geometry"));
    assert_eq!(physical.primary_key.as_deref(), Some("fid"));
    assert_eq!(physical.geometry_type.as_deref(), Some("POINT"));
    assert_eq!(source.row_estimate(&physical).await.unwrap(), Some(3));
    assert_eq!(
        source.extent(&physical).await.unwrap().unwrap().bbox,
        [2.0, 3.0, 5.0, 7.0]
    );
    assert_eq!(
        source.attribute_schema(&physical).await.unwrap().unwrap()[0].name,
        "name"
    );

    let page = source
        .items(
            &collection(),
            &ItemsQuery {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.features_geojson[1]["properties"]["name"], "café");
    assert!(page.features_geojson[2]["geometry"].is_null());
    assert!(page.features_geojson[2]["properties"]["name"].is_null());
}

#[tokio::test]
async fn missing_or_ambiguous_prj_is_rejected_before_catalog_exposure() {
    let archive = archive_without_prj(&[Record::point(0.0, 0.0, "one")]);
    let root = tempfile::tempdir().unwrap().keep();
    let files = ArchiveSpool::new(&root, ArchiveLimits::default())
        .unwrap()
        .materialize(Arc::new(FixtureObject::new(archive)))
        .await
        .unwrap();
    assert!(ShapefileBackend::new(files).collections().await.is_err());
}

#[tokio::test]
async fn natural_earth_esri_wgs84_prj_is_accepted_as_epsg_4326() {
    let records = [Record::point(12.45, 41.90, "one")];
    let (shp, shx) = shape_files(&records);
    let files = materialize(archive_from_components(
        shp,
        shx,
        dbf(&records),
        Some(
            b"GEOGCS[\"GCS_WGS_1984\",DATUM[\"D_WGS_1984\",SPHEROID[\"WGS_1984\",6378137.0,298.257223563]],PRIMEM[\"Greenwich\",0.0],UNIT[\"Degree\",0.017453292519943295]]"
                .to_vec(),
        ),
    ))
    .await;
    let source = ShapefileBackend::new(files);

    let physical = source.collections().await.unwrap().pop().unwrap();
    assert_eq!(physical.srid, Some(4326));
    assert_eq!(
        source.extent(&physical).await.unwrap().unwrap().bbox,
        [12.45, 41.90, 12.45, 41.90]
    );
}

#[tokio::test]
async fn all_null_shapes_have_no_spatial_extent() {
    let source = backend(&[Record::null("none")], None).await;
    let physical = source.collections().await.unwrap().pop().unwrap();
    assert!(source.extent(&physical).await.unwrap().is_none());
}

#[tokio::test]
async fn textual_numeric_values_preserve_their_original_decimal_spelling() {
    let (shp, shx) = shape_files(&[Record::point(0.0, 0.0, "")]);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [
        ("dataset.shp", shp),
        ("dataset.shx", shx),
        ("dataset.dbf", numeric_dbf("9007199254740993.123456789")),
        (
            "dataset.prj",
            b"GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]]".to_vec(),
        ),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    let root = tempfile::tempdir().unwrap().keep();
    let files = ArchiveSpool::new(&root, ArchiveLimits::default())
        .unwrap()
        .materialize(Arc::new(FixtureObject::new(
            writer.finish().unwrap().into_inner(),
        )))
        .await
        .unwrap();
    let page = ShapefileBackend::new(files)
        .items(&collection(), &ItemsQuery::default())
        .await
        .unwrap();
    assert_eq!(
        page.features_geojson[0]["properties"]["value"],
        "9007199254740993.123456789"
    );
}

#[tokio::test]
async fn textual_numeric_overflow_and_null_values_remain_null() {
    for value in ["******************************", ""] {
        let (shp, shx) = shape_files(&[Record::point(0.0, 0.0, "")]);
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, bytes) in [
            ("dataset.shp", shp),
            ("dataset.shx", shx),
            ("dataset.dbf", numeric_dbf(value)),
            (
                "dataset.prj",
                b"GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]]".to_vec(),
            ),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        let root = tempfile::tempdir().unwrap().keep();
        let files = ArchiveSpool::new(&root, ArchiveLimits::default())
            .unwrap()
            .materialize(Arc::new(FixtureObject::new(
                writer.finish().unwrap().into_inner(),
            )))
            .await
            .unwrap();
        let page = ShapefileBackend::new(files)
            .items(&collection(), &ItemsQuery::default())
            .await
            .unwrap();
        assert!(page.features_geojson[0]["properties"]["value"].is_null());
    }
}

#[tokio::test]
async fn float_text_and_date_values_keep_their_wire_contracts() {
    let (shp, shx) = shape_files(&[Record::point(0.0, 0.0, "")]);
    let files = materialize(archive_from_components(
        shp.clone(),
        shx.clone(),
        numeric_dbf_type("9007199254740993.123456789", b'F'),
        Some(wgs84_prj()),
    ))
    .await;
    assert_eq!(
        ShapefileBackend::new(files)
            .items(&collection(), &ItemsQuery::default())
            .await
            .unwrap()
            .features_geojson[0]["properties"]["value"],
        "9007199254740993.123456789"
    );

    let files = materialize(archive_from_components(
        shp,
        shx,
        date_time_dbf(),
        Some(wgs84_prj()),
    ))
    .await;
    let feature = ShapefileBackend::new(files)
        .items(&collection(), &ItemsQuery::default())
        .await
        .unwrap()
        .features_geojson
        .remove(0);
    assert_eq!(feature["properties"]["date"], "2024-01-02");
    assert_eq!(feature["properties"]["time"], "2024-01-02T01:02:03");
}

#[tokio::test]
async fn textual_numeric_and_float_columns_have_text_schema_types() {
    let (shp, shx) = shape_files(&[Record::point(0.0, 0.0, "")]);
    for field_type in *b"NF" {
        let source = ShapefileBackend::new(
            materialize(archive_from_components(
                shp.clone(),
                shx.clone(),
                numeric_dbf_type("9007199254740993.123456789", field_type),
                Some(wgs84_prj()),
            ))
            .await,
        );
        let physical = source.collections().await.unwrap().pop().unwrap();
        assert_eq!(
            source.attribute_schema(&physical).await.unwrap().unwrap()[0].sql_type,
            "text"
        );
    }
}

#[tokio::test]
async fn malformed_dbf_descriptor_width_is_rejected_before_catalog_exposure() {
    let (shp, shx) = shape_files(&[Record::point(0.0, 0.0, "")]);
    let mut dbf = numeric_dbf("1");
    dbf[10..12].copy_from_slice(&2_u16.to_le_bytes());
    assert_rejected_everywhere(archive_from_components(shp, shx, dbf, Some(wgs84_prj()))).await;
}

#[tokio::test]
async fn shx_entries_must_match_sequential_shp_record_boundaries() {
    let records = [
        Record::point(0.0, 0.0, "one"),
        Record::point(1.0, 1.0, "two"),
    ];
    let (shp, shx) = shape_files(&records);
    let mut duplicate_offset = shx.clone();
    duplicate_offset[108..112].copy_from_slice(&50_u32.to_be_bytes());
    assert_rejected_everywhere(archive_from_components(
        shp.clone(),
        duplicate_offset,
        dbf(&records),
        Some(wgs84_prj()),
    ))
    .await;

    let mut wrong_content_length = shx;
    wrong_content_length[104..108].copy_from_slice(&11_u32.to_be_bytes());
    assert_rejected_everywhere(archive_from_components(
        shp,
        wrong_content_length,
        dbf(&records),
        Some(wgs84_prj()),
    ))
    .await;
}

#[tokio::test]
async fn binary_nonfinite_values_are_rejected_before_catalog_exposure() {
    let (shp, shx) = shape_files(&[Record::point(0.0, 0.0, "")]);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [
        ("dataset.shp", shp),
        ("dataset.shx", shx),
        ("dataset.dbf", binary_double_dbf(f64::NAN)),
        (
            "dataset.prj",
            b"GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]]".to_vec(),
        ),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    let root = tempfile::tempdir().unwrap().keep();
    let files = ArchiveSpool::new(&root, ArchiveLimits::default())
        .unwrap()
        .materialize(Arc::new(FixtureObject::new(
            writer.finish().unwrap().into_inner(),
        )))
        .await
        .unwrap();
    assert!(ShapefileBackend::new(files).collections().await.is_err());
}

#[tokio::test]
async fn orphan_polygon_inner_ring_is_rejected_instead_of_becoming_a_null_geometry() {
    let source = backend(
        &[Record::polygon(
            vec![vec![
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ]],
            "orphan",
        )],
        None,
    )
    .await;
    assert!(source
        .items(&collection(), &ItemsQuery::default())
        .await
        .is_err());
}

#[tokio::test]
async fn polygon_holes_are_grouped_by_containing_exterior_not_ring_order() {
    let source = backend(
        &[Record::polygon(
            vec![
                vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)],
                vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
                vec![
                    (10.0, 0.0),
                    (10.0, 4.0),
                    (14.0, 4.0),
                    (14.0, 0.0),
                    (10.0, 0.0),
                ],
            ],
            "inner-first",
        )],
        None,
    )
    .await;
    let geometry = &source
        .items(&collection(), &ItemsQuery::default())
        .await
        .unwrap()
        .features_geojson[0]["geometry"];
    assert_eq!(geometry["type"], "MultiPolygon");
    assert_eq!(geometry["coordinates"][0].as_array().unwrap().len(), 2);
    assert_eq!(geometry["coordinates"][1].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn polygon_holes_with_ambiguous_exteriors_are_rejected() {
    let source = backend(
        &[Record::polygon(
            vec![
                vec![(2.0, 1.0), (3.0, 1.0), (3.0, 2.0), (2.0, 2.0), (2.0, 1.0)],
                vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
                vec![(1.0, 0.0), (1.0, 4.0), (5.0, 4.0), (5.0, 0.0), (1.0, 0.0)],
            ],
            "ambiguous",
        )],
        None,
    )
    .await;
    assert!(source
        .items(&collection(), &ItemsQuery::default())
        .await
        .is_err());
}

#[tokio::test]
async fn polygon_hole_edges_cannot_cross_a_concave_exterior_void() {
    let source = backend(
        &[Record::polygon(
            vec![
                vec![(1.0, 1.0), (5.0, 1.0), (5.0, 3.0), (1.0, 3.0), (1.0, 1.0)],
                vec![
                    (0.0, 0.0),
                    (0.0, 6.0),
                    (2.0, 6.0),
                    (2.0, 2.0),
                    (4.0, 2.0),
                    (4.0, 6.0),
                    (6.0, 6.0),
                    (6.0, 0.0),
                    (0.0, 0.0),
                ],
            ],
            "crosses-void",
        )],
        None,
    )
    .await;
    assert!(source
        .items(&collection(), &ItemsQuery::default())
        .await
        .is_err());
}

#[tokio::test]
async fn strongly_concave_polygon_holes_are_accepted_without_an_interior_point_heuristic() {
    let source = backend(
        &[Record::polygon(
            vec![
                vec![
                    (3.0, 3.0),
                    (1.0, 3.0),
                    (1.0, 1.0),
                    (7.0, 1.0),
                    (7.0, 7.0),
                    (1.0, 7.0),
                    (1.0, 5.0),
                    (5.0, 5.0),
                    (5.0, 3.0),
                    (3.0, 3.0),
                ],
                vec![(0.0, 0.0), (0.0, 8.0), (8.0, 8.0), (8.0, 0.0), (0.0, 0.0)],
            ],
            "concave-hole",
        )],
        None,
    )
    .await;
    let geometry = &source
        .items(&collection(), &ItemsQuery::default())
        .await
        .unwrap()
        .features_geojson[0]["geometry"];
    assert_eq!(geometry["type"], "Polygon");
    assert_eq!(geometry["coordinates"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn physical_record_index_is_stable_for_pages_and_item_lookup() {
    let source = backend(
        &[
            Record::point(0.0, 0.0, "one"),
            Record::point(1.0, 1.0, "two"),
            Record::point(2.0, 2.0, "three"),
        ],
        None,
    )
    .await;
    let first = source
        .items(
            &collection(),
            &ItemsQuery {
                limit: 2,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        first
            .features_geojson
            .iter()
            .map(|f| f["id"].as_str())
            .collect::<Vec<_>>(),
        vec![Some("0"), Some("1")]
    );
    assert_eq!(first.next_token.as_deref(), Some("2"));
    let second = source
        .items(
            &collection(),
            &ItemsQuery {
                limit: 2,
                token: first.next_token,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(second.features_geojson[0]["id"], "2");
    assert_eq!(
        source
            .item(&collection(), "1", None)
            .await
            .unwrap()
            .unwrap()["properties"]["name"],
        "two"
    );
    assert!(source
        .item(&collection(), "not-an-index", None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn bbox_is_exact_and_refuses_an_oversized_scan_before_a_page() {
    let source = backend(
        &[
            Record::point(0.0, 0.0, "inside"),
            Record::point(10.0, 10.0, "outside"),
        ],
        None,
    )
    .await;
    let page = source
        .items(
            &collection(),
            &ItemsQuery {
                limit: 10,
                bbox: Some([-1.0, -1.0, 1.0, 1.0]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.features_geojson.len(), 1);
    assert_eq!(page.features_geojson[0]["id"], "0");

    let constrained = source.with_scan_limits(ScanLimits {
        max_records: 1,
        max_bytes: u64::MAX,
    });
    let error = constrained
        .items(
            &collection(),
            &ItemsQuery {
                limit: 1,
                bbox: Some([-1.0, -1.0, 1.0, 1.0]),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("scan limit"));
}

#[tokio::test]
async fn bbox_reports_the_exact_match_count_while_paging() {
    let source = backend(
        &[
            Record::point(0.0, 0.0, "one"),
            Record::point(1.0, 1.0, "two"),
            Record::point(2.0, 2.0, "three"),
        ],
        None,
    )
    .await;
    let page = source
        .items(
            &collection(),
            &ItemsQuery {
                limit: 1,
                bbox: Some([-1.0, -1.0, 3.0, 3.0]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.number_matched, Some(3));
    assert_eq!(page.next_token.as_deref(), Some("1"));
}

#[tokio::test]
async fn invalid_physical_alignment_is_rejected_from_every_public_surface() {
    let records = [
        Record::point(0.0, 0.0, "first"),
        Record::point(1.0, 1.0, "middle"),
        Record::point(2.0, 2.0, "last"),
    ];
    for deleted in 0..records.len() {
        let (shp, shx) = shape_files(&records);
        let mut rows = dbf(&records);
        rows[65 + deleted * 21] = b'*';
        assert_rejected_everywhere(archive_from_components(shp, shx, rows, Some(wgs84_prj())))
            .await;
    }
    let (shp, shx) = shape_files(&records);
    let mut rows = dbf(&records);
    rows[4..8].copy_from_slice(&0_u32.to_le_bytes());
    assert_rejected_everywhere(archive_from_components(shp, shx, rows, Some(wgs84_prj()))).await;
}

#[tokio::test]
async fn only_a_root_epsg_4326_prj_is_accepted() {
    let records = [Record::point(0.0, 0.0, "one")];
    let (shp, shx) = shape_files(&records);
    for prj in [
        None,
        Some(b"GEOGCS[\"WGS 84\",AUTHORITY[\"OGC\",\"CRS84\"]]".to_vec()),
        Some(b"PROJCS[\"WGS 84 / Pseudo-Mercator\",GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]],AUTHORITY[\"EPSG\",\"3857\"]]".to_vec()),
        Some(b"GEOGCS[\"GCS_WGS_1984\",DATUM[\"D_WGS_1984\",SPHEROID[\"WGS_1984\",6378135.0,298.257223563]],PRIMEM[\"Greenwich\",0.0],UNIT[\"Degree\",0.017453292519943295]]".to_vec()),
    ] {
        let files = materialize(archive_from_components(shp.clone(), shx.clone(), dbf(&records), prj)).await;
        assert!(ShapefileBackend::new(files).collections().await.is_err());
    }
}

#[tokio::test]
async fn bbox_filtering_handles_holes_concavity_multilines_and_boundaries_exactly() {
    let hole = backend(
        &[Record::polygon(
            vec![
                vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
                vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)],
            ],
            "hole",
        )],
        None,
    )
    .await;
    assert!(hole
        .items(&collection(), &bbox([1.5, 1.5, 2.5, 2.5]))
        .await
        .unwrap()
        .features_geojson
        .is_empty());
    assert_eq!(
        hole.items(&collection(), &bbox([0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap()
            .features_geojson
            .len(),
        1
    );

    let concave = backend(
        &[Record::polygon(
            vec![vec![
                (0.0, 0.0),
                (0.0, 4.0),
                (1.0, 4.0),
                (1.0, 1.0),
                (4.0, 1.0),
                (4.0, 0.0),
                (0.0, 0.0),
            ]],
            "concave",
        )],
        None,
    )
    .await;
    assert!(concave
        .items(&collection(), &bbox([2.0, 2.0, 3.0, 3.0]))
        .await
        .unwrap()
        .features_geojson
        .is_empty());

    let multiline = backend(
        &[Record::multiline(
            vec![
                vec![(0.0, 0.0), (3.0, 3.0)],
                vec![(10.0, 10.0), (11.0, 11.0)],
            ],
            "multiline",
        )],
        None,
    )
    .await;
    assert_eq!(
        multiline
            .items(&collection(), &bbox([1.9, 1.9, 2.1, 2.1]))
            .await
            .unwrap()
            .features_geojson[0]["geometry"]["type"],
        "MultiLineString"
    );
}

#[tokio::test]
async fn line_and_polygon_hole_are_emitted_as_geojson() {
    let line = backend(&[Record::line(vec![(0.0, 0.0), (2.0, 2.0)], "line")], None).await;
    assert_eq!(
        line.items(&collection(), &ItemsQuery::default())
            .await
            .unwrap()
            .features_geojson[0]["geometry"]["type"],
        "LineString"
    );
    let polygon = backend(
        &[Record::polygon(
            vec![
                vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
                vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)],
            ],
            "hole",
        )],
        None,
    )
    .await;
    let geometry = &polygon
        .items(&collection(), &ItemsQuery::default())
        .await
        .unwrap()
        .features_geojson[0]["geometry"];
    assert_eq!(geometry["type"], "Polygon");
    assert_eq!(geometry["coordinates"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn tiles_encode_point_line_and_polygon_with_the_external_layer_name() {
    for (records, geometry_type, mvt_type) in [
        (
            vec![Record::point(10.0, 10.0, "point")],
            "Point",
            GeomType::Point,
        ),
        (
            vec![Record::line(vec![(9.0, 9.0), (11.0, 11.0)], "line")],
            "LineString",
            GeomType::Linestring,
        ),
        (
            vec![Record::polygon(
                vec![vec![
                    (9.0, 9.0),
                    (9.0, 11.0),
                    (11.0, 11.0),
                    (11.0, 9.0),
                    (9.0, 9.0),
                ]],
                "polygon",
            )],
            "Polygon",
            GeomType::Polygon,
        ),
    ] {
        let source = backend(&records, None).await;
        let mut declaration = tile_collection();
        declaration.external_id = Some("public-layer".to_string());
        let tile = source
            .mvt_tile(&declaration, TileCoord { z: 0, x: 0, y: 0 }, None)
            .await
            .expect("supported CRS and valid coordinate")
            .expect("world tile contains the fixture geometry");
        let decoded = Tile::decode(tile.as_ref()).expect("valid MVT document");
        assert_eq!(decoded.layers.len(), 1, "{geometry_type}");
        assert_eq!(decoded.layers[0].name, "public-layer", "{geometry_type}");
        assert_eq!(decoded.layers[0].features.len(), 1, "{geometry_type}");
        assert_eq!(
            decoded.layers[0].features[0].r#type,
            Some(mvt_type as i32),
            "{geometry_type}"
        );
    }
}

#[tokio::test]
async fn tiles_accept_pinned_collections_without_a_derived_srid() {
    let source = backend(&[Record::point(10.0, 10.0, "point")], None).await;
    let declaration: tellurion_core::CollectionDecl = serde_yaml::from_str(
        "id: dataset\ncatalog: demo\nstorage: shp\ntable: dataset\ngeometry_column: geometry\nprimary_key: fid\n",
    )
    .unwrap();

    assert!(source.tile_capable(&declaration));
    assert!(source
        .mvt_tile(&declaration, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn tiles_preserve_normalized_polygon_topology() {
    let inner_first = Record::polygon(
        vec![
            vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)],
            vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
            vec![
                (10.0, 0.0),
                (10.0, 4.0),
                (14.0, 4.0),
                (14.0, 0.0),
                (10.0, 0.0),
            ],
        ],
        "inner-first",
    );
    let source = backend(&[inner_first], None).await;
    let tile = source
        .mvt_tile(&tile_collection(), TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("normalized inner-first rings are tileable")
        .expect("world tile contains the polygons");
    let decoded = Tile::decode(tile.as_ref()).expect("valid MVT document");
    assert_eq!(
        decoded.layers[0].features[0].r#type,
        Some(GeomType::Polygon as i32)
    );

    let disjoint = backend(
        &[Record::polygon(
            vec![
                vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
                vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)],
                vec![
                    (10.0, 0.0),
                    (10.0, 4.0),
                    (14.0, 4.0),
                    (14.0, 0.0),
                    (10.0, 0.0),
                ],
            ],
            "disjoint",
        )],
        None,
    )
    .await;
    assert!(disjoint
        .mvt_tile(&tile_collection(), TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .unwrap()
        .is_some());

    let ambiguous = backend(
        &[Record::polygon(
            vec![
                vec![(2.0, 1.0), (3.0, 1.0), (3.0, 2.0), (2.0, 2.0), (2.0, 1.0)],
                vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
                vec![(1.0, 0.0), (1.0, 4.0), (5.0, 4.0), (5.0, 0.0), (1.0, 0.0)],
            ],
            "ambiguous",
        )],
        None,
    )
    .await;
    assert!(ambiguous
        .mvt_tile(&tile_collection(), TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .is_err());
}

#[tokio::test]
async fn tiles_refuse_vertex_and_scan_budget_before_decoding_records() {
    let source = backend(
        &[Record::line(
            vec![(9.0, 9.0), (10.0, 10.0), (11.0, 11.0)],
            "line",
        )],
        None,
    )
    .await;
    let mut vertex_limited = tile_collection();
    vertex_limited.settings.tile_vertex_budget = Some(2);
    assert_eq!(
        source
            .mvt_tile(&vertex_limited, TileCoord { z: 0, x: 0, y: 0 }, None)
            .await
            .unwrap(),
        None
    );

    let files = validated(&[Record::point(10.0, 10.0, "point")], None).await;
    std::fs::write(&files.shp, vec![0; 128]).unwrap();
    let constrained = ShapefileBackend::new(files).with_scan_limits(ScanLimits {
        max_records: 0,
        max_bytes: u64::MAX,
    });
    let items_error = constrained
        .items(&collection(), &bbox([-180.0, -90.0, 180.0, 90.0]))
        .await
        .unwrap_err();
    assert!(items_error.to_string().contains("scan limit"));
    let tile_error = constrained
        .mvt_tile(&tile_collection(), TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .unwrap_err();
    assert!(tile_error.to_string().contains("tiles:scan-budget"));
}

#[tokio::test]
async fn tiles_honor_feature_caps_and_return_empty_for_uncovered_valid_coordinates() {
    let source = backend(
        &[
            Record::point(10.0, 10.0, "one"),
            Record::point(11.0, 11.0, "two"),
        ],
        None,
    )
    .await;
    let mut declaration = tile_collection();
    declaration.tiles.caps.0.insert(0, 1);

    let capped = source
        .mvt_tile(&declaration, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .unwrap()
        .expect("capped tile remains a valid MVT response");
    let decoded = Tile::decode(capped.as_ref()).expect("valid capped MVT");
    assert_eq!(decoded.layers[0].features.len(), 1);

    assert_eq!(
        source
            .mvt_tile(&declaration, TileCoord { z: 1, x: 0, y: 1 }, None)
            .await
            .unwrap(),
        None,
        "valid but uncovered coordinates are empty rather than errors"
    );
    assert!(source
        .mvt_tile(&declaration, TileCoord { z: 1, x: 2, y: 0 }, None)
        .await
        .is_err());
}

#[tokio::test]
async fn tiles_refuse_missing_or_projected_prj_before_encoding() {
    let records = [Record::point(10.0, 10.0, "one")];
    let (shp, shx) = shape_files(&records);
    for prj in [
        None,
        Some(
            b"PROJCS[\"WGS 84 / Pseudo-Mercator\",GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]],AUTHORITY[\"EPSG\",\"3857\"]]"
                .to_vec(),
        ),
    ] {
        let source = ShapefileBackend::new(
            materialize(archive_from_components(shp.clone(), shx.clone(), dbf(&records), prj))
                .await,
        );
        assert!(source
            .mvt_tile(&tile_collection(), TileCoord { z: 0, x: 0, y: 0 }, None)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn disjoint_exteriors_keep_their_own_holes() {
    let source = backend(
        &[Record::polygon(
            vec![
                vec![(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0), (0.0, 0.0)],
                vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)],
                vec![
                    (10.0, 0.0),
                    (10.0, 4.0),
                    (14.0, 4.0),
                    (14.0, 0.0),
                    (10.0, 0.0),
                ],
            ],
            "multipolygon",
        )],
        None,
    )
    .await;
    let geometry = &source
        .items(&collection(), &ItemsQuery::default())
        .await
        .unwrap()
        .features_geojson[0]["geometry"];
    assert_eq!(geometry["type"], "MultiPolygon");
    assert_eq!(geometry["coordinates"][0].as_array().unwrap().len(), 2);
    assert_eq!(geometry["coordinates"][1].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn z_geometry_families_preserve_their_third_ordinate() {
    let point = shapefile::PointZ::new(1.0, 2.0, 3.0, shapefile::NO_DATA);
    let line = shapefile::PolylineZ::new(vec![
        shapefile::PointZ::new(0.0, 0.0, 4.0, shapefile::NO_DATA),
        shapefile::PointZ::new(1.0, 1.0, 5.0, shapefile::NO_DATA),
    ]);
    let polygon = shapefile::PolygonZ::new(shapefile::PolygonRing::Outer(vec![
        shapefile::PointZ::new(0.0, 0.0, 6.0, shapefile::NO_DATA),
        shapefile::PointZ::new(0.0, 1.0, 7.0, shapefile::NO_DATA),
        shapefile::PointZ::new(1.0, 1.0, 8.0, shapefile::NO_DATA),
        shapefile::PointZ::new(1.0, 0.0, 9.0, shapefile::NO_DATA),
    ]));
    let multipoint = shapefile::MultipointZ::new(vec![
        shapefile::PointZ::new(0.0, 0.0, 10.0, shapefile::NO_DATA),
        shapefile::PointZ::new(1.0, 1.0, 11.0, shapefile::NO_DATA),
    ]);
    for (archive, kind) in [
        (archive_z(point), "Point"),
        (archive_z(line), "LineString"),
        (archive_z(polygon), "Polygon"),
        (archive_z(multipoint), "MultiPoint"),
    ] {
        let feature = ShapefileBackend::new(materialize(archive).await)
            .items(&collection(), &ItemsQuery::default())
            .await
            .unwrap()
            .features_geojson
            .remove(0);
        assert_eq!(feature["geometry"]["type"], kind);
        assert!(
            feature.to_string().contains(",3.0")
                || feature.to_string().contains(",4.0")
                || feature.to_string().contains(",6.0")
                || feature.to_string().contains(",10.0")
        );
    }
}

#[tokio::test]
async fn memo_fields_and_multipatch_headers_are_refused() {
    let records = [Record::point(0.0, 0.0, "one")];
    let (shp, shx) = shape_files(&records);
    let files = materialize(archive_from_components(
        shp.clone(),
        shx.clone(),
        memo_dbf(),
        Some(wgs84_prj()),
    ))
    .await;
    assert!(ShapefileBackend::new(files).collections().await.is_err());

    let mut multipatch_shp = shp;
    let mut multipatch_shx = shx;
    multipatch_shp[32..36].copy_from_slice(&31_i32.to_le_bytes());
    multipatch_shx[32..36].copy_from_slice(&31_i32.to_le_bytes());
    let files = materialize(archive_from_components(
        multipatch_shp,
        multipatch_shx,
        dbf(&records),
        Some(wgs84_prj()),
    ))
    .await;
    assert!(ShapefileBackend::new(files).collections().await.is_err());
}

#[tokio::test]
async fn datetime_and_cql2_are_refused_honestly() {
    let source = backend(&[Record::point(0.0, 0.0, "one")], None).await;
    let datetime = ItemsQuery {
        datetime: Some(Default::default()),
        ..Default::default()
    };
    assert!(source
        .items(&collection(), &datetime)
        .await
        .unwrap_err()
        .to_string()
        .contains("datetime"));
    let filter = ItemsQuery {
        filter: Some(tellurion_core::Filter::IsNull {
            property: "name".into(),
            negated: false,
        }),
        ..Default::default()
    };
    assert!(source
        .items(&collection(), &filter)
        .await
        .unwrap_err()
        .to_string()
        .contains("filter"));
    assert!(!FeatureSource::filter_capable(&source));
}

#[tokio::test]
async fn reader_errors_do_not_expose_the_private_component_locator() {
    let files = validated(&[Record::point(0.0, 0.0, "one")], None).await;
    let locator = files.shp.display().to_string();
    std::fs::remove_file(&files.shp).unwrap();
    let error = ShapefileBackend::new(files)
        .collections()
        .await
        .unwrap_err();
    assert!(!error.to_string().contains(&locator));
}

#[tokio::test]
async fn local_factory_uses_the_same_archive_validation_path() {
    let directory = tempfile::tempdir().unwrap();
    let archive_path = directory.path().join("dataset.zip");
    std::fs::write(
        &archive_path,
        archive(&[Record::point(0.0, 0.0, "one")], None),
    )
    .unwrap();
    let variable = "TELLURION_SHAPEFILE_DRIVER_LOCAL_TEST";
    std::env::set_var(variable, &archive_path);
    let driver = ShapefileDriverFactory::new()
        .build(&StorageDecl {
            id: "shapes".into(),
            driver: "shapefile".into(),
            url_env: variable.into(),
            pool_size: None,
        })
        .unwrap();
    std::env::remove_var(variable);
    assert_eq!(
        driver.catalog_source().collections().await.unwrap()[0].name,
        "dataset"
    );
}

fn collection() -> tellurion_core::CollectionDecl {
    serde_yaml::from_str("id: dataset\ncatalog: demo\nstorage: shp\n").unwrap()
}

fn tile_collection() -> tellurion_core::CollectionDecl {
    let mut declaration = collection();
    declaration.srid = Some(4326);
    declaration
}

#[derive(Clone)]
enum Geometry {
    Point(f64, f64),
    Line(Vec<(f64, f64)>),
    MultiLine(Vec<Vec<(f64, f64)>>),
    Polygon(Vec<Vec<(f64, f64)>>),
    Null,
}

#[derive(Clone)]
struct Record {
    geometry: Geometry,
    name: Vec<u8>,
}
impl Record {
    fn point(x: f64, y: f64, name: &str) -> Self {
        Self {
            geometry: Geometry::Point(x, y),
            name: name.as_bytes().to_vec(),
        }
    }
    fn bytes(x: f64, y: f64, name: &[u8]) -> Self {
        Self {
            geometry: Geometry::Point(x, y),
            name: name.to_vec(),
        }
    }
    fn null(name: &str) -> Self {
        Self {
            geometry: Geometry::Null,
            name: name.as_bytes().to_vec(),
        }
    }
    fn line(points: Vec<(f64, f64)>, name: &str) -> Self {
        Self {
            geometry: Geometry::Line(points),
            name: name.as_bytes().to_vec(),
        }
    }
    fn multiline(parts: Vec<Vec<(f64, f64)>>, name: &str) -> Self {
        Self {
            geometry: Geometry::MultiLine(parts),
            name: name.as_bytes().to_vec(),
        }
    }
    fn polygon(rings: Vec<Vec<(f64, f64)>>, name: &str) -> Self {
        Self {
            geometry: Geometry::Polygon(rings),
            name: name.as_bytes().to_vec(),
        }
    }
}

fn wgs84_prj() -> Vec<u8> {
    b"GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]]".to_vec()
}

fn archive_from_components(
    shp: Vec<u8>,
    shx: Vec<u8>,
    dbf: Vec<u8>,
    prj: Option<Vec<u8>>,
) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [
        ("dataset.shp", shp),
        ("dataset.shx", shx),
        ("dataset.dbf", dbf),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    if let Some(prj) = prj {
        writer.start_file("dataset.prj", options).unwrap();
        writer.write_all(&prj).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn archive_z<S: shapefile::record::EsriShape>(shape: S) -> Vec<u8> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dataset.shp");
    let mut writer = shapefile::ShapeWriter::from_path(&path).unwrap();
    writer.write_shape(&shape).unwrap();
    writer.finalize().unwrap();
    archive_from_components(
        std::fs::read(&path).unwrap(),
        std::fs::read(path.with_extension("shx")).unwrap(),
        dbf(&[Record::point(0.0, 0.0, "z")]),
        Some(wgs84_prj()),
    )
}

fn archive(records: &[Record], cpg: Option<&str>) -> Vec<u8> {
    let (shp, shx) = shape_files(records);
    let dbf = dbf(records);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [
        ("dataset.shp", shp),
        ("dataset.shx", shx),
        ("dataset.dbf", dbf),
        (
            "dataset.prj",
            b"GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]]".to_vec(),
        ),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    if let Some(cpg) = cpg {
        writer.start_file("dataset.cpg", options).unwrap();
        writer.write_all(cpg.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn archive_without_prj(records: &[Record]) -> Vec<u8> {
    let (shp, shx) = shape_files(records);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [
        ("dataset.shp", shp),
        ("dataset.shx", shx),
        ("dataset.dbf", dbf(records)),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn shape_files(records: &[Record]) -> (Vec<u8>, Vec<u8>) {
    let type_code = match records.first().map(|r| &r.geometry) {
        Some(Geometry::Point(_, _)) => 1,
        Some(Geometry::Line(_)) => 3,
        Some(Geometry::MultiLine(_)) => 3,
        Some(Geometry::Polygon(_)) => 5,
        _ => 1,
    };
    let mut bodies = Vec::new();
    for record in records {
        bodies.push(shape_body(&record.geometry, type_code));
    }
    let all = records
        .iter()
        .flat_map(|record| match &record.geometry {
            Geometry::Point(x, y) => vec![(*x, *y)],
            Geometry::Line(points) => points.clone(),
            Geometry::MultiLine(parts) => parts.iter().flatten().copied().collect(),
            Geometry::Polygon(rings) => rings.iter().flatten().copied().collect(),
            Geometry::Null => vec![],
        })
        .collect::<Vec<_>>();
    let bbox = all
        .iter()
        .copied()
        .fold(None, |bbox: Option<[f64; 4]>, (x, y)| {
            Some(match bbox {
                Some([minx, miny, maxx, maxy]) => {
                    [minx.min(x), miny.min(y), maxx.max(x), maxy.max(y)]
                }
                None => [x, y, x, y],
            })
        })
        .unwrap_or([0.0; 4]);
    let shp_length = 100 + bodies.iter().map(|b| 8 + b.len()).sum::<usize>();
    let mut shp = header(shp_length, type_code, bbox);
    let mut shx = header(100 + records.len() * 8, type_code, bbox);
    let mut offset = 50u32;
    for (index, body) in bodies.iter().enumerate() {
        shp.extend_from_slice(&u32::try_from(index + 1).unwrap().to_be_bytes());
        shp.extend_from_slice(&u32::try_from(body.len() / 2).unwrap().to_be_bytes());
        shp.extend_from_slice(body);
        shx.extend_from_slice(&offset.to_be_bytes());
        shx.extend_from_slice(&u32::try_from(body.len() / 2).unwrap().to_be_bytes());
        offset += u32::try_from(4 + body.len() / 2).unwrap();
    }
    (shp, shx)
}

fn header(byte_len: usize, type_code: i32, bbox: [f64; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(100);
    out.extend_from_slice(&9994_i32.to_be_bytes());
    out.extend_from_slice(&[0; 20]);
    out.extend_from_slice(&u32::try_from(byte_len / 2).unwrap().to_be_bytes());
    out.extend_from_slice(&1000_i32.to_le_bytes());
    out.extend_from_slice(&type_code.to_le_bytes());
    for value in bbox {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[0; 32]);
    out
}

fn shape_body(geometry: &Geometry, type_code: i32) -> Vec<u8> {
    let mut out = Vec::new();
    match geometry {
        Geometry::Null => out.extend_from_slice(&0_i32.to_le_bytes()),
        Geometry::Point(x, y) => {
            out.extend_from_slice(&1_i32.to_le_bytes());
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
        }
        Geometry::Line(points) => multipart(&mut out, type_code, std::slice::from_ref(points)),
        Geometry::MultiLine(parts) => multipart(&mut out, type_code, parts),
        Geometry::Polygon(rings) => multipart(&mut out, type_code, rings),
    }
    out
}

fn multipart(out: &mut Vec<u8>, type_code: i32, parts: &[Vec<(f64, f64)>]) {
    let points = parts.iter().flatten().copied().collect::<Vec<_>>();
    let bbox = points.iter().copied().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[minx, miny, maxx, maxy], (x, y)| [minx.min(x), miny.min(y), maxx.max(x), maxy.max(y)],
    );
    out.extend_from_slice(&type_code.to_le_bytes());
    for value in bbox {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&i32::try_from(parts.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&i32::try_from(points.len()).unwrap().to_le_bytes());
    let mut index = 0i32;
    for part in parts {
        out.extend_from_slice(&index.to_le_bytes());
        index += i32::try_from(part.len()).unwrap();
    }
    for (x, y) in points {
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
    }
}

fn dbf(records: &[Record]) -> Vec<u8> {
    let header_len = 65u16;
    let record_len = 21u16;
    let mut out = vec![0x03, 126, 1, 1];
    out.extend_from_slice(&u32::try_from(records.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&record_len.to_le_bytes());
    out.extend_from_slice(&[0; 20]);
    let mut field = [0u8; 32];
    field[..4].copy_from_slice(b"name");
    field[11] = b'C';
    field[16] = 20;
    out.extend_from_slice(&field);
    out.push(0x0d);
    for record in records {
        out.push(b' ');
        let mut value = record.name.clone();
        value.truncate(20);
        out.extend_from_slice(&value);
        out.extend(std::iter::repeat_n(b' ', 20 - value.len()));
    }
    out.push(0x1a);
    out
}

fn numeric_dbf(value: &str) -> Vec<u8> {
    numeric_dbf_type(value, b'N')
}

fn numeric_dbf_type(value: &str, field_type: u8) -> Vec<u8> {
    let header_len = 65u16;
    let record_len = 31u16;
    let mut out = vec![0x03, 126, 1, 1];
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&record_len.to_le_bytes());
    out.extend_from_slice(&[0; 20]);
    let mut field = [0u8; 32];
    field[..5].copy_from_slice(b"value");
    field[11] = field_type;
    field[16] = 30;
    field[17] = 9;
    out.extend_from_slice(&field);
    out.push(0x0d);
    out.push(b' ');
    out.extend_from_slice(format!("{:>30}", value).as_bytes());
    out.push(0x1a);
    out
}

fn date_time_dbf() -> Vec<u8> {
    let header_len = 97u16;
    let record_len = 17u16;
    let mut out = vec![0x03, 126, 1, 1];
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&record_len.to_le_bytes());
    out.extend_from_slice(&[0; 20]);
    for (name, kind) in [(b"date".as_slice(), b'D'), (b"time".as_slice(), b'T')] {
        let mut field = [0u8; 32];
        field[..name.len()].copy_from_slice(name);
        field[11] = kind;
        field[16] = 8;
        out.extend_from_slice(&field);
    }
    out.push(0x0d);
    out.push(b' ');
    out.extend_from_slice(b"20240102");
    out.extend_from_slice(&2_460_312_i32.to_le_bytes());
    out.extend_from_slice(&3_723_000_i32.to_le_bytes());
    out.push(0x1a);
    out
}

fn binary_double_dbf(value: f64) -> Vec<u8> {
    let header_len = 65u16;
    let record_len = 9u16;
    let mut out = vec![0x03, 126, 1, 1];
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&record_len.to_le_bytes());
    out.extend_from_slice(&[0; 20]);
    let mut field = [0u8; 32];
    field[..5].copy_from_slice(b"value");
    field[11] = b'B';
    field[16] = 8;
    out.extend_from_slice(&field);
    out.push(0x0d);
    out.push(b' ');
    out.extend_from_slice(&value.to_le_bytes());
    out.push(0x1a);
    out
}

fn memo_dbf() -> Vec<u8> {
    let header_len = 65u16;
    let record_len = 5u16;
    let mut out = vec![0x03, 126, 1, 1];
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&record_len.to_le_bytes());
    out.extend_from_slice(&[0; 20]);
    let mut field = [0u8; 32];
    field[..4].copy_from_slice(b"memo");
    field[11] = b'M';
    field[16] = 4;
    out.extend_from_slice(&field);
    out.push(0x0d);
    out.extend_from_slice(&[b' ', 0, 0, 0, 0]);
    out.push(0x1a);
    out
}
