//! Parses the single locator string this driver reads from
//! `StorageDecl.url_env`'s named environment variable.
//!
//! `StorageDecl` (`tellurion-core`) carries no field beyond `url_env` for a
//! driver's own configuration — the same shape every other file-backed
//! driver in this workspace uses. `postgis`'s own `url_env` already carries
//! a full connection URI, not a bare secret, so a single structured string
//! in this field is an established convention here, not a new one. Iceberg
//! table metadata has no comparable geometry concept at all — the geometry
//! column and its four covering bbox columns are pure operator declarations
//! with no backend-derivable default — so this driver folds the REST
//! catalog endpoint plus those declarations into `url_env`'s value as a
//! small query-string-shaped locator rather than adding a new `StorageDecl`
//! field, which would touch `tellurion-core::config` (out of this slice's
//! file list):
//!
//! ```text
//! <rest-catalog-base-url>?namespace=<ns1.ns2>&table=<name>&geometry=<column>&bbox=<xmin>,<ymin>,<xmax>,<ymax>[&plan_cache_ttl_s=<seconds>]
//! ```
//!
//! - `rest-catalog-base-url`: the REST catalog service's base URL (e.g.
//!   `http://localhost:8181`) — passed straight through as the REST
//!   catalog client's `uri` property (`iceberg_catalog_rest::
//!   REST_CATALOG_PROP_URI`). This driver never talks to any other catalog
//!   transport (`driver.rs`'s crate docs).
//! - `namespace`: dot-separated namespace segments (`ns`, or `ns1.ns2`).
//! - `table`: the bare table name.
//! - `geometry`: the declared WKB geometry column name.
//! - `bbox`: exactly four comma-separated column names, in
//!   `xmin,ymin,xmax,ymax` order — the covering bbox columns scan planning
//!   prunes on (`driver.rs`).
//! - `plan_cache_ttl_s`: optional, defaults to
//!   [`DEFAULT_PLAN_CACHE_TTL_S`] — see `driver.rs`'s "Planned-file cache"
//!   docs for what this bounds.
//! - `s3_endpoint`, `s3_region`, `s3_access_key_env`,
//!   `s3_secret_key_env`: optional here, and REQUIRED as a set the moment
//!   the table's own metadata says its files live on S3 (`#123`). Parsing
//!   never refuses their absence — a local-filesystem table must keep
//!   parsing byte-for-byte as it did before this slice — so the refusal
//!   lands at table-load time instead, where the actual storage scheme is
//!   known and the message can name both the table and the missing key
//!   (`driver.rs`'s `require_supported_storage`). See [`S3Declaration`].
//!
//! The first four query keys are mandatory; a missing one refuses with a
//! precise `Error::Config` naming it (`IcebergDriverError::
//! MissingDeclaration`). Unknown query keys are ignored, not refused,
//! leaving room for a later slice to add optional declarations without
//! breaking existing config.
//!
//! ## Why the S3 connection settings live here and not in `config.yaml`
//!
//! `s3_endpoint`/`s3_region` are ordinary, non-secret connection facts, and
//! `s3_access_key_env`/`s3_secret_key_env` are the NAMES of environment
//! variables — never the credentials themselves. That is the identical
//! shape `tellurion_core::config::ObjectStoreProfile::S3` already uses for
//! this workspace's own object-store profile: config names the variable,
//! infrastructure supplies its value. Carrying them in this locator rather
//! than in a new `StorageDecl` field keeps them PER STORAGE (two Iceberg
//! storages can address two different S3 endpoints) and keeps this slice
//! out of `tellurion-core::config` entirely; and because the locator is
//! itself read out of `StorageDecl.url_env`, no part of it — not even the
//! variable names — ever has to appear in `config.yaml`.

