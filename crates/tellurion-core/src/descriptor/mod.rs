//! Derived collection descriptors (`#19`): the effective physical shape of a
//! collection — table, geometry column, primary key, spatial extent, row
//! estimate, attribute schema, and datetime column — resolved from the
//! driver's `CatalogSource` and cached by `Router` with a TTL. Precedence is
//! override (`CollectionDecl`) > derived (`PhysicalCollection`) > error; an
//! override that contradicts what the backend reports is honored but never
//! silent — `merge_descriptor` logs the divergence. Nothing computed here is
//! ever written back into config; see the driver-contract design doc,
//! section 2.
//!
//! [`heuristics`] builds on this: derived per-zoom tile serving parameters
//! (feature caps, simplification tolerance, buffer) computed from a
//! descriptor's `row_estimate`.

pub mod canonical;
pub mod heuristics;

use std::time::Instant;

use crate::catalog::{AttributeColumn, PhysicalCollection, ProjectionFacts, SpatialExtent};
use crate::config::{CollectionDecl, CollectionKind, PropertyType, SchemaDecl};
use crate::error::{Error, Result};

/// The effective physical shape of a collection, after precedence. `table`
/// is always concrete (every driver, table-shaped or not, resolves a target
/// name — see `target_table`). `geometry`/`pk`/`datetime` are `None` when
/// neither an override nor a derived value exists — legitimate for a driver
/// with no table-shaped concept of either (PMTiles: no geometry column, no
/// primary key, `#20`) or a collection with no single obvious datetime
/// column, not just an unresolved config. A collection that actually needs
/// `geometry`/`pk` concrete (anything routed through a `FeatureSource`) is
/// enforced separately by [`require_feature_capable`], not by this struct's
/// shape — see `Router::validate_catalog`. `row_estimate`/`attributes` are
/// always backend-derived (there is no config field for either) and purely
/// additive richness (`#19`) — nothing enforces their presence.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionDescriptor {
    pub table: String,
    pub geometry: Option<String>,
    pub pk: Option<String>,
    /// Datetime column, override > derived (single timestamp/timestamptz/
    /// date column — see `CatalogSource::temporal_column`) — same precedence
    /// rule as `geometry`/`pk`, resolved by the same [`merge_descriptor`]
    /// call.
    pub datetime: Option<String>,
    /// This collection's native storage SRID (`PhysicalCollection::srid`,
    /// filtered to `None` for the PostGIS "unset" sentinel `0` — same rule
    /// `extent` derivation already applies), backend-derived only, no
    /// override concept. Feeds OGC API Features Part 2 CRS support: a
    /// collection's `storageCrs`/`crs` metadata (`tellurion_core::crs`) and
    /// `tellurion-postgis::sql`'s `ST_Transform`/`ST_FlipCoordinates`
    /// reprojection both derive from this field alone.
    pub srid: Option<i32>,
    pub extent: Option<SpatialExtent>,
    /// Cheap row-count estimate (PostGIS: `pg_class.reltuples`). Feeds
    /// [`heuristics::effective_feature_cap`]; `None` when the backend
    /// couldn't estimate.
    pub row_estimate: Option<u64>,
    /// This collection's backend-known projection facts
    /// (`CatalogSource::projection`, `#36` — STAC `projection` extension),
    /// backend-derived only, no override concept — same plain carry-through
    /// shape as `srid`/`geometry_type`. `None` for every driver that never
    /// overrides the accessor; see `ProjectionFacts` for the per-field
    /// omission contract.
    pub projection: Option<ProjectionFacts>,
    /// Non-geometry columns, name plus broad type. `None` when the backend
    /// couldn't introspect columns at all; `Some(vec![])` is a legitimate
    /// answer for a collection with no non-geometry columns.
    pub attributes: Option<Vec<AttributeColumn>>,
    /// This collection's geometry column type exactly as the backend reports
    /// it (`PhysicalCollection::geometry_type`, e.g. `"POLYGON"`,
    /// `"POLYHEDRALSURFACE"`), backend-derived only, no override concept —
    /// same plain-carry-through shape as `srid`. `#70`: `Router::
    /// resolve_volume` reads this to decide, per collection, whether a
    /// driver-wide `VolumeSource` answer actually fits this collection's own
    /// geometry column (see `is_volume_capable_geometry_type`).
    pub geometry_type: Option<String>,
}

/// Backend-derived fields `merge_descriptor` cannot compute itself — each
/// costs a `CatalogSource` call, so callers gather them once
/// (`Router::validate_catalog`/`resolved_descriptor`) rather than
/// `merge_descriptor` reaching out on its own. Grouped into one struct so a
/// new derived field (`#19`'s richer descriptor) touches one call site, not
/// every `merge_descriptor` caller's argument list.
pub struct DerivedFields {
    pub extent: Option<SpatialExtent>,
    pub row_estimate: Option<u64>,
    pub attributes: Option<Vec<AttributeColumn>>,
    /// The backend's own projection facts (`CatalogSource::projection`,
    /// `#36`) — carried through untouched, no override concept, same as
    /// `extent`/`row_estimate` above.
    pub projection: Option<ProjectionFacts>,
    /// The backend's single-candidate temporal column, if any — the
    /// *derived* half of `datetime`'s override > derived precedence, not the
    /// resolved value itself (that's `CollectionDescriptor::datetime`, which
    /// `merge_descriptor` computes from this plus `decl.datetime`).
    pub temporal_column: Option<String>,
}

