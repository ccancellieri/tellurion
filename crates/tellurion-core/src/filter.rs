//! Backend-neutral CQL2 filter AST (`#33`): the single representation that
//! feeds OGC API — Features Part 3 filtering today, and is meant to also back
//! attribute-based access-control expressions and STAC search filtering later
//! (same design note as the issue this module closes) — none of those callers
//! ever see a third-party parser's type, only [`Filter`].
//!
//! ## Why this crate hand-rolls the parser instead of depending on `cql2`
//!
//! `cql2` (crates.io, MIT, actively maintained, v0.5.6 as evaluated for
//! `#33`) parses both CQL2-text and CQL2-JSON to a rich `Expr` type with
//! SQL-dialect conversion (`ToSqlAst`, `ToDuckSQL`) and JSON Schema
//! validation (`Validator`) built in. None of that surface is usable here
//! regardless of the dependency cost: every value that crosses a crate
//! boundary in this workspace must already be this crate's own type (see the
//! driver-contract design doc), so `cql2::Expr` would only ever be an
//! intermediate value immediately converted to [`Filter`] — the parsing
//! itself is the only part actually reusable. Its `Cargo.toml` declares no
//! `[features]` to shed the unused parts, so a single `cql2 = "0.5"`
//! dependency pulls the whole thing: `cargo tree` against a throwaway crate
//! depending on it alone resolved **149 unique transitive crates**, including
//! the pair `pest`/`pest_derive` (the PEG parser generator, needed for text
//! parsing), the pair `sqlparser`/`sqlparser_derive` (a full SQL AST library,
//! needed only for `ToSqlAst`), `jsonschema` (needed only for `Validator`),
//! `geo`, `wkt`, and `geo-types` (needed only for `Geometry`'s own geometry
//! algebra), plus `jiff`, `regex`, `phf`, `rand`, and `uuid-simd` — for a
//! ~63s cold build. This workspace's own `[workspace.dependencies]` table is
//! a deliberately short,
//! hand-curated list (see `Cargo.toml`), and the `--no-default-features`
//! PMTiles/FlatGeobuf builds exist specifically to stay database-free and
//! light; pulling in a SQL-dialect library and a JSON Schema validator to
//! reach a text/JSON parser for a grammar this small (see "Scope" below)
//! would work directly against that. Basic CQL2 plus `S_INTERSECTS` and the
//! three temporal predicates is a small, well-specified grammar — the parser
//! below is a few hundred lines with zero new dependencies (only
//! `serde_json`, already a `tellurion-core` dependency).
//!
//! ## Scope
//!
//! Both encodings support exactly: comparison predicates (`=`, `<>`, `<`,
//! `>`, `<=`, `>=`), `IS [NOT] NULL`, `AND`/`OR`/`NOT` with parenthesized
//! grouping, `S_INTERSECTS(property, geometry)`, and `T_AFTER`/`T_BEFORE`/
//! `T_DURING(property, ...)` — the OGC CQL2 "Basic CQL2" conformance class
//! plus the minimum spatial/temporal surface OGC API Features Part 3 needs
//! (`#33`) — plus, added since, the "Advanced comparison operators" class
//! (`LIKE`/`NOT LIKE`, `BETWEEN`/`NOT BETWEEN`, `IN`/`NOT IN`,
//! [`Filter::Like`]/[`Filter::Between`]/[`Filter::In`]), a narrow slice of
//! case-insensitive comparison (`CASEI(property) = CASEI('literal')`/`<>`
//! only — see [`Filter::CaseInsensitiveCompare`]'s own doc for why this
//! doesn't cover every `CASEI` use the standard allows), the rest of the
//! binary spatial-predicate set beyond `S_INTERSECTS` (`S_WITHIN`,
//! `S_CONTAINS`, `S_DISJOINT`, `S_TOUCHES`, `S_OVERLAPS`, `S_CROSSES`,
//! `S_EQUALS` — [`Filter::Spatial`]), full 2D WKT geometry literals in
//! CQL2-text ([`GeometryLiteral::Wkt`]), and the rest of the binary temporal
//! predicate set beyond `T_AFTER`/`T_BEFORE`/`T_DURING`
//! ([`Filter::Temporal`]). Accent-insensitive comparison, arrays, functions,
//! arithmetic, and property-property comparisons remain out of scope.
//!
//! **Spatial literals and the two spatial-function conformance classes**:
//! CQL2 (OGC 21-065r2) `basic-spatial-functions` (Requirement 11) requires
//! only the `S_INTERSECTS` *operator*; Permission 7 of that same class
//! (`/per/basic-spatial-functions/spatial-data-types`) explicitly lets a
//! server restrict `spatialInstance` to `pointTaggedText` and
//! `bboxTaggedText` only, so POINT + BBOX literal support is all the
//! *literal grammar* it asks for. That is a statement about operators and
//! literals, and nothing more: the class also names Basic CQL2 as a
//! Dependency, and Basic CQL2's Requirement 1 (`/req/basic-cql2/cql2-filter`)
//! lists `spatialPredicate` among the rules that "do not have to be
//! supported" — declaring `basic-spatial-functions` is exactly what removes
//! that exception, so it additionally promises `S_INTERSECTS` anywhere the
//! `booleanExpression` BNF admits a predicate (under `NOT`, in any `OR`
//! branch, any number of times). A compiler that parses the operator and the
//! literals but can only place the predicate in restricted positions does
//! *not* satisfy the class — see `tellurion-geopackage`'s own
//! `cql2_conformance_classes` doc, which is where `#134` settled that
//! against the class's own Abstract Test Suite and withheld it.
//! `spatial-functions` (Requirement 13) is stricter about operators: "the
//! server SHALL support all standardized spatial comparison functions as
//! defined by the BNF rule `spatialFunction`" — the full WKT literal grammar
//! (Point, LineString, Polygon, MultiPoint, MultiLineString, MultiPolygon,
//! GeometryCollection, BBox), with no equivalent permission to narrow it.
//! [`GeometryLiteral::Wkt`] parses all seven WKT tagged-text shapes (2D
//! only — `Z`/`M`/`ZM` and `EMPTY` are rejected with a named-cause 400,
//! never silently accepted), which is what lets a driver whose compiler
//! places these predicates freely (`tellurion-postgis`, which emits each one
//! as an ordinary inline SQL boolean) declare both classes honestly, now that
//! every operator either class requires (`S_INTERSECTS`/`S_WITHIN`/
//! `S_CONTAINS`/`S_DISJOINT`/`S_TOUCHES`/`S_OVERLAPS`/`S_CROSSES`/
//! `S_EQUALS`) is implemented against that grammar.
//!
//! **Design decision (still standing)**: CQL2-JSON's spatial literal stays
//! `bbox`/GeoJSON-geometry only — its structured format already makes an
//! arbitrary GeoJSON geometry trivial to accept without a WKT grammar, so
//! there is no reason to also parse WKT text nested inside JSON.
//! CQL2-text's spatial literal is `BBOX(...)` or a WKT tagged-text literal;
//! it never accepts a nested GeoJSON object (CQL2-text has no JSON-literal
//! production). Both shapes funnel into the one [`GeometryLiteral`] enum
//! ([`GeometryLiteral::Bbox`]/[`GeometryLiteral::GeoJson`]/
//! [`GeometryLiteral::Wkt`]) that every spatial predicate, `validate`, and
//! `tellurion-postgis`'s `sql::compile_filter` consume identically
//! regardless of which parser produced it.
//!
//! **Temporal functions**: `T_AFTER`/`T_BEFORE`/`T_DURING` were already
//! implemented; this lane adds the remaining twelve
//! ([`Filter::Temporal`]/[`TemporalOp`]) so the "Temporal functions"
//! conformance class (Requirement 14, `/req/temporal-functions/
//! temporal-functions` — "the server SHALL support all standardized
//! temporal comparison functions as defined by the BNF rule
//! `temporalFunction`": `T_AFTER`, `T_BEFORE`, `T_CONTAINS`, `T_DISJOINT`,
//! `T_DURING`, `T_EQUALS`, `T_FINISHEDBY`, `T_FINISHES`, `T_INTERSECTS`,
//! `T_MEETS`, `T_METBY`, `T_OVERLAPPEDBY`, `T_OVERLAPS`, `T_STARTEDBY`,
//! `T_STARTS`) can be declared honestly. Every operator compiles to the
//! Allen interval-algebra relation between two intervals (the requirement's
//! own dependency is the W3C/OGC Time Ontology in OWL, which formalizes
//! these thirteen relations plus equality); this crate's schema only ever
//! has an *instant*-valued datetime column, never an interval column, so
//! `sql::compile_filter` treats the property side as the degenerate interval
//! `[p, p]` against the literal's instant (`[t, t]`) or interval (`[start,
//! end]`) bound. That degeneracy is mathematically correct, not a
//! workaround: five relations (`T_OVERLAPS`/`T_OVERLAPPEDBY`/
//! `T_STARTEDBY`/`T_FINISHEDBY`/`T_CONTAINS`) require the *first* interval
//! to have positive duration, which an instant column never does, so those
//! five compile to a SQL condition that can never match any row — exactly
//! what Allen's algebra says an instant's relationship to a proper interval
//! must be for those five relations, not a stub. See `sql::temporal_op_sql`'s
//! own doc for the full per-operator derivation.
//!
//! ## Property validation
//!
//! [`validate`] checks every property a [`Filter`] references against a
//! collection's derived [`CollectionDescriptor`] (`#19`): a comparison or
//! `IS NULL` predicate's property must be a real attribute column (or the
//! collection's geometry/datetime column, both of which still appear as
//! ordinary GeoJSON properties — see `tellurion-postgis`'s `properties_expr`),
//! an `S_INTERSECTS` predicate's property must be exactly the geometry
//! column, and a temporal predicate's property must be exactly the datetime
//! column. Any mismatch — including a collection with no datetime column at
//! all — fails with [`Error::Invalid`], naming the offending property, so a
//! typo or an operator used against the wrong column becomes a 400 before
//! ever reaching a driver.
//!
//! [`validate`] also takes the collection's optional declared [`SchemaDecl`]
//! (`#44`). `None` (the default, free-form collection) leaves every rule
//! above unchanged. When a schema is declared with `additional_properties:
//! false`, a comparison/`IS NULL` property must additionally name one of the
//! schema's own declared properties — the collection's geometry/datetime
//! columns are always exempt from that narrowing, since they are
//! structurally part of the collection regardless of what the declared
//! schema enumerates.

use crate::config::SchemaDecl;
use crate::descriptor::CollectionDescriptor;
use crate::error::{Error, Result};

/// `filter-lang` query parameter value selecting the CQL2-text parser.
pub const FILTER_LANG_CQL2_TEXT: &str = "cql2-text";
/// `filter-lang` query parameter value selecting the CQL2-JSON parser.
pub const FILTER_LANG_CQL2_JSON: &str = "cql2-json";

/// One CQL2 (1.0, OGC 21-065r2) conformance class URI, named so every driver
/// crate that declares its own satisfied subset
/// ([`FeatureSource::cql2_conformance_classes`](crate::storage::FeatureSource::cql2_conformance_classes),
/// `#105`) references the same constant rather than retyping the URI —
/// see this module's own "Scope" section above for what each class covers,
/// and each driver's own `cql2_conformance_classes` doc for exactly which
/// of these it declares and why.
pub const CQL2_CLASS_BASIC: &str = "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2";
pub const CQL2_CLASS_CQL2_TEXT: &str = "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text";
pub const CQL2_CLASS_CQL2_JSON: &str = "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json";
pub const CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS: &str =
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-spatial-functions";
pub const CQL2_CLASS_ADVANCED_COMPARISON_OPERATORS: &str =
    "http://www.opengis.net/spec/cql2/1.0/conf/advanced-comparison-operators";
pub const CQL2_CLASS_SPATIAL_FUNCTIONS: &str =
    "http://www.opengis.net/spec/cql2/1.0/conf/spatial-functions";
pub const CQL2_CLASS_TEMPORAL_FUNCTIONS: &str =
    "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions";
/// Never appears in any driver's declared [`FeatureSource::
/// cql2_conformance_classes`](crate::storage::FeatureSource::cql2_conformance_classes)
/// today — see this constant's own "withheld" paragraph below for why. Kept
/// as a named constant anyway so a test pinning its absence (every driver
/// crate has one) names the same URI the rest of this module does, rather
/// than a hand-typed string that could quietly drift from it.
pub const CQL2_CLASS_CASE_INSENSITIVE_COMPARISON: &str =
    "http://www.opengis.net/spec/cql2/1.0/conf/case-insensitive-comparison";

/// The full set of CQL2 conformance classes this crate's shared parser and
/// `Filter` AST could ever let some driver satisfy — see this module's own
/// "Scope" section above for what each class covers. This is no longer a
/// single workspace-wide declaration read by every collection regardless of
/// backend (`#105`): each driver crate now declares its own, narrower
/// subset via
/// [`FeatureSource::cql2_conformance_classes`](crate::storage::FeatureSource::cql2_conformance_classes) —
/// PostGIS compiles every class here in full; GeoPackage and the Iceberg driver
/// each compile a real subset (see their own `cql2_conformance_classes`
/// docs for exactly which, and why); FlatGeobuf, GeoParquet, and the memory
/// driver decline CQL2 filtering outright and declare none of them.
///
/// This constant now serves two purposes instead of being read directly by
/// a protocol crate's own conformance list:
///
/// 1. Every driver's own declared subset is built from these same named
///    URIs ([`CQL2_CLASS_BASIC`] and friends), so a driver crate never
///    hand-types a class URI that could drift from this one.
/// 2. [`crate::router::Router::cql2_conformance_classes`] folds this whole
///    set down by intersecting every in-use driver's own declared subset —
///    this is the candidate universe the fold starts from when at least one
///    features-capable driver participates. With no such driver, the fold's
///    capability policy discards the seed because the deployment has no CQL2
///    evaluator (see that method's own doc for the reasoning).
///
/// `case-insensitive-comparison` is deliberately excluded from this set, for
/// a different reason than a driver-capability gap — see this doc's own
/// section below.
///
/// ## `case-insensitive-comparison` stays withheld, for a correctness reason
///
/// This one was previously declared on the strength of the narrow
/// `CASEI(property) = CASEI('literal')`/`<>` shape every filter-capable
/// driver parses and compiles ([`Filter::CaseInsensitiveCompare`]) — but
/// parsing the shape and conforming to the class turned out to be different
/// claims. Both `tellurion-postgis` and `tellurion-geopackage` fold case via
/// their own engine's `lower()` (see each crate's `sql::compile_filter`),
/// and neither engine's `lower()` performs the full Unicode case folding
/// CQL2 requires — PostgreSQL's `lower()` is locale-dependent and, under the
/// common `C`/`POSIX` collation, folds ASCII bytes only (verified live:
/// `lower()` on a `C`-collation column leaves `İstanbul`, `ΑΘΗΝΑ`, and
/// `МОСКВА` byte-for-byte unchanged); SQLite's built-in `lower()` is
/// ASCII-only unconditionally, in every locale (no ICU extension linked —
/// see `tellurion-geopackage`'s own `rusqlite` feature list). Even under a
/// Unicode-friendly locale the two case-mapping strategies still diverge:
/// `lower()` performs simple, length-preserving case mapping, never full
/// Unicode case *folding*, so a pair that a full fold considers
/// case-equivalent — German `STRASSE`/`straße` (`ß` case-folds to `ss` only
/// under full folding) is the standard example — never matches, in any
/// locale.
///
/// The per-driver conformance model (`#105`) this doc's own history once
/// called for as the fix now exists, and it still doesn't change the
/// answer: a per-driver declaration lets PostGIS earn a class GeoPackage
/// never will (see `spatial-functions`/`temporal-functions`), but
/// `case-insensitive-comparison` is not that kind of gap — PostGIS's own
/// `lower()` is exactly as ASCII-bound as every other driver's. Earning it
/// back needs a real fix on the compiler side first (PostGIS creating or
/// discovering a deterministic ICU collation and compiling a genuinely
/// correct fold under it, tracked separately) before any driver's declared
/// set can honestly include it. Every driver crate's own
/// `cql2_conformance_classes` implementation is expected to omit
/// [`CQL2_CLASS_CASE_INSENSITIVE_COMPARISON`] until then — pinned by a test
/// in each one.
pub const CQL2_CONFORMANCE_CLASSES: &[&str] = &[
    CQL2_CLASS_BASIC,
    CQL2_CLASS_CQL2_TEXT,
    CQL2_CLASS_CQL2_JSON,
    CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS,
    CQL2_CLASS_ADVANCED_COMPARISON_OPERATORS,
    CQL2_CLASS_SPATIAL_FUNCTIONS,
    CQL2_CLASS_TEMPORAL_FUNCTIONS,
];

