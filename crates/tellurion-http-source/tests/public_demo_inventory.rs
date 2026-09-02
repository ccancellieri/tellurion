use std::{collections::HashSet, fs, path::PathBuf};

use reqwest::{header, redirect::Policy, StatusCode};
use serde::Deserialize;
use url::Url;

const INVENTORY_PATH: &str = "../../demo/sources/public-examples.yaml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    version: u16,
    examples: Vec<Example>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Example {
    id: String,
    status: Status,
    executable: bool,
    title: String,
    provider: String,
    license: License,
    attribution: String,
    source_page: String,
    url: String,
    transport: Transport,
    format: Format,
    connector: Connector,
    activation: Activation,
    content: Content,
    resource: Option<Resource>,
    render: Option<Render>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Active,
    Candidate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Transport {
    RangeNative,
    ChunkNative,
    BoundedSpool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Format {
    TiledGeotiff,
    Geoparquet,
    ShapefileZip,
    Geojson,
    Geozarr,
    Grib2,
    Hdf5,
    Netcdf,
    Geopackage,
    Flatgeobuf,
    Pmtiles,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct License {
    verification: LicenseVerification,
    label: String,
    terms_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum LicenseVerification {
    Confirmed,
    ReviewRequired,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connector {
    state: ConnectorState,
    reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum ConnectorState {
    Ready,
    Planned,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    state: ActivationState,
    reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum ActivationState {
    Approved,
    ConnectorUnavailable,
    ReviewBlocked,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Content {
    kind: ContentKind,
    revision: String,
    expected_length: Option<u64>,
    expected_strong_etag: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum ContentKind {
    Object,
    Prefix,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resource {
    selected: String,
    crs: Option<String>,
    extent: Option<[f64; 4]>,
    geometry_type: Option<String>,
    feature_count: Option<u64>,
    attributes: Option<Vec<String>>,
    tested_initial_view: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Render {
    profile: String,
    band: Option<u8>,
}

fn inventory_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(INVENTORY_PATH)
}

fn read_inventory() -> Inventory {
    let path = inventory_path();
    let yaml = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&yaml).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn assert_https_443(label: &str, raw: &str) {
    let url = Url::parse(raw).unwrap_or_else(|error| panic!("{label}: invalid URL: {error}"));
    assert_eq!(url.scheme(), "https", "{label}: URL must use HTTPS");
    assert_eq!(
        url.port_or_known_default(),
        Some(443),
        "{label}: URL must use port 443"
    );
    assert!(
        url.username().is_empty() && url.password().is_none(),
        "{label}: URL must not carry userinfo"
    );
    assert!(url.query().is_none(), "{label}: URL must not carry a query");
    assert!(
        url.fragment().is_none(),
        "{label}: URL must not carry a fragment"
    );
}

#[test]
fn public_demo_inventory_is_honest_and_complete() {
    let inventory = read_inventory();
    assert_eq!(inventory.version, 1, "inventory schema version changed");
    assert!(
        !inventory.examples.is_empty(),
        "inventory must contain examples"
    );

    let mut ids = HashSet::new();
    let mut formats = HashSet::new();
    let mut active = 0;
    for example in &inventory.examples {
        assert!(
            example
                .id
                .chars()
                .all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'),
            "{}: id must be a lowercase slug",
            example.id
        );
        assert!(ids.insert(&example.id), "{}: duplicate id", example.id);
        assert!(
            !example.title.trim().is_empty(),
            "{}: title missing",
            example.id
        );
        assert!(
            !example.provider.trim().is_empty(),
            "{}: provider missing",
            example.id
        );
        assert!(
            !example.license.label.trim().is_empty(),
            "{}: license missing",
            example.id
        );
        assert!(
            !example.attribution.trim().is_empty(),
            "{}: attribution missing",
            example.id
        );
        assert_https_443(&format!("{} source page", example.id), &example.source_page);
        assert_https_443(&example.id, &example.url);
        assert!(
            !example.connector.reason.trim().is_empty(),
            "{}: connector reason missing",
            example.id
        );
        assert!(
            !example.activation.reason.trim().is_empty(),
            "{}: activation reason missing",
            example.id
        );
        assert!(
            !example.content.revision.trim().is_empty(),
            "{}: revision missing",
            example.id
        );
        formats.insert(example.format);

        match example.license.verification {
            LicenseVerification::Confirmed => {
                assert!(
                    example.license.terms_url.is_some(),
                    "{}: confirmed license needs verified terms URL",
                    example.id
                );
                assert_https_443(
                    &format!("{} license", example.id),
                    example.license.terms_url.as_deref().expect("checked above"),
                );
            }
            LicenseVerification::ReviewRequired => {
                assert!(
                    example.license.terms_url.is_none(),
                    "{}: review-required license must not expose a terms URL",
                    example.id
                );
                assert!(
                    !example.executable,
                    "{}: unconfirmed license must not be executable",
                    example.id
                );
                assert_eq!(
                    example.activation.state,
                    ActivationState::ReviewBlocked,
                    "{}: unconfirmed license must remain review-blocked",
                    example.id
                );
            }
        }

        match example.status {
            Status::Active => {
                active += 1;
                assert!(
                    example.executable,
                    "{}: active example must be executable",
                    example.id
                );
                assert_eq!(
                    example.connector.state,
                    ConnectorState::Ready,
                    "{}: active connector must be ready",
                    example.id
                );
                assert_eq!(
                    example.activation.state,
                    ActivationState::Approved,
                    "{}: active example must be approved",
                    example.id
                );
                assert_eq!(
                    example.content.kind,
                    ContentKind::Object,
                    "{}: active object must have one identity",
                    example.id
                );
                assert!(
                    example
                        .content
                        .expected_length
                        .is_some_and(|length| length > 0),
                    "{}: active example needs length",
                    example.id
                );
                assert!(
                    example
                        .content
                        .expected_strong_etag
                        .as_deref()
                        .is_some_and(|etag| etag.starts_with('"')
                            && etag.ends_with('"')
                            && !etag.starts_with("W/")),
                    "{}: active example needs a strong ETag",
                    example.id
                );
                let resource = example
                    .resource
                    .as_ref()
                    .expect("active example needs resource selection");
                assert!(
                    !resource.selected.trim().is_empty(),
                    "{}: selected resource missing",
                    example.id
                );
                assert!(resource.crs.is_some(), "{}: CRS missing", example.id);
                assert!(resource.extent.is_some(), "{}: extent missing", example.id);
                let render = example
                    .render
                    .as_ref()
                    .expect("active example needs render profile");
                assert!(
                    !render.profile.trim().is_empty(),
                    "{}: render profile missing",
                    example.id
                );
                if example.format != Format::TiledGeotiff {
                    assert!(
                        resource
                            .geometry_type
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty()),
                        "{}: vector geometry type missing",
                        example.id
                    );
                    assert!(
                        resource.feature_count.is_some(),
                        "{}: vector feature count missing",
                        example.id
                    );
                    assert!(
                        resource
                            .attributes
                            .as_ref()
                            .is_some_and(|values| !values.is_empty()),
                        "{}: vector attributes missing",
                        example.id
                    );
                    assert!(
                        resource
                            .tested_initial_view
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty()),
                        "{}: tested initial view missing",
                        example.id
                    );
                }

                let attributes = || {
                    resource
                        .attributes
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                };
                match example.id.as_str() {
                    "esa-worldcover-2021-italy" => {
                        assert_eq!(
                            example.source_page,
                            "https://esa-worldcover.org/en/data-access"
                        );
                        assert_eq!(example.url, "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_N39E012_Map.tif");
                        assert_eq!(example.transport, Transport::RangeNative);
                        assert_eq!(example.format, Format::TiledGeotiff);
                        assert_eq!(
                            example.content.revision,
                            "ESA WorldCover v200 object identity verified 2026-08-28"
                        );
                        assert_eq!(example.content.expected_length, Some(41_236_803));
                        assert_eq!(
                            example.content.expected_strong_etag.as_deref(),
                            Some("\"493c0cdb8f7b96acb9f326575f7c9b8b-5\"")
                        );
                        assert_eq!(resource.selected, "land-cover map band");
                        assert_eq!(resource.crs.as_deref(), Some("EPSG:4326"));
                        assert_eq!(resource.extent, Some([12.0, 39.0, 15.0, 42.0]));
                        assert_eq!(render.profile, "categorical-land-cover");
                        assert_eq!(render.band, Some(1));
                        assert_eq!(example.license.verification, LicenseVerification::Confirmed);
                        assert_eq!(example.license.label, "CC BY 4.0");
                        assert_eq!(
                            example.license.terms_url.as_deref(),
                            Some("https://creativecommons.org/licenses/by/4.0/")
                        );
                    }
                    "google-microsoft-open-buildings-monaco" => {
                        assert_eq!(
                            example.source_page,
                            "https://source.coop/vida/google-microsoft-open-buildings"
                        );
                        assert_eq!(example.url, "https://data.source.coop/vida/google-microsoft-open-buildings/geoparquet/by_country/country_iso=MCO/MCO.parquet");
                        assert_eq!(example.transport, Transport::RangeNative);
                        assert_eq!(example.format, Format::Geoparquet);
                        assert_eq!(
                            example.content.revision,
                            "Source Cooperative object identity verified 2026-09-02"
                        );
                        assert_eq!(example.content.expected_length, Some(181_283));
                        assert_eq!(
                            example.content.expected_strong_etag.as_deref(),
                            Some("\"249d53f3e864d33f6998d9ad5f6c0225\"")
                        );
                        assert_eq!(resource.selected, "Open Buildings footprint features");
                        assert_eq!(resource.crs.as_deref(), Some("EPSG:4326"));
                        assert_eq!(
                            resource.extent,
                            Some([7.409199, 43.725495, 7.439595, 43.750982])
                        );
                        assert_eq!(resource.geometry_type.as_deref(), Some("Polygon"));
                        assert_eq!(resource.feature_count, Some(868));
                        assert_eq!(
                            attributes(),
                            [
                                "boundary_id",
                                "bf_source",
                                "confidence",
                                "area_in_meters",
                                "s2_id",
                                "country_iso",
                                "geohash"
                            ]
                        );
                        assert_eq!(
                            resource.tested_initial_view.as_deref(),
                            Some("Fit the verified Monaco extent.")
                        );
                        assert_eq!(render.profile, "vector-default");
                        assert_eq!(render.band, None);
                        assert_eq!(example.license.label, "ODbL 1.0");
                        assert_eq!(
                            example.license.terms_url.as_deref(),
                            Some("https://opendatacommons.org/licenses/odbl/1-0/")
                        );
                        assert_eq!(example.attribution, "Google Open Buildings and Microsoft GlobalMLBuildingFootprints, distributed by Source Cooperative under ODbL 1.0.");
                    }
                    "natural-earth-110m-coastline-shapefile" => {
                        assert_eq!(example.source_page, "https://www.naturalearthdata.com/downloads/110m-physical-vectors/110m-coastline/");
                        assert_eq!(example.url, "https://naturalearth.s3.amazonaws.com/110m_physical/ne_110m_coastline.zip");
                        assert_eq!(example.transport, Transport::BoundedSpool);
                        assert_eq!(example.format, Format::ShapefileZip);
                        assert_eq!(
                            example.content.revision,
                            "Natural Earth S3 object identity verified 2026-09-01"
                        );
                        assert_eq!(example.content.expected_length, Some(85_352));
                        assert_eq!(
                            example.content.expected_strong_etag.as_deref(),
                            Some("\"2defae9f229bf50e4f6d26ee8a8cca7d\"")
                        );
                        assert_eq!(resource.selected, "Coastline features");
                        assert_eq!(resource.crs.as_deref(), Some("EPSG:4326"));
                        assert_eq!(resource.extent, Some([-180.0, -85.609038, 180.0, 83.64513]));
                        assert_eq!(resource.geometry_type.as_deref(), Some("LineString"));
                        assert_eq!(resource.feature_count, Some(134));
                        assert_eq!(attributes(), ["scalerank", "featurecla", "min_zoom"]);
                        assert_eq!(
                            resource.tested_initial_view.as_deref(),
                            Some("Fit the verified world extent.")
                        );
                        assert_eq!(render.profile, "vector-default");
                        assert_eq!(render.band, None);
                        assert_eq!(example.license.label, "Public domain");
                        assert_eq!(
                            example.license.terms_url.as_deref(),
                            Some("https://www.naturalearthdata.com/about/terms-of-use/")
                        );
                        assert_eq!(example.attribution, "Made with Natural Earth.");
                    }
                    unexpected => {
                        panic!("active example {unexpected} needs exact contract assertions")
                    }
                }
            }
            Status::Candidate => {
                assert!(
                    !example.executable,
                    "{}: candidate must not be executable",
                    example.id
                );
                assert_eq!(
                    example.connector.state,
                    ConnectorState::Planned,
                    "{}: candidate connector must be planned",
                    example.id
                );
                assert_ne!(
                    example.activation.state,
                    ActivationState::Approved,
                    "{}: candidate must remain gated",
                    example.id
                );
                if example.content.kind == ContentKind::Prefix {
                    assert!(
                        example.content.expected_length.is_none(),
                        "{}: prefix must not claim object length",
                        example.id
                    );
                    assert!(
                        example.content.expected_strong_etag.is_none(),
                        "{}: prefix must not claim one ETag",
                        example.id
                    );
                }
            }
        }
    }

    for id in [
        "pangeo-geozarr-tci",
        "gdal-grib2-gfswave",
        "gdal-hdf5-geoeos",
        "gdal-netcdf-era5-t2m",
        "gdal-geopackage-poly",
        "flatgeobuf-countries",
        "pmtiles-florence",
    ] {
        let example = inventory
            .examples
            .iter()
            .find(|example| example.id == id)
            .unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(
            example.license.verification,
            LicenseVerification::ReviewRequired
        );
        assert!(example.license.terms_url.is_none());
        assert!(!example.executable);
        assert_eq!(example.activation.state, ActivationState::ReviewBlocked);
    }

    assert_eq!(active, 3, "all and only verified connectors are active");
    for format in [
        Format::TiledGeotiff,
        Format::Geoparquet,
        Format::ShapefileZip,
        Format::Geojson,
        Format::Geozarr,
        Format::Grib2,
        Format::Hdf5,
        Format::Netcdf,
        Format::Geopackage,
        Format::Flatgeobuf,
        Format::Pmtiles,
    ] {
        assert!(
            formats.contains(&format),
            "missing requested {format:?} example"
        );
    }
}

#[tokio::test]
#[ignore = "requires public network; run with -- --ignored"]
async fn active_objects_keep_their_verified_range_identity() {
    let inventory = read_inventory();
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .expect("build isolated verifier client");

    for example in inventory
        .examples
        .iter()
        .filter(|example| example.status == Status::Active)
    {
        assert_ne!(
            example.transport,
            Transport::ChunkNative,
            "{}: active verifier requires one immutable object",
            example.id
        );
        let response = client
            .get(&example.url)
            .header(header::RANGE, "bytes=0-0")
            .header(header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .unwrap_or_else(|error| panic!("{}: range probe failed: {error}", example.id));
        assert_eq!(
            response.status(),
            StatusCode::PARTIAL_CONTENT,
            "{}: probe must return 206",
            example.id
        );
        assert_eq!(
            response.headers().get(header::CONTENT_ENCODING),
            None,
            "{}: response must be identity encoded",
            example.id
        );
        assert_eq!(
            response.headers().get(header::LOCATION),
            None,
            "{}: redirects are not accepted",
            example.id
        );

        let expected_length = example.content.expected_length.expect("active length");
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            Some(format!("bytes 0-0/{expected_length}").as_str()),
            "{}: range interval or total changed",
            example.id
        );
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok()),
            example.content.expected_strong_etag.as_deref(),
            "{}: strong identity changed",
            example.id
        );
        assert_eq!(
            response.bytes().await.expect("read range body").len(),
            1,
            "{}: range body length changed",
            example.id
        );
    }
}
