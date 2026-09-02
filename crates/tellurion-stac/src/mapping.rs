//! Pure descriptor -> STAC Collection mapping (`#36`, slice A; `#50`
//! convergence): no I/O and no link-building here — the caller
//! (`handlers.rs`) resolves the `CanonicalDescriptor` and builds `links`
//! from the request's own URL, so this module stays unit-testable without a
//! router, a resolver, or an HTTP request.
//!
//! `to_stac_collection` reads ONLY `CanonicalDescriptor`
//! (`tellurion_core::descriptor::canonical`) — never a raw
//! `CollectionDescriptor`/`StacConf`/`SchemaDecl` directly — so this crate's
//! Collection mapping and `tellurion-features`' own metadata emission always
//! read the same merged truth (`#50`, "one canonical descriptor/asset model
//! feeds OGC + STAC").

use std::collections::BTreeMap;

use serde_json::Value;

use tellurion_core::{AssetDecl, CanonicalDescriptor};

use crate::model::{
    Link, StacAsset, StacCollection, StacExtent, StacSpatialExtent, StacTemporalExtent,
};
use crate::projection::{log_override_disagreement, DerivedProjection, PROJECTION_EXTENSION_URI};

/// The STAC core spec version this crate's Collection/Catalog documents
/// declare — verified against the latest released `stac-spec` tag
/// (`v1.1.0`, 2024-09-11), not the separate `stac-api-spec` conformance-class
/// versioning (`v1.0.0`, see `CONFORMANCE_CLASSES` in `lib.rs`): the two
/// repos version independently.
pub const STAC_VERSION: &str = "1.1.0";

/// STAC Collection `license` value for a collection with no configured
/// `stac.license` (`#36`). `"other"` — not the older `"proprietary"`/
/// `"various"` strings — is the STAC 1.1.0 Collection spec's own answer for
/// "license unknown, not an SPDX identifier"; both alternatives are
/// explicitly marked deprecated in the current spec text.
const DEFAULT_LICENSE: &str = "other";

/// Whole-Earth bbox — the STAC Collection spec's own example value for an
/// unbounded spatial extent (`[-180, -90, 180, 90]`) — served when the
/// backend couldn't derive a real one: an empty collection, or a derivation
/// failure (see `handlers::resolved_descriptor`'s identical
/// never-fail-the-request reasoning, mirroring
/// `tellurion_features::handlers::collection_extent` at the OGC API
/// Features layer). STAC requires `extent.spatial.bbox` to be present, so
/// unlike OGC API Features' `extent: null`, there is no "omit it" option
/// here.
const WHOLE_EARTH_BBOX: [f64; 4] = [-180.0, -90.0, 180.0, 90.0];

/// Maps `canonical` (this collection's merged `CanonicalDescriptor`, already
/// resolved for `(tenant, catalog, collection)` by `Router::
/// canonical_descriptor`, or `None` when that resolution failed outright —
/// an unresolvable collection id, not a mere derivation gap, which
/// `CanonicalDescriptor` itself already absorbs internally) into a STAC
/// Collection body. `links` is supplied by the caller — see the module doc.
///
/// `assets` is the caller's already-materialized, capability-derived map
/// (`#36` slice B, `#48` — see `handlers::asset_capabilities`/
/// `assets::collection_assets`); this function layers `canonical.stac.
/// assets` (`#36` slice 1, operator-declared) on top before returning, with
/// a declared entry winning outright over a capability-derived one sharing
/// the same id — the same override-beats-derived precedence
/// `CanonicalField`'s own `Provenance::Override` already uses for physical
/// identity fields. This merge only happens here, not in `handlers.rs`'s
/// items/search call sites, which build a collection's items with the
/// capability-derived map alone — declared collection-level assets
/// deliberately do not also ride onto every item (`#36` slice 1's own
/// scope; see this crate's `assets` module for the capability-derived
/// mechanism items DO share).
///
/// Temporal extent is always the fully-open `[[null, null]]` interval in
/// this slice: `CanonicalDescriptor` carries a `datetime` *column name*
/// (`#19`), never the actual min/max timestamps observed in the data, and
/// computing those would mean scanning items — out of scope until the
/// `items` endpoint lands (`#36` slice B). An open interval is a legitimate,
/// spec-sanctioned STAC value for "temporal extent not (yet) known", not a
/// fabricated one.
///
/// `canonical.stac.contacts` (`#187`, first slice) is deliberately *not*
/// projected. The STAC Collection spec has no responsible-party field: its
/// nearest concept is `providers[]`, which this function already emits from
/// `canonical.stac.providers` and which means an *organization's* role in
/// producing/hosting the data, not a person to contact. Folding contacts
/// into `providers` would silently merge two distinct operator-declared
/// lists and put individual names and email addresses into a field the
/// spec's consumers read as organizational attribution. So contacts stay
/// where they have a real, schema-mandated home — the ISO 19115 projection
/// (`iso19139::to_iso19139`, `MD_Metadata/contact`) — and an operator who
/// wants an organization advertised in STAC declares it under `providers`,
/// which is exactly what that field is for. Revisit only if STAC gains a
/// contact field or this deployment adopts an extension that defines one.
pub fn to_stac_collection(
    canonical: Option<&CanonicalDescriptor>,
    external_id: &str,
    links: Vec<Link>,
    mut assets: BTreeMap<String, StacAsset>,
) -> StacCollection {
    let bbox = canonical
        .and_then(|c| c.extent)
        .map(|extent| extent.bbox)
        .unwrap_or(WHOLE_EARTH_BBOX);

    let stac = canonical.and_then(|c| c.stac.as_ref());
    let license = stac
        .and_then(|conf| conf.license.clone())
        .unwrap_or_else(|| DEFAULT_LICENSE.to_string());
    let keywords = stac.map(|conf| conf.keywords.clone()).unwrap_or_default();
    let providers = stac.map(|conf| conf.providers.clone()).unwrap_or_default();

    if let Some(stac) = stac {
        for (asset_id, declared) in &stac.assets {
            assets.insert(asset_id.clone(), to_declared_asset(declared));
        }
    }

    // `#36` projection extension, Collection half: a raster-backed
    // collection (COG/Zarr) implements no `FeatureSource`, so this document
    // is the only STAC surface its driver-read georeferencing can honestly
    // appear on — emitted as `summaries` (the Collection spec's own place
    // for Item Properties fields), each entry the array-of-unique-values
    // form with the collection's single genuine value. Derived from
    // `canonical.projection` ALONE — `CatalogSource::projection`, which
    // only a driver that reads georeferencing out of its own storage ever
    // overrides — deliberately NOT from `canonical.srid`: the `#36`
    // decision emits a vector collection's EPSG per Item (where the
    // sidecar override channel and its disagreement log live), and
    // summarizing it here too would change every SQL-backed collection
    // document for no consumer that asked. No projection knowledge means
    // no `summaries`, no `stac_extensions`, and a byte-identical document.
    let facts = canonical.and_then(|c| c.projection);
    let derived = crate::projection::derive_projection(facts.as_ref(), None);
    let (stac_extensions, summaries) = match derived {
        Some(derived) => (
            vec![PROJECTION_EXTENSION_URI.to_string()],
            derived
                .fields
                .into_iter()
                .map(|(field, value)| (field.to_string(), Value::Array(vec![value])))
                .collect(),
        ),
        None => (Vec::new(), BTreeMap::new()),
    };

    StacCollection {
        type_: "Collection",
        stac_version: STAC_VERSION,
        stac_extensions,
        id: external_id.to_string(),
        title: external_id.to_string(),
        description: format!("STAC Collection for '{external_id}'."),
        license,
        keywords,
        providers,
        extent: StacExtent {
            spatial: StacSpatialExtent { bbox: vec![bbox] },
            temporal: StacTemporalExtent {
                interval: vec![[None, None]],
            },
        },
        summaries,
        links,
        assets,
    }
}