/// A resolved descriptor, or (registry scale-out, `#42`) a failed
/// derivation's `Error::Config` message, plus when it was computed — so
/// `Router` can decide whether the entry is still within TTL before serving
/// it again, and so a permanently misconfigured collection costs one
/// backend round trip rather than one per request in `registry.validation:
/// lazy` mode (see `config::RegistryValidationMode`). Only a `Config`
/// failure is ever cached this way; `Router::resolved_descriptor` never
/// caches a transient error (a storage outage, a timeout), matching
/// `TileCache::get_or_populate`'s own "a failed populate never poisons the
/// key" rule — a transient failure must keep retrying, not calcify into a
/// standing verdict.
#[derive(Debug, Clone)]
pub struct CachedDescriptor {
    pub outcome: std::result::Result<CollectionDescriptor, String>,
    pub computed_at: Instant,
}

impl CachedDescriptor {
    pub fn is_stale(&self, ttl: std::time::Duration) -> bool {
        self.computed_at.elapsed() >= ttl
    }
}

/// The physical target name to look up in the backend's catalog: the
/// operator's override if declared, else the collection id by convention —
/// the one part of a `CollectionDescriptor` derivable without any I/O.
pub fn target_table(decl: &CollectionDecl) -> &str {
    decl.table.as_deref().unwrap_or(decl.id.as_str())
}

/// Merges a collection's declared overrides with what the backend reports
/// for `physical`/`derived`, applying override > derived precedence to
/// `geometry`/`pk`/`datetime` and logging a warning wherever a present
/// override contradicts the backend. `extent`/`row_estimate`/`attributes`
/// are always backend-derived (there is no config field for any of them)
/// and passed straight through.
///
/// Never fails: a field with neither an override nor a derived value simply
/// resolves to `None` (`#20`) — some drivers (PMTiles) have no table-shaped
/// concept of geometry/pk at all, and a collection can genuinely have no
/// single obvious datetime column. A collection that needs `geometry`/`pk`
/// concrete enforces that itself via [`require_feature_capable`].
pub fn merge_descriptor(
    decl: &CollectionDecl,
    physical: &PhysicalCollection,
    derived: DerivedFields,
) -> CollectionDescriptor {
    let geometry = resolve_optional_field(
        &decl.id,
        "geometry",
        decl.geometry.as_deref(),
        physical.geometry_column.as_deref(),
    );
    let pk = resolve_optional_field(
        &decl.id,
        "pk",
        decl.pk.as_deref(),
        physical.primary_key.as_deref(),
    );
    let datetime = resolve_optional_field(
        &decl.id,
        "datetime",
        decl.datetime.as_deref(),
        derived.temporal_column.as_deref(),
    );

    CollectionDescriptor {
        table: target_table(decl).to_string(),
        geometry,
        pk,
        datetime,
        srid: physical.srid.filter(|&srid| srid > 0),
        extent: derived.extent,
        row_estimate: derived.row_estimate,
        attributes: derived.attributes,
        geometry_type: physical.geometry_type.clone(),
        projection: derived.projection,
    }
}

/// Fails when `descriptor` is missing `geometry` or `pk` (checked in that
/// order, matching the field order `merge_descriptor` resolves them in).
/// Only meaningful for a collection whose anchor storage actually implements
/// `FeatureSource` — `Router::validate_catalog`/`resolved_descriptor` call
/// this conditionally on that capability, since a tiles-only archive driver
/// (PMTiles, `#20`) has neither concept and must never be required to pass
/// it. Kept separate from `merge_descriptor` so a capability-blind caller
/// (anything only deriving `extent`, e.g. `Router::collection_descriptor`)
/// never trips it by accident.
pub fn require_feature_capable(
    collection_id: &str,
    descriptor: &CollectionDescriptor,
    kind: CollectionKind,
) -> Result<()> {
    // `#192`: a record collection has no geometry story to require. OGC API
    // — Records — Part 1: Core (OGC 20-004r1, approved 1.0) makes `geometry`
    // an OPTIONAL core property of a record (Table 9 — "Can be null if there
    // is no associated spatial extent") and Permission 4
    // (`/per/record-core/geometry`) says only a specific community of
    // interest MAY make it mandatory. Demanding a geometry column of a
    // `kind: record` collection would therefore refuse at boot exactly the
    // collections the Records lane exists to serve.
    //
    // `pk` below stays required for every kind, records included:
    // Requirement 1 (`/req/record-core/mandatory-properties-record`, clause
    // B) says a record's `id` "cannot be NULL or the empty string", and the
    // pk column is where that id comes from.
    if kind.has_geometry() && descriptor.geometry.is_none() {
        return Err(Error::Config(format!(
            "collection '{collection_id}': 'geometry' is not declared and the backend does not report one — set it explicitly"
        )));
    }
    if descriptor.pk.is_none() {
        return Err(Error::Config(format!(
            "collection '{collection_id}': 'pk' is not declared and the backend does not report one — set it explicitly"
        )));
    }
    Ok(())
}