/// One OGC API — Features Part 3: Filtering (19-079r2, Approved 1.0)
/// conformance class URI (`#217`). These are the classes that describe the
/// `filter`/`filter-lang` query parameters themselves — the protocol seam —
/// as opposed to the CQL2 classes above, which describe the expression
/// language a driver's compiler can handle. Named here, next to CQL2's own
/// constants, so the deployment fold and the protocol crate's tests reference
/// the same strings.
pub const FILTERING_CLASS_FILTER: &str =
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/filter";
pub const FILTERING_CLASS_FEATURES_FILTER: &str =
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/features-filter";
pub const FILTERING_CLASS_QUERYABLES_QUERY_PARAMETERS: &str =
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables-query-parameters";

/// The Part 3 classes a deployment may claim only where the drivers behind it
/// actually accept a `filter` — the seed
/// [`crate::router::Router::filtering_conformance_classes`] folds over
/// [`FeatureSource::filter_capable`](crate::storage::FeatureSource::filter_capable),
/// exactly as [`CQL2_CONFORMANCE_CLASSES`] is the seed the CQL2 fold narrows
/// (`#217`). FlatGeobuf, GeoParquet, and the memory driver answer 400 to any
/// `filter`, so a deployment built on them can honour none of these three;
/// `tellurion-features`' own static list therefore names none of them.
///
/// The fold gates on more than `filter_capable` (`#217`): Requirement 8
/// (`/req/filter/filter-crs-param`) is conditional on "Server supports
/// additional coordinate reference systems", so a `crs_capable` driver must
/// also honour `filter-crs`
/// ([`FeatureSource::filter_crs_capable`](crate::storage::FeatureSource::filter_crs_capable))
/// before a deployment built on it may claim these. See
/// [`crate::router::Router::filtering_conformance_classes`] for the full
/// derivation.
///
/// `conf/queryables` is deliberately NOT here: the
/// `/collections/{collectionId}/queryables` document is served for every
/// collection regardless of driver, so it stays in that crate's static list
/// where it is always honest. `queryables-query-parameters` is a different
/// claim — it promises those queryables also work as *filters* on `/items`
/// (`tellurion-features`' `params::build_queryable_filter` compiles them into
/// the same `Filter` a `filter` parameter produces, behind the same
/// `filter_capable` gate), so it folds with the other two.
pub const FILTERING_CONFORMANCE_CLASSES: &[&str] = &[
    FILTERING_CLASS_FILTER,
    FILTERING_CLASS_FEATURES_FILTER,
    FILTERING_CLASS_QUERYABLES_QUERY_PARAMETERS,
];

/// STAC API — Item Search: Filter Extension (`#248`), the class that binds
/// filtering to the STAC `/search` endpoint. Verified 2026-08 against the
/// `stac-api-extensions/filter` repo's `README.md` at its `v1.0.0-rc.4` tag —
/// the tag `tellurion-stac` already pins for this extension, the Filter
/// Extension having no non-prerelease release.
///
/// The URI is a STAC one, but it lives here beside Part 3's own classes rather
/// than in `tellurion-stac` for the reason `#105` already moved the CQL2
/// classes out of both protocol crates' static lists: whether this class is
/// honest is a property of the *drivers this deployment configured*, which
/// only [`crate::router::Router`] can see. A `pub const` in a protocol crate
/// could only ever be declared unconditionally.
pub const ITEM_SEARCH_FILTER_CLASS: &str = "https://api.stacspec.org/v1.0.0/item-search#filter";

/// The seed [`crate::router::Router::item_search_filter_conformance_classes`]
/// folds — the Filter Extension defines exactly one class binding filtering to
/// `/search`, so this is [`ITEM_SEARCH_FILTER_CLASS`] alone, spelled as a slice
/// for the same reason [`crate::crs::CRS_CONFORMANCE_CLASSES`] is.
///
/// Why it folds at all: the extension's own "Conformance Classes" section says
/// an implementation declaring this class **must** also support Basic CQL2
/// (`http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2`), because Item
/// Search Filter "binds the Filter and Basic CQL2 conformance classes to apply
/// to the Item Search endpoint (`/search`)". Basic CQL2 is already folded per
/// deployment ([`crate::router::Router::cql2_conformance_classes`]), so a
/// deployment whose drivers accept no `filter` at all declares no CQL2 class —
/// and declaring this one alongside would claim a binding to a class the same
/// document withholds.
pub const ITEM_SEARCH_FILTER_CONFORMANCE_CLASSES: &[&str] = &[ITEM_SEARCH_FILTER_CLASS];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

/// The comparison operator a `CASEI(...) = CASEI(...)`/`<>` predicate uses
/// ([`Filter::CaseInsensitiveCompare`]) — a deliberate subset of [`CompareOp`]
/// (`<`/`>`/`<=`/`>=` on case-folded strings has no well-defined meaning this
/// lane needs to support).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaseInsensitiveCompareOp {
    Eq,
    Ne,
}

/// The binary spatial predicates CQL2's "Spatial functions" conformance
/// class adds beyond `S_INTERSECTS` ([`Filter::Spatial`]). Argument order
/// matches the CQL2 function call directly: `S_WITHIN(a, b)` means "a is
/// within b", the same order PostGIS's own `ST_Within(a, b)` uses — no
/// argument swap needed for any of these seven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpatialOp {
    Within,
    Contains,
    Disjoint,
    Touches,
    Overlaps,
    Crosses,
    Equals,
}

/// The binary temporal predicates CQL2's "Temporal functions" conformance
/// class adds beyond `T_AFTER`/`T_BEFORE`/`T_DURING` ([`Filter::Temporal`]).
/// See this module's top-level "Temporal functions" doc for how each op
/// compiles against an instant-valued property column, and
/// `sql::temporal_op_sql` for the exact per-operator SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemporalOp {
    Contains,
    Disjoint,
    Equals,
    FinishedBy,
    Finishes,
    Intersects,
    Meets,
    MetBy,
    OverlappedBy,
    Overlaps,
    StartedBy,
    Starts,
}

/// The right-hand side of a [`Filter::Temporal`] predicate: either a single
/// instant or a `[start, end]` interval, mirroring `T_AFTER`/`T_BEFORE`'s
/// single instant and `T_DURING`'s interval pair respectively. Both fields
/// stay raw RFC 3339 text — same rationale as `Filter::After::instant` and
/// `Filter::During::start`/`::end`: parsing/validating the timestamp is
/// `sql::compile_filter`'s job (`::text::timestamptz`), not this crate's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TemporalValue {
    Instant(String),
    Interval(String, String),
}

/// A scalar literal on the right-hand side of a comparison predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Text(String),
    Number(f64),
    Bool(bool),
}

/// A spatial literal for `S_INTERSECTS`/[`Filter::Spatial`]. `GeoJson` and
/// `Wkt` are kept as data, never as a real geometry type — `tellurion-core`
/// stays free of a geometry-parsing dependency, the same reasoning that
/// keeps `storage::DatetimeRange` a raw string; turning either into a real
/// geometry is each driver's job (PostGIS: `ST_GeomFromGeoJSON`/
/// `ST_GeomFromText`, both parameter-bound — see `sql::geometry_literal_expr`).
#[derive(Debug, Clone, PartialEq)]
pub enum GeometryLiteral {
    /// `[minx, miny, maxx, maxy]`, same axis order as `ItemsQuery::bbox`.
    Bbox([f64; 4]),
    GeoJson(serde_json::Value),
    /// A CQL2-text WKT tagged-text geometry literal (`POINT(...)`,
    /// `POLYGON(...)`, ...) — see [`WktGeometry`].
    Wkt(WktGeometry),
}

/// A parsed 2D WKT geometry literal (CQL2-text only — CQL2-JSON uses
/// [`GeometryLiteral::GeoJson`] instead). Every coordinate is `[x, y]`; the
/// text parser ([`parse_wkt_geometry`]) rejects `Z`/`M`/`ZM` dimensionality
/// and `EMPTY` geometries before this type is ever constructed, so every
/// value here is always exactly 2D and non-empty. Stored as parsed
/// coordinates rather than the original source text — the same "canonical
/// form, not source text" choice [`GeometryLiteral::Bbox`] already makes —
/// so [`Filter::fingerprint`] and `sql::compile_filter`'s `ST_GeomFromText`
/// call both work from one unambiguous representation
/// ([`WktGeometry::to_wkt_text`]) regardless of how the original literal was
/// formatted (whitespace, coordinate precision).
#[derive(Debug, Clone, PartialEq)]
pub enum WktGeometry {
    Point([f64; 2]),
    LineString(Vec<[f64; 2]>),
    /// Each inner `Vec` is one ring (exterior first, holes after) — WKT
    /// itself doesn't distinguish exterior from interior rings structurally,
    /// same as PostGIS's own `ST_GeomFromText`.
    Polygon(Vec<Vec<[f64; 2]>>),
    MultiPoint(Vec<[f64; 2]>),
    MultiLineString(Vec<Vec<[f64; 2]>>),
    MultiPolygon(Vec<Vec<Vec<[f64; 2]>>>),
    GeometryCollection(Vec<WktGeometry>),
}

/// The backend-neutral filter AST (`#33`). Every variant carries plain
/// property-name strings rather than a resolved column handle — [`validate`]
/// is the seam that checks those names against a collection's physical shape
/// before a driver ever compiles this tree, so a driver's own compiler
/// (`tellurion-postgis`'s `sql::compile_filter`) can trust every property it
/// sees names a real, already-checked column.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Compare {
        property: String,
        op: CompareOp,
        value: Literal,
    },
    IsNull {
        property: String,
        negated: bool,
    },
    /// `property [NOT] LIKE 'pattern'` (CQL2 "Advanced comparison
    /// operators"). `pattern` travels through to SQL as a bound parameter
    /// unchanged — Postgres's own `LIKE` already uses `%`/`_` wildcards and a
    /// backslash escape by default, the same convention CQL2's `LIKE` grammar
    /// itself uses, so no translation step is needed between the two.
    Like {
        property: String,
        pattern: String,
        negated: bool,
    },
    /// `property [NOT] BETWEEN low AND high` (inclusive range test).
    Between {
        property: String,
        low: Literal,
        high: Literal,
        negated: bool,
    },
    /// `property [NOT] IN (v1, v2, ...)`. An empty `values` list is not
    /// reachable through either parser (`IN ()` is a syntax error in both
    /// encodings) but is still handled if a caller ever hand-builds one — see
    /// `tellurion-postgis::sql::compile_filter`'s own doc for that fallback.
    In {
        property: String,
        values: Vec<Literal>,
        negated: bool,
    },
    /// `CASEI(property) = CASEI('literal')` or `<>` — CQL2's
    /// case-insensitive-comparison conformance class, narrowed to exactly
    /// this shape: `CASEI()` wrapping a property reference on one side and a
    /// string literal on the other, joined by `=`/`<>`. The standard's own
    /// `CASEI` is a general string-valued function usable anywhere a string
    /// expression is (inside `LIKE`, `IN`, or wrapping only one side of a
    /// comparison against another expression, not necessarily a literal);
    /// this lane implements only the common real-world pattern — a
    /// case-insensitive equality/inequality test against a literal — which is
    /// also the only shape both parsers below accept.
    ///
    /// Parsed and compiled by every filter-capable driver, but the
    /// conformance class this shape is named after stays withheld — each
    /// driver folds case via its own engine's `lower()`, which only
    /// ASCII-folds (PostgreSQL, under the common `C`/`POSIX` collation) or
    /// never folds beyond ASCII at all (SQLite, unconditionally), and even
    /// under a Unicode-friendly locale `lower()`'s simple case mapping still
    /// misses full Unicode case *folding* (`ß`/`ss`, for one). See
    /// [`CQL2_CONFORMANCE_CLASSES`]'s own doc for the full reasoning.
    CaseInsensitiveCompare {
        property: String,
        op: CaseInsensitiveCompareOp,
        value: String,
    },
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
    Intersects {
        property: String,
        geometry: GeometryLiteral,
    },
    /// A binary spatial predicate beyond `S_INTERSECTS` (`S_WITHIN`,
    /// `S_CONTAINS`, `S_DISJOINT`, `S_TOUCHES`, `S_OVERLAPS`, `S_CROSSES`,
    /// `S_EQUALS` — CQL2's "Spatial functions" conformance class). Kept as
    /// its own variant rather than folded into `Intersects` — `Intersects`
    /// already has real callers/tests outside this module (`tellurion-stac`
    /// included) that this lane has no reason to touch.
    Spatial {
        property: String,
        op: SpatialOp,
        geometry: GeometryLiteral,
    },
    After {
        property: String,
        instant: String,
    },
    Before {
        property: String,
        instant: String,
    },
    During {
        property: String,
        start: String,
        end: String,
    },
    /// A binary temporal predicate beyond `T_AFTER`/`T_BEFORE`/`T_DURING`
    /// (CQL2's "Temporal functions" conformance class). Kept as its own
    /// variant rather than folded into `After`/`Before`/`During` for the
    /// same reason `Spatial` stays separate from `Intersects` — those three
    /// already have real callers/tests this lane has no reason to touch.
    Temporal {
        property: String,
        op: TemporalOp,
        value: TemporalValue,
    },
}

impl Filter {
    /// Every property name this filter references, deduplicated in
    /// first-seen order. Exists so a caller that needs "which columns does
    /// this filter touch" (a future ABAC merge point, or a driver wanting to
    /// short-circuit before compiling) doesn't reimplement the tree walk.
    pub fn properties(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_properties(&mut out);
        out
    }