use crate::driver::DEFAULT_PLAN_CACHE_TTL_S;
use crate::error::{IcebergDriverError as DriverError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BboxColumns {
    pub xmin: String,
    pub ymin: String,
    pub xmax: String,
    pub ymax: String,
}

impl BboxColumns {
    pub(crate) fn as_array(&self) -> [&str; 4] {
        [
            self.xmin.as_str(),
            self.ymin.as_str(),
            self.xmax.as_str(),
            self.ymax.as_str(),
        ]
    }
}

/// The four locator keys that, together, let this driver read a table whose
/// files live on an S3-protocol store. All four or none: a partially
/// declared set is refused by name at table-load time
/// (`driver.rs`'s `require_supported_storage`), never silently completed
/// with a guessed endpoint or a guessed region.
///
/// `access_key_env`/`secret_key_env` are variable NAMES. This struct never
/// holds a credential, is never compared against one, and nothing here is
/// read from `config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct S3Declaration {
    pub endpoint: String,
    pub region: String,
    pub access_key_env: String,
    pub secret_key_env: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IcebergLocation {
    pub catalog_uri: String,
    pub namespace: Vec<String>,
    pub table: String,
    pub geometry_column: String,
    pub bbox: BboxColumns,
    pub plan_cache_ttl_s: u64,
    /// `None` when the locator declares none of the four `s3_*` keys — the
    /// only shape that existed before `#123`, and still the shape every
    /// local-filesystem table uses. `Some` only when ALL four are present;
    /// a partial set is carried through as [`PartialS3Declaration`] so the
    /// load-time refusal can name exactly which key is missing.
    pub s3: Option<S3Declaration>,
    /// Whatever subset of the four `s3_*` keys the locator actually
    /// declared, kept verbatim so `require_supported_storage` can name the
    /// missing one rather than reporting a generic "S3 is not configured".
    pub s3_partial: PartialS3Declaration,
}

/// The raw, unvalidated `s3_*` half of a parsed locator — see
/// [`IcebergLocation::s3_partial`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PartialS3Declaration {
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub access_key_env: Option<String>,
    pub secret_key_env: Option<String>,
}

impl PartialS3Declaration {
    /// `Some(field-name)` for the first declared-key gap, in the order the
    /// module doc lists them — the exact string a refusal names. `None`
    /// only when all four are present and non-empty.
    pub(crate) fn first_missing(&self) -> Option<&'static str> {
        for (name, value) in [
            ("s3_endpoint", &self.endpoint),
            ("s3_region", &self.region),
            ("s3_access_key_env", &self.access_key_env),
            ("s3_secret_key_env", &self.secret_key_env),
        ] {
            if value.as_deref().unwrap_or_default().is_empty() {
                return Some(name);
            }
        }
        None
    }

    fn complete(&self) -> Option<S3Declaration> {
        if self.first_missing().is_some() {
            return None;
        }
        Some(S3Declaration {
            endpoint: self.endpoint.clone()?,
            region: self.region.clone()?,
            access_key_env: self.access_key_env.clone()?,
            secret_key_env: self.secret_key_env.clone()?,
        })
    }
}

impl IcebergLocation {
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        let (uri, query) = raw.split_once('?').unwrap_or((raw, ""));