/// Reconciles a collection's declared [`SchemaDecl`] (`#44`) against its
/// derived `descriptor`: every declared property must name a real column in
/// `descriptor.attributes` whose classified [`PropertyType`]
/// (`PropertyType::from_sql_type`) agrees with the declaration. Fails fast,
/// naming the collection, the property, and declared vs. actual — the same
/// discipline [`require_feature_capable`] applies to `geometry`/`pk`. Called
/// from `Router`'s `merge_and_enforce`, which already runs at
/// boot (`validate_catalog`) and lazily on first touch/TTL expiry
/// (`resolved_descriptor`) — no separate validation phase.
///
/// A declared property never names the collection's geometry column: that
/// column has no `PropertyType` (see `PropertyType`'s own doc comment) and
/// is already covered by `filter::validate`'s dedicated `S_INTERSECTS`
/// handling, so declaring it here is itself the mismatch this function
/// reports.
pub fn reconcile_schema(
    collection_id: &str,
    schema: &SchemaDecl,
    descriptor: &CollectionDescriptor,
) -> Result<()> {
    for property in &schema.properties {
        if descriptor.geometry.as_deref() == Some(property.name.as_str()) {
            return Err(Error::Config(format!(
                "collection '{collection_id}': schema declares property '{}' as '{}', but that is this collection's geometry column — geometry has no place in a declared schema's flat property model",
                property.name,
                property.type_.as_str()
            )));
        }

        let actual = descriptor
            .attributes
            .as_ref()
            .and_then(|attrs| attrs.iter().find(|a| a.name == property.name));
        let Some(actual) = actual else {
            return Err(Error::Config(format!(
                "collection '{collection_id}': schema declares property '{}' but the backend reports no such column",
                property.name
            )));
        };

        let actual_type = PropertyType::from_sql_type(&actual.sql_type);
        if actual_type != property.type_ {
            return Err(Error::Config(format!(
                "collection '{collection_id}': schema declares property '{}' as '{}', but backend column '{}' (SQL type '{}') classifies as '{}'",
                property.name,
                property.type_.as_str(),
                actual.name,
                actual.sql_type,
                actual_type.as_str()
            )));
        }
    }
    Ok(())
}

/// Reconciles a collection's declared `CollectionDecl::modified_column`
/// (OGC API Features — Part 4, 20-002r1 draft, Optimistic Locking:
/// Timestamps class, `#107`) against its derived `descriptor`, when one is
/// declared at all — `None` (the default, no declared source) is this
/// collection's honest "no Timestamps class" answer and is never itself an
/// error here; see `CollectionDecl::modified_column`'s own doc. When a
/// column IS declared, the same discipline [`reconcile_schema`] applies to
/// a schema property applies to it: it must name a real column
/// `descriptor.attributes` reports, that column must classify as
/// `PropertyType::DateTime` (a text/integer column would make
/// `Last-Modified`/`If-Unmodified-Since` compare garbage against an
/// HTTP-date), and it must not be this collection's own geometry column.
/// One check beyond what `reconcile_schema` needs: a collection with a
/// *closed* declared schema (`SchemaDecl::additional_properties: false`)
/// that doesn't list `modified_column` among its own properties would
/// silently exclude that column from `FeatureSource::item`'s served
/// `properties` (`reconcile_schema`'s own merge rule) — this module's
/// `Last-Modified` machinery has nothing to read a value out of in that
/// case, so it fails fast at boot instead of silently never emitting the
/// header despite the operator believing they declared a working source.
pub fn reconcile_modified_column(
    collection_id: &str,
    modified_column: &str,
    schema: Option<&SchemaDecl>,
    descriptor: &CollectionDescriptor,
) -> Result<()> {
    if descriptor.geometry.as_deref() == Some(modified_column) {
        return Err(Error::Config(format!(
            "collection '{collection_id}': modified_column names '{modified_column}', but that is this collection's geometry column"
        )));
    }

    let actual = descriptor
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.iter().find(|a| a.name == modified_column));
    let Some(actual) = actual else {
        return Err(Error::Config(format!(
            "collection '{collection_id}': modified_column names '{modified_column}' but the backend reports no such column"
        )));
    };

    let actual_type = PropertyType::from_sql_type(&actual.sql_type);
    if actual_type != PropertyType::DateTime {
        return Err(Error::Config(format!(
            "collection '{collection_id}': modified_column '{modified_column}' (SQL type '{}', classifies as '{}') must be a timestamp column",
            actual.sql_type,
            actual_type.as_str()
        )));
    }

    if let Some(schema) = schema {
        if !schema.additional_properties
            && !schema.properties.iter().any(|p| p.name == modified_column)
        {
            return Err(Error::Config(format!(
                "collection '{collection_id}': modified_column '{modified_column}' is not among this collection's closed schema's declared properties, so it would never reach a served feature's 'properties'"
            )));
        }
    }

    Ok(())
}