    fn collect_properties<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Filter::Compare { property, .. }
            | Filter::IsNull { property, .. }
            | Filter::Like { property, .. }
            | Filter::Between { property, .. }
            | Filter::In { property, .. }
            | Filter::CaseInsensitiveCompare { property, .. }
            | Filter::Intersects { property, .. }
            | Filter::Spatial { property, .. }
            | Filter::After { property, .. }
            | Filter::Before { property, .. }
            | Filter::During { property, .. }
            | Filter::Temporal { property, .. } => {
                if !out.contains(&property.as_str()) {
                    out.push(property.as_str());
                }
            }
            Filter::And(items) | Filter::Or(items) => {
                for item in items {
                    item.collect_properties(out);
                }
            }
            Filter::Not(inner) => inner.collect_properties(out),
        }
    }

    /// Whether this filter carries a spatial literal anywhere in its tree —
    /// an `S_INTERSECTS` or one of the six other binary spatial predicates
    /// ([`Filter::Intersects`]/[`Filter::Spatial`]), each of which holds a
    /// [`GeometryLiteral`] whose coordinates are expressed in some CRS
    /// (`#247`).
    ///
    /// This is the "is there anything here a `filter-crs` could be about"
    /// question, and it exists because the honest answer is not "does the
    /// request have a filter". A `population > 10` filter has no geometry to
    /// process in any CRS at all, so a driver that cannot transform a spatial
    /// literal has no reason to refuse it, whatever the collection's storage
    /// SRID. The callers are the two protocol handlers that decide whether a
    /// filter is servable at all against a projected collection
    /// (`tellurion-features`' items handler and `tellurion-stac`'s
    /// `unservable_filter_reason`) — without this, their refusal would name a
    /// transform the request never asked for.
    ///
    /// Sits beside [`properties`](Self::properties) as the second reason to
    /// walk this tree without compiling it, and is deliberately not derived
    /// from it: a property name says nothing about whether the predicate
    /// holding it carries coordinates.
    pub fn has_spatial_literal(&self) -> bool {
        match self {
            Filter::Intersects { .. } | Filter::Spatial { .. } => true,
            Filter::And(items) | Filter::Or(items) => items.iter().any(Filter::has_spatial_literal),
            Filter::Not(inner) => inner.has_spatial_literal(),
            Filter::Compare { .. }
            | Filter::IsNull { .. }
            | Filter::Like { .. }
            | Filter::Between { .. }
            | Filter::In { .. }
            | Filter::CaseInsensitiveCompare { .. }
            | Filter::After { .. }
            | Filter::Before { .. }
            | Filter::During { .. }
            | Filter::Temporal { .. } => false,
        }
    }

    /// A stable, process-local hash of this filter's structure (`#34`): the
    /// tile-lane policy fingerprint that partitions the tile cache by
    /// effective grant filter (see `cache::TileKey::policy_fingerprint`'s own
    /// doc for the cache-key composition this feeds). Two filters built from
    /// different subjects' claims but resolving to the same structure — same
    /// operators, properties, and literal values — fingerprint identically,
    /// so those subjects share one cache entry; two structurally different
    /// filters fingerprint differently (collisions aside — this is a hash,
    /// not a full equality check, the same tradeoff every cache key makes).
    ///
    /// Walked by hand rather than derived: [`Literal::Number`] and
    /// [`GeometryLiteral`]'s `f64`/`serde_json::Value` payloads implement
    /// neither `Eq` nor `Hash`, so a plain `#[derive(Hash)]` on [`Filter`]
    /// isn't possible. A JSON geometry literal hashes its canonical
    /// `to_string()` form rather than walking its structure directly —
    /// `serde_json::Value` (this workspace builds without the
    /// `preserve_order` feature) always serializes object keys in sorted
    /// order, so two structurally-equal geometries always produce the same
    /// string regardless of how they were originally ordered.
    ///
    /// Uses `std::collections::hash_map::DefaultHasher` rather than a new
    /// dependency: nothing here needs a cryptographic hash or a guarantee
    /// that outlives one process (the in-memory tile cache this feeds never
    /// does either) — only that two hashes computed in the same process
    /// agree, which `DefaultHasher`'s fixed construction already provides.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash_fingerprint(&mut hasher);
        hasher.finish()
    }

    fn hash_fingerprint<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        match self {
            Filter::Compare {
                property,
                op,
                value,
            } => {
                0u8.hash(state);
                property.hash(state);
                op.hash(state);
                value.hash_fingerprint(state);
            }
            Filter::IsNull { property, negated } => {
                1u8.hash(state);
                property.hash(state);
                negated.hash(state);
            }
            Filter::And(items) => {
                2u8.hash(state);
                items.len().hash(state);
                for item in items {
                    item.hash_fingerprint(state);
                }
            }
            Filter::Or(items) => {
                3u8.hash(state);
                items.len().hash(state);
                for item in items {
                    item.hash_fingerprint(state);
                }
            }
            Filter::Not(inner) => {
                4u8.hash(state);
                inner.hash_fingerprint(state);
            }
            Filter::Intersects { property, geometry } => {
                5u8.hash(state);
                property.hash(state);
                geometry.hash_fingerprint(state);
            }
            Filter::After { property, instant } => {
                6u8.hash(state);
                property.hash(state);
                instant.hash(state);
            }
            Filter::Before { property, instant } => {
                7u8.hash(state);
                property.hash(state);
                instant.hash(state);
            }
            Filter::During {
                property,
                start,
                end,
            } => {
                8u8.hash(state);
                property.hash(state);
                start.hash(state);
                end.hash(state);
            }
            Filter::Like {
                property,
                pattern,
                negated,
            } => {
                9u8.hash(state);
                property.hash(state);
                pattern.hash(state);
                negated.hash(state);
            }
            Filter::Between {
                property,
                low,
                high,
                negated,
            } => {
                10u8.hash(state);
                property.hash(state);
                low.hash_fingerprint(state);
                high.hash_fingerprint(state);
                negated.hash(state);
            }
            Filter::In {
                property,
                values,
                negated,
            } => {
                11u8.hash(state);
                property.hash(state);
                values.len().hash(state);
                for value in values {
                    value.hash_fingerprint(state);
                }
                negated.hash(state);
            }
            Filter::CaseInsensitiveCompare {
                property,
                op,
                value,
            } => {
                12u8.hash(state);
                property.hash(state);
                op.hash(state);
                value.hash(state);
            }
            Filter::Spatial {
                property,
                op,
                geometry,
            } => {
                13u8.hash(state);
                property.hash(state);
                op.hash(state);
                geometry.hash_fingerprint(state);
            }
            Filter::Temporal {
                property,
                op,
                value,
            } => {
                14u8.hash(state);
                property.hash(state);
                op.hash(state);
                value.hash(state);
            }
        }
    }
}

impl Literal {
    fn hash_fingerprint<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        match self {
            Literal::Text(s) => {
                0u8.hash(state);
                s.hash(state);
            }
            Literal::Number(n) => {
                1u8.hash(state);
                n.to_bits().hash(state);
            }
            Literal::Bool(b) => {
                2u8.hash(state);
                b.hash(state);
            }
        }
    }
}

impl GeometryLiteral {
    fn hash_fingerprint<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        match self {
            GeometryLiteral::Bbox(bbox) => {
                0u8.hash(state);
                for v in bbox {
                    v.to_bits().hash(state);
                }
            }
            GeometryLiteral::GeoJson(value) => {
                1u8.hash(state);
                value.to_string().hash(state);
            }
            GeometryLiteral::Wkt(geometry) => {
                2u8.hash(state);
                geometry.hash_fingerprint(state);
            }
        }
    }
}

fn hash_coord<H: std::hash::Hasher>(coord: &[f64; 2], state: &mut H) {
    use std::hash::Hash;
    coord[0].to_bits().hash(state);
    coord[1].to_bits().hash(state);
}

fn hash_point_list<H: std::hash::Hasher>(points: &[[f64; 2]], state: &mut H) {
    use std::hash::Hash;
    points.len().hash(state);
    for point in points {
        hash_coord(point, state);
    }
}

fn hash_ring_list<H: std::hash::Hasher>(rings: &[Vec<[f64; 2]>], state: &mut H) {
    use std::hash::Hash;
    rings.len().hash(state);
    for ring in rings {
        hash_point_list(ring, state);
    }
}