/// Converts one operator-declared `stac.assets` entry (`config::AssetDecl`,
/// `#36` slice 1) into the wire `StacAsset` shape: `href` verbatim; `type`/
/// `title`/`roles` carried through exactly as declared — absent stays
/// absent (see `StacAsset`'s own doc for why `media_type`/`title` are
/// `Option`, not a fabricated empty string); `templated: false` always,
/// since a config-declared asset is a literal href, never one of the
/// `{tileMatrix}`/`{tileRow}`/`{tileCol}` templates
/// `assets::collection_assets` produces for its own, capability-derived
/// assets.
fn to_declared_asset(decl: &AssetDecl) -> StacAsset {
    StacAsset {
        href: decl.href.clone(),
        media_type: decl.media_type.clone(),
        title: decl.title.clone(),
        // `config::AssetDecl` carries no description field, so a declared
        // asset has none to give — absent stays absent (`StacAsset`'s own
        // clean-omission rule), never a fabricated empty string.
        description: None,
        roles: decl.roles.clone(),
        templated: false,
    }
}

/// Maps one raw GeoJSON Feature `feature` (as returned verbatim by
/// `FeatureSource::items`/`item` — see `tellurion_core::storage`) into a
/// STAC Item, in place: `stac_version`/`collection`/`links`/`assets` are
/// inserted, `bbox` is derived from the feature's own `geometry` (STAC Item
/// spec, `v1.1.0`: `bbox` is REQUIRED when `geometry` is not `null`), and
/// `properties.datetime` is set from `datetime_column`'s value on this row —
/// `decl.datetime`, the same column `Router::effective_decl` already
/// resolved for the caller (override-or-derived, `#19`), never re-derived
/// here.
///
/// Datetime null rule (verified 2026-07 against `stac-spec`'s
/// `item-spec/item-spec.md` and `commons/common-metadata.md` at the `v1.1.0`
/// tag): `properties.datetime` is `string|null` and REQUIRED; `null` is only
/// legal when `start_datetime`/`end_datetime` (both typed as plain,
/// non-nullable `string` — never `..` or empty) are ALSO present. This
/// server has no genuine per-item interval to offer when the row's own
/// datetime value is absent — the collection has no datetime column at all,
/// or this particular row's value is SQL NULL — and computing one (a real
/// min/max scan) is explicitly out of scope for this slice. Fabricating a
/// `start_datetime`/`end_datetime` pair just to make `null` spec-legal would
/// be actively misleading (a client would read it as a genuine known
/// interval); this function instead takes the documented, honest partial
/// answer: `properties.datetime` is `null` with no `start_datetime`/
/// `end_datetime` alongside it — a known, intentional deviation from full
/// Item-spec validity for temporal-less collections/rows, not a silent one.
///
/// `sidecar` (`#202`) is this item's row from the collection's
/// `"<table>_stac"` metadata sidecar, when it has one — `None` for every
/// collection that never opted in (`CollectionDecl::stac_metadata`), for
/// every item with no sidecar row, and therefore for every Item this
/// function produced before `#202` existed: the merge below is the ONLY
/// thing the parameter changes, so `None` reproduces today's document byte
/// for byte. See [`merge_sidecar_doc`] for the precedence rule.
///
/// `projection` (`#36`, STAC projection extension) is this collection's
/// derived `proj:*` fields (`crate::projection::derive_projection`, built
/// once per collection by the caller from the effective decl's
/// `projection`/`srid` carriers) — `None` for every collection whose driver
/// knows nothing, which reproduces the pre-extension document byte for
/// byte. When present, the derived fields are inserted into `properties`
/// BEFORE the sidecar merge, so a sidecar-supplied `proj:` value wins
/// exactly like any other sidecar property — that override is the point —
/// and a genuine derived-vs-override disagreement is then logged once per
/// collection per field (`projection::log_override_disagreement`; an
/// agreeing override and a pure gap-fill both stay silent). The extension
/// URI joins `stac_extensions` only because at least one field was
/// genuinely derived — appended to (never replacing) a sidecar-supplied
/// `stac_extensions` array, deduplicated.
pub fn to_stac_item(
    mut feature: Value,
    collection_external_id: &str,
    datetime_column: Option<&str>,
    assets: &BTreeMap<String, StacAsset>,
    links: Vec<Link>,
    sidecar: Option<&Value>,
    projection: Option<&DerivedProjection>,
) -> Value {
    let Value::Object(map) = &mut feature else {
        // `FeatureSource` always returns a GeoJSON Feature object; if a
        // driver ever handed back something else there is nothing sensible
        // to attach STAC fields to — return it unchanged rather than panic.
        return feature;
    };

    map.insert(
        "stac_version".to_string(),
        Value::String(STAC_VERSION.to_string()),
    );
    map.insert(
        "collection".to_string(),
        Value::String(collection_external_id.to_string()),
    );
    map.insert("links".to_string(), serde_json::to_value(links).unwrap());
    map.insert("assets".to_string(), serde_json::to_value(assets).unwrap());

    if let Some(bbox) = map.get("geometry").and_then(bbox_from_geometry) {
        map.insert(
            "bbox".to_string(),
            Value::Array(bbox.iter().map(|v| Value::from(*v)).collect()),
        );
    }

    let datetime_value = datetime_column.and_then(|column| {
        map.get("properties")
            .and_then(|properties| properties.get(column))
            .filter(|value| !value.is_null())
            .cloned()
    });

    let properties = map
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(properties) = properties {
        properties.insert(
            "datetime".to_string(),
            datetime_value.unwrap_or(Value::Null),
        );
        if let Some(projection) = projection {
            for (field, value) in &projection.fields {
                properties.insert((*field).to_string(), value.clone());
            }
        }
    }

    if let Some(doc) = sidecar {
        merge_sidecar_doc(map, doc);
    }

    if let Some(projection) = projection {
        // The sidecar merge just ran, so whatever now sits under each
        // derived field is what this document actually serves: the derived
        // value (no override, or an agreeing one — indistinguishable on
        // purpose, neither is a signal) or the operator's override. A
        // difference is a genuine disagreement between the two sources of
        // truth the `#36` decision knowingly created — logged, once per
        // collection per field, never per request.
        for (field, derived_value) in &projection.fields {
            let served = map
                .get("properties")
                .and_then(|properties| properties.get(*field));
            if let Some(served) = served {
                if served != derived_value {
                    log_override_disagreement(collection_external_id, field, derived_value, served);
                }
            }
        }
        declare_projection_extension(map);
    }

    feature
}

