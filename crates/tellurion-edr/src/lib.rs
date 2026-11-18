//! OGC API - Environmental Data Retrieval (EDR) Part 1 v1.1 for Tellurion.
//!
//! Exposes `/collections`, `/instances`, `/locations`, `/items`, `/position`,
//! `/area`, `/cube`, `/radius`, `/trajectory`, and `/corridor` with native sparse
//! multi-item datacube aggregation (`S ⊆ D`), OGC API - Features Part 3 / CQL2
//! filtering, and CoverageJSON (`OGC 21-069`) / GeoJSON output.

pub mod crs;
pub mod handlers;
pub mod models;
pub mod params;
pub mod problem;
pub mod router;
pub mod sparse;

pub use problem::ApiError;
pub use router::{coverages_router, router, standalone_router};

pub const JSON_MEDIA_TYPE: &str = "application/json";
pub const GEOJSON_MEDIA_TYPE: &str = "application/geo+json";
pub const COVJSON_MEDIA_TYPE: &str = "application/prs.coverage+json";
pub const COVJSON_LEGACY_MEDIA_TYPE: &str = "application/vnd.cov+json";
pub const DOMAIN_PROFILE_URI: &str = "http://www.opengis.net/def/profile/covjson/domain";
pub const COVJSON_DOMAIN_MEDIA_TYPE: &str =
    "application/prs.coverage+json; profile=\"http://www.opengis.net/def/profile/covjson/domain\"";

pub const CONFORMANCE_CLASSES: &[&str] = &[
    "http://www.opengis.net/spec/ogcapi-edr-1/1.1/conf/core",
    "http://www.opengis.net/spec/ogcapi-edr-1/1.1/conf/collections",
    "http://www.opengis.net/spec/ogcapi-edr-1/1.1/conf/json",
    "http://www.opengis.net/spec/ogcapi-edr-1/1.1/conf/edr-geojson",
    "http://www.opengis.net/spec/ogcapi-edr-1/1.1/conf/queries",
    "http://www.opengis.net/spec/ogcapi-edr-1/1.1/conf/covjson",
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/filter",
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
    "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
    "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-spatial-functions",
    "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions",
];

pub const COVERAGES_CONFORMANCE_CLASSES: &[&str] = &[
    "https://www.opengis.net/spec/ogcapi-coverages-1/1.0/conf/core",
    "https://www.opengis.net/spec/ogcapi-coverages-1/1.0/conf/spatial-subsetting",
    "https://www.opengis.net/spec/ogcapi-coverages-1/1.0/conf/temporal-subsetting",
    "https://www.opengis.net/spec/ogcapi-coverages-1/1.0/conf/coveragejson",
    "https://www.opengis.net/spec/ogcapi-coverages-3/1.0/conf/scenes",
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/filter",
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
    "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
    "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-spatial-functions",
    "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions",
];