/// Every [`PropertyType`] this slice (`#85`) can project into a vector-tile
/// feature's attribute table — scalar columns only, per the issue's own
/// "First slice" scope. `Date`/`DateTime` are deliberately excluded: MVT has
/// no native temporal attribute type, and silently text-ifying one would be
/// the kind of lossy, capability-dishonest fallback `reconcile_tile_properties`
/// exists to refuse instead of accepting quietly. A later slice can widen
/// this set; nothing about the config shape or the reconciliation call site
/// needs to change to do that.
fn is_projectable_tile_property_type(type_: PropertyType) -> bool {
    matches!(
        type_,
        PropertyType::String | PropertyType::Integer | PropertyType::Number | PropertyType::Boolean
    )
}

/// Reconciles a collection's resolved vector-tile property allowlist (`#85`,
/// `settings.tile_properties` resolved through the platform -> tenant ->
/// catalog -> collection chain — see `settings::resolve_effective_settings`)
/// against its derived `descriptor`: every allowlisted name must name a real
/// column in `descriptor.attributes`, classify as a scalar
/// [`PropertyType`] this slice actually projects (see
/// [`is_projectable_tile_property_type`]), and not collide with either
/// reserved MVT attribute name a `TileSource` encoder already emits
/// unconditionally — `id` (the primary key, always written under that
/// literal name regardless of the pk column's own physical name; see
/// `tellurion-postgis::sql::build_mvt_plan`/`tellurion-geopackage::driver::
/// encode_mvt_feature`) or the geometry column itself (no place in a flat
/// property model — the same rule [`reconcile_schema`] already applies to a
/// declared `SchemaDecl`). Fails fast, naming the collection and the
/// offending property — same discipline `reconcile_schema`/
/// `require_feature_capable` apply. Called from `Router`'s
/// `merge_and_enforce`, which already runs at boot (`validate_catalog`) and
/// lazily on first touch/TTL expiry (`resolved_descriptor`) — no separate
/// validation phase.
pub fn reconcile_tile_properties(
    collection_id: &str,
    tile_properties: &[String],
    descriptor: &CollectionDescriptor,
) -> Result<()> {
    for property in tile_properties {
        if property == "id" {
            return Err(Error::Config(format!(
                "collection '{collection_id}': tile_properties declares '{property}', but 'id' is the reserved vector-tile property name for this collection's primary key"
            )));
        }
        if descriptor.geometry.as_deref() == Some(property.as_str()) {
            return Err(Error::Config(format!(
                "collection '{collection_id}': tile_properties declares '{property}', but that is this collection's geometry column — geometry has no place in a vector-tile property allowlist"
            )));
        }

        let actual = descriptor
            .attributes
            .as_ref()
            .and_then(|attrs| attrs.iter().find(|a| &a.name == property));
        let Some(actual) = actual else {
            return Err(Error::Config(format!(
                "collection '{collection_id}': tile_properties declares '{property}' but the backend reports no such column"
            )));
        };

        let actual_type = PropertyType::from_sql_type(&actual.sql_type);
        if !is_projectable_tile_property_type(actual_type) {
            return Err(Error::Config(format!(
                "collection '{collection_id}': tile_properties declares '{property}' (SQL type '{}', classifies as '{}'), but only string/integer/number/boolean columns can be projected into a vector tile in this slice",
                actual.sql_type,
                actual_type.as_str()
            )));
        }
    }
    Ok(())
}

/// `true` when an override is present, a derived value is also present, and
/// the two disagree — the case `merge_descriptor` must never resolve
/// silently. Split out from `resolve_field` so this precedence rule is
/// testable without a tracing subscriber capturing log output.
fn diverges(overridden: Option<&str>, derived: Option<&str>) -> bool {
    matches!((overridden, derived), (Some(a), Some(b)) if a != b)
}