/// Adds [`PROJECTION_EXTENSION_URI`] to the Item's `stac_extensions` —
/// called only when at least one `proj:` field was genuinely derived for
/// this collection, so the declaration can never outrun the emission
/// (`#287`'s defect, refused structurally). A sidecar-supplied
/// `stac_extensions` array (the `#202` verbatim passthrough) is appended
/// to, never replaced — the operator's own entries survive — and an
/// already-present URI is not duplicated. A sidecar `stac_extensions` that
/// is not an array at all is left exactly as the operator wrote it (the
/// passthrough is verbatim; rewriting a malformed value would trade one
/// invalid document for a differently invalid one this lane then owns).
fn declare_projection_extension(map: &mut serde_json::Map<String, Value>) {
    let extensions = map
        .entry("stac_extensions".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(extensions) = extensions {
        if !extensions
            .iter()
            .any(|entry| entry == PROJECTION_EXTENSION_URI)
        {
            extensions.push(Value::String(PROJECTION_EXTENSION_URI.to_string()));
        }
    }
}

/// Top-level Item members the sidecar may never set (`#202`) — every one of
/// them is a fact this lane derives from something the sidecar has no view
/// of, so letting a stored document win would let a stale or hand-edited
/// row rewrite the item's identity, its geometry, or the links of the very
/// request being served:
///
/// - `type`/`stac_version`: fixed by the spec version this crate emits.
/// - `id`/`geometry`/`bbox`: the feature's own, straight from the primary
///   table (`bbox` is derived from that geometry — a sidecar bbox could
///   disagree with the geometry sitting next to it in the same document).
/// - `collection`: the resolved external id of the collection being served.
/// - `links`: built from the request's own URL by the handlers.
/// - `assets`: already owned by the collection's `settings.stac` assets
///   plus the `"<table>_assets"` sidecar; two sidecars racing to write one
///   map is exactly the "two stores that can disagree" shape this codebase
///   rules out, so the assets sidecar keeps sole ownership.
///
/// A sidecar doc carrying one of these is not an error — it is ignored, and
/// only that member: the rest of the document still merges. Refusing the
/// whole request over one stale key would take a collection's STAC lane
/// down for a data problem the operator can fix out-of-band.
const RESERVED_ITEM_MEMBERS: &[&str] = &[
    "type",
    "id",
    "geometry",
    "bbox",
    "collection",
    "links",
    "assets",
    "stac_version",
];

/// Merges one `"<table>_stac"` sidecar document into the Item
/// [`to_stac_item`] has just derived.
///
/// ## Precedence: the sidecar wins
///
/// On a key present in both, the sidecar's value replaces the feature's.
/// That is the whole point of the sidecar's existence: it holds
/// STAC-specific per-item metadata precisely *because* the operator does
/// not want it in the primary table, so a key written there is a deliberate
/// statement that the STAC view of that field differs from the feature's
/// own. Losing to the feature would make the sidecar unable to correct
/// anything — it could only add — and "add-only" is not a rule an operator
/// can reason about when a column is later added to the primary table under
/// the same name.
///
/// The rule is safe in exactly one direction, and that is deliberate: the
/// Features lane never reads this table (`Router::resolve_stac_metadata` is
/// called from the STAC handlers alone), so a sidecar override can never
/// change what OGC API Features serves for the same row. The two lanes
/// disagreeing about `properties.foo` is the feature being asked for, not a
/// bug.
///
/// ## Shape: one level, not a deep merge
///
/// - `properties` merges key by key into the derived properties, so a
///   sidecar naming one key leaves every other feature property intact.
///   Nested objects under a key are replaced wholesale, never merged
///   recursively: a half-merged nested object is a value neither side ever
///   wrote, and no STAC extension's semantics survive that.
/// - Every other top-level member (`stac_extensions`, and any future
///   member) is set verbatim, except the reserved ones — see
///   [`RESERVED_ITEM_MEMBERS`].
/// - `properties.datetime` is merged like any other property, and therefore
///   overrides the value [`to_stac_item`] just derived from the collection's
///   datetime column. That is the sidecar's most useful single job: a
///   collection with no datetime column at all can carry an honest
///   per-item `datetime` (or a real `start_datetime`/`end_datetime` pair)
///   here instead of the documented `null` this crate would otherwise have
///   to serve.
///
/// A sidecar doc that is not a JSON object merges nothing. The PostGIS
/// driver already refuses that row by name (`MalformedStacRow`) before it
/// ever reaches this function, so this arm only guards a future source with
/// a laxer store.
fn merge_sidecar_doc(map: &mut serde_json::Map<String, Value>, doc: &Value) {
    let Value::Object(doc) = doc else {
        return;
    };
    for (key, value) in doc {
        if key == "properties" {
            let Value::Object(sidecar_properties) = value else {
                continue;
            };
            let properties = map
                .entry("properties".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(properties) = properties {
                for (property, property_value) in sidecar_properties {
                    properties.insert(property.clone(), property_value.clone());
                }
            }
            continue;
        }
        if RESERVED_ITEM_MEMBERS.contains(&key.as_str()) {
            continue;
        }
        map.insert(key.clone(), value.clone());
    }
}

/// `[minx, miny, maxx, maxy]` from `geometry`'s own coordinate arrays — no
/// geometry library involved, just a walk of GeoJSON's nested coordinate
/// shape (Point/LineString/Polygon/Multi*/GeometryCollection). `None` for a
/// `null` geometry or one with no numeric coordinates at all.
fn bbox_from_geometry(geometry: &Value) -> Option<[f64; 4]> {
    if geometry.is_null() {
        return None;
    }
    let mut min = [f64::INFINITY, f64::INFINITY];
    let mut max = [f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut found = false;
    walk_geometry(geometry, &mut min, &mut max, &mut found);
    found.then_some([min[0], min[1], max[0], max[1]])
}

/// Recurses into a GeoJSON Geometry object's `coordinates` (or, for a
/// GeometryCollection, its `geometries`), delegating the coordinate-array
/// walk itself to [`walk_coordinates`].
fn walk_geometry(geometry: &Value, min: &mut [f64; 2], max: &mut [f64; 2], found: &mut bool) {
    let Value::Object(object) = geometry else {
        return;
    };
    if let Some(geometries) = object.get("geometries").and_then(Value::as_array) {
        for nested in geometries {
            walk_geometry(nested, min, max, found);
        }
    }
    if let Some(coordinates) = object.get("coordinates") {
        walk_coordinates(coordinates, min, max, found);
    }
}

/// Recurses through nested coordinate arrays until it finds an `[x, y]` (or
/// `[x, y, z]`, `z` ignored — a STAC bbox is always 2D) leaf pair, folding
/// every one it finds into `min`/`max`.
fn walk_coordinates(value: &Value, min: &mut [f64; 2], max: &mut [f64; 2], found: &mut bool) {
    let Value::Array(items) = value else {
        return;
    };
    let is_leaf_pair = items.len() >= 2 && items.iter().take(2).all(Value::is_number);
    if is_leaf_pair {
        let x = items[0].as_f64().unwrap_or(f64::NAN);
        let y = items[1].as_f64().unwrap_or(f64::NAN);
        if x.is_finite() && y.is_finite() {
            min[0] = min[0].min(x);
            min[1] = min[1].min(y);
            max[0] = max[0].max(x);
            max[1] = max[1].max(y);
            *found = true;
        }
        return;
    }
    for item in items {
        walk_coordinates(item, min, max, found);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use tellurion_core::{
        CanonicalCapabilities, CanonicalField, CanonicalStac, ContactDecl, Provenance,
        SpatialExtent, StacProvider,
    };

    /// A `CanonicalDescriptor` fixture with concrete physical identity
    /// fields (matching `descriptor_with_extent`'s pre-`#50` shape) and the
    /// given `extent`/`stac` group — everything else absent.
    fn canonical_with(
        extent: Option<SpatialExtent>,
        stac: Option<CanonicalStac>,
    ) -> CanonicalDescriptor {
        CanonicalDescriptor {
            kind: tellurion_core::CollectionKind::Vector,
            table: Some(CanonicalField {
                value: "demo".to_string(),
                provenance: Provenance::Derived,
            }),
            geometry: Some(CanonicalField {
                value: "geom".to_string(),
                provenance: Provenance::Derived,
            }),
            pk: Some(CanonicalField {
                value: "id".to_string(),
                provenance: Provenance::Derived,
            }),
            datetime: None,
            srid: None,
            projection: None,
            extent,
            row_estimate: None,
            schema: None,
            stac,
            capabilities: CanonicalCapabilities::default(),
            geometry_profile: None,
        }
    }

    #[test]
    fn license_defaults_to_other_when_no_stac_config_is_present() {
        let collection = to_stac_collection(None, "demo", vec![], BTreeMap::new());
        assert_eq!(collection.license, "other");
    }

    #[test]
    fn license_uses_the_configured_value_when_present() {
        let stac = CanonicalStac {
            license: Some("CC-BY-4.0".to_string()),
            keywords: vec![],
            providers: vec![],
            assets: BTreeMap::new(),
            contacts: vec![],
            lineage: None,
        };
        let canonical = canonical_with(None, Some(stac));
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        assert_eq!(collection.license, "CC-BY-4.0");
    }

    #[test]
    fn keywords_and_providers_are_empty_by_default() {
        let collection = to_stac_collection(None, "demo", vec![], BTreeMap::new());
        assert!(collection.keywords.is_empty());
        assert!(collection.providers.is_empty());
    }

    /// Absent stays absent all the way to the wire (`#50`): a collection
    /// with no `stac:` block anywhere in the settings chain — `canonical.stac
    /// == None`, not a `CanonicalStac` with empty fields — serializes with
    /// `keywords`/`providers` omitted entirely, never `[]`. Exercises the
    /// same "no canonical at all" and "canonical present but its `stac`
    /// group absent" cases together, since both must produce identical JSON.
    #[test]
    fn absent_stac_omits_keywords_and_providers_from_the_serialized_json() {
        let no_canonical = to_stac_collection(None, "demo", vec![], BTreeMap::new());
        let canonical = canonical_with(None, None);
        let stac_present_but_empty =
            to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());

        for collection in [&no_canonical, &stac_present_but_empty] {
            let value = serde_json::to_value(collection).unwrap();
            assert!(
                value.get("keywords").is_none(),
                "keywords must be omitted, not an empty array"
            );
            assert!(
                value.get("providers").is_none(),
                "providers must be omitted, not an empty array"
            );
        }
    }

    #[test]
    fn keywords_and_providers_carry_through_from_configured_stac_settings() {
        let stac = CanonicalStac {
            license: None,
            keywords: vec!["imagery".to_string(), "satellite".to_string()],
            providers: vec![StacProvider {
                name: "Example Provider".to_string(),
                roles: vec!["producer".to_string()],
                url: Some("https://example.com".to_string()),
            }],
            assets: BTreeMap::new(),
            contacts: vec![],
            lineage: None,
        };
        let canonical = canonical_with(None, Some(stac));
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        assert_eq!(collection.keywords, vec!["imagery", "satellite"]);
        assert_eq!(collection.providers.len(), 1);
        assert_eq!(collection.providers[0].name, "Example Provider");
    }

    /// `#187`: declared contacts have no home in a STAC Collection and must
    /// not be smuggled into `providers` (or anywhere else in the body) —
    /// see `to_stac_collection`'s own doc for why. This pins the decision:
    /// a collection declaring only contacts serializes exactly like one
    /// declaring nothing at all.
    #[test]
    fn declared_contacts_are_not_projected_into_the_stac_collection() {
        let stac = CanonicalStac {
            license: None,
            keywords: vec![],
            providers: vec![],
            assets: BTreeMap::new(),
            contacts: vec![ContactDecl {
                name: "Ada Lovelace".to_string(),
                organization: Some("Example Org".to_string()),
                email: Some("ada@example.com".to_string()),
                role: Some("pointOfContact".to_string()),
                url: Some("https://example.com/ada".to_string()),
            }],
            lineage: None,
        };
        let canonical = canonical_with(None, Some(stac));
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        assert!(collection.providers.is_empty());

        let json = serde_json::to_string(&collection).unwrap();
        assert!(!json.contains("Ada Lovelace"));
        assert!(!json.contains("ada@example.com"));
        assert!(!json.contains("contact"));

        let bare = canonical_with(None, None);
        let bare = to_stac_collection(Some(&bare), "demo", vec![], BTreeMap::new());
        assert_eq!(json, serde_json::to_string(&bare).unwrap());
    }

    /// Same split for lineage (`#50`, lineage slice) as for contacts above:
    /// STAC has no collection-level lineage slot, so a declared
    /// `stac.lineage` reaches the ISO 19139 projection only and a
    /// collection declaring only lineage serializes exactly like one
    /// declaring nothing at all.
    #[test]
    fn declared_lineage_is_not_projected_into_the_stac_collection() {
        let stac = CanonicalStac {
            license: None,
            keywords: vec![],
            providers: vec![],
            assets: BTreeMap::new(),
            contacts: vec![],
            lineage: Some(tellurion_core::LineageDecl {
                statement: Some("Digitised from the 1:25000 IGM series.".to_string()),
                sources: vec![],
                process_steps: vec![],
            }),
        };
        let canonical = canonical_with(None, Some(stac));
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        let json = serde_json::to_string(&collection).unwrap();
        assert!(!json.contains("lineage"));
        assert!(!json.contains("Digitised from"));

        let bare = canonical_with(None, None);
        let bare = to_stac_collection(Some(&bare), "demo", vec![], BTreeMap::new());
        assert_eq!(json, serde_json::to_string(&bare).unwrap());
    }

    #[test]
    fn extent_mapping_uses_the_descriptors_real_bbox_when_present() {
        let extent = SpatialExtent {
            bbox: [1.0, 2.0, 3.0, 4.0],
        };
        let canonical = canonical_with(Some(extent), None);
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        assert_eq!(collection.extent.spatial.bbox, vec![[1.0, 2.0, 3.0, 4.0]]);
    }

    #[test]
    fn extent_mapping_falls_back_to_the_whole_earth_bbox_when_the_descriptor_has_none() {
        let canonical = canonical_with(None, None);
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        assert_eq!(
            collection.extent.spatial.bbox,
            vec![[-180.0, -90.0, 180.0, 90.0]]
        );
    }

    #[test]
    fn extent_mapping_falls_back_to_the_whole_earth_bbox_when_the_descriptor_itself_is_absent() {
        let collection = to_stac_collection(None, "demo", vec![], BTreeMap::new());
        assert_eq!(
            collection.extent.spatial.bbox,
            vec![[-180.0, -90.0, 180.0, 90.0]]
        );
    }

    #[test]
    fn temporal_extent_is_always_a_fully_open_interval() {
        let extent = SpatialExtent {
            bbox: [1.0, 2.0, 3.0, 4.0],
        };
        let canonical = canonical_with(Some(extent), None);
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        assert_eq!(collection.extent.temporal.interval, vec![[None, None]]);

        let collection_no_descriptor = to_stac_collection(None, "demo", vec![], BTreeMap::new());
        assert_eq!(
            collection_no_descriptor.extent.temporal.interval,
            vec![[None, None]]
        );
    }

    #[test]
    fn id_and_title_echo_the_external_id() {
        let collection = to_stac_collection(None, "my-collection", vec![], BTreeMap::new());
        assert_eq!(collection.id, "my-collection");
        assert_eq!(collection.title, "my-collection");
    }

    #[test]
    fn type_and_stac_version_are_fixed() {
        let collection = to_stac_collection(None, "demo", vec![], BTreeMap::new());
        assert_eq!(collection.type_, "Collection");
        assert_eq!(collection.stac_version, "1.1.0");
    }

    #[test]
    fn assets_are_carried_through_onto_the_collection() {
        let mut assets = BTreeMap::new();
        assets.insert(
            "mvt".to_string(),
            StacAsset {
                href: "/public/tiles/.../{tileMatrix}/{tileRow}/{tileCol}.mvt".to_string(),
                media_type: Some("application/vnd.mapbox-vector-tile".to_string()),
                title: Some("Vector tiles (MVT)".to_string()),
                description: None,
                roles: vec!["data".to_string()],
                templated: true,
            },
        );
        let collection = to_stac_collection(None, "demo", vec![], assets);
        assert!(collection.assets.contains_key("mvt"));
    }

    // -- declared `stac.assets` (`#36` slice 1) ------------------------------

    fn declared_asset(href: &str) -> AssetDecl {
        AssetDecl {
            href: href.to_string(),
            media_type: None,
            title: None,
            roles: vec![],
        }
    }

    #[test]
    fn a_declared_asset_appears_on_the_collection_even_with_no_capability_derived_assets() {
        let mut declared = BTreeMap::new();
        declared.insert(
            "thumbnail".to_string(),
            declared_asset("https://example.com/thumb.png"),
        );
        let stac = CanonicalStac {
            license: None,
            keywords: vec![],
            providers: vec![],
            assets: declared,
            contacts: vec![],
            lineage: None,
        };
        let canonical = canonical_with(None, Some(stac));
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());

        let thumbnail = collection
            .assets
            .get("thumbnail")
            .expect("expected a declared thumbnail asset");
        assert_eq!(thumbnail.href, "https://example.com/thumb.png");
        assert!(!thumbnail.templated, "a declared href is never a template");
    }

    /// `type`/`title`/`roles` are genuinely optional on the STAC Asset
    /// Object — a declared asset that left them unset must serialize with
    /// those keys omitted entirely, never a fabricated `""`/`[]`.
    #[test]
    fn a_declared_asset_with_no_optional_fields_omits_them_from_the_serialized_json() {
        let mut declared = BTreeMap::new();
        declared.insert(
            "doc".to_string(),
            declared_asset("https://example.com/doc.pdf"),
        );
        let stac = CanonicalStac {
            license: None,
            keywords: vec![],
            providers: vec![],
            assets: declared,
            contacts: vec![],
            lineage: None,
        };
        let canonical = canonical_with(None, Some(stac));
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());

        let value = serde_json::to_value(&collection).unwrap();
        let doc = &value["assets"]["doc"];
        assert_eq!(doc["href"], "https://example.com/doc.pdf");
        assert!(doc.get("type").is_none(), "type must be omitted: {doc}");
        assert!(doc.get("title").is_none(), "title must be omitted: {doc}");
        assert!(doc.get("roles").is_none(), "roles must be omitted: {doc}");
    }

    /// A declared asset id colliding with a capability-derived one wins
    /// outright — see `to_stac_collection`'s own doc for why: the operator's
    /// explicit intent beats a generated default, the same
    /// override-beats-derived precedence used elsewhere in this codebase.
    #[test]
    fn a_declared_asset_overrides_a_capability_derived_asset_of_the_same_id() {
        let mut capability_derived = BTreeMap::new();
        capability_derived.insert(
            "mvt".to_string(),
            StacAsset {
                href: "/public/tiles/.../{tileMatrix}/{tileRow}/{tileCol}.mvt".to_string(),
                media_type: Some("application/vnd.mapbox-vector-tile".to_string()),
                title: Some("Vector tiles (MVT)".to_string()),
                description: None,
                roles: vec!["data".to_string()],
                templated: true,
            },
        );
        let mut declared = BTreeMap::new();
        declared.insert(
            "mvt".to_string(),
            AssetDecl {
                href: "https://example.com/custom-mvt".to_string(),
                media_type: None,
                title: Some("Operator-declared override".to_string()),
                roles: vec![],
            },
        );
        let stac = CanonicalStac {
            license: None,
            keywords: vec![],
            providers: vec![],
            assets: declared,
            contacts: vec![],
            lineage: None,
        };
        let canonical = canonical_with(None, Some(stac));
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], capability_derived);

        let mvt = &collection.assets["mvt"];
        assert_eq!(mvt.href, "https://example.com/custom-mvt");
        assert_eq!(mvt.title.as_deref(), Some("Operator-declared override"));
        assert!(!mvt.templated);
    }

    // -- to_stac_item / datetime rule (`#36` slice B) -----------------------

    fn raw_feature(id: &str, properties: Value) -> Value {
        json!({
            "type": "Feature",
            "id": id,
            "geometry": { "type": "Point", "coordinates": [1.5, 2.5] },
            "properties": properties,
        })
    }

    #[test]
    fn item_carries_stac_version_and_collection() {
        let item = to_stac_item(
            raw_feature("a", json!({})),
            "demo",
            None,
            &BTreeMap::new(),
            vec![],
            None,
            None,
        );
        assert_eq!(item["stac_version"], "1.1.0");
        assert_eq!(item["collection"], "demo");
        assert_eq!(item["type"], "Feature");
    }

    #[test]
    fn item_bbox_is_derived_from_a_point_geometry() {
        let item = to_stac_item(
            raw_feature("a", json!({})),
            "demo",
            None,
            &BTreeMap::new(),
            vec![],
            None,
            None,
        );
        assert_eq!(item["bbox"], json!([1.5, 2.5, 1.5, 2.5]));
    }

    #[test]
    fn item_bbox_is_absent_when_geometry_is_null() {
        let feature = json!({ "type": "Feature", "id": "a", "geometry": null, "properties": {} });
        let item = to_stac_item(feature, "demo", None, &BTreeMap::new(), vec![], None, None);
        assert!(item.get("bbox").is_none());
    }

    #[test]
    fn item_bbox_spans_a_polygons_full_coordinate_ring() {
        let feature = json!({
            "type": "Feature",
            "id": "a",
            "geometry": {
                "type": "Polygon",
                "coordinates": [[[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0], [0.0, 0.0]]],
            },
            "properties": {},
        });
        let item = to_stac_item(feature, "demo", None, &BTreeMap::new(), vec![], None, None);
        assert_eq!(item["bbox"], json!([0.0, 0.0, 4.0, 2.0]));
    }

    /// The row's own datetime column has a real, non-null value:
    /// `properties.datetime` is that value verbatim, no start/end pair
    /// needed since a non-null `datetime` never requires one.
    #[test]
    fn datetime_is_sourced_from_the_configured_column_when_present() {
        let feature = raw_feature("a", json!({ "observed_at": "2020-06-01T00:00:00Z" }));
        let item = to_stac_item(
            feature,
            "demo",
            Some("observed_at"),
            &BTreeMap::new(),
            vec![],
            None,
            None,
        );
        assert_eq!(item["properties"]["datetime"], "2020-06-01T00:00:00Z");
        assert!(item["properties"].get("start_datetime").is_none());
        assert!(item["properties"].get("end_datetime").is_none());
    }

    /// No datetime column configured at all for this collection (`decl.datetime
    /// == None`): there is no per-item interval to offer and this slice
    /// refuses to fabricate one, so `datetime` is `null` with no
    /// `start_datetime`/`end_datetime` — the documented honest fallback (see
    /// `to_stac_item`'s own doc comment for the spec citation).
    #[test]
    fn datetime_is_null_with_no_start_end_pair_when_no_column_is_configured() {
        let feature = raw_feature("a", json!({}));
        let item = to_stac_item(feature, "demo", None, &BTreeMap::new(), vec![], None, None);
        assert!(item["properties"]["datetime"].is_null());
        assert!(item["properties"].get("start_datetime").is_none());
        assert!(item["properties"].get("end_datetime").is_none());
    }

    /// The collection has a datetime column, but this specific row's value is
    /// SQL NULL (surfaces as JSON `null` in `properties`) — same honest
    /// fallback as no column at all, not a crash and not a fabricated value.
    #[test]
    fn datetime_is_null_when_the_configured_columns_value_is_null_on_this_row() {
        let feature = raw_feature("a", json!({ "observed_at": null }));
        let item = to_stac_item(
            feature,
            "demo",
            Some("observed_at"),
            &BTreeMap::new(),
            vec![],
            None,
            None,
        );
        assert!(item["properties"]["datetime"].is_null());
    }

    #[test]
    fn item_assets_are_always_present_even_when_empty() {
        let item = to_stac_item(
            raw_feature("a", json!({})),
            "demo",
            None,
            &BTreeMap::new(),
            vec![],
            None,
            None,
        );
        assert_eq!(item["assets"], json!({}));
    }

    #[test]
    fn item_links_are_carried_through_verbatim() {
        let links = vec![Link::new("/x/root", "root", "application/json")];
        let item = to_stac_item(
            raw_feature("a", json!({})),
            "demo",
            None,
            &BTreeMap::new(),
            links,
            None,
            None,
        );
        assert_eq!(item["links"][0]["rel"], "root");
        assert_eq!(item["links"][0]["href"], "/x/root");
    }

    // -- the metadata sidecar merge (`#202`) --------------------------------

    fn item_with_sidecar(properties: Value, sidecar: Value) -> Value {
        to_stac_item(
            raw_feature("a", properties),
            "demo",
            None,
            &BTreeMap::new(),
            vec![Link::new("/x/root", "root", "application/json")],
            Some(&sidecar),
            None,
        )
    }

    /// The documented precedence rule: on a colliding `properties` key the
    /// sidecar's value replaces the feature's, and every non-colliding
    /// feature property survives (a per-key merge, not a replacement of the
    /// whole `properties` object).
    #[test]
    fn sidecar_properties_win_over_the_features_own_and_leave_the_rest_intact() {
        let item = item_with_sidecar(
            json!({ "name": "from-feature", "kept": 1 }),
            json!({ "properties": { "name": "from-sidecar", "added": 2 } }),
        );
        assert_eq!(item["properties"]["name"], "from-sidecar");
        assert_eq!(item["properties"]["kept"], 1);
        assert_eq!(item["properties"]["added"], 2);
    }

    /// A nested object under a merged key is replaced wholesale, never
    /// merged recursively — a half-merged nested value is one neither side
    /// ever wrote.
    #[test]
    fn a_nested_property_object_is_replaced_wholesale_not_deep_merged() {
        let item = item_with_sidecar(
            json!({ "view": { "azimuth": 10, "zenith": 20 } }),
            json!({ "properties": { "view": { "azimuth": 99 } } }),
        );
        assert_eq!(item["properties"]["view"], json!({ "azimuth": 99 }));
    }

    /// `properties.datetime` is merged like any other property, so it
    /// overrides the value derived from the collection's datetime column —
    /// the sidecar's most useful single job for a temporal-less collection.
    #[test]
    fn a_sidecar_datetime_overrides_the_column_derived_one() {
        let item = to_stac_item(
            raw_feature("a", json!({ "observed_at": "2020-06-01T00:00:00Z" })),
            "demo",
            Some("observed_at"),
            &BTreeMap::new(),
            vec![],
            Some(&json!({ "properties": { "datetime": "2021-01-01T00:00:00Z" } })),
            None,
        );
        assert_eq!(item["properties"]["datetime"], "2021-01-01T00:00:00Z");
    }

    /// Every reserved structural member is ignored, one by one, while the
    /// rest of the same document still merges.
    #[test]
    fn reserved_item_members_are_never_settable_from_the_sidecar() {
        let item = item_with_sidecar(
            json!({}),
            json!({
                "type": "NotAFeature",
                "id": "hijacked",
                "geometry": { "type": "Point", "coordinates": [99.0, 99.0] },
                "bbox": [9.0, 9.0, 9.0, 9.0],
                "collection": "hijacked",
                "links": [],
                "assets": { "ghost": { "href": "https://example.test/ghost" } },
                "stac_version": "0.0.1",
                "properties": { "merged": true }
            }),
        );
        assert_eq!(item["type"], "Feature");
        assert_eq!(item["id"], "a");
        assert_eq!(item["geometry"]["coordinates"], json!([1.5, 2.5]));
        assert_eq!(item["bbox"], json!([1.5, 2.5, 1.5, 2.5]));
        assert_eq!(item["collection"], "demo");
        assert_eq!(item["links"][0]["rel"], "root");
        assert_eq!(item["assets"], json!({}));
        assert_eq!(item["stac_version"], "1.1.0");
        assert_eq!(item["properties"]["merged"], true);
    }

    /// Any non-reserved top-level member is set verbatim — `stac_extensions`
    /// today, anything the spec grows tomorrow without a change here.
    #[test]
    fn a_non_reserved_top_level_member_is_set_verbatim() {
        let item = item_with_sidecar(
            json!({}),
            json!({ "stac_extensions": ["https://example.test/eo.json"] }),
        );
        assert_eq!(
            item["stac_extensions"],
            json!(["https://example.test/eo.json"])
        );
    }

    /// `None` — every collection with no sidecar configured, and every item
    /// with no sidecar row — must reproduce today's document byte for byte.
    #[test]
    fn no_sidecar_produces_the_identical_item() {
        let properties = json!({ "name": "acme" });
        let with_none = to_stac_item(
            raw_feature("a", properties.clone()),
            "demo",
            None,
            &BTreeMap::new(),
            vec![Link::new("/x/root", "root", "application/json")],
            None,
            None,
        );
        // An empty sidecar object is also a no-op, so an "opted in but this
        // page has no rows" collection cannot drift from the opted-out one.
        let with_empty = item_with_sidecar(properties, json!({}));
        assert_eq!(with_none, with_empty);
    }

    /// A stored doc that is not an object merges nothing rather than
    /// panicking — the PostGIS driver already refuses that row by name, so
    /// this only guards a future source with a laxer store.
    #[test]
    fn a_non_object_sidecar_doc_merges_nothing() {
        let item = item_with_sidecar(json!({ "name": "acme" }), json!("not-an-object"));
        assert_eq!(item["properties"]["name"], "acme");
    }

    // -- the projection extension (`#36`) -----------------------------------

    use crate::projection::{derive_projection, PROJECTION_EXTENSION_URI};

    /// Serialized bytes of the exact Item this crate produced BEFORE the
    /// projection extension existed (captured verbatim from this test
    /// module's own fixtures on the pre-change tree, commit `5bff9d7`) —
    /// the campaign's byte-invariance bar: an Item that gains no `proj:`
    /// field must not change by a single byte.
    const PRE_PROJECTION_ITEM: &str = r#"{"assets":{},"bbox":[1.5,2.5,1.5,2.5],"collection":"demo","geometry":{"coordinates":[1.5,2.5],"type":"Point"},"id":"a","links":[{"href":"/x/root","rel":"root","type":"application/json"}],"properties":{"datetime":null,"name":"acme"},"stac_version":"1.1.0","type":"Feature"}"#;

    /// The Collection counterpart of [`PRE_PROJECTION_ITEM`], captured the
    /// same way from the same pre-change tree.
    const PRE_PROJECTION_COLLECTION: &str = r#"{"type":"Collection","stac_version":"1.1.0","id":"demo","title":"demo","description":"STAC Collection for 'demo'.","license":"other","extent":{"spatial":{"bbox":[[-180.0,-90.0,180.0,90.0]]},"temporal":{"interval":[[null,null]]}},"links":[{"href":"/x/root","rel":"root","type":"application/json"}]}"#;

    /// Runs `f` under a scoped `tracing` subscriber and returns everything
    /// it logged — the seam that lets the disagreement-log tests assert on
    /// the actual signal an operator would see, not a side-channel flag.
    fn capture_logs<F: FnOnce()>(f: F) -> String {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl Write for Buffer {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
            type Writer = Buffer;
            fn make_writer(&'a self) -> Buffer {
                self.clone()
            }
        }

        let buffer = Buffer(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buffer.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    fn item_with_projection(collection: &str, sidecar: Option<Value>) -> Value {
        to_stac_item(
            raw_feature("a", json!({ "name": "acme" })),
            collection,
            None,
            &BTreeMap::new(),
            vec![Link::new("/x/root", "root", "application/json")],
            sidecar.as_ref(),
            derive_projection(None, Some(4326)).as_ref(),
        )
    }

    /// The byte-invariance gate, Item half: a collection with no projection
    /// knowledge anywhere (`derive_projection` -> `None`) serializes
    /// byte-for-byte what this crate served before the extension existed —
    /// no `proj:` key, no `stac_extensions`, nothing.
    #[test]
    fn an_item_with_no_projection_knowledge_is_byte_identical_to_the_pre_extension_document() {
        let item = to_stac_item(
            raw_feature("a", json!({ "name": "acme" })),
            "demo",
            None,
            &BTreeMap::new(),
            vec![Link::new("/x/root", "root", "application/json")],
            None,
            derive_projection(None, None).as_ref(),
        );
        assert_eq!(serde_json::to_string(&item).unwrap(), PRE_PROJECTION_ITEM);
    }

    /// The byte-invariance gate, Collection half — same rule, same captured
    /// pre-change bytes, exercised through the canonical-descriptor path a
    /// real request takes (a vector collection's `srid` is deliberately NOT
    /// collection-level projection knowledge — see `to_stac_collection`'s
    /// own comment — so it is pinned here alongside the no-knowledge case).
    #[test]
    fn a_collection_without_driver_projection_facts_is_byte_identical_to_the_pre_extension_document(
    ) {
        let no_canonical = to_stac_collection(
            None,
            "demo",
            vec![Link::new("/x/root", "root", "application/json")],
            BTreeMap::new(),
        );
        assert_eq!(
            serde_json::to_string(&no_canonical).unwrap(),
            PRE_PROJECTION_COLLECTION
        );

        let mut vector = canonical_with(None, None);
        vector.srid = Some(4326);
        let vector = to_stac_collection(
            Some(&vector),
            "demo",
            vec![Link::new("/x/root", "root", "application/json")],
            BTreeMap::new(),
        );
        assert_eq!(
            serde_json::to_string(&vector).unwrap(),
            PRE_PROJECTION_COLLECTION,
            "a vector collection's srid belongs on its Items, never on Collection summaries"
        );
    }

    /// Derived knowledge is emitted, and ONLY derived knowledge: an
    /// epsg-only collection carries `proj:epsg` and no `proj:transform`/
    /// `proj:shape` under any spelling (no key, not a null, and certainly
    /// not an identity transform), with the extension URI declared because
    /// one field was genuinely emitted.
    #[test]
    fn derived_epsg_is_emitted_with_the_extension_declared_and_nothing_else_invented() {
        let item = item_with_projection("epsg-only-collection", None);
        assert_eq!(item["properties"]["proj:epsg"], json!(4326));
        assert!(item["properties"].get("proj:transform").is_none());
        assert!(item["properties"].get("proj:shape").is_none());
        assert_eq!(item["stac_extensions"], json!([PROJECTION_EXTENSION_URI]));
    }

    /// The `#287` gate: no derived field, no declaration — even when the
    /// operator's own sidecar carries a `proj:` property. That sidecar
    /// passthrough predates this extension and stays exactly the verbatim
    /// channel it was: the operator owns that document, and this lane only
    /// declares what IT emits.
    #[test]
    fn no_derived_field_means_no_extension_declaration() {
        let item = to_stac_item(
            raw_feature("a", json!({})),
            "demo",
            None,
            &BTreeMap::new(),
            vec![],
            Some(&json!({ "properties": { "proj:epsg": 3857 } })),
            derive_projection(None, None).as_ref(),
        );
        assert_eq!(
            item["properties"]["proj:epsg"],
            json!(3857),
            "the sidecar passthrough itself is untouched"
        );
        assert!(
            item.get("stac_extensions").is_none(),
            "declaring an extension this lane emitted nothing for is the #287 defect"
        );
    }

    /// The `#36` decision's override contract, all three clauses at once:
    /// a disagreeing sidecar override WINS (that is the point of the
    /// option) and is LOGGED — naming the collection, the field, the
    /// derived value and the override — exactly once per collection per
    /// field, not per request.
    #[test]
    fn a_disagreeing_override_wins_and_is_logged_once_per_collection() {
        let sidecar = json!({ "properties": { "proj:epsg": 3857 } });
        let mut first = Value::Null;
        let logs = capture_logs(|| {
            first = item_with_projection("disagree-collection", Some(sidecar.clone()));
            // Second materialization of the same collection: the override
            // still wins, but the log already fired.
            item_with_projection("disagree-collection", Some(sidecar.clone()));
        });
        assert_eq!(
            first["properties"]["proj:epsg"],
            json!(3857),
            "the operator's override must win over the derived value"
        );
        assert_eq!(
            logs.matches("disagrees with the driver-derived value")
                .count(),
            1,
            "one collection, one field, one log — not one per request: {logs}"
        );
        for named in ["disagree-collection", "proj:epsg", "4326", "3857"] {
            assert!(logs.contains(named), "the log must name {named}: {logs}");
        }
    }

    /// An agreeing override is indistinguishable from no override and logs
    /// nothing — the log stays a signal, not noise.
    #[test]
    fn an_agreeing_override_logs_nothing() {
        let logs = capture_logs(|| {
            let item = item_with_projection(
                "agree-collection",
                Some(json!({ "properties": { "proj:epsg": 4326 } })),
            );
            assert_eq!(item["properties"]["proj:epsg"], json!(4326));
        });
        assert!(
            !logs.contains("disagrees"),
            "an agreeing override must not be logged: {logs}"
        );
    }

    /// An override for a field the driver could NOT derive is pure
    /// gap-filling: served verbatim, nothing to disagree with, silent.
    #[test]
    fn a_gap_filling_override_for_an_underived_field_is_silent() {
        let logs = capture_logs(|| {
            let item = item_with_projection(
                "gap-fill-collection",
                Some(json!({ "properties": { "proj:shape": [512, 1024] } })),
            );
            assert_eq!(item["properties"]["proj:shape"], json!([512, 1024]));
            assert_eq!(item["properties"]["proj:epsg"], json!(4326));
        });
        assert!(
            !logs.contains("disagrees"),
            "gap-filling has nothing to disagree with: {logs}"
        );
    }

    /// A sidecar-supplied `stac_extensions` array survives (the `#202`
    /// passthrough contract) and the projection URI joins it exactly once —
    /// appended when missing, never duplicated when the operator already
    /// declared it.
    #[test]
    fn the_extension_uri_joins_a_sidecar_supplied_stac_extensions_without_duplication() {
        let item = item_with_projection(
            "sidecar-extensions-collection",
            Some(json!({ "stac_extensions": ["https://example.test/eo.json"] })),
        );
        assert_eq!(
            item["stac_extensions"],
            json!(["https://example.test/eo.json", PROJECTION_EXTENSION_URI])
        );

        let item = item_with_projection(
            "sidecar-extensions-collection",
            Some(json!({ "stac_extensions": [PROJECTION_EXTENSION_URI] })),
        );
        assert_eq!(item["stac_extensions"], json!([PROJECTION_EXTENSION_URI]));
    }

    /// Collection half of the emission (`#36`): a raster-backed collection
    /// whose driver read real georeferencing (COG/Zarr —
    /// `CatalogSource::projection`) surfaces it as `summaries` (the
    /// Collection spec's own home for Item Properties fields) with the
    /// extension declared — its only STAC surface, since a raster driver
    /// implements no `FeatureSource` and so has no Items.
    #[test]
    fn driver_projection_facts_surface_as_collection_summaries() {
        let mut canonical = canonical_with(None, None);
        canonical.projection = Some(tellurion_core::ProjectionFacts {
            epsg: Some(4326),
            transform: Some([0.01, 0.0, -1.28, 0.0, -0.01, 1.28]),
            shape: Some([512, 1024]),
        });
        let collection = to_stac_collection(Some(&canonical), "demo", vec![], BTreeMap::new());
        assert_eq!(
            collection.stac_extensions,
            vec![PROJECTION_EXTENSION_URI.to_string()]
        );
        let value = serde_json::to_value(&collection).unwrap();
        assert_eq!(value["summaries"]["proj:epsg"], json!([4326]));
        assert_eq!(
            value["summaries"]["proj:transform"],
            json!([[0.01, 0.0, -1.28, 0.0, -0.01, 1.28]])
        );
        assert_eq!(value["summaries"]["proj:shape"], json!([[512, 1024]]));
    }
}
