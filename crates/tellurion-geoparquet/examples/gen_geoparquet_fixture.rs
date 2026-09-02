//! Regenerates `tests/fixtures/tiny.parquet`, the committed test archive:
//! five real point features (WGS84/CRS84) — the same coordinates
//! `tellurion-flatgeobuf`'s own fixture uses, for family resemblance between
//! the two file-driver fixtures — each carrying a `name`/`value` property
//! pair plus a GeoParquet 1.1 `bbox` covering column, and the file-level
//! `"geo"` key-value metadata this driver requires. Run with:
//!
//! ```sh
//! cargo run -p tellurion-geoparquet --example gen_geoparquet_fixture
//! ```
//!
//! Unlike `tellurion-pmtiles`' fixture generator, this needs no separate
//! cargo feature to isolate a write path: `parquet`'s `ArrowWriter` is part
//! of the same `"arrow"` feature this crate's own reader already requires
//! (see `Cargo.toml`), so this example carries no extra weight for a plain
//! `cargo build`/`cargo test` of the driver crate.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray, StructArray,
};
use arrow_schema::{DataType, Field, Fields, Schema};
use geozero::GeozeroGeometry;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;

/// `(name, value, longitude, latitude)` — same five points
/// `tellurion-flatgeobuf`'s own `examples/gen_flatgeobuf_fixture.rs` uses.
const FEATURES: [(&str, i64, f64, f64); 5] = [
    ("alpha", 1, -4.0, 46.0),
    ("bravo", 2, -2.0, 48.0),
    ("charlie", 3, 0.0, 50.0),
    ("delta", 4, 2.0, 52.0),
    ("echo", 5, 4.0, 54.0),
];

/// Encodes a `POINT(lon lat)` geometry to plain WKB (GeoParquet's fixed
/// encoding — never EWKB) via the same `geozero` writer this crate's own
/// reader decodes with, a real round-trip through the dependency rather than
/// a hand-written byte literal.
fn point_wkb(lon: f64, lat: f64) -> Vec<u8> {
    let geojson = format!(r#"{{"type":"Point","coordinates":[{lon},{lat}]}}"#);
    let mut buf = Vec::new();
    {
        let mut writer = geozero::wkb::WkbWriter::new(&mut buf, geozero::wkb::WkbDialect::Wkb);
        geozero::geojson::GeoJson(&geojson)
            .process_geom(&mut writer)
            .expect("encodes the point geometry to WKB");
    }
    buf
}

fn main() {
    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet");

    let bbox_fields = Fields::from(vec![
        Field::new("xmin", DataType::Float64, false),
        Field::new("ymin", DataType::Float64, false),
        Field::new("xmax", DataType::Float64, false),
        Field::new("ymax", DataType::Float64, false),
    ]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("name", DataType::Utf8, false),
        Field::new("value", DataType::Int64, false),
        Field::new("geometry", DataType::Binary, false),
        Field::new("bbox", DataType::Struct(bbox_fields.clone()), false),
    ]));

    let names: Vec<&str> = FEATURES.iter().map(|(name, ..)| *name).collect();
    let values: Vec<i64> = FEATURES.iter().map(|(_, value, ..)| *value).collect();
    let geometries: Vec<Vec<u8>> = FEATURES
        .iter()
        .map(|(_, _, lon, lat)| point_wkb(*lon, *lat))
        .collect();
    let geometry_refs: Vec<&[u8]> = geometries.iter().map(Vec::as_slice).collect();

    // A point's own bbox degenerates to [lon, lon, lat, lat] — still a real
    // per-row covering value, exercising the same struct-column read path a
    // polygon dataset's genuinely distinct min/max would.
    let xmins: Vec<f64> = FEATURES.iter().map(|(_, _, lon, _)| *lon).collect();
    let xmaxs = xmins.clone();
    let ymins: Vec<f64> = FEATURES.iter().map(|(_, _, _, lat)| *lat).collect();
    let ymaxs = ymins.clone();

    let bbox_array = StructArray::new(
        bbox_fields,
        vec![
            Arc::new(Float64Array::from(xmins)) as ArrayRef,
            Arc::new(Float64Array::from(ymins)) as ArrayRef,
            Arc::new(Float64Array::from(xmaxs)) as ArrayRef,
            Arc::new(Float64Array::from(ymaxs)) as ArrayRef,
        ],
        None,
    );

    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(StringArray::from(names)) as ArrayRef,
            Arc::new(Int64Array::from(values)) as ArrayRef,
            Arc::new(BinaryArray::from(geometry_refs)) as ArrayRef,
            Arc::new(bbox_array) as ArrayRef,
        ],
    )
    .expect("builds the record batch");

    let geo_metadata = serde_json::json!({
        "version": "1.1.0",
        "primary_column": "geometry",
        "columns": {
            "geometry": {
                "encoding": "WKB",
                "geometry_types": ["Point"],
                "bbox": [-4.0, 46.0, 4.0, 54.0],
                "covering": {
                    "bbox": {
                        "xmin": ["bbox", "xmin"],
                        "ymin": ["bbox", "ymin"],
                        "xmax": ["bbox", "xmax"],
                        "ymax": ["bbox", "ymax"]
                    }
                }
            }
        }
    })
    .to_string();

    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_string(), geo_metadata)]))
        .set_compression(Compression::SNAPPY)
        .build();

    let file = File::create(&out_path).expect("creates the fixture file");
    let mut writer =
        ArrowWriter::try_new(file, schema, Some(props)).expect("creates the arrow writer");
    writer.write(&batch).expect("writes the record batch");
    writer.close().expect("finalizes the parquet file");

    let size = std::fs::metadata(&out_path).unwrap().len();
    println!("wrote {} ({} bytes)", out_path.display(), size);
}