        let mut namespace: Option<String> = None;
        let mut table: Option<String> = None;
        let mut geometry: Option<String> = None;
        let mut bbox: Option<String> = None;
        let mut plan_cache_ttl_s: Option<String> = None;
        let mut s3_partial = PartialS3Declaration::default();

        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| DriverError::MalformedQuery(pair.to_string()))?;
            match key {
                "namespace" => namespace = Some(value.to_string()),
                "table" => table = Some(value.to_string()),
                "geometry" => geometry = Some(value.to_string()),
                "bbox" => bbox = Some(value.to_string()),
                "plan_cache_ttl_s" => plan_cache_ttl_s = Some(value.to_string()),
                "s3_endpoint" => s3_partial.endpoint = Some(value.to_string()),
                "s3_region" => s3_partial.region = Some(value.to_string()),
                "s3_access_key_env" => s3_partial.access_key_env = Some(value.to_string()),
                // This parses an environment-variable name, never a credential.
                "s3_secret_key_env" => s3_partial.secret_key_env = Some(value.to_string()), // gitleaks:allow
                _ => {}
            }
        }

        let namespace_raw = namespace
            .filter(|v| !v.is_empty())
            .ok_or(DriverError::MissingDeclaration { field: "namespace" })?;
        let namespace: Vec<String> = namespace_raw
            .split('.')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if namespace.is_empty() {
            return Err(DriverError::MissingDeclaration { field: "namespace" });
        }

        let table = table
            .filter(|v| !v.is_empty())
            .ok_or(DriverError::MissingDeclaration { field: "table" })?;
        let geometry_column = geometry
            .filter(|v| !v.is_empty())
            .ok_or(DriverError::MissingDeclaration { field: "geometry" })?;

        let bbox_raw = bbox.ok_or(DriverError::MissingDeclaration { field: "bbox" })?;
        let columns: Vec<&str> = bbox_raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let [xmin, ymin, xmax, ymax] = columns[..] else {
            return Err(DriverError::InvalidBboxDeclaration(bbox_raw));
        };

        let plan_cache_ttl_s = match plan_cache_ttl_s {
            None => DEFAULT_PLAN_CACHE_TTL_S,
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| DriverError::InvalidPlanCacheTtl(raw))?,
        };

        Ok(Self {
            catalog_uri: uri.to_string(),
            namespace,
            table,
            geometry_column,
            bbox: BboxColumns {
                xmin: xmin.to_string(),
                ymin: ymin.to_string(),
                xmax: xmax.to_string(),
                ymax: ymax.to_string(),
            },
            plan_cache_ttl_s,
            s3: s3_partial.complete(),
            s3_partial,
        })
    }

    /// Dot-joined `namespace.table` — used only in error messages naming
    /// the table; never a physical identity fact reported to a caller (see
    /// `driver.rs`'s `PhysicalCollection.name`, which reports the bare
    /// table name instead).
    pub(crate) fn identifier(&self) -> String {
        let mut parts = self.namespace.clone();
        parts.push(self.table.clone());
        parts.join(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_location() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=geo&table=points&geometry=geom&bbox=xmin,ymin,xmax,ymax",
        )
        .unwrap();
        assert_eq!(location.catalog_uri, "http://localhost:8181");
        assert_eq!(location.namespace, vec!["geo".to_string()]);
        assert_eq!(location.table, "points");
        assert_eq!(location.geometry_column, "geom");
        assert_eq!(location.bbox.as_array(), ["xmin", "ymin", "xmax", "ymax"]);
        assert_eq!(location.plan_cache_ttl_s, DEFAULT_PLAN_CACHE_TTL_S);
    }

    #[test]
    fn parses_a_dotted_multi_segment_namespace() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=a.b.c&table=t&geometry=g&bbox=a,b,c,d",
        )
        .unwrap();
        assert_eq!(location.namespace, vec!["a", "b", "c"]);
    }

    #[test]
    fn identifier_is_dot_joined_namespace_and_table() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=a.b&table=t&geometry=g&bbox=a,b,c,d",
        )
        .unwrap();
        assert_eq!(location.identifier(), "a.b.t");
    }

    #[test]
    fn missing_namespace_is_a_precise_refusal() {
        let err = IcebergLocation::parse("http://localhost:8181?table=t&geometry=g&bbox=a,b,c,d")
            .unwrap_err();
        assert!(matches!(
            err,
            DriverError::MissingDeclaration { field: "namespace" }
        ));
    }

    #[test]
    fn missing_table_is_a_precise_refusal() {
        let err =
            IcebergLocation::parse("http://localhost:8181?namespace=ns&geometry=g&bbox=a,b,c,d")
                .unwrap_err();
        assert!(matches!(
            err,
            DriverError::MissingDeclaration { field: "table" }
        ));
    }

    #[test]
    fn missing_geometry_is_a_precise_refusal() {
        let err = IcebergLocation::parse("http://localhost:8181?namespace=ns&table=t&bbox=a,b,c,d")
            .unwrap_err();
        assert!(matches!(
            err,
            DriverError::MissingDeclaration { field: "geometry" }
        ));
    }

    #[test]
    fn missing_bbox_is_a_precise_refusal() {
        let err = IcebergLocation::parse("http://localhost:8181?namespace=ns&table=t&geometry=g")
            .unwrap_err();
        assert!(matches!(
            err,
            DriverError::MissingDeclaration { field: "bbox" }
        ));
    }

    #[test]
    fn a_bbox_declaration_with_the_wrong_column_count_is_refused() {
        let err = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=xmin,ymin,xmax",
        )
        .unwrap_err();
        assert!(matches!(err, DriverError::InvalidBboxDeclaration(_)));
    }

    #[test]
    fn no_query_at_all_reports_the_first_missing_field() {
        let err = IcebergLocation::parse("http://localhost:8181").unwrap_err();
        assert!(matches!(
            err,
            DriverError::MissingDeclaration { field: "namespace" }
        ));
    }

    #[test]
    fn an_unknown_query_key_is_ignored_not_refused() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=a,b,c,d&future=1",
        )
        .unwrap();
        assert_eq!(location.table, "t");
    }

    #[test]
    fn plan_cache_ttl_s_overrides_the_default_when_declared() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=a,b,c,d&plan_cache_ttl_s=15",
        )
        .unwrap();
        assert_eq!(location.plan_cache_ttl_s, 15);
    }

    #[test]
    fn a_locator_declaring_no_s3_keys_parses_with_no_s3_declaration() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=a,b,c,d",
        )
        .unwrap();
        // The pre-`#123` shape, unchanged: parsing never demands S3
        // settings a local-filesystem table has no use for.
        assert_eq!(location.s3, None);
        assert_eq!(location.s3_partial, PartialS3Declaration::default());
    }

    #[test]
    fn all_four_s3_keys_parse_into_a_complete_declaration() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=a,b,c,d\
             &s3_endpoint=http://minio:9000&s3_region=us-east-1\
             &s3_access_key_env=MY_KEY&s3_secret_key_env=MY_SECRET",
        )
        .unwrap();
        assert_eq!(
            location.s3,
            Some(S3Declaration {
                endpoint: "http://minio:9000".to_string(),
                region: "us-east-1".to_string(),
                access_key_env: "MY_KEY".to_string(),
                secret_key_env: "MY_SECRET".to_string(),
            })
        );
        assert_eq!(location.s3_partial.first_missing(), None);
    }

    #[test]
    fn a_partial_s3_declaration_parses_but_names_the_first_missing_key() {
        // Parsing still succeeds — the refusal belongs at table-load time,
        // where this driver knows whether the table is on S3 at all (see
        // `driver.rs`'s `require_supported_storage`). What parsing owes the
        // refusal is the NAME of the gap, not a generic "S3 unconfigured".
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=a,b,c,d\
             &s3_endpoint=http://minio:9000&s3_access_key_env=MY_KEY\
             &s3_secret_key_env=MY_SECRET",
        )
        .unwrap();
        assert_eq!(location.s3, None);
        assert_eq!(location.s3_partial.first_missing(), Some("s3_region"));
    }

    #[test]
    fn an_empty_s3_key_counts_as_missing_not_as_a_declared_empty_string() {
        let location = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=a,b,c,d\
             &s3_endpoint=&s3_region=us-east-1&s3_access_key_env=MY_KEY\
             &s3_secret_key_env=MY_SECRET",
        )
        .unwrap();
        assert_eq!(location.s3, None);
        assert_eq!(location.s3_partial.first_missing(), Some("s3_endpoint"));
    }

    #[test]
    fn a_non_numeric_plan_cache_ttl_s_is_a_precise_refusal() {
        let err = IcebergLocation::parse(
            "http://localhost:8181?namespace=ns&table=t&geometry=g&bbox=a,b,c,d&plan_cache_ttl_s=soon",
        )
        .unwrap_err();
        assert!(matches!(err, DriverError::InvalidPlanCacheTtl(_)));
    }
}