/// One field's override > derived resolution. An override is always
/// honored, even when it contradicts the backend, but the contradiction is
/// never silent — see [`diverges`]. `None` when neither is present; whether
/// that's acceptable is the caller's call (see [`require_feature_capable`]).
fn resolve_optional_field(
    collection_id: &str,
    field: &str,
    overridden: Option<&str>,
    derived: Option<&str>,
) -> Option<String> {
    if diverges(overridden, derived) {
        tracing::warn!(
            collection = collection_id,
            field,
            override_value = overridden.unwrap_or_default(),
            derived_value = derived.unwrap_or_default(),
            "collection override contradicts what the backend reports; the override wins"
        );
    }

    overridden.or(derived).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(table: Option<&str>, geometry: Option<&str>, pk: Option<&str>) -> CollectionDecl {
        decl_with_datetime(table, geometry, pk, None)
    }

    fn decl_with_datetime(
        table: Option<&str>,
        geometry: Option<&str>,
        pk: Option<&str>,
        datetime: Option<&str>,
    ) -> CollectionDecl {
        let mut yaml = "id: demo\ncatalog: default\nstorage: main\n".to_string();
        if let Some(table) = table {
            yaml.push_str(&format!("table: {table}\n"));
        }
        if let Some(geometry) = geometry {
            yaml.push_str(&format!("geometry: {geometry}\n"));
        }
        if let Some(pk) = pk {
            yaml.push_str(&format!("pk: {pk}\n"));
        }
        if let Some(datetime) = datetime {
            yaml.push_str(&format!("datetime: {datetime}\n"));
        }
        serde_yaml::from_str(&yaml).unwrap()
    }

    fn physical(geometry_column: Option<&str>, primary_key: Option<&str>) -> PhysicalCollection {
        PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: geometry_column.map(str::to_string),
            primary_key: primary_key.map(str::to_string),
            srid: Some(4326),
            geometry_type: None,
        }
    }

    /// Empty derived-fields fixture: no extent/row_estimate/attributes/
    /// temporal_column, the shape most `merge_descriptor` tests need since
    /// they only exercise `geometry`/`pk`/`table` precedence.
    fn no_derived() -> DerivedFields {
        DerivedFields {
            extent: None,
            row_estimate: None,
            attributes: None,
            temporal_column: None,
            projection: None,
        }
    }

    /// `#36`: `projection` has no override concept, same as `srid`/
    /// `geometry_type` — carried through from `DerivedFields` unchanged,
    /// and `None` (the every-existing-driver default) stays `None`.
    #[test]
    fn merge_descriptor_carries_the_projection_facts_through_unchanged() {
        let decl = decl(None, Some("geom"), Some("id"));
        let physical = physical(Some("geom"), Some("id"));
        let facts = ProjectionFacts {
            epsg: Some(4326),
            transform: Some([0.01, 0.0, -1.28, 0.0, -0.01, 1.28]),
            shape: Some([256, 256]),
        };
        let descriptor = merge_descriptor(
            &decl,
            &physical,
            DerivedFields {
                projection: Some(facts),
                ..no_derived()
            },
        );
        assert_eq!(descriptor.projection, Some(facts));

        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(descriptor.projection, None);
    }

    #[test]
    fn target_table_uses_the_override_when_present() {
        let decl = decl(Some("physical_table"), Some("geom"), Some("id"));
        assert_eq!(target_table(&decl), "physical_table");
    }

    #[test]
    fn target_table_falls_back_to_the_collection_id_when_omitted() {
        let decl = decl(None, Some("geom"), Some("id"));
        assert_eq!(target_table(&decl), "demo");
    }

    #[test]
    fn merge_descriptor_fills_derived_values_when_overrides_are_absent() {
        let decl = decl(None, None, None);
        let physical = physical(Some("geom"), Some("id"));
        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(descriptor.table, "demo");
        assert_eq!(descriptor.geometry.as_deref(), Some("geom"));
        assert_eq!(descriptor.pk.as_deref(), Some("id"));
    }

    #[test]
    fn merge_descriptor_prefers_the_override_over_the_derived_value() {
        let decl = decl(None, Some("the_geom"), Some("gid"));
        let physical = physical(Some("geom"), Some("id"));
        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(
            descriptor.geometry.as_deref(),
            Some("the_geom"),
            "override must win even though it diverges from the backend"
        );
        assert_eq!(descriptor.pk.as_deref(), Some("gid"));
    }

    /// `#20`: a field with neither an override nor a derived value used to be
    /// a hard error (any driver was assumed table-shaped). Now it resolves to
    /// `None` — legitimate for a driver like PMTiles that has no geometry
    /// column or primary key at all; whether that's acceptable is enforced
    /// separately by `require_feature_capable`, not by `merge_descriptor`.
    #[test]
    fn merge_descriptor_leaves_a_field_none_when_neither_override_nor_derived_value_is_present() {
        let decl = decl(None, None, Some("id"));
        let physical = physical(None, Some("id"));
        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(descriptor.geometry, None);
        assert_eq!(descriptor.pk.as_deref(), Some("id"));
    }

    #[test]
    fn merge_descriptor_carries_the_extent_through_unchanged() {
        let decl = decl(None, Some("geom"), Some("id"));
        let physical = physical(Some("geom"), Some("id"));
        let extent = SpatialExtent {
            bbox: [1.0, 2.0, 3.0, 4.0],
        };
        let descriptor = merge_descriptor(
            &decl,
            &physical,
            DerivedFields {
                extent: Some(extent),
                ..no_derived()
            },
        );
        assert_eq!(descriptor.extent, Some(extent));
    }

    #[test]
    fn merge_descriptor_carries_the_physical_srid_through() {
        let decl = decl(None, Some("geom"), Some("id"));
        let physical = physical(Some("geom"), Some("id"));
        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(
            descriptor.srid,
            Some(4326),
            "fixture's physical() reports srid 4326"
        );
    }

    /// SRID `0` is PostGIS's "unset" sentinel — `merge_descriptor` treats it
    /// exactly like `extent_inner` (`tellurion-postgis`) already does: as if
    /// no SRID were reported at all, not a literal CRS with SRID zero.
    #[test]
    fn merge_descriptor_treats_srid_zero_as_unset() {
        let decl = decl(None, Some("geom"), Some("id"));
        let mut physical = physical(Some("geom"), Some("id"));
        physical.srid = Some(0);
        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(descriptor.srid, None);
    }

    /// `#70`: `geometry_type` has no override concept, same as `srid` —
    /// carried through from `PhysicalCollection::geometry_type` unchanged.
    #[test]
    fn merge_descriptor_carries_the_geometry_type_through() {
        let decl = decl(None, Some("geom"), Some("id"));
        let mut physical = physical(Some("geom"), Some("id"));
        physical.geometry_type = Some("POLYHEDRALSURFACE".to_string());
        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(
            descriptor.geometry_type.as_deref(),
            Some("POLYHEDRALSURFACE")
        );
    }

    #[test]
    fn merge_descriptor_carries_row_estimate_and_attributes_through_unchanged() {
        let decl = decl(None, Some("geom"), Some("id"));
        let physical = physical(Some("geom"), Some("id"));
        let attributes = vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }];
        let descriptor = merge_descriptor(
            &decl,
            &physical,
            DerivedFields {
                row_estimate: Some(500),
                attributes: Some(attributes.clone()),
                ..no_derived()
            },
        );
        assert_eq!(descriptor.row_estimate, Some(500));
        assert_eq!(descriptor.attributes, Some(attributes));
    }

    #[test]
    fn merge_descriptor_derives_datetime_from_the_single_temporal_column_when_not_overridden() {
        let decl = decl(None, Some("geom"), Some("id"));
        let physical = physical(Some("geom"), Some("id"));
        let descriptor = merge_descriptor(
            &decl,
            &physical,
            DerivedFields {
                temporal_column: Some("observed_at".to_string()),
                ..no_derived()
            },
        );
        assert_eq!(descriptor.datetime.as_deref(), Some("observed_at"));
    }

    #[test]
    fn merge_descriptor_prefers_a_datetime_override_over_the_derived_temporal_column() {
        let decl = decl_with_datetime(None, Some("geom"), Some("id"), Some("captured_at"));
        let physical = physical(Some("geom"), Some("id"));
        let descriptor = merge_descriptor(
            &decl,
            &physical,
            DerivedFields {
                temporal_column: Some("observed_at".to_string()),
                ..no_derived()
            },
        );
        assert_eq!(
            descriptor.datetime.as_deref(),
            Some("captured_at"),
            "override must win even though it diverges from the derived temporal column"
        );
    }

    /// Zero or multiple temporal candidates both resolve to `None` — the
    /// "keep it dumb" rule `CatalogSource::temporal_column` documents.
    /// `merge_descriptor` itself has no opinion on why `temporal_column` is
    /// `None`; this only asserts that a `None` derived value with no
    /// override stays `None`.
    #[test]
    fn merge_descriptor_leaves_datetime_none_when_no_temporal_column_was_derived() {
        let decl = decl(None, Some("geom"), Some("id"));
        let physical = physical(Some("geom"), Some("id"));
        let descriptor = merge_descriptor(&decl, &physical, no_derived());
        assert_eq!(descriptor.datetime, None);
    }

    fn feature_capable_descriptor() -> CollectionDescriptor {
        CollectionDescriptor {
            table: "demo".to_string(),
            geometry: Some("geom".to_string()),
            pk: Some("id".to_string()),
            datetime: None,
            srid: None,
            extent: None,
            row_estimate: None,
            attributes: None,
            geometry_type: None,
            projection: None,
        }
    }

    #[test]
    fn require_feature_capable_passes_when_geometry_and_pk_are_present() {
        let descriptor = feature_capable_descriptor();
        assert!(require_feature_capable("demo", &descriptor, CollectionKind::Vector).is_ok());
    }

    #[test]
    fn require_feature_capable_fails_when_geometry_is_missing() {
        let descriptor = CollectionDescriptor {
            geometry: None,
            ..feature_capable_descriptor()
        };
        match require_feature_capable("demo", &descriptor, CollectionKind::Vector) {
            Err(Error::Config(message)) => {
                assert!(message.contains("geometry"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn require_feature_capable_fails_when_pk_is_missing() {
        let descriptor = CollectionDescriptor {
            pk: None,
            ..feature_capable_descriptor()
        };
        match require_feature_capable("demo", &descriptor, CollectionKind::Vector) {
            Err(Error::Config(message)) => {
                assert!(message.contains("pk"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    // -- kind-aware geometry relaxation (`#192`) -----------------------------

    /// The relaxation itself: a record collection whose backing table
    /// reports no geometry column at all still passes. OGC API - Records -
    /// Part 1: Core makes `geometry` an OPTIONAL core property of a record
    /// (Table 9) and leaves making it mandatory to a specific community of
    /// interest (Permission 4, `/per/record-core/geometry`), so requiring
    /// one here would refuse exactly what the Records lane exists to serve.
    #[test]
    fn require_feature_capable_allows_a_record_collection_with_no_geometry_column() {
        let descriptor = CollectionDescriptor {
            geometry: None,
            ..feature_capable_descriptor()
        };
        assert!(require_feature_capable("demo", &descriptor, CollectionKind::Record).is_ok());
    }

    /// The relaxation is *only* about geometry. Requirement 1
    /// (`/req/record-core/mandatory-properties-record`, clause B) says a
    /// record's `id` "cannot be NULL or the empty string", and the pk column
    /// is where that id comes from — so a record collection with no pk is
    /// still refused, by name, at boot.
    #[test]
    fn require_feature_capable_still_requires_a_pk_for_a_record_collection() {
        let descriptor = CollectionDescriptor {
            geometry: None,
            pk: None,
            ..feature_capable_descriptor()
        };
        match require_feature_capable("demo", &descriptor, CollectionKind::Record) {
            Err(Error::Config(message)) => {
                assert!(message.contains("pk"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// A raster collection is not a record collection: `raster` is a label
    /// for a coverage, and coverages have geometry. The relaxation must not
    /// leak to it, or a genuinely misconfigured raster collection would boot
    /// and then fail every request instead of failing fast.
    #[test]
    fn require_feature_capable_does_not_relax_geometry_for_a_raster_collection() {
        let descriptor = CollectionDescriptor {
            geometry: None,
            ..feature_capable_descriptor()
        };
        assert!(require_feature_capable("demo", &descriptor, CollectionKind::Raster).is_err());
    }

    // -- reconcile_schema (`#44`) --------------------------------------------

    use crate::config::PropertyDecl;

    fn schema_declaring(properties: Vec<PropertyDecl>) -> SchemaDecl {
        SchemaDecl {
            properties,
            additional_properties: true,
        }
    }

    fn property(name: &str, type_: PropertyType) -> PropertyDecl {
        PropertyDecl {
            name: name.to_string(),
            type_,
            required: false,
        }
    }

    fn descriptor_with_attributes(attributes: Vec<AttributeColumn>) -> CollectionDescriptor {
        CollectionDescriptor {
            attributes: Some(attributes),
            ..feature_capable_descriptor()
        }
    }

    #[test]
    fn reconcile_schema_passes_when_every_declared_property_matches_the_backend() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "population".to_string(),
            sql_type: "integer".to_string(),
        }]);
        let schema = schema_declaring(vec![property("population", PropertyType::Integer)]);
        assert!(reconcile_schema("demo", &schema, &descriptor).is_ok());
    }

    #[test]
    fn reconcile_schema_fails_naming_the_collection_and_property_when_the_column_is_missing() {
        let descriptor = descriptor_with_attributes(vec![]);
        let schema = schema_declaring(vec![property("population", PropertyType::Integer)]);
        match reconcile_schema("demo", &schema, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("population"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_schema_fails_naming_declared_vs_actual_type_on_a_mismatch() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "population".to_string(),
            sql_type: "text".to_string(),
        }]);
        let schema = schema_declaring(vec![property("population", PropertyType::Integer)]);
        match reconcile_schema("demo", &schema, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("population"), "message was: {message}");
                assert!(message.contains("integer"), "message was: {message}");
                assert!(message.contains("text"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_schema_fails_when_a_declared_property_names_the_geometry_column() {
        let descriptor = descriptor_with_attributes(vec![]);
        let schema = schema_declaring(vec![property("geom", PropertyType::String)]);
        match reconcile_schema("demo", &schema, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// No-regression guard: an empty declared schema (no properties at all)
    /// always reconciles, regardless of what the backend reports.
    #[test]
    fn reconcile_schema_passes_trivially_for_an_empty_schema() {
        let descriptor = descriptor_with_attributes(vec![]);
        let schema = schema_declaring(vec![]);
        assert!(reconcile_schema("demo", &schema, &descriptor).is_ok());
    }

    // -- reconcile_modified_column (`#107`) ----------------------------------

    #[test]
    fn reconcile_modified_column_passes_when_the_declared_column_is_a_real_timestamp() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "updated_at".to_string(),
            sql_type: "timestamp with time zone".to_string(),
        }]);
        assert!(reconcile_modified_column("demo", "updated_at", None, &descriptor).is_ok());
    }

    #[test]
    fn reconcile_modified_column_fails_naming_the_collection_and_column_when_missing() {
        let descriptor = descriptor_with_attributes(vec![]);
        match reconcile_modified_column("demo", "updated_at", None, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("updated_at"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_modified_column_fails_when_the_column_is_not_a_timestamp_type() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "updated_at".to_string(),
            sql_type: "text".to_string(),
        }]);
        match reconcile_modified_column("demo", "updated_at", None, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("updated_at"), "message was: {message}");
                assert!(message.contains("text"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_modified_column_fails_when_it_names_the_geometry_column() {
        let descriptor = descriptor_with_attributes(vec![]);
        match reconcile_modified_column("demo", "geom", None, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_modified_column_fails_when_a_closed_schema_omits_it() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "updated_at".to_string(),
            sql_type: "timestamptz".to_string(),
        }]);
        let mut schema = schema_declaring(vec![property("name", PropertyType::String)]);
        schema.additional_properties = false;
        match reconcile_modified_column("demo", "updated_at", Some(&schema), &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("updated_at"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_modified_column_passes_when_a_closed_schema_declares_it() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "updated_at".to_string(),
            sql_type: "timestamptz".to_string(),
        }]);
        let mut schema = schema_declaring(vec![property("updated_at", PropertyType::DateTime)]);
        schema.additional_properties = false;
        assert!(
            reconcile_modified_column("demo", "updated_at", Some(&schema), &descriptor).is_ok()
        );
    }

    #[test]
    fn reconcile_modified_column_passes_when_an_open_schema_omits_it() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "updated_at".to_string(),
            sql_type: "timestamptz".to_string(),
        }]);
        let schema = schema_declaring(vec![property("name", PropertyType::String)]);
        assert!(
            reconcile_modified_column("demo", "updated_at", Some(&schema), &descriptor).is_ok()
        );
    }

    // -- reconcile_tile_properties (`#85`) -----------------------------------

    #[test]
    fn reconcile_tile_properties_passes_when_every_allowlisted_column_is_a_projectable_scalar() {
        let descriptor = descriptor_with_attributes(vec![
            AttributeColumn {
                name: "name".to_string(),
                sql_type: "text".to_string(),
            },
            AttributeColumn {
                name: "pop".to_string(),
                sql_type: "integer".to_string(),
            },
            AttributeColumn {
                name: "active".to_string(),
                sql_type: "boolean".to_string(),
            },
        ]);
        let allowlist = vec!["name".to_string(), "pop".to_string(), "active".to_string()];
        assert!(reconcile_tile_properties("demo", &allowlist, &descriptor).is_ok());
    }

    #[test]
    fn reconcile_tile_properties_fails_naming_the_collection_and_property_when_the_column_is_missing(
    ) {
        let descriptor = descriptor_with_attributes(vec![]);
        let allowlist = vec!["population".to_string()];
        match reconcile_tile_properties("demo", &allowlist, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("population"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_tile_properties_fails_when_an_allowlisted_column_names_the_geometry_column() {
        let descriptor = descriptor_with_attributes(vec![]);
        let allowlist = vec!["geom".to_string()];
        match reconcile_tile_properties("demo", &allowlist, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_tile_properties_fails_when_an_allowlisted_column_is_literally_named_id() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "id".to_string(),
            sql_type: "integer".to_string(),
        }]);
        let allowlist = vec!["id".to_string()];
        match reconcile_tile_properties("demo", &allowlist, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("id"), "message was: {message}");
                assert!(message.contains("reserved"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn reconcile_tile_properties_fails_on_a_non_scalar_type_this_slice_does_not_project() {
        let descriptor = descriptor_with_attributes(vec![AttributeColumn {
            name: "observed_at".to_string(),
            sql_type: "timestamp with time zone".to_string(),
        }]);
        let allowlist = vec!["observed_at".to_string()];
        match reconcile_tile_properties("demo", &allowlist, &descriptor) {
            Err(Error::Config(message)) => {
                assert!(message.contains("observed_at"), "message was: {message}");
                assert!(message.contains("datetime"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// No-regression guard: an empty allowlist (pk-only, the default) always
    /// reconciles, regardless of what the backend reports.
    #[test]
    fn reconcile_tile_properties_passes_trivially_for_an_empty_allowlist() {
        let descriptor = descriptor_with_attributes(vec![]);
        assert!(reconcile_tile_properties("demo", &[], &descriptor).is_ok());
    }

    #[test]
    fn diverges_is_false_when_no_override_is_declared() {
        assert!(!diverges(None, Some("geom")));
    }

    #[test]
    fn diverges_is_false_when_the_override_agrees_with_the_derived_value() {
        assert!(!diverges(Some("geom"), Some("geom")));
    }

    #[test]
    fn diverges_is_true_when_the_override_contradicts_the_derived_value() {
        assert!(diverges(Some("the_geom"), Some("geom")));
    }

    #[test]
    fn cached_descriptor_is_stale_once_the_ttl_elapses() {
        let cached = CachedDescriptor {
            outcome: Ok(feature_capable_descriptor()),
            computed_at: Instant::now(),
        };
        assert!(!cached.is_stale(std::time::Duration::from_secs(60)));
        assert!(cached.is_stale(std::time::Duration::from_secs(0)));
    }

    /// A cached failure verdict (`#42`) is staleness-governed exactly like a
    /// cached success — the same TTL clock, just a different `outcome` arm.
    #[test]
    fn cached_descriptor_failure_verdict_is_also_ttl_governed() {
        let cached = CachedDescriptor {
            outcome: Err("collection 'demo': storage 'main' does not report a table".to_string()),
            computed_at: Instant::now(),
        };
        assert!(!cached.is_stale(std::time::Duration::from_secs(60)));
        assert!(cached.is_stale(std::time::Duration::from_secs(0)));
    }
}