impl WktGeometry {
    fn hash_fingerprint<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        match self {
            WktGeometry::Point(c) => {
                0u8.hash(state);
                hash_coord(c, state);
            }
            WktGeometry::LineString(pts) => {
                1u8.hash(state);
                hash_point_list(pts, state);
            }
            WktGeometry::Polygon(rings) => {
                2u8.hash(state);
                hash_ring_list(rings, state);
            }
            WktGeometry::MultiPoint(pts) => {
                3u8.hash(state);
                hash_point_list(pts, state);
            }
            WktGeometry::MultiLineString(lines) => {
                4u8.hash(state);
                hash_ring_list(lines, state);
            }
            WktGeometry::MultiPolygon(polys) => {
                5u8.hash(state);
                polys.len().hash(state);
                for rings in polys {
                    hash_ring_list(rings, state);
                }
            }
            WktGeometry::GeometryCollection(geoms) => {
                6u8.hash(state);
                geoms.len().hash(state);
                for geom in geoms {
                    geom.hash_fingerprint(state);
                }
            }
        }
    }

    /// Canonical uppercase WKT text (`POINT(1 2)`, `POLYGON((...))`, ...) —
    /// `sql::geometry_literal_expr` binds this whole string as a single
    /// `ST_GeomFromText` parameter, never string-interpolated. `f64`'s
    /// `Display` impl already produces the shortest round-trippable decimal
    /// form (no trailing `.0` for integral values, e.g. `1` not `1.0`),
    /// which `ST_GeomFromText` parses identically either way.
    pub fn to_wkt_text(&self) -> String {
        match self {
            WktGeometry::Point(c) => format!("POINT({} {})", c[0], c[1]),
            WktGeometry::LineString(pts) => format!("LINESTRING({})", fmt_point_list(pts)),
            WktGeometry::Polygon(rings) => format!("POLYGON({})", fmt_ring_list(rings)),
            WktGeometry::MultiPoint(pts) => format!(
                "MULTIPOINT({})",
                pts.iter()
                    .map(|c| format!("({} {})", c[0], c[1]))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            WktGeometry::MultiLineString(lines) => {
                format!("MULTILINESTRING({})", fmt_ring_list(lines))
            }
            WktGeometry::MultiPolygon(polys) => format!(
                "MULTIPOLYGON({})",
                polys
                    .iter()
                    .map(|rings| format!("({})", fmt_ring_list(rings)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            WktGeometry::GeometryCollection(geoms) => format!(
                "GEOMETRYCOLLECTION({})",
                geoms
                    .iter()
                    .map(WktGeometry::to_wkt_text)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

fn fmt_point_list(points: &[[f64; 2]]) -> String {
    points
        .iter()
        .map(|c| format!("{} {}", c[0], c[1]))
        .collect::<Vec<_>>()
        .join(",")
}

fn fmt_ring_list(rings: &[Vec<[f64; 2]>]) -> String {
    rings
        .iter()
        .map(|ring| format!("({})", fmt_point_list(ring)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Parses `input` under `filter_lang` (`FILTER_LANG_CQL2_TEXT` or
/// `FILTER_LANG_CQL2_JSON`) — the single entry point `tellurion-features`
/// dispatches the `filter`/`filter-lang` query parameters through. An
/// unrecognized `filter_lang` fails the same way a syntax error inside the
/// filter itself does: `Error::Invalid`, a 400 at the protocol layer.
pub fn parse(filter_lang: &str, input: &str) -> Result<Filter> {
    match filter_lang {
        FILTER_LANG_CQL2_TEXT => parse_text(input),
        FILTER_LANG_CQL2_JSON => parse_json(input),
        other => Err(Error::Invalid(format!(
            "unsupported filter-lang '{other}': expected '{FILTER_LANG_CQL2_TEXT}' or '{FILTER_LANG_CQL2_JSON}'"
        ))),
    }
}

/// Checks every property `filter` references against `descriptor` (`#33`,
/// building on `#19`'s derived attribute schema), and against `schema`
/// (`#44`) when the collection declares one. See this module's top-level
/// "Property validation" docs for the precise rule per predicate kind.
pub fn validate(
    filter: &Filter,
    descriptor: &CollectionDescriptor,
    schema: Option<&SchemaDecl>,
) -> Result<()> {
    match filter {
        Filter::Compare { property, .. }
        | Filter::IsNull { property, .. }
        | Filter::Like { property, .. }
        | Filter::Between { property, .. }
        | Filter::In { property, .. }
        | Filter::CaseInsensitiveCompare { property, .. } => {
            validate_attribute_property(property, descriptor, schema)
        }
        Filter::Intersects { property, .. } => {
            if descriptor.geometry.as_deref() == Some(property.as_str()) {
                Ok(())
            } else {
                Err(Error::Invalid(format!(
                    "unknown property '{property}': S_INTERSECTS only supports this collection's geometry column"
                )))
            }
        }
        Filter::Spatial { property, op, .. } => {
            if descriptor.geometry.as_deref() == Some(property.as_str()) {
                Ok(())
            } else {
                Err(Error::Invalid(format!(
                    "unknown property '{property}': {op:?} only supports this collection's geometry column"
                )))
            }
        }
        Filter::After { property, .. }
        | Filter::Before { property, .. }
        | Filter::During { property, .. }
        | Filter::Temporal { property, .. } => {
            if descriptor.datetime.as_deref() == Some(property.as_str()) {
                Ok(())
            } else {
                Err(Error::Invalid(format!(
                    "unknown property '{property}': temporal operators only support this collection's datetime column"
                )))
            }
        }
        Filter::And(items) | Filter::Or(items) => items
            .iter()
            .try_for_each(|item| validate(item, descriptor, schema)),
        Filter::Not(inner) => validate(inner, descriptor, schema),
    }
}

/// A comparison/`IS NULL` property is known when it is the collection's
/// geometry or datetime column (always allowed, regardless of `schema` —
/// see this module's "Property validation" docs), or a real attribute
/// column reported by `descriptor` that `schema` doesn't exclude: absent a
/// declared schema, or with one that leaves `additional_properties: true`
/// (the default), any attribute column qualifies; a schema declaring
/// `additional_properties: false` narrows that to properties it actually
/// lists — `descriptor::reconcile_schema` already guarantees every such
/// property is a real attribute column by the time a request reaches here.
fn validate_attribute_property(
    property: &str,
    descriptor: &CollectionDescriptor,
    schema: Option<&SchemaDecl>,
) -> Result<()> {
    if descriptor.geometry.as_deref() == Some(property)
        || descriptor.datetime.as_deref() == Some(property)
    {
        return Ok(());
    }
    let known = descriptor
        .attributes
        .as_ref()
        .is_some_and(|attrs| attrs.iter().any(|a| a.name == property));
    if !known {
        return Err(Error::Invalid(format!(
            "unknown property '{property}': not part of this collection's attribute schema"
        )));
    }
    if let Some(schema) = schema {
        if !schema.additional_properties && !schema.properties.iter().any(|p| p.name == property) {
            return Err(Error::Invalid(format!(
                "unknown property '{property}': not part of this collection's declared schema, which disallows additional properties"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CQL2-text parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Num(f64),
    True,
    False,
    Null,
    And,
    Or,
    Not,
    Is,
    LParen,
    RParen,
    Comma,
    Op(CompareOp),
    SIntersects,
    SWithin,
    SContains,
    SDisjoint,
    STouches,
    SOverlaps,
    SCrosses,
    SEquals,
    TAfter,
    TBefore,
    TDuring,
    TContains,
    TDisjoint,
    TEquals,
    TFinishedBy,
    TFinishes,
    TIntersects,
    TMeets,
    TMetBy,
    TOverlappedBy,
    TOverlaps,
    TStartedBy,
    TStarts,
    Bbox,
    Timestamp,
    Interval,
    Like,
    Between,
    In,
    Casei,
    WktPoint,
    WktLineString,
    WktPolygon,
    WktMultiPoint,
    WktMultiLineString,
    WktMultiPolygon,
    WktGeometryCollection,
}

fn keyword_or_ident(word: &str) -> Token {
    match word.to_ascii_uppercase().as_str() {
        "AND" => Token::And,
        "OR" => Token::Or,
        "NOT" => Token::Not,
        "IS" => Token::Is,
        "NULL" => Token::Null,
        "TRUE" => Token::True,
        "FALSE" => Token::False,
        "S_INTERSECTS" => Token::SIntersects,
        "S_WITHIN" => Token::SWithin,
        "S_CONTAINS" => Token::SContains,
        "S_DISJOINT" => Token::SDisjoint,
        "S_TOUCHES" => Token::STouches,
        "S_OVERLAPS" => Token::SOverlaps,
        "S_CROSSES" => Token::SCrosses,
        "S_EQUALS" => Token::SEquals,
        "T_AFTER" => Token::TAfter,
        "T_BEFORE" => Token::TBefore,
        "T_DURING" => Token::TDuring,
        "T_CONTAINS" => Token::TContains,
        "T_DISJOINT" => Token::TDisjoint,
        "T_EQUALS" => Token::TEquals,
        "T_FINISHEDBY" => Token::TFinishedBy,
        "T_FINISHES" => Token::TFinishes,
        "T_INTERSECTS" => Token::TIntersects,
        "T_MEETS" => Token::TMeets,
        "T_METBY" => Token::TMetBy,
        "T_OVERLAPPEDBY" => Token::TOverlappedBy,
        "T_OVERLAPS" => Token::TOverlaps,
        "T_STARTEDBY" => Token::TStartedBy,
        "T_STARTS" => Token::TStarts,
        "BBOX" => Token::Bbox,
        "TIMESTAMP" => Token::Timestamp,
        "INTERVAL" => Token::Interval,
        "LIKE" => Token::Like,
        "BETWEEN" => Token::Between,
        "IN" => Token::In,
        "CASEI" => Token::Casei,
        "POINT" => Token::WktPoint,
        "LINESTRING" => Token::WktLineString,
        "POLYGON" => Token::WktPolygon,
        "MULTIPOINT" => Token::WktMultiPoint,
        "MULTILINESTRING" => Token::WktMultiLineString,
        "MULTIPOLYGON" => Token::WktMultiPolygon,
        "GEOMETRYCOLLECTION" => Token::WktGeometryCollection,
        _ => Token::Ident(word.to_string()),
    }
}

/// Tokenizes CQL2-text over `char`s (not bytes) so string-literal content may
/// carry any UTF-8 property/value text without manual boundary arithmetic.
fn lex(input: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '=' => {
                tokens.push(Token::Op(CompareOp::Eq));
                i += 1;
            }
            '<' => {
                if chars.get(i + 1) == Some(&'>') {
                    tokens.push(Token::Op(CompareOp::Ne));
                    i += 2;
                } else if chars.get(i + 1) == Some(&'=') {
                    tokens.push(Token::Op(CompareOp::Le));
                    i += 2;
                } else {
                    tokens.push(Token::Op(CompareOp::Lt));
                    i += 1;
                }
            }
            '>' => {
                if chars.get(i + 1) == Some(&'=') {
                    tokens.push(Token::Op(CompareOp::Ge));
                    i += 2;
                } else {
                    tokens.push(Token::Op(CompareOp::Gt));
                    i += 1;
                }
            }
            '\'' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => {
                            return Err(Error::Invalid(
                                "unterminated string literal in CQL2 filter".to_string(),
                            ))
                        }
                        Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                            s.push('\'');
                            i += 2;
                        }
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(ch) => {
                            s.push(*ch);
                            i += 1;
                        }
                    }
                }
                tokens.push(Token::Str(s));
            }
            c if c.is_ascii_digit()
                || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                let start = i;
                i += 1;
                while chars
                    .get(i)
                    .is_some_and(|d| d.is_ascii_digit() || *d == '.')
                {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let n: f64 = text.parse().map_err(|_| {
                    Error::Invalid(format!("invalid number literal '{text}' in CQL2 filter"))
                })?;
                tokens.push(Token::Num(n));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while chars
                    .get(i)
                    .is_some_and(|d| d.is_ascii_alphanumeric() || *d == '_')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                tokens.push(keyword_or_ident(&word));
            }
            other => {
                return Err(Error::Invalid(format!(
                    "unexpected character '{other}' in CQL2 filter"
                )))
            }
        }
    }

    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<()> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "expected {expected:?} in CQL2 filter, found {:?}",
                self.peek()
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.advance() {
            Some(Token::Ident(name)) => Ok(name),
            other => Err(Error::Invalid(format!(
                "expected a property name in CQL2 filter, found {other:?}"
            ))),
        }
    }

    fn expect_op(&mut self) -> Result<CompareOp> {
        match self.advance() {
            Some(Token::Op(op)) => Ok(op),
            other => Err(Error::Invalid(format!(
                "expected a comparison operator in CQL2 filter, found {other:?}"
            ))),
        }
    }

    fn expect_string(&mut self) -> Result<String> {
        match self.advance() {
            Some(Token::Str(s)) => Ok(s),
            other => Err(Error::Invalid(format!(
                "expected a string literal in CQL2 filter, found {other:?}"
            ))),
        }
    }

    fn expect_number(&mut self) -> Result<f64> {
        match self.advance() {
            Some(Token::Num(n)) => Ok(n),
            other => Err(Error::Invalid(format!(
                "expected a number in CQL2 filter, found {other:?}"
            ))),
        }
    }

    fn expect_literal(&mut self) -> Result<Literal> {
        match self.advance() {
            Some(Token::Str(s)) => Ok(Literal::Text(s)),
            Some(Token::Num(n)) => Ok(Literal::Number(n)),
            Some(Token::True) => Ok(Literal::Bool(true)),
            Some(Token::False) => Ok(Literal::Bool(false)),
            other => Err(Error::Invalid(format!(
                "expected a scalar literal in CQL2 filter, found {other:?}"
            ))),
        }
    }

    /// `TIMESTAMP('...')` or a bare string literal.
    fn expect_temporal_literal(&mut self) -> Result<String> {
        if self.eat(&Token::Timestamp) {
            self.expect(&Token::LParen)?;
            let s = self.expect_string()?;
            self.expect(&Token::RParen)?;
            Ok(s)
        } else {
            self.expect_string()
        }
    }

    /// `BBOX(minx,miny,maxx,maxy)` or a WKT tagged-text geometry literal
    /// (`POINT(...)`, `POLYGON(...)`, ...) — see this module's top-level
    /// "Spatial literals" doc for which CQL2 conformance class each shape
    /// feeds.
    fn expect_geometry_literal(&mut self) -> Result<GeometryLiteral> {
        match self.peek() {
            Some(Token::Bbox) => self.expect_bbox_literal(),
            Some(token) if is_wkt_tag(token) => Ok(GeometryLiteral::Wkt(parse_wkt_geometry(self)?)),
            other => Err(Error::Invalid(format!(
                "expected a BBOX(...) or WKT geometry literal (POINT, LINESTRING, POLYGON, \
                 MULTIPOINT, MULTILINESTRING, MULTIPOLYGON, GEOMETRYCOLLECTION) in CQL2 \
                 filter, found {other:?}"
            ))),
        }
    }

    fn expect_bbox_literal(&mut self) -> Result<GeometryLiteral> {
        self.expect(&Token::Bbox)?;
        self.expect(&Token::LParen)?;
        let minx = self.expect_number()?;
        self.expect(&Token::Comma)?;
        let miny = self.expect_number()?;
        self.expect(&Token::Comma)?;
        let maxx = self.expect_number()?;
        self.expect(&Token::Comma)?;
        let maxy = self.expect_number()?;
        self.expect(&Token::RParen)?;
        Ok(GeometryLiteral::Bbox([minx, miny, maxx, maxy]))
    }

    /// A single `x y` coordinate pair. Rejects a third bare number before the
    /// closing delimiter — the common WKT convention for an implicit `Z`
    /// ordinate (`POINT(1 2 3)`) with no `Z`/`ZM` tag — with a precise
    /// message naming the cause, the same way an explicit `Z`/`M`/`ZM` tag
    /// is rejected in [`parse_wkt_geometry`].
    fn expect_wkt_coordinate(&mut self) -> Result<[f64; 2]> {
        let x = self.expect_number()?;
        let y = self.expect_number()?;
        if matches!(self.peek(), Some(Token::Num(_))) {
            return Err(Error::Invalid(
                "WKT geometry literal has more than 2 ordinates in a coordinate in CQL2 \
                 filter: only 2D (X, Y) coordinates are supported, Z/M ordinates are not"
                    .to_string(),
            ));
        }
        Ok([x, y])
    }

    /// `(coord, coord, ...)` — a WKT point list: a `LINESTRING`'s own
    /// coordinates, or one ring of a `POLYGON`.
    fn expect_wkt_point_list(&mut self) -> Result<Vec<[f64; 2]>> {
        self.expect(&Token::LParen)?;
        let mut coords = vec![self.expect_wkt_coordinate()?];
        while self.eat(&Token::Comma) {
            coords.push(self.expect_wkt_coordinate()?);
        }
        self.expect(&Token::RParen)?;
        Ok(coords)
    }

    /// `(point_list, point_list, ...)` — a `POLYGON`'s rings, or a
    /// `MULTILINESTRING`'s member linestrings (identical WKT shape).
    fn expect_wkt_ring_list(&mut self) -> Result<Vec<Vec<[f64; 2]>>> {
        self.expect(&Token::LParen)?;
        let mut rings = vec![self.expect_wkt_point_list()?];
        while self.eat(&Token::Comma) {
            rings.push(self.expect_wkt_point_list()?);
        }
        self.expect(&Token::RParen)?;
        Ok(rings)
    }

    /// A single `MULTIPOINT` member: either a bare coordinate
    /// (`MULTIPOINT(1 2, 3 4)`) or a parenthesized one
    /// (`MULTIPOINT((1 2), (3 4))`) — WKT allows both spellings and real
    /// producers use either, so both are accepted.
    fn expect_wkt_multipoint_member(&mut self) -> Result<[f64; 2]> {
        if self.eat(&Token::LParen) {
            let coord = self.expect_wkt_coordinate()?;
            self.expect(&Token::RParen)?;
            Ok(coord)
        } else {
            self.expect_wkt_coordinate()
        }
    }

    fn expect_wkt_multipoint_list(&mut self) -> Result<Vec<[f64; 2]>> {
        self.expect(&Token::LParen)?;
        let mut coords = vec![self.expect_wkt_multipoint_member()?];
        while self.eat(&Token::Comma) {
            coords.push(self.expect_wkt_multipoint_member()?);
        }
        self.expect(&Token::RParen)?;
        Ok(coords)
    }

    /// `(ring_list, ring_list, ...)` — a `MULTIPOLYGON`'s member polygons.
    fn expect_wkt_polygon_list(&mut self) -> Result<Vec<Vec<Vec<[f64; 2]>>>> {
        self.expect(&Token::LParen)?;
        let mut polygons = vec![self.expect_wkt_ring_list()?];
        while self.eat(&Token::Comma) {
            polygons.push(self.expect_wkt_ring_list()?);
        }
        self.expect(&Token::RParen)?;
        Ok(polygons)
    }

    /// `(geometry, geometry, ...)` — a `GEOMETRYCOLLECTION`'s members, each
    /// itself a full WKT geometry literal (recurses through
    /// [`parse_wkt_geometry`]).
    fn expect_wkt_geometry_collection_members(&mut self) -> Result<Vec<WktGeometry>> {
        self.expect(&Token::LParen)?;
        let mut geometries = vec![parse_wkt_geometry(self)?];
        while self.eat(&Token::Comma) {
            geometries.push(parse_wkt_geometry(self)?);
        }
        self.expect(&Token::RParen)?;
        Ok(geometries)
    }

    /// Rejects a WKT geometry with an `EMPTY` body (`POINT EMPTY`) with a
    /// precise message naming the cause, without consuming anything when the
    /// geometry isn't empty (so the caller can go on to `expect(&Token::
    /// LParen)`).
    fn reject_wkt_empty(&mut self) -> Result<()> {
        if let Some(Token::Ident(word)) = self.peek() {
            if word.eq_ignore_ascii_case("EMPTY") {
                return Err(Error::Invalid(
                    "empty WKT geometry literals are not supported in CQL2 filter".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Rejects a `Z`/`M`/`ZM` dimensionality tag following a WKT geometry
    /// tag (`POINT Z (...)`) with a precise message naming the cause,
    /// without consuming anything otherwise.
    fn reject_wkt_dimensionality(&mut self) -> Result<()> {
        if let Some(Token::Ident(word)) = self.peek() {
            let upper = word.to_ascii_uppercase();
            if upper == "Z" || upper == "M" || upper == "ZM" {
                return Err(Error::Invalid(format!(
                    "WKT geometry literal dimensionality tag '{upper}' is not supported in \
                     CQL2 filter: only 2D geometries are supported"
                )));
            }
        }
        Ok(())
    }
}

/// `true` when `token` starts a WKT tagged-text geometry literal — used by
/// [`Parser::expect_geometry_literal`] to pick between `BBOX(...)` and a WKT
/// literal.
fn is_wkt_tag(token: &Token) -> bool {
    matches!(
        token,
        Token::WktPoint
            | Token::WktLineString
            | Token::WktPolygon
            | Token::WktMultiPoint
            | Token::WktMultiLineString
            | Token::WktMultiPolygon
            | Token::WktGeometryCollection
    )
}

/// Parses one WKT tagged-text geometry literal, recursively for
/// `GEOMETRYCOLLECTION` members. `p.peek()` must already be one of the seven
/// tag tokens ([`is_wkt_tag`]) — every caller checks that first.
fn parse_wkt_geometry(p: &mut Parser) -> Result<WktGeometry> {
    let tag = p.advance().expect("caller already peeked a WKT tag token");
    p.reject_wkt_dimensionality()?;
    p.reject_wkt_empty()?;
    match tag {
        Token::WktPoint => {
            p.expect(&Token::LParen)?;
            let coord = p.expect_wkt_coordinate()?;
            p.expect(&Token::RParen)?;
            Ok(WktGeometry::Point(coord))
        }
        Token::WktLineString => Ok(WktGeometry::LineString(p.expect_wkt_point_list()?)),
        Token::WktPolygon => Ok(WktGeometry::Polygon(p.expect_wkt_ring_list()?)),
        Token::WktMultiPoint => Ok(WktGeometry::MultiPoint(p.expect_wkt_multipoint_list()?)),
        Token::WktMultiLineString => Ok(WktGeometry::MultiLineString(p.expect_wkt_ring_list()?)),
        Token::WktMultiPolygon => Ok(WktGeometry::MultiPolygon(p.expect_wkt_polygon_list()?)),
        Token::WktGeometryCollection => Ok(WktGeometry::GeometryCollection(
            p.expect_wkt_geometry_collection_members()?,
        )),
        other => unreachable!("is_wkt_tag guarantees a WKT tag token, got {other:?}"),
    }
}

fn parse_or(p: &mut Parser) -> Result<Filter> {
    let mut terms = vec![parse_and(p)?];
    while p.eat(&Token::Or) {
        terms.push(parse_and(p)?);
    }
    Ok(if terms.len() == 1 {
        terms.pop().expect("just checked len == 1")
    } else {
        Filter::Or(terms)
    })
}

fn parse_and(p: &mut Parser) -> Result<Filter> {
    let mut terms = vec![parse_not(p)?];
    while p.eat(&Token::And) {
        terms.push(parse_not(p)?);
    }
    Ok(if terms.len() == 1 {
        terms.pop().expect("just checked len == 1")
    } else {
        Filter::And(terms)
    })
}

fn parse_not(p: &mut Parser) -> Result<Filter> {
    if p.eat(&Token::Not) {
        Ok(Filter::Not(Box::new(parse_not(p)?)))
    } else {
        parse_primary(p)
    }
}

fn parse_primary(p: &mut Parser) -> Result<Filter> {
    if p.eat(&Token::LParen) {
        let inner = parse_or(p)?;
        p.expect(&Token::RParen)?;
        return Ok(inner);
    }
    parse_predicate(p)
}

fn parse_predicate(p: &mut Parser) -> Result<Filter> {
    match p.peek() {
        Some(Token::SIntersects) => parse_intersects(p),
        Some(Token::SWithin) => parse_spatial(p, SpatialOp::Within),
        Some(Token::SContains) => parse_spatial(p, SpatialOp::Contains),
        Some(Token::SDisjoint) => parse_spatial(p, SpatialOp::Disjoint),
        Some(Token::STouches) => parse_spatial(p, SpatialOp::Touches),
        Some(Token::SOverlaps) => parse_spatial(p, SpatialOp::Overlaps),
        Some(Token::SCrosses) => parse_spatial(p, SpatialOp::Crosses),
        Some(Token::SEquals) => parse_spatial(p, SpatialOp::Equals),
        Some(Token::TAfter) => parse_temporal_after_before(p, true),
        Some(Token::TBefore) => parse_temporal_after_before(p, false),
        Some(Token::TDuring) => parse_temporal_during(p),
        Some(Token::TContains) => parse_temporal(p, TemporalOp::Contains),
        Some(Token::TDisjoint) => parse_temporal(p, TemporalOp::Disjoint),
        Some(Token::TEquals) => parse_temporal(p, TemporalOp::Equals),
        Some(Token::TFinishedBy) => parse_temporal(p, TemporalOp::FinishedBy),
        Some(Token::TFinishes) => parse_temporal(p, TemporalOp::Finishes),
        Some(Token::TIntersects) => parse_temporal(p, TemporalOp::Intersects),
        Some(Token::TMeets) => parse_temporal(p, TemporalOp::Meets),
        Some(Token::TMetBy) => parse_temporal(p, TemporalOp::MetBy),
        Some(Token::TOverlappedBy) => parse_temporal(p, TemporalOp::OverlappedBy),
        Some(Token::TOverlaps) => parse_temporal(p, TemporalOp::Overlaps),
        Some(Token::TStartedBy) => parse_temporal(p, TemporalOp::StartedBy),
        Some(Token::TStarts) => parse_temporal(p, TemporalOp::Starts),
        Some(Token::Casei) => parse_casei_compare(p),
        Some(Token::Ident(_)) => parse_comparison_or_is_null(p),
        other => Err(Error::Invalid(format!(
            "expected a predicate in CQL2 filter, found {other:?}"
        ))),
    }
}

fn parse_comparison_or_is_null(p: &mut Parser) -> Result<Filter> {
    let property = p.expect_ident()?;
    if p.eat(&Token::Is) {
        let negated = p.eat(&Token::Not);
        p.expect(&Token::Null)?;
        return Ok(Filter::IsNull { property, negated });
    }
    // `NOT` precedes `LIKE`/`BETWEEN`/`IN` here (`property NOT LIKE ...`) —
    // a different position than `parse_not`'s own prefix `NOT <predicate>`
    // (logical negation of a whole subtree) and than `IS [NOT] NULL` above
    // (`NOT` between `IS` and `NULL`). Each of the three keeps its own
    // negated CQL2-text form rather than only ever being reachable by
    // wrapping in `Filter::Not` — see this module's CQL2-JSON parser for why
    // that side never needs the equivalent (it always uses the generic `not`
    // wrapper instead).
    let negated = p.eat(&Token::Not);
    if p.eat(&Token::Like) {
        let pattern = p.expect_string()?;
        return Ok(Filter::Like {
            property,
            pattern,
            negated,
        });
    }
    if p.eat(&Token::Between) {
        let low = p.expect_literal()?;
        p.expect(&Token::And)?;
        let high = p.expect_literal()?;
        return Ok(Filter::Between {
            property,
            low,
            high,
            negated,
        });
    }
    if p.eat(&Token::In) {
        p.expect(&Token::LParen)?;
        let mut values = vec![p.expect_literal()?];
        while p.eat(&Token::Comma) {
            values.push(p.expect_literal()?);
        }
        p.expect(&Token::RParen)?;
        return Ok(Filter::In {
            property,
            values,
            negated,
        });
    }
    if negated {
        return Err(Error::Invalid(format!(
            "expected LIKE, BETWEEN, or IN after 'NOT' in CQL2 filter, found {:?}",
            p.peek()
        )));
    }
    let op = p.expect_op()?;
    let value = p.expect_literal()?;
    Ok(Filter::Compare {
        property,
        op,
        value,
    })
}

/// `CASEI(property) = CASEI('literal')` / `<>` — see [`Filter::
/// CaseInsensitiveCompare`]'s own doc for exactly how narrow this shape is.
fn parse_casei_compare(p: &mut Parser) -> Result<Filter> {
    p.expect(&Token::Casei)?;
    p.expect(&Token::LParen)?;
    let property = p.expect_ident()?;
    p.expect(&Token::RParen)?;
    let op = match p.expect_op()? {
        CompareOp::Eq => CaseInsensitiveCompareOp::Eq,
        CompareOp::Ne => CaseInsensitiveCompareOp::Ne,
        _ => {
            return Err(Error::Invalid(
                "CASEI() comparison only supports '=' or '<>' in CQL2 filter".to_string(),
            ))
        }
    };
    p.expect(&Token::Casei)?;
    p.expect(&Token::LParen)?;
    let value = p.expect_string()?;
    p.expect(&Token::RParen)?;
    Ok(Filter::CaseInsensitiveCompare {
        property,
        op,
        value,
    })
}

fn parse_intersects(p: &mut Parser) -> Result<Filter> {
    p.expect(&Token::SIntersects)?;
    p.expect(&Token::LParen)?;
    let property = p.expect_ident()?;
    p.expect(&Token::Comma)?;
    let geometry = p.expect_geometry_literal()?;
    p.expect(&Token::RParen)?;
    Ok(Filter::Intersects { property, geometry })
}

/// Shared by every `S_WITHIN`/`S_CONTAINS`/`S_DISJOINT`/`S_TOUCHES`/
/// `S_OVERLAPS`/`S_CROSSES`/`S_EQUALS` branch in [`parse_predicate`] — same
/// `(property, geometry-literal)` shape [`parse_intersects`] hand-rolls for
/// `S_INTERSECTS`, generalized over which token/operator it's for.
fn parse_spatial(p: &mut Parser, op: SpatialOp) -> Result<Filter> {
    p.advance();
    p.expect(&Token::LParen)?;
    let property = p.expect_ident()?;
    p.expect(&Token::Comma)?;
    let geometry = p.expect_geometry_literal()?;
    p.expect(&Token::RParen)?;
    Ok(Filter::Spatial {
        property,
        op,
        geometry,
    })
}

fn parse_temporal_after_before(p: &mut Parser, is_after: bool) -> Result<Filter> {
    p.advance();
    p.expect(&Token::LParen)?;
    let property = p.expect_ident()?;
    p.expect(&Token::Comma)?;
    let instant = p.expect_temporal_literal()?;
    p.expect(&Token::RParen)?;
    Ok(if is_after {
        Filter::After { property, instant }
    } else {
        Filter::Before { property, instant }
    })
}

fn parse_temporal_during(p: &mut Parser) -> Result<Filter> {
    p.advance();
    p.expect(&Token::LParen)?;
    let property = p.expect_ident()?;
    p.expect(&Token::Comma)?;
    let (start, end) = if p.eat(&Token::Interval) {
        p.expect(&Token::LParen)?;
        let start = p.expect_string()?;
        p.expect(&Token::Comma)?;
        let end = p.expect_string()?;
        p.expect(&Token::RParen)?;
        (start, end)
    } else {
        let start = p.expect_temporal_literal()?;
        p.expect(&Token::Comma)?;
        let end = p.expect_temporal_literal()?;
        (start, end)
    };
    p.expect(&Token::RParen)?;
    Ok(Filter::During {
        property,
        start,
        end,
    })
}

/// `T_CONTAINS`/`T_DISJOINT`/`T_EQUALS`/`T_FINISHEDBY`/`T_FINISHES`/
/// `T_INTERSECTS`/`T_MEETS`/`T_METBY`/`T_OVERLAPPEDBY`/`T_OVERLAPS`/
/// `T_STARTEDBY`/`T_STARTS(property, <instant-or-interval>)` — shared by
/// every one of the twelve new temporal-predicate branches in
/// [`parse_predicate`], the temporal counterpart of [`parse_spatial`].
fn parse_temporal(p: &mut Parser, op: TemporalOp) -> Result<Filter> {
    p.advance();
    p.expect(&Token::LParen)?;
    let property = p.expect_ident()?;
    p.expect(&Token::Comma)?;
    let value = expect_temporal_value(p)?;
    p.expect(&Token::RParen)?;
    Ok(Filter::Temporal {
        property,
        op,
        value,
    })
}

/// A [`TemporalValue`]: `INTERVAL('start', 'end')`, a bare `'start', 'end'`
/// pair, or a single `TIMESTAMP('...')`/bare string instant — the same three
/// shapes [`parse_temporal_during`] already accepts for `T_DURING`, but
/// resolved to whichever [`TemporalValue`] variant the argument count
/// implies rather than always requiring two.
fn expect_temporal_value(p: &mut Parser) -> Result<TemporalValue> {
    if p.eat(&Token::Interval) {
        p.expect(&Token::LParen)?;
        let start = p.expect_string()?;
        p.expect(&Token::Comma)?;
        let end = p.expect_string()?;
        p.expect(&Token::RParen)?;
        return Ok(TemporalValue::Interval(start, end));
    }
    let first = p.expect_temporal_literal()?;
    if p.eat(&Token::Comma) {
        let second = p.expect_temporal_literal()?;
        Ok(TemporalValue::Interval(first, second))
    } else {
        Ok(TemporalValue::Instant(first))
    }
}

/// Parses a CQL2-text filter expression into a [`Filter`]. Every syntax
/// error is `Error::Invalid` (a 400 at the protocol layer), never a panic.
pub fn parse_text(input: &str) -> Result<Filter> {
    let tokens = lex(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let filter = parse_or(&mut parser)?;
    if parser.pos != parser.tokens.len() {
        return Err(Error::Invalid(format!(
            "unexpected trailing input in CQL2 filter starting at token {:?}",
            parser.tokens[parser.pos]
        )));
    }
    Ok(filter)
}

// ---------------------------------------------------------------------------
// CQL2-JSON parsing
// ---------------------------------------------------------------------------

/// Parses a CQL2-JSON filter document (`{"op": ..., "args": [...]}`) into a
/// [`Filter`]. Every shape error is `Error::Invalid` (a 400 at the protocol
/// layer), never a panic.
pub fn parse_json(input: &str) -> Result<Filter> {
    let value: serde_json::Value = serde_json::from_str(input)
        .map_err(|e| Error::Invalid(format!("invalid CQL2-JSON filter: {e}")))?;
    filter_from_json(&value)
}

fn filter_from_json(value: &serde_json::Value) -> Result<Filter> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::Invalid("CQL2-JSON filter node must be a JSON object".to_string()))?;
    let op = obj
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Invalid("CQL2-JSON filter object is missing 'op'".to_string()))?;
    let args = obj
        .get("args")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::Invalid(format!("CQL2-JSON '{op}' is missing an 'args' array")))?;

    match op {
        "and" => Ok(Filter::And(
            args.iter().map(filter_from_json).collect::<Result<_>>()?,
        )),
        "or" => Ok(Filter::Or(
            args.iter().map(filter_from_json).collect::<Result<_>>()?,
        )),
        "not" => {
            let inner = args.first().ok_or_else(|| {
                Error::Invalid("CQL2-JSON 'not' requires exactly one argument".to_string())
            })?;
            Ok(Filter::Not(Box::new(filter_from_json(inner)?)))
        }
        "isNull" => {
            let property = json_property(args.first())?;
            Ok(Filter::IsNull {
                property,
                negated: false,
            })
        }
        "=" | "<>"
            if args.len() == 2 && json_is_casei_call(&args[0]) && json_is_casei_call(&args[1]) =>
        {
            let casei_op = match op {
                "=" => CaseInsensitiveCompareOp::Eq,
                _ => CaseInsensitiveCompareOp::Ne,
            };
            json_casei_compare(&args[0], &args[1], casei_op)
        }
        "=" | "<>" | "<" | ">" | "<=" | ">=" => {
            let compare_op = compare_op_from_json(op)?;
            let property = json_property(args.first())?;
            let value = json_literal(args.get(1).ok_or_else(|| {
                Error::Invalid(format!("CQL2-JSON '{op}' requires two arguments"))
            })?)?;
            Ok(Filter::Compare {
                property,
                op: compare_op,
                value,
            })
        }
        "like" => {
            let property = json_property(args.first())?;
            let pattern = args
                .get(1)
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    Error::Invalid(
                        "CQL2-JSON 'like' requires a string pattern as its second argument"
                            .to_string(),
                    )
                })?
                .to_string();
            Ok(Filter::Like {
                property,
                pattern,
                negated: false,
            })
        }
        "between" => {
            let property = json_property(args.first())?;
            let low = json_literal(args.get(1).ok_or_else(|| {
                Error::Invalid("CQL2-JSON 'between' requires a low bound".to_string())
            })?)?;
            let high = json_literal(args.get(2).ok_or_else(|| {
                Error::Invalid("CQL2-JSON 'between' requires a high bound".to_string())
            })?)?;
            Ok(Filter::Between {
                property,
                low,
                high,
                negated: false,
            })
        }
        "in" => {
            let property = json_property(args.first())?;
            let values = args
                .get(1)
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    Error::Invalid(
                        "CQL2-JSON 'in' requires an array of literals as its second argument"
                            .to_string(),
                    )
                })?
                .iter()
                .map(json_literal)
                .collect::<Result<_>>()?;
            Ok(Filter::In {
                property,
                values,
                negated: false,
            })
        }
        "s_intersects" => {
            let property = json_property(args.first())?;
            let geometry = json_geometry(args.get(1).ok_or_else(|| {
                Error::Invalid("CQL2-JSON 's_intersects' requires two arguments".to_string())
            })?)?;
            Ok(Filter::Intersects { property, geometry })
        }
        "s_within" | "s_contains" | "s_disjoint" | "s_touches" | "s_overlaps" | "s_crosses"
        | "s_equals" => {
            let spatial_op = spatial_op_from_json(op);
            let property = json_property(args.first())?;
            let geometry = json_geometry(args.get(1).ok_or_else(|| {
                Error::Invalid(format!("CQL2-JSON '{op}' requires two arguments"))
            })?)?;
            Ok(Filter::Spatial {
                property,
                op: spatial_op,
                geometry,
            })
        }
        "t_after" | "t_before" => {
            let property = json_property(args.first())?;
            let instant = json_temporal_instant(args.get(1).ok_or_else(|| {
                Error::Invalid(format!("CQL2-JSON '{op}' requires two arguments"))
            })?)?;
            Ok(if op == "t_after" {
                Filter::After { property, instant }
            } else {
                Filter::Before { property, instant }
            })
        }
        "t_during" => {
            let property = json_property(args.first())?;
            let (start, end) = json_temporal_interval(args.get(1).ok_or_else(|| {
                Error::Invalid("CQL2-JSON 't_during' requires two arguments".to_string())
            })?)?;
            Ok(Filter::During {
                property,
                start,
                end,
            })
        }
        "t_contains" | "t_disjoint" | "t_equals" | "t_finishedby" | "t_finishes"
        | "t_intersects" | "t_meets" | "t_metby" | "t_overlappedby" | "t_overlaps"
        | "t_startedby" | "t_starts" => {
            let temporal_op = temporal_op_from_json(op);
            let property = json_property(args.first())?;
            let value = json_temporal_value(args.get(1).ok_or_else(|| {
                Error::Invalid(format!("CQL2-JSON '{op}' requires two arguments"))
            })?)?;
            Ok(Filter::Temporal {
                property,
                op: temporal_op,
                value,
            })
        }
        other => Err(Error::Invalid(format!(
            "unsupported CQL2 operator '{other}'"
        ))),
    }
}

fn json_property(value: Option<&serde_json::Value>) -> Result<String> {
    let value = value.ok_or_else(|| {
        Error::Invalid("expected a property reference in CQL2-JSON filter".to_string())
    })?;
    value
        .as_object()
        .and_then(|o| o.get("property"))
        .and_then(|p| p.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            Error::Invalid(
                "expected a {\"property\": ...} reference in CQL2-JSON filter".to_string(),
            )
        })
}

fn json_literal(value: &serde_json::Value) -> Result<Literal> {
    match value {
        serde_json::Value::String(s) => Ok(Literal::Text(s.clone())),
        serde_json::Value::Number(n) => n.as_f64().map(Literal::Number).ok_or_else(|| {
            Error::Invalid(format!("invalid numeric literal '{n}' in CQL2-JSON filter"))
        }),
        serde_json::Value::Bool(b) => Ok(Literal::Bool(*b)),
        other => Err(Error::Invalid(format!(
            "unsupported literal shape in CQL2-JSON filter: {other}"
        ))),
    }
}

fn compare_op_from_json(op: &str) -> Result<CompareOp> {
    match op {
        "=" => Ok(CompareOp::Eq),
        "<>" => Ok(CompareOp::Ne),
        "<" => Ok(CompareOp::Lt),
        ">" => Ok(CompareOp::Gt),
        "<=" => Ok(CompareOp::Le),
        ">=" => Ok(CompareOp::Ge),
        other => Err(Error::Invalid(format!(
            "unsupported comparison operator '{other}' in CQL2-JSON filter"
        ))),
    }
}

/// Maps a CQL2-JSON `"op"` string to its [`SpatialOp`] — the JSON-encoding
/// counterpart of the text parser's `keyword_or_ident`, for the seven
/// operators the "Spatial functions" conformance class adds beyond
/// `S_INTERSECTS`. Only ever called with one of the six strings this
/// module's own `"s_within" | "s_contains" | ...` match arm already matched.
fn spatial_op_from_json(op: &str) -> SpatialOp {
    match op {
        "s_within" => SpatialOp::Within,
        "s_contains" => SpatialOp::Contains,
        "s_disjoint" => SpatialOp::Disjoint,
        "s_touches" => SpatialOp::Touches,
        "s_overlaps" => SpatialOp::Overlaps,
        "s_crosses" => SpatialOp::Crosses,
        _ => SpatialOp::Equals,
    }
}

/// Maps a CQL2-JSON `"op"` string to its [`TemporalOp`] — the JSON-encoding
/// counterpart of the text parser's `keyword_or_ident`, for the twelve
/// operators the "Temporal functions" conformance class adds beyond
/// `T_AFTER`/`T_BEFORE`/`T_DURING`. Only ever called with one of the twelve
/// strings this module's own `"t_contains" | "t_disjoint" | ...` match arm
/// already matched.
fn temporal_op_from_json(op: &str) -> TemporalOp {
    match op {
        "t_contains" => TemporalOp::Contains,
        "t_disjoint" => TemporalOp::Disjoint,
        "t_equals" => TemporalOp::Equals,
        "t_finishedby" => TemporalOp::FinishedBy,
        "t_finishes" => TemporalOp::Finishes,
        "t_intersects" => TemporalOp::Intersects,
        "t_meets" => TemporalOp::Meets,
        "t_metby" => TemporalOp::MetBy,
        "t_overlappedby" => TemporalOp::OverlappedBy,
        "t_overlaps" => TemporalOp::Overlaps,
        "t_startedby" => TemporalOp::StartedBy,
        _ => TemporalOp::Starts,
    }
}

/// `true` when `value` is a CQL2-JSON function call to `casei` — functions
/// are encoded as ordinary op nodes in CQL2-JSON (`{"op": "casei", "args":
/// [...]}`), not a nested `"function"` wrapper; verified against the
/// standard's own case-insensitive-comparison examples.
fn json_is_casei_call(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .and_then(|o| o.get("op"))
        .and_then(|op| op.as_str())
        == Some("casei")
}

/// A `CASEI(...)` call's single operand — either the property reference or
/// the string literal it wraps, the only two shapes [`Filter::
/// CaseInsensitiveCompare`] supports on either side.
enum CaseiOperand {
    Property(String),
    Literal(String),
}

/// Extracts `value`'s `CASEI(...)` operand — `value` has already been
/// confirmed a `casei` call by [`json_is_casei_call`] before this is called.
fn json_casei_operand(value: &serde_json::Value) -> Result<CaseiOperand> {
    let args = value
        .as_object()
        .and_then(|o| o.get("args"))
        .and_then(|a| a.as_array())
        .ok_or_else(|| Error::Invalid("CQL2-JSON 'casei' requires an 'args' array".to_string()))?;
    let arg = args.first().ok_or_else(|| {
        Error::Invalid("CQL2-JSON 'casei' requires exactly one argument".to_string())
    })?;
    if let Ok(property) = json_property(Some(arg)) {
        return Ok(CaseiOperand::Property(property));
    }
    match arg {
        serde_json::Value::String(s) => Ok(CaseiOperand::Literal(s.clone())),
        other => Err(Error::Invalid(format!(
            "unsupported CASEI() operand in CQL2-JSON filter: {other}"
        ))),
    }
}

/// Builds a [`Filter::CaseInsensitiveCompare`] from two already-confirmed
/// `CASEI(...)` call nodes (`json_is_casei_call`) — exactly one side must be
/// a property reference and the other a string literal, in either order;
/// two properties or two literals is a shape this lane doesn't support (see
/// [`Filter::CaseInsensitiveCompare`]'s own doc).
fn json_casei_compare(
    left: &serde_json::Value,
    right: &serde_json::Value,
    op: CaseInsensitiveCompareOp,
) -> Result<Filter> {
    let left = json_casei_operand(left)?;
    let right = json_casei_operand(right)?;
    match (left, right) {
        (CaseiOperand::Property(property), CaseiOperand::Literal(value))
        | (CaseiOperand::Literal(value), CaseiOperand::Property(property)) => {
            Ok(Filter::CaseInsensitiveCompare {
                property,
                op,
                value,
            })
        }
        _ => Err(Error::Invalid(
            "CQL2-JSON case-insensitive comparison requires one CASEI(property) and one \
             CASEI('literal') operand"
                .to_string(),
        )),
    }
}

fn json_geometry(value: &serde_json::Value) -> Result<GeometryLiteral> {
    if let Some(obj) = value.as_object() {
        if let Some(bbox) = obj.get("bbox").and_then(|b| b.as_array()) {
            let nums: Vec<f64> = bbox.iter().filter_map(|v| v.as_f64()).collect();
            return match <[f64; 4]>::try_from(nums) {
                Ok(arr) => Ok(GeometryLiteral::Bbox(arr)),
                Err(_) => Err(Error::Invalid(
                    "'bbox' literal in CQL2-JSON filter must have exactly 4 numbers".to_string(),
                )),
            };
        }
        if obj.contains_key("type") {
            return Ok(GeometryLiteral::GeoJson(value.clone()));
        }
    }
    Err(Error::Invalid(
        "expected a {\"bbox\": [...]} or GeoJSON geometry literal in CQL2-JSON filter".to_string(),
    ))
}

fn json_temporal_instant(value: &serde_json::Value) -> Result<String> {
    if let Some(s) = value.as_str() {
        return Ok(s.to_string());
    }
    if let Some(obj) = value.as_object() {
        if let Some(ts) = obj.get("timestamp").and_then(|v| v.as_str()) {
            return Ok(ts.to_string());
        }
        if let Some(d) = obj.get("date").and_then(|v| v.as_str()) {
            return Ok(d.to_string());
        }
    }
    Err(Error::Invalid(
        "expected a temporal instant ({\"timestamp\": ...} or a plain string) in CQL2-JSON filter"
            .to_string(),
    ))
}

fn json_temporal_interval(value: &serde_json::Value) -> Result<(String, String)> {
    let interval = value
        .as_object()
        .and_then(|o| o.get("interval"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            Error::Invalid(
                "expected a {\"interval\": [start, end]} literal in CQL2-JSON filter".to_string(),
            )
        })?;
    if interval.len() != 2 {
        return Err(Error::Invalid(
            "'interval' literal in CQL2-JSON filter must have exactly 2 entries".to_string(),
        ));
    }
    let start = interval[0]
        .as_str()
        .ok_or_else(|| Error::Invalid("interval start must be a string".to_string()))?
        .to_string();
    let end = interval[1]
        .as_str()
        .ok_or_else(|| Error::Invalid("interval end must be a string".to_string()))?
        .to_string();
    Ok((start, end))
}

/// A [`TemporalValue`] for one of the twelve new temporal predicates: an
/// `{"interval": [start, end]}` object resolves to
/// [`TemporalValue::Interval`], anything [`json_temporal_instant`] accepts
/// resolves to [`TemporalValue::Instant`].
fn json_temporal_value(value: &serde_json::Value) -> Result<TemporalValue> {
    if value
        .as_object()
        .is_some_and(|o| o.contains_key("interval"))
    {
        let (start, end) = json_temporal_interval(value)?;
        return Ok(TemporalValue::Interval(start, end));
    }
    Ok(TemporalValue::Instant(json_temporal_instant(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::AttributeColumn;
    use crate::config::{PropertyDecl, PropertyType};

    fn descriptor() -> CollectionDescriptor {
        CollectionDescriptor {
            table: "demo".to_string(),
            geometry: Some("geom".to_string()),
            pk: Some("id".to_string()),
            datetime: Some("observed_at".to_string()),
            srid: None,
            extent: None,
            row_estimate: None,
            attributes: Some(vec![
                AttributeColumn {
                    name: "name".to_string(),
                    sql_type: "text".to_string(),
                },
                AttributeColumn {
                    name: "population".to_string(),
                    sql_type: "integer".to_string(),
                },
            ]),
            geometry_type: None,
            projection: None,
        }
    }

    // -- AST --------------------------------------------------------------

    #[test]
    fn properties_deduplicates_in_first_seen_order() {
        let filter = Filter::And(vec![
            Filter::Compare {
                property: "name".to_string(),
                op: CompareOp::Eq,
                value: Literal::Text("a".to_string()),
            },
            Filter::Compare {
                property: "population".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(0.0),
            },
            Filter::IsNull {
                property: "name".to_string(),
                negated: true,
            },
        ]);
        assert_eq!(filter.properties(), vec!["name", "population"]);
    }

    /// `#247`: the whole point of this predicate is the *negative* answers.
    /// A filter that names the geometry column without carrying coordinates
    /// (`geom IS NOT NULL`) is not a spatial filter, and a spatial predicate
    /// buried under `AND`/`NOT` still is one — the two mistakes a caller
    /// checking "does this filter mention geometry" or "is this a top-level
    /// S_INTERSECTS" would make.
    #[test]
    fn has_spatial_literal_finds_a_nested_predicate_and_ignores_a_geometry_column_mention() {
        for (source, expected) in [
            ("population > 10", false),
            ("geom IS NOT NULL", false),
            ("S_INTERSECTS(geom, BBOX(1,2,3,4))", true),
            ("S_WITHIN(geom, POINT(1 2))", true),
            ("name = 'a' AND S_INTERSECTS(geom, BBOX(1,2,3,4))", true),
            ("NOT S_CROSSES(geom, BBOX(1,2,3,4))", true),
            ("name = 'a' AND geom IS NULL", false),
        ] {
            let filter = parse_text(source).expect("fixture parses");
            assert_eq!(
                filter.has_spatial_literal(),
                expected,
                "has_spatial_literal() was wrong for: {source}"
            );
        }
    }

    // -- text parser: comparisons + logic ---------------------------------

    #[test]
    fn parses_a_simple_equality() {
        let filter = parse_text("name = 'a'").unwrap();
        assert_eq!(
            filter,
            Filter::Compare {
                property: "name".to_string(),
                op: CompareOp::Eq,
                value: Literal::Text("a".to_string()),
            }
        );
    }

    #[test]
    fn parses_every_comparison_operator() {
        let cases = [
            ("population <> 5", CompareOp::Ne),
            ("population < 5", CompareOp::Lt),
            ("population > 5", CompareOp::Gt),
            ("population <= 5", CompareOp::Le),
            ("population >= 5", CompareOp::Ge),
        ];
        for (text, expected_op) in cases {
            match parse_text(text).unwrap() {
                Filter::Compare { op, .. } => assert_eq!(op, expected_op, "for input '{text}'"),
                other => panic!("expected Compare for '{text}', got {other:?}"),
            }
        }
    }

    #[test]
    fn parses_a_negative_number_literal() {
        let filter = parse_text("population > -5").unwrap();
        assert_eq!(
            filter,
            Filter::Compare {
                property: "population".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(-5.0),
            }
        );
    }

    #[test]
    fn parses_boolean_literals() {
        assert_eq!(
            parse_text("active = true").unwrap(),
            Filter::Compare {
                property: "active".to_string(),
                op: CompareOp::Eq,
                value: Literal::Bool(true),
            }
        );
        assert_eq!(
            parse_text("active = false").unwrap(),
            Filter::Compare {
                property: "active".to_string(),
                op: CompareOp::Eq,
                value: Literal::Bool(false),
            }
        );
    }

    #[test]
    fn parses_is_null_and_is_not_null() {
        assert_eq!(
            parse_text("name IS NULL").unwrap(),
            Filter::IsNull {
                property: "name".to_string(),
                negated: false,
            }
        );
        assert_eq!(
            parse_text("name IS NOT NULL").unwrap(),
            Filter::IsNull {
                property: "name".to_string(),
                negated: true,
            }
        );
    }

    #[test]
    fn keywords_are_case_insensitive() {
        assert_eq!(
            parse_text("name is not null").unwrap(),
            Filter::IsNull {
                property: "name".to_string(),
                negated: true,
            }
        );
    }

    #[test]
    fn string_literal_supports_doubled_quote_escape() {
        let filter = parse_text("name = 'O''Brien'").unwrap();
        assert_eq!(
            filter,
            Filter::Compare {
                property: "name".to_string(),
                op: CompareOp::Eq,
                value: Literal::Text("O'Brien".to_string()),
            }
        );
    }

    #[test]
    fn and_and_or_flatten_left_associatively() {
        let filter = parse_text("a = 1 AND b = 2 AND c = 3").unwrap();
        match filter {
            Filter::And(terms) => assert_eq!(terms.len(), 3),
            other => panic!("expected And, got {other:?}"),
        }
        let filter = parse_text("a = 1 OR b = 2 OR c = 3").unwrap();
        match filter {
            Filter::Or(terms) => assert_eq!(terms.len(), 3),
            other => panic!("expected Or, got {other:?}"),
        }
    }

    #[test]
    fn and_binds_tighter_than_or() {
        // a OR (b AND c): the AND must nest inside the Or's second term, not
        // the other way around.
        let filter = parse_text("a = 1 OR b = 2 AND c = 3").unwrap();
        match filter {
            Filter::Or(terms) => {
                assert_eq!(terms.len(), 2);
                assert!(matches!(terms[0], Filter::Compare { .. }));
                assert!(matches!(terms[1], Filter::And(_)));
            }
            other => panic!("expected Or, got {other:?}"),
        }
    }

    #[test]
    fn parentheses_override_precedence() {
        // (a OR b) AND c: parens force the Or to nest inside the And.
        let filter = parse_text("(a = 1 OR b = 2) AND c = 3").unwrap();
        match filter {
            Filter::And(terms) => {
                assert_eq!(terms.len(), 2);
                assert!(matches!(terms[0], Filter::Or(_)));
            }
            other => panic!("expected And, got {other:?}"),
        }
    }

    #[test]
    fn not_applies_to_the_nearest_predicate() {
        let filter = parse_text("NOT a = 1 AND b = 2").unwrap();
        match filter {
            Filter::And(terms) => {
                assert!(matches!(terms[0], Filter::Not(_)));
                assert!(matches!(terms[1], Filter::Compare { .. }));
            }
            other => panic!("expected And, got {other:?}"),
        }
    }

    // -- text parser: spatial + temporal ------------------------------------

    #[test]
    fn parses_s_intersects_with_a_bbox_literal() {
        let filter = parse_text("S_INTERSECTS(geom, BBOX(1, 2, 3, 4))").unwrap();
        assert_eq!(
            filter,
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }
        );
    }

    #[test]
    fn parses_t_after_with_a_timestamp_wrapper_and_a_bare_string() {
        assert_eq!(
            parse_text("T_AFTER(observed_at, TIMESTAMP('2020-01-01T00:00:00Z'))").unwrap(),
            Filter::After {
                property: "observed_at".to_string(),
                instant: "2020-01-01T00:00:00Z".to_string(),
            }
        );
        assert_eq!(
            parse_text("T_AFTER(observed_at, '2020-01-01T00:00:00Z')").unwrap(),
            Filter::After {
                property: "observed_at".to_string(),
                instant: "2020-01-01T00:00:00Z".to_string(),
            }
        );
    }

    #[test]
    fn parses_t_before() {
        assert_eq!(
            parse_text("T_BEFORE(observed_at, '2020-01-01T00:00:00Z')").unwrap(),
            Filter::Before {
                property: "observed_at".to_string(),
                instant: "2020-01-01T00:00:00Z".to_string(),
            }
        );
    }

    #[test]
    fn parses_t_during_with_interval_wrapper_and_bare_pair() {
        assert_eq!(
            parse_text("T_DURING(observed_at, INTERVAL('2020-01-01', '2021-01-01'))").unwrap(),
            Filter::During {
                property: "observed_at".to_string(),
                start: "2020-01-01".to_string(),
                end: "2021-01-01".to_string(),
            }
        );
        assert_eq!(
            parse_text("T_DURING(observed_at, '2020-01-01', '2021-01-01')").unwrap(),
            Filter::During {
                property: "observed_at".to_string(),
                start: "2020-01-01".to_string(),
                end: "2021-01-01".to_string(),
            }
        );
    }

    #[test]
    fn combines_spatial_and_attribute_predicates() {
        let filter =
            parse_text("S_INTERSECTS(geom, BBOX(1, 2, 3, 4)) AND population > 100").unwrap();
        match filter {
            Filter::And(terms) => assert_eq!(terms.len(), 2),
            other => panic!("expected And, got {other:?}"),
        }
    }

    // -- text parser: temporal functions beyond T_AFTER/T_BEFORE/T_DURING ----

    #[test]
    fn parses_every_new_temporal_predicate_with_an_instant_literal() {
        let cases = [
            (
                "T_CONTAINS(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Contains,
            ),
            (
                "T_DISJOINT(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Disjoint,
            ),
            (
                "T_EQUALS(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Equals,
            ),
            (
                "T_FINISHEDBY(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::FinishedBy,
            ),
            (
                "T_FINISHES(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Finishes,
            ),
            (
                "T_INTERSECTS(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Intersects,
            ),
            (
                "T_MEETS(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Meets,
            ),
            (
                "T_METBY(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::MetBy,
            ),
            (
                "T_OVERLAPPEDBY(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::OverlappedBy,
            ),
            (
                "T_OVERLAPS(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Overlaps,
            ),
            (
                "T_STARTEDBY(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::StartedBy,
            ),
            (
                "T_STARTS(observed_at, '2020-06-01T00:00:00Z')",
                TemporalOp::Starts,
            ),
        ];
        for (text, expected_op) in cases {
            match parse_text(text).unwrap() {
                Filter::Temporal {
                    property,
                    op,
                    value,
                } => {
                    assert_eq!(property, "observed_at", "for input '{text}'");
                    assert_eq!(op, expected_op, "for input '{text}'");
                    assert_eq!(
                        value,
                        TemporalValue::Instant("2020-06-01T00:00:00Z".to_string())
                    );
                }
                other => panic!("expected Temporal for '{text}', got {other:?}"),
            }
        }
    }

    #[test]
    fn parses_a_new_temporal_predicate_with_an_interval_wrapper_and_a_bare_pair() {
        assert_eq!(
            parse_text(
                "T_OVERLAPS(observed_at, INTERVAL('2020-01-01T00:00:00Z', '2020-12-31T00:00:00Z'))"
            )
            .unwrap(),
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Overlaps,
                value: TemporalValue::Interval(
                    "2020-01-01T00:00:00Z".to_string(),
                    "2020-12-31T00:00:00Z".to_string()
                ),
            }
        );
        assert_eq!(
            parse_text("T_OVERLAPS(observed_at, '2020-01-01T00:00:00Z', '2020-12-31T00:00:00Z')")
                .unwrap(),
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Overlaps,
                value: TemporalValue::Interval(
                    "2020-01-01T00:00:00Z".to_string(),
                    "2020-12-31T00:00:00Z".to_string()
                ),
            }
        );
    }

    // -- text parser: advanced comparison operators --------------------------

    #[test]
    fn parses_like_and_not_like() {
        assert_eq!(
            parse_text("name LIKE 'Sm%'").unwrap(),
            Filter::Like {
                property: "name".to_string(),
                pattern: "Sm%".to_string(),
                negated: false,
            }
        );
        assert_eq!(
            parse_text("name NOT LIKE 'Sm%'").unwrap(),
            Filter::Like {
                property: "name".to_string(),
                pattern: "Sm%".to_string(),
                negated: true,
            }
        );
    }

    #[test]
    fn parses_between_and_not_between() {
        assert_eq!(
            parse_text("population BETWEEN 10 AND 20").unwrap(),
            Filter::Between {
                property: "population".to_string(),
                low: Literal::Number(10.0),
                high: Literal::Number(20.0),
                negated: false,
            }
        );
        assert_eq!(
            parse_text("population NOT BETWEEN 10 AND 20").unwrap(),
            Filter::Between {
                property: "population".to_string(),
                low: Literal::Number(10.0),
                high: Literal::Number(20.0),
                negated: true,
            }
        );
    }

    #[test]
    fn parses_in_and_not_in() {
        assert_eq!(
            parse_text("name IN ('a', 'b', 'c')").unwrap(),
            Filter::In {
                property: "name".to_string(),
                values: vec![
                    Literal::Text("a".to_string()),
                    Literal::Text("b".to_string()),
                    Literal::Text("c".to_string()),
                ],
                negated: false,
            }
        );
        assert_eq!(
            parse_text("name NOT IN ('a', 'b')").unwrap(),
            Filter::In {
                property: "name".to_string(),
                values: vec![
                    Literal::Text("a".to_string()),
                    Literal::Text("b".to_string())
                ],
                negated: true,
            }
        );
    }

    #[test]
    fn parses_a_single_element_in_list() {
        assert_eq!(
            parse_text("population IN (20)").unwrap(),
            Filter::In {
                property: "population".to_string(),
                values: vec![Literal::Number(20.0)],
                negated: false,
            }
        );
    }

    #[test]
    fn not_before_an_unrecognized_keyword_is_a_syntax_error() {
        assert!(matches!(
            parse_text("name NOT population > 1"),
            Err(Error::Invalid(_))
        ));
    }

    // -- text parser: CASEI() case-insensitive comparison --------------------

    #[test]
    fn parses_casei_equality_and_inequality() {
        assert_eq!(
            parse_text("CASEI(name) = CASEI('john')").unwrap(),
            Filter::CaseInsensitiveCompare {
                property: "name".to_string(),
                op: CaseInsensitiveCompareOp::Eq,
                value: "john".to_string(),
            }
        );
        assert_eq!(
            parse_text("CASEI(name) <> CASEI('john')").unwrap(),
            Filter::CaseInsensitiveCompare {
                property: "name".to_string(),
                op: CaseInsensitiveCompareOp::Ne,
                value: "john".to_string(),
            }
        );
    }

    #[test]
    fn casei_rejects_an_ordering_operator() {
        assert!(matches!(
            parse_text("CASEI(name) < CASEI('john')"),
            Err(Error::Invalid(_))
        ));
    }

    // -- text parser: spatial functions beyond S_INTERSECTS -------------------

    #[test]
    fn parses_every_new_spatial_predicate() {
        let cases = [
            ("S_WITHIN(geom, BBOX(1, 2, 3, 4))", SpatialOp::Within),
            ("S_CONTAINS(geom, BBOX(1, 2, 3, 4))", SpatialOp::Contains),
            ("S_DISJOINT(geom, BBOX(1, 2, 3, 4))", SpatialOp::Disjoint),
            ("S_TOUCHES(geom, BBOX(1, 2, 3, 4))", SpatialOp::Touches),
            ("S_OVERLAPS(geom, BBOX(1, 2, 3, 4))", SpatialOp::Overlaps),
            ("S_CROSSES(geom, BBOX(1, 2, 3, 4))", SpatialOp::Crosses),
            ("S_EQUALS(geom, BBOX(1, 2, 3, 4))", SpatialOp::Equals),
        ];
        for (text, expected_op) in cases {
            match parse_text(text).unwrap() {
                Filter::Spatial {
                    property,
                    op,
                    geometry,
                } => {
                    assert_eq!(property, "geom", "for input '{text}'");
                    assert_eq!(op, expected_op, "for input '{text}'");
                    assert_eq!(geometry, GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]));
                }
                other => panic!("expected Spatial for '{text}', got {other:?}"),
            }
        }
    }

    // -- text parser: WKT geometry literals -----------------------------------

    #[test]
    fn parses_a_wkt_point() {
        let filter = parse_text("S_INTERSECTS(geom, POINT(1 2))").unwrap();
        assert_eq!(
            filter,
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Wkt(WktGeometry::Point([1.0, 2.0])),
            }
        );
    }

    #[test]
    fn parses_a_wkt_linestring() {
        let filter = parse_text("S_INTERSECTS(geom, LINESTRING(1 2, 3 4, 5 6))").unwrap();
        assert_eq!(
            filter,
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Wkt(WktGeometry::LineString(vec![
                    [1.0, 2.0],
                    [3.0, 4.0],
                    [5.0, 6.0],
                ])),
            }
        );
    }

    #[test]
    fn parses_a_wkt_polygon_with_a_hole() {
        let filter = parse_text(
            "S_WITHIN(geom, POLYGON((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2)))",
        )
        .unwrap();
        match filter {
            Filter::Spatial {
                geometry: GeometryLiteral::Wkt(WktGeometry::Polygon(rings)),
                ..
            } => {
                assert_eq!(rings.len(), 2, "exterior ring plus one hole");
                assert_eq!(rings[0].len(), 5);
                assert_eq!(rings[1].len(), 5);
            }
            other => panic!("expected Spatial/Wkt(Polygon), got {other:?}"),
        }
    }

    #[test]
    fn parses_a_wkt_multipoint_in_both_spellings() {
        for text in [
            "S_INTERSECTS(geom, MULTIPOINT(1 2, 3 4))",
            "S_INTERSECTS(geom, MULTIPOINT((1 2), (3 4)))",
        ] {
            let filter = parse_text(text).unwrap();
            assert_eq!(
                filter,
                Filter::Intersects {
                    property: "geom".to_string(),
                    geometry: GeometryLiteral::Wkt(WktGeometry::MultiPoint(vec![
                        [1.0, 2.0],
                        [3.0, 4.0],
                    ])),
                },
                "for input '{text}'"
            );
        }
    }

    #[test]
    fn parses_a_wkt_multilinestring() {
        let filter =
            parse_text("S_INTERSECTS(geom, MULTILINESTRING((1 2, 3 4), (5 6, 7 8)))").unwrap();
        assert_eq!(
            filter,
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Wkt(WktGeometry::MultiLineString(vec![
                    vec![[1.0, 2.0], [3.0, 4.0]],
                    vec![[5.0, 6.0], [7.0, 8.0]],
                ])),
            }
        );
    }

    #[test]
    fn parses_a_wkt_multipolygon() {
        let filter = parse_text(
            "S_INTERSECTS(geom, MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0)), ((10 10, 11 10, 11 11, 10 10))))",
        )
        .unwrap();
        match filter {
            Filter::Intersects {
                geometry: GeometryLiteral::Wkt(WktGeometry::MultiPolygon(polys)),
                ..
            } => assert_eq!(polys.len(), 2),
            other => panic!("expected Intersects/Wkt(MultiPolygon), got {other:?}"),
        }
    }

    #[test]
    fn parses_a_wkt_geometrycollection() {
        let filter =
            parse_text("S_INTERSECTS(geom, GEOMETRYCOLLECTION(POINT(1 2), LINESTRING(3 4, 5 6)))")
                .unwrap();
        match filter {
            Filter::Intersects {
                geometry: GeometryLiteral::Wkt(WktGeometry::GeometryCollection(members)),
                ..
            } => {
                assert_eq!(members.len(), 2);
                assert!(matches!(members[0], WktGeometry::Point(_)));
                assert!(matches!(members[1], WktGeometry::LineString(_)));
            }
            other => panic!("expected Intersects/Wkt(GeometryCollection), got {other:?}"),
        }
    }

    #[test]
    fn wkt_to_wkt_text_round_trips_through_st_geomfromtext_syntax() {
        assert_eq!(WktGeometry::Point([1.0, 2.0]).to_wkt_text(), "POINT(1 2)");
        assert_eq!(
            WktGeometry::LineString(vec![[1.0, 2.0], [3.0, 4.0]]).to_wkt_text(),
            "LINESTRING(1 2,3 4)"
        );
        assert_eq!(
            WktGeometry::Polygon(vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 0.0]]])
                .to_wkt_text(),
            "POLYGON((0 0,1 0,0 1,0 0))"
        );
    }

    #[test]
    fn rejects_a_wkt_z_dimensionality_tag() {
        let err = parse_text("S_INTERSECTS(geom, POINT Z (1 2 3))").unwrap_err();
        match err {
            Error::Invalid(message) => assert!(message.contains('Z'), "message was: {message}"),
            other => panic!("expected Error::Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_wkt_m_and_zm_dimensionality_tag() {
        assert!(matches!(
            parse_text("S_INTERSECTS(geom, POINT M (1 2 3))"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            parse_text("S_INTERSECTS(geom, POINT ZM (1 2 3 4))"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn rejects_an_implicit_third_ordinate_with_no_dimensionality_tag() {
        // `POINT(1 2 3)` with no `Z` keyword at all is a common WKT
        // convention for an implicit Z ordinate — still rejected, and named.
        let err = parse_text("S_INTERSECTS(geom, POINT(1 2 3))").unwrap_err();
        match err {
            Error::Invalid(message) => {
                assert!(message.contains("2D"), "message was: {message}")
            }
            other => panic!("expected Error::Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_an_empty_wkt_geometry() {
        let err = parse_text("S_INTERSECTS(geom, POINT EMPTY)").unwrap_err();
        match err {
            Error::Invalid(message) => {
                assert!(message.contains("empty"), "message was: {message}")
            }
            other => panic!("expected Error::Invalid, got {other:?}"),
        }
    }

    // -- text parser: errors -------------------------------------------------

    #[test]
    fn rejects_unterminated_string() {
        assert!(matches!(
            parse_text("name = 'unterminated"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(matches!(
            parse_text("name = 'a' extra"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn rejects_unbalanced_parens() {
        assert!(matches!(parse_text("(name = 'a'"), Err(Error::Invalid(_))));
    }

    #[test]
    fn rejects_empty_input() {
        assert!(matches!(parse_text(""), Err(Error::Invalid(_))));
    }

    // -- json parser ----------------------------------------------------------

    #[test]
    fn parses_json_equality() {
        let filter = parse_json(r#"{"op":"=","args":[{"property":"name"},"a"]}"#).unwrap();
        assert_eq!(
            filter,
            Filter::Compare {
                property: "name".to_string(),
                op: CompareOp::Eq,
                value: Literal::Text("a".to_string()),
            }
        );
    }

    #[test]
    fn parses_json_and_or_not() {
        let and_filter = parse_json(
            r#"{"op":"and","args":[{"op":"=","args":[{"property":"a"},1]},{"op":"=","args":[{"property":"b"},2]}]}"#,
        )
        .unwrap();
        assert!(matches!(and_filter, Filter::And(terms) if terms.len() == 2));

        let not_filter =
            parse_json(r#"{"op":"not","args":[{"op":"isNull","args":[{"property":"name"}]}]}"#)
                .unwrap();
        assert!(matches!(not_filter, Filter::Not(_)));
    }

    #[test]
    fn parses_json_s_intersects_with_bbox() {
        let filter =
            parse_json(r#"{"op":"s_intersects","args":[{"property":"geom"},{"bbox":[1,2,3,4]}]}"#)
                .unwrap();
        assert_eq!(
            filter,
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }
        );
    }

    #[test]
    fn parses_json_s_intersects_with_a_geojson_geometry() {
        let filter = parse_json(
            r#"{"op":"s_intersects","args":[{"property":"geom"},{"type":"Point","coordinates":[1,2]}]}"#,
        )
        .unwrap();
        match filter {
            Filter::Intersects {
                geometry: GeometryLiteral::GeoJson(value),
                ..
            } => {
                assert_eq!(value["type"], "Point");
            }
            other => panic!("expected Intersects/GeoJson, got {other:?}"),
        }
    }

    // -- json parser: advanced comparison operators ---------------------------

    #[test]
    fn parses_json_like() {
        let filter = parse_json(r#"{"op":"like","args":[{"property":"name"},"Sm%"]}"#).unwrap();
        assert_eq!(
            filter,
            Filter::Like {
                property: "name".to_string(),
                pattern: "Sm%".to_string(),
                negated: false,
            }
        );
    }

    #[test]
    fn parses_json_not_like_via_the_generic_not_wrapper() {
        // CQL2-JSON has no native negated form for LIKE/BETWEEN/IN — negation
        // always goes through the generic `not` op, exactly like `IS NOT
        // NULL` already does for `isNull` in this same parser.
        let filter =
            parse_json(r#"{"op":"not","args":[{"op":"like","args":[{"property":"name"},"Sm%"]}]}"#)
                .unwrap();
        assert_eq!(
            filter,
            Filter::Not(Box::new(Filter::Like {
                property: "name".to_string(),
                pattern: "Sm%".to_string(),
                negated: false,
            }))
        );
    }

    #[test]
    fn parses_json_between() {
        let filter =
            parse_json(r#"{"op":"between","args":[{"property":"population"},10,20]}"#).unwrap();
        assert_eq!(
            filter,
            Filter::Between {
                property: "population".to_string(),
                low: Literal::Number(10.0),
                high: Literal::Number(20.0),
                negated: false,
            }
        );
    }

    #[test]
    fn parses_json_in() {
        let filter =
            parse_json(r#"{"op":"in","args":[{"property":"name"},["a","b","c"]]}"#).unwrap();
        assert_eq!(
            filter,
            Filter::In {
                property: "name".to_string(),
                values: vec![
                    Literal::Text("a".to_string()),
                    Literal::Text("b".to_string()),
                    Literal::Text("c".to_string()),
                ],
                negated: false,
            }
        );
    }

    // -- json parser: CASEI() case-insensitive comparison ---------------------

    #[test]
    fn parses_json_casei_equality_property_first() {
        let filter = parse_json(
            r#"{"op":"=","args":[{"op":"casei","args":[{"property":"name"}]},{"op":"casei","args":["john"]}]}"#,
        )
        .unwrap();
        assert_eq!(
            filter,
            Filter::CaseInsensitiveCompare {
                property: "name".to_string(),
                op: CaseInsensitiveCompareOp::Eq,
                value: "john".to_string(),
            }
        );
    }

    #[test]
    fn parses_json_casei_inequality_literal_first() {
        // Either operand order resolves to the same `Filter` — the property
        // vs. literal role is what matters, not which side of `<>` it's on.
        let filter = parse_json(
            r#"{"op":"<>","args":[{"op":"casei","args":["john"]},{"op":"casei","args":[{"property":"name"}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            filter,
            Filter::CaseInsensitiveCompare {
                property: "name".to_string(),
                op: CaseInsensitiveCompareOp::Ne,
                value: "john".to_string(),
            }
        );
    }

    // -- json parser: spatial functions beyond S_INTERSECTS -------------------

    #[test]
    fn parses_every_new_json_spatial_predicate() {
        let cases = [
            ("s_within", SpatialOp::Within),
            ("s_contains", SpatialOp::Contains),
            ("s_disjoint", SpatialOp::Disjoint),
            ("s_touches", SpatialOp::Touches),
            ("s_overlaps", SpatialOp::Overlaps),
            ("s_crosses", SpatialOp::Crosses),
            ("s_equals", SpatialOp::Equals),
        ];
        for (op, expected_op) in cases {
            let filter = parse_json(&format!(
                r#"{{"op":"{op}","args":[{{"property":"geom"}},{{"bbox":[1,2,3,4]}}]}}"#
            ))
            .unwrap();
            assert_eq!(
                filter,
                Filter::Spatial {
                    property: "geom".to_string(),
                    op: expected_op,
                    geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
                },
                "for op '{op}'"
            );
        }
    }

    #[test]
    fn parses_json_temporal_operators() {
        assert_eq!(
            parse_json(
                r#"{"op":"t_after","args":[{"property":"observed_at"},{"timestamp":"2020-01-01T00:00:00Z"}]}"#
            )
            .unwrap(),
            Filter::After {
                property: "observed_at".to_string(),
                instant: "2020-01-01T00:00:00Z".to_string(),
            }
        );
        assert_eq!(
            parse_json(
                r#"{"op":"t_during","args":[{"property":"observed_at"},{"interval":["2020-01-01T00:00:00Z","2021-01-01T00:00:00Z"]}]}"#
            )
            .unwrap(),
            Filter::During {
                property: "observed_at".to_string(),
                start: "2020-01-01T00:00:00Z".to_string(),
                end: "2021-01-01T00:00:00Z".to_string(),
            }
        );
    }

    #[test]
    fn parses_every_new_json_temporal_predicate() {
        let cases = [
            ("t_contains", TemporalOp::Contains),
            ("t_disjoint", TemporalOp::Disjoint),
            ("t_equals", TemporalOp::Equals),
            ("t_finishedby", TemporalOp::FinishedBy),
            ("t_finishes", TemporalOp::Finishes),
            ("t_intersects", TemporalOp::Intersects),
            ("t_meets", TemporalOp::Meets),
            ("t_metby", TemporalOp::MetBy),
            ("t_overlappedby", TemporalOp::OverlappedBy),
            ("t_overlaps", TemporalOp::Overlaps),
            ("t_startedby", TemporalOp::StartedBy),
            ("t_starts", TemporalOp::Starts),
        ];
        for (op, expected_op) in cases {
            let filter = parse_json(&format!(
                r#"{{"op":"{op}","args":[{{"property":"observed_at"}},{{"timestamp":"2020-06-01T00:00:00Z"}}]}}"#
            ))
            .unwrap();
            assert_eq!(
                filter,
                Filter::Temporal {
                    property: "observed_at".to_string(),
                    op: expected_op,
                    value: TemporalValue::Instant("2020-06-01T00:00:00Z".to_string()),
                },
                "for op '{op}'"
            );
        }
    }

    #[test]
    fn parses_a_new_json_temporal_predicate_with_an_interval_literal() {
        let filter = parse_json(
            r#"{"op":"t_overlaps","args":[{"property":"observed_at"},{"interval":["2020-01-01T00:00:00Z","2020-12-31T00:00:00Z"]}]}"#,
        )
        .unwrap();
        assert_eq!(
            filter,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Overlaps,
                value: TemporalValue::Interval(
                    "2020-01-01T00:00:00Z".to_string(),
                    "2020-12-31T00:00:00Z".to_string()
                ),
            }
        );
    }

    #[test]
    fn rejects_unknown_json_operator() {
        assert!(matches!(
            parse_json(r#"{"op":"a_contains","args":[{"property":"tags"},["a"]]}"#),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(parse_json("not json"), Err(Error::Invalid(_))));
    }

    // -- filter-lang dispatch -----------------------------------------------

    #[test]
    fn parse_dispatches_on_filter_lang() {
        assert!(parse(FILTER_LANG_CQL2_TEXT, "name = 'a'").is_ok());
        assert!(parse(
            FILTER_LANG_CQL2_JSON,
            r#"{"op":"=","args":[{"property":"name"},"a"]}"#
        )
        .is_ok());
        assert!(matches!(
            parse("bogus-lang", "name = 'a'"),
            Err(Error::Invalid(_))
        ));
    }

    // -- validate -------------------------------------------------------------

    #[test]
    fn validate_accepts_a_known_attribute() {
        let filter = parse_text("population > 100").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
    }

    #[test]
    fn validate_accepts_the_geometry_and_datetime_columns_as_plain_properties() {
        let filter = parse_text("geom IS NULL").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
        let filter = parse_text("observed_at IS NULL").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
    }

    #[test]
    fn validate_rejects_an_unknown_property_naming_it() {
        let filter = parse_text("bogus = 1").unwrap();
        match validate(&filter, &descriptor(), None) {
            Err(Error::Invalid(message)) => assert!(message.contains("bogus")),
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_advanced_comparison_operators_against_a_known_attribute() {
        assert!(validate(&parse_text("name LIKE 'a%'").unwrap(), &descriptor(), None).is_ok());
        assert!(validate(
            &parse_text("population BETWEEN 1 AND 10").unwrap(),
            &descriptor(),
            None
        )
        .is_ok());
        assert!(validate(
            &parse_text("name IN ('a', 'b')").unwrap(),
            &descriptor(),
            None
        )
        .is_ok());
    }

    #[test]
    fn validate_rejects_advanced_comparison_operators_against_an_unknown_property() {
        assert!(matches!(
            validate(&parse_text("bogus LIKE 'a%'").unwrap(), &descriptor(), None),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            validate(
                &parse_text("bogus BETWEEN 1 AND 10").unwrap(),
                &descriptor(),
                None
            ),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            validate(&parse_text("bogus IN (1, 2)").unwrap(), &descriptor(), None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn validate_accepts_casei_against_a_known_attribute() {
        let filter = parse_text("CASEI(name) = CASEI('john')").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
    }

    #[test]
    fn validate_rejects_casei_against_an_unknown_property() {
        let filter = parse_text("CASEI(bogus) = CASEI('john')").unwrap();
        assert!(matches!(
            validate(&filter, &descriptor(), None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn validate_accepts_the_new_spatial_predicates_against_the_geometry_column() {
        let filter = parse_text("S_WITHIN(geom, BBOX(1, 2, 3, 4))").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
    }

    #[test]
    fn validate_rejects_the_new_spatial_predicates_against_a_non_geometry_property() {
        let filter = parse_text("S_WITHIN(population, BBOX(1, 2, 3, 4))").unwrap();
        assert!(matches!(
            validate(&filter, &descriptor(), None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn validate_accepts_s_intersects_against_the_geometry_column() {
        let filter = parse_text("S_INTERSECTS(geom, BBOX(1, 2, 3, 4))").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
    }

    #[test]
    fn validate_rejects_s_intersects_against_a_non_geometry_property() {
        let filter = parse_text("S_INTERSECTS(population, BBOX(1, 2, 3, 4))").unwrap();
        assert!(matches!(
            validate(&filter, &descriptor(), None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn validate_accepts_temporal_operators_against_the_datetime_column() {
        let filter = parse_text("T_AFTER(observed_at, '2020-01-01T00:00:00Z')").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
    }

    #[test]
    fn validate_rejects_temporal_operators_against_a_non_datetime_property() {
        let filter = parse_text("T_AFTER(name, '2020-01-01T00:00:00Z')").unwrap();
        assert!(matches!(
            validate(&filter, &descriptor(), None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn validate_rejects_temporal_operators_when_the_collection_has_no_datetime_column() {
        let mut descriptor = descriptor();
        descriptor.datetime = None;
        let filter = parse_text("T_AFTER(observed_at, '2020-01-01T00:00:00Z')").unwrap();
        assert!(matches!(
            validate(&filter, &descriptor, None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn validate_accepts_the_new_temporal_predicates_against_the_datetime_column() {
        let filter = parse_text("T_OVERLAPS(observed_at, '2020-01-01T00:00:00Z')").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_ok());
    }

    #[test]
    fn validate_rejects_the_new_temporal_predicates_against_a_non_datetime_property() {
        let filter = parse_text("T_OVERLAPS(name, '2020-01-01T00:00:00Z')").unwrap();
        assert!(matches!(
            validate(&filter, &descriptor(), None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn validate_recurses_into_and_or_not() {
        let filter = parse_text("population > 100 AND bogus = 1").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_err());

        let filter = parse_text("NOT bogus = 1").unwrap();
        assert!(validate(&filter, &descriptor(), None).is_err());
    }

    // -- validate: declared schema (`#44`) -----------------------------------

    fn schema_with(properties: Vec<PropertyDecl>, additional_properties: bool) -> SchemaDecl {
        SchemaDecl {
            properties,
            additional_properties,
        }
    }

    fn property(name: &str, type_: PropertyType) -> PropertyDecl {
        PropertyDecl {
            name: name.to_string(),
            type_,
            required: false,
        }
    }

    /// No-regression guard: a schema left at its default
    /// (`additional_properties: true`, the "declares some properties but
    /// stays open" shape) behaves exactly like the undeclared case — every
    /// real attribute column still validates, whether or not it's in
    /// `properties`.
    #[test]
    fn validate_with_an_open_schema_still_accepts_any_real_attribute() {
        let schema = schema_with(vec![property("name", PropertyType::String)], true);
        let filter = parse_text("population > 100").unwrap();
        assert!(validate(&filter, &descriptor(), Some(&schema)).is_ok());
    }

    #[test]
    fn validate_rejects_an_attribute_the_closed_schema_does_not_declare() {
        let schema = schema_with(vec![property("population", PropertyType::Integer)], false);
        let filter = parse_text("name = 'a'").unwrap();
        match validate(&filter, &descriptor(), Some(&schema)) {
            Err(Error::Invalid(message)) => assert!(message.contains("name")),
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_an_attribute_the_closed_schema_declares() {
        let schema = schema_with(vec![property("population", PropertyType::Integer)], false);
        let filter = parse_text("population > 100").unwrap();
        assert!(validate(&filter, &descriptor(), Some(&schema)).is_ok());
    }

    /// The geometry/datetime columns stay filterable as plain properties
    /// even under a closed schema that never lists them — they are
    /// structural, not part of the declared property enumeration.
    #[test]
    fn validate_closed_schema_still_exempts_the_geometry_and_datetime_columns() {
        let schema = schema_with(vec![property("population", PropertyType::Integer)], false);
        let filter = parse_text("geom IS NULL").unwrap();
        assert!(validate(&filter, &descriptor(), Some(&schema)).is_ok());
        let filter = parse_text("observed_at IS NULL").unwrap();
        assert!(validate(&filter, &descriptor(), Some(&schema)).is_ok());
    }

    // -- fingerprint (`#34`, tile-cache policy partitioning) -----------------

    #[test]
    fn fingerprint_is_stable_for_repeated_calls_on_the_same_filter() {
        let filter = parse_text("org = 'acme'").unwrap();
        assert_eq!(filter.fingerprint(), filter.fingerprint());
    }

    #[test]
    fn fingerprint_agrees_across_two_structurally_identical_filters() {
        // Simulates two different subjects whose claims substitute to the
        // same effective filter text — parsed independently, they must
        // still fingerprint identically so both subjects share one tile
        // cache entry.
        let a = parse_text("org = 'acme'").unwrap();
        let b = parse_text("org = 'acme'").unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_for_a_different_literal_value() {
        let a = parse_text("org = 'acme'").unwrap();
        let b = parse_text("org = 'globex'").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_for_a_different_property() {
        let a = parse_text("org = 'acme'").unwrap();
        let b = parse_text("team = 'acme'").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_for_a_different_operator() {
        let a = parse_text("population = 100").unwrap();
        let b = parse_text("population > 100").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_between_and_and_or_of_the_same_terms() {
        let a = parse_text("a = 1 AND b = 2").unwrap();
        let b = parse_text("a = 1 OR b = 2").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_between_a_bbox_and_a_wkt_geometry_literal_for_the_same_predicate() {
        let a = parse_text("S_INTERSECTS(geom, BBOX(1, 2, 3, 4))").unwrap();
        let b = parse_text("S_INTERSECTS(geom, POINT(1 2))").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_between_two_structurally_different_wkt_geometries() {
        let a = parse_text("S_INTERSECTS(geom, POINT(1 2))").unwrap();
        let b = parse_text("S_INTERSECTS(geom, POINT(3 4))").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_agrees_for_two_structurally_identical_wkt_geometries() {
        let a = parse_text("S_INTERSECTS(geom, POLYGON((0 0, 1 0, 0 1, 0 0)))").unwrap();
        let b = parse_text("S_INTERSECTS(geom, POLYGON((0 0, 1 0, 0 1, 0 0)))").unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_between_two_new_temporal_operators_on_the_same_literal() {
        let a = parse_text("T_MEETS(observed_at, '2020-01-01T00:00:00Z')").unwrap();
        let b = parse_text("T_METBY(observed_at, '2020-01-01T00:00:00Z')").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_differs_between_a_temporal_instant_and_an_equal_start_interval() {
        let instant = parse_text("T_OVERLAPS(observed_at, '2020-01-01T00:00:00Z')").unwrap();
        let interval = parse_text(
            "T_OVERLAPS(observed_at, INTERVAL('2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z'))",
        )
        .unwrap();
        assert_ne!(instant.fingerprint(), interval.fingerprint());
    }
}
