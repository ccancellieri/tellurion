//! `CanonicalDescriptor`: the one read-side merge of every metadata source
//! this workspace has for a collection (issue `#50`, first half — read side
//! only). Four sources feed it: backend-derived physical facts
//! ([`CollectionDescriptor`]), the operator's declared property contract
//! ([`SchemaDecl`]), the operator's static STAC metadata ([`StacConf`]), and
//! live capability advertisement (what this collection's lanes actually
//! resolve to right now). Both `tellurion-stac`'s Collection mapping and
//! `tellurion-features`' collection-metadata emission consume this one
//! struct instead of separately re-deriving/re-reading the same four
//! sources, so the two protocols can never quietly drift on what a
//! collection's license, property schema, or spatial extent is. Absent stays
//! absent throughout — nothing here fabricates a value none of the four
//! sources actually reported; see each field's own doc for its precedence
//! rule.
//!
//! [`build`] is pure (no I/O, unit-testable with hand-built inputs) — the
//! same "gather async, merge sync" split `descriptor::merge_descriptor` /
//! `Router::derive_one_descriptor` already use. `Router::canonical_descriptor`
//! is the async seam that gathers the four inputs (reusing the existing
//! TTL-bounded `descriptor_cache` for the physical facts — no second cache
//! concept) and calls this module's [`build`].
//!
//! Provenance is tracked per field where it can actually vary within one
//! collection (physical identity: `table`/`geometry`/`pk`/`datetime`, and
//! per schema property), and per whole group where every member shares one
//! rule (`stac`: every field is [`Provenance::Declared`] when the group is
//! present at all, since there is no backend-derivable license/keywords/
//! providers/contacts to speak of). Per-field everywhere would be noise;
//! per-group everywhere would hide the one place (physical identity) where a single
//! collection genuinely mixes overridden and derived values.
//!
//! Deliberately excluded: MVT `vector_layers` (the tiles protocol's own
//! source-layer names, `TileSource::vector_layers`). It requires a live,
//! per-request driver call and is consumed exclusively by
//! `tellurion-tiles`' own `TileSet` resource — folding it in here would cost
//! every `CanonicalDescriptor` build an extra driver round trip that only
//! one, out-of-scope-for-this-slice consumer ever needs. See the wave-11
//! convergence report for this deviation's rationale.

use std::collections::BTreeMap;

use crate::catalog::{AttributeColumn, GeometryProfile, ProjectionFacts, SpatialExtent};
use crate::config::{
    AssetDecl, CollectionDecl, CollectionKind, ContactDecl, LineageDecl, PropertyType, SchemaDecl,
    StacConf, StacProvider,
};
use crate::descriptor::CollectionDescriptor;

/// Where one fact in a [`CanonicalDescriptor`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Computed entirely from backend introspection — no operator input at
    /// all (a collection's spatial extent, row estimate, or a physical
    /// identity field the operator left unconfigured).
    Derived,
    /// Operator-authored, with no backend-derivable equivalent: STAC
    /// metadata (there is no "derived" license), or a schema property's own
    /// declared type/required-ness (`SchemaDecl`, `#44`).
    Declared,
    /// An operator value that exists precisely because it supersedes a value
    /// the backend could also supply — `CollectionDecl`'s `table`/
    /// `geometry`/`pk`/`datetime` overrides, when set.
    Override,
}

/// One physical identity field (table/geometry/pk/datetime), tagged with
/// where its value came from. Mirrors `descriptor::merge_descriptor`'s own
/// override > derived precedence, but keeps the `Provenance` that function's
/// own return type discards.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalField {
    pub value: String,
    pub provenance: Provenance,
}

/// One property in the merged schema view: a `descriptor.attributes` column
/// refined by a declared `SchemaDecl` property of the same name, if any.
/// Never includes the geometry or datetime columns — those live on
/// [`CanonicalDescriptor::geometry`]/`::datetime`, the same boundary
/// `SchemaDecl` itself draws (see that type's own doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalProperty {
    pub type_: PropertyType,
    /// `SchemaDecl`'s own `required: true`. Always `false` for a
    /// [`Provenance::Derived`] property — a column the backend reports but
    /// the operator never declared has no "required" concept to inherit.
    pub required: bool,
    pub provenance: Provenance,
}

/// The merged property-schema view (`SchemaDecl` refining
/// `descriptor.attributes`, `#44`). `None` on [`CanonicalDescriptor::schema`]
/// when this collection has neither a declared schema nor any backend-
/// reported attribute column at all — absent, never an empty map standing in
/// for "nothing here."
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalSchema {
    pub properties: BTreeMap<String, CanonicalProperty>,
    /// `SchemaDecl::additional_properties`'s own default (`true`) when no
    /// schema was declared at all — an undeclared collection stays as
    /// open/free-form here as it always has been.
    pub additional_properties: bool,
}

/// Operator-declared static metadata (`StacConf`, `#36`/`#187`). Every
/// field here is [`Provenance::Declared`] by construction whenever
/// [`CanonicalDescriptor::stac`] is `Some` at all — there is no backend
/// equivalent for a license/keywords/providers/assets/contacts to be
/// `Derived` from, so this group carries no per-field provenance of its own.
///
/// Not every field projects into STAC despite the group's name (inherited
/// from the `stac:` config key it mirrors): `contacts` exists for the ISO
/// 19115 projection, which requires a responsible party STAC has no slot
/// for. See [`CanonicalStac::contacts`].
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalStac {
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub providers: Vec<StacProvider>,
    /// Declared STAC Collection `assets` (`#36` slice 1) — carried straight
    /// through from `StacConf::assets`, the same reuse-the-config-type
    /// shortcut `providers` above already takes (`StacProvider` is the
    /// config type itself, not a separate canonical wrapper).
    pub assets: BTreeMap<String, AssetDecl>,
    /// Declared responsible-party contacts (`#187`, first slice) — carried
    /// straight through from `StacConf::contacts`, same reuse-the-config-
    /// type shortcut as `providers`/`assets`. Consumed by the ISO 19139
    /// projection only (`tellurion_stac::iso19139`), where
    /// `MD_Metadata/contact` is schema-mandatory; the STAC Collection
    /// projection deliberately ignores it — see
    /// `tellurion_stac::mapping::to_stac_collection`.
    pub contacts: Vec<ContactDecl>,
    /// Declared lineage/provenance (`#50`, lineage slice) — carried straight
    /// through from `StacConf::lineage`, same reuse-the-config-type shortcut
    /// as `providers`/`assets`/`contacts`. Consumed by the ISO 19139
    /// projection only (`tellurion_stac::iso19139`'s
    /// `gmd:dataQualityInfo`); the STAC Collection projection deliberately
    /// ignores it (STAC has no collection-level lineage slot — same split
    /// `contacts` draws). `None` — the default for every collection whose
    /// settings chain never declares one — emits nothing at all; see
    /// `LineageDecl`'s own doc for why the operator's declaration is the
    /// only lineage fact this workspace genuinely has.
    pub lineage: Option<LineageDecl>,
}

/// What this collection can actually serve, resolved live through `Router`
/// each call — never provenance-tagged (this is an observation of current
/// routing state, not a declared-vs-derived fact about the data). Excludes
/// `vector_layers`; see this module's doc for why.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CanonicalCapabilities {
    /// `Router::resolve_features` resolves for this collection.
    pub features: bool,
    /// This collection's tiles lane resolves — vector (`Router::
    /// resolve_tiles`) OR raster (`Router::resolve_raster`, `#37`): a
    /// raster-only COG/Zarr collection must still report `true` here or
    /// every `#50` consumer (starting with `tellurion-features`' own
    /// `/collections` listing, which omits a collection when neither
    /// capability is set) would treat it as capability-less. Deliberately
    /// coarse, which is why it must never gate a *vector*-specific
    /// advertisement — that is `tiles_vector` below's job (`#287`).
    pub tiles: bool,
    /// `Router::resolve_tiles` alone — a vector `TileSource` — resolves for
    /// this collection (`#287`). `tiles` above merges the vector and raster
    /// lanes, which makes it exactly the coarse signal a capability-bearing
    /// advertisement must NOT hang on: a raster-only collection is
    /// `tiles: true` while its `.mvt` route refuses with a 400. A consumer
    /// advertising the MVT lane (`tellurion_features::handlers::
    /// collection_summary`'s `tilesets-vector` link) gates on this field
    /// instead, mirroring `TilesLinkContributor`'s own independent
    /// `resolve_tiles` probe — the two surfaces answer from the same
    /// signal, never from a second ad-hoc check.
    pub tiles_vector: bool,
    /// This collection declares `places3d` (`CollectionDecl::places3d.
    /// is_some()`) — mirrors `tellurion_features::handlers`' own
    /// unconditional-on-tiles-resolving reading of this flag, not
    /// `tellurion_stac::assets`' stricter `has_tiles && places3d` asset-
    /// gating rule (a separate, still-direct-path concern; see the wave-11
    /// convergence report).
    pub places3d: bool,
    /// The resolved `FeatureSource::crs_capable()` answer for this
    /// collection, `false` when the features lane doesn't resolve at all.
    /// Exists so a metadata consumer (`tellurion_features::handlers::
    /// collection_summary`) can advertise a CRS in its `crs` list only when
    /// the driver backing this collection can actually serve it — a
    /// collection's storage SRID alone says nothing about whether the driver
    /// can reproject into it (most drivers can't; PostGIS is the only one
    /// that overrides `crs_capable` to `true`), so folding the storage SRID
    /// straight into the advertised list regardless of this flag would
    /// advertise a CRS the enforcement gate (`crs::resolve` plus the
    /// `crs_capable` check every CRS-aware handler already runs) then
    /// refuses with a 400.
    pub crs_capable: bool,
    /// The resolved `FeatureSource::cql2_conformance_classes()` answer for
    /// this collection (`#105`), `None` when the features lane doesn't
    /// resolve at all (`#287`) — `crate::router::fold_conformance_classes`'
    /// two-sided contract carried onto the per-collection surface. `None`:
    /// this collection does not participate in CQL2 filtering (no
    /// `FeatureSource` at all — a raster COG/Zarr, a tiles-only PMTiles
    /// archive), and its metadata must not carry the member. `Some(vec![])`:
    /// it participates and honours nothing (a features-capable driver whose
    /// compiler declines CQL2 entirely — FlatGeobuf, GeoParquet, memory),
    /// and its metadata carries the honest empty list. Before `#287` both
    /// cases collapsed to an empty `Vec`, and the consumer serialized a
    /// filtering claim (an empty-but-present member, next to an
    /// unconditional `itemType`/`crs`) for collections with no features
    /// lane to honour any of it. Exists so a metadata consumer
    /// (`tellurion_features::handlers::collection_summary`)
    /// can advertise this collection's true, per-driver CQL2 filter
    /// capability rather than the conservative workspace-wide intersection
    /// `Router::cql2_conformance_classes` computes for the landing page —
    /// see that method's own doc for why the two are deliberately different
    /// answers. This struct is no longer `Copy` because of this field
    /// (`Vec` isn't); every prior caller either already used `Clone` or
    /// needed only one owned copy per call, so this cost only a handful of
    /// explicit `.clone()`s at the few call sites that read `capabilities`
    /// more than once.
    pub cql2_conformance_classes: Option<Vec<&'static str>>,
    /// `Router::resolve_write` resolves for this collection — i.e. `PUT
    /// /collections/{collectionId}/items/{featureId}` can create a new item
    /// with the caller-supplied `{featureId}` rather than a server-assigned
    /// one (`WriteSink::apply`'s `Upsert` is a real create-or-replace on
    /// every implementer, never an update-only that 404s on a missing id).
    /// Feeds `tellurion_features::handlers`' `supportsNonAutogeneratedResourceIds`
    /// collection property (OGC API Features — Part 4, Requirement 38,
    /// `/req/features/collection-endpoint`).
    pub write: bool,
    /// The OGC API Features — Part 4 (20-002r1 draft) Optimistic Locking
    /// classes this collection genuinely honors right now (`#107`): the
    /// ETags class when this collection's resolved write sink declares it
    /// (`WriteSink::locking_conformance_classes`) AND its features lane
    /// also resolves (the guard reads current state through
    /// `FeatureSource::item` before comparing). `None` when the features
    /// lane doesn't resolve at all (`#287`) — the same participates/
    /// doesn't-participate distinction `cql2_conformance_classes` above
    /// carries, and for the same consumer: a collection with no
    /// `FeatureSource` says nothing about locking, so its metadata carries
    /// no member. `Some(vec![])` when the features lane resolves but the
    /// write lane doesn't, or the sink declares nothing — the honest empty
    /// list every features-capable read-only collection has always shown.
    /// The Timestamps class joins
    /// this list under a different, non-driver condition: this collection's
    /// own `CollectionDecl::modified_column.is_some()`, again gated on both
    /// lanes resolving — see `locking`'s own module doc for why Timestamps
    /// is a per-collection declaration rather than something any driver
    /// declares or withholds, and
    /// `tellurion_features::handlers::collection_summary` for how this
    /// feeds `CollectionSummary::locking_conformance_classes`, the
    /// per-collection counterpart of `cql2_conformance_classes` above.
    pub locking_conformance_classes: Option<Vec<&'static str>>,
}

/// The one read-side merge of every metadata source this workspace has for a
/// collection. See this module's own doc for the four sources, the
/// provenance-granularity rule, and the `vector_layers` exclusion.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalDescriptor {
    /// What this collection *is* (`#192`) — `CollectionDecl::kind`, carried
    /// straight through. No [`Provenance`]: unlike `table`/`geometry`/`pk`/
    /// `datetime`, there is no backend-derivable answer for a kind that an
    /// operator declaration could be said to *override* — a driver reports
    /// what a table physically holds, never what the collection means — so
    /// this is a plain field, the same shape `srid`/`row_estimate` take.
    ///
    /// This is the field that makes Records a third *projection* of this one
    /// descriptor rather than a parallel catalog: Features, STAC and Records
    /// all read the same `CanonicalDescriptor`, and each decides from this
    /// field alone whether the collection is theirs to serve.
    pub kind: CollectionKind,
    /// Physical target table/layer name. `None` only when descriptor
    /// derivation failed entirely (a misconfigured collection, or a
    /// transient backend error) — see `Router::canonical_descriptor`'s
    /// tolerant handling of that case; a collection whose derivation
    /// succeeded always has a concrete table name, override or derived by
    /// convention (see `descriptor::target_table`).
    pub table: Option<CanonicalField>,
    pub geometry: Option<CanonicalField>,
    pub pk: Option<CanonicalField>,
    pub datetime: Option<CanonicalField>,
    /// This collection's native storage SRID, carried straight through from
    /// `CollectionDescriptor::srid` — no override concept, so no
    /// `Provenance` (plain field, same shape as `row_estimate`). Feeds
    /// `tellurion-features`' `storageCrs`/`crs` collection metadata
    /// (`tellurion_core::crs`).
    pub srid: Option<i32>,
    /// This collection's backend-known projection facts
    /// (`CatalogSource::projection`, `#36` — STAC `projection` extension):
    /// what the driver can read out of its own storage's georeferencing,
    /// carried straight through from `CollectionDescriptor::projection` —
    /// no override concept, so no `Provenance` (plain field, same shape as
    /// `srid` above). `None` for every driver that never overrides the
    /// accessor; see `ProjectionFacts` for the per-field omission contract
    /// consumers must honor (absent is absent — never null, never a
    /// default).
    pub projection: Option<ProjectionFacts>,
    pub extent: Option<SpatialExtent>,
    pub row_estimate: Option<u64>,
    pub schema: Option<CanonicalSchema>,
    pub stac: Option<CanonicalStac>,
    pub capabilities: CanonicalCapabilities,
    /// This collection's geometry statistics profile (`#101`), when one has
    /// been computed — `None` for a collection whose driver never overrides
    /// `CatalogSource::geometry_profile`, or whose profile computation
    /// itself failed (logged, never fails the whole canonical descriptor —
    /// see `Router::canonical_descriptor`'s handling of `physical` for the
    /// same "never fail the request over metadata" rule). Never
    /// provenance-tagged, same as `capabilities` above: this is a live,
    /// separately-cached observation (`Router::geometry_profile`), not a
    /// declared-vs-derived fact about the data.
    pub geometry_profile: Option<GeometryProfile>,
}

/// One field's `(overridden, resolved)` pair into a `CanonicalField`, or
/// `None` when the resolved side has nothing — the shared shape
/// `geometry`/`pk`/`datetime` each reduce to. `table` is handled separately
/// in [`build`] since it always resolves when derivation succeeds at all
/// (see `CanonicalDescriptor::table`'s own doc), where `geometry`/`pk`/
/// `datetime` may legitimately stay `None` even on a successful derivation
/// (`#20`, a driver with no table-shaped concept of either).
fn resolved_field(overridden: Option<&str>, resolved: Option<&str>) -> Option<CanonicalField> {
    resolved.map(|value| CanonicalField {
        value: value.to_string(),
        provenance: if overridden.is_some() {
            Provenance::Override
        } else {
            Provenance::Derived
        },
    })
}

/// Merges every declared property in `schema` with every attribute column
/// `attributes` reports: a column present in both takes the declared type/
/// required-ness ([`Provenance::Declared`], operator wins); a column only
/// `attributes` reports takes the SQL-type-inferred shape
/// ([`Provenance::Derived`]); a column `additional_properties: false`
/// excludes (declared elsewhere, not this one) is skipped entirely, mirroring
/// `tellurion_features::queryables::queryable_properties`'s identical rule so
/// the two can never drift apart. `None` when there is nothing to report at
/// all — no attributes and no declared schema.
fn build_schema(
    attributes: Option<&[AttributeColumn]>,
    schema: Option<&SchemaDecl>,
) -> Option<CanonicalSchema> {
    if attributes.is_none() && schema.is_none() {
        return None;
    }

    let additional_properties = schema.map(|s| s.additional_properties).unwrap_or(true);
    let mut properties = BTreeMap::new();

    for attribute in attributes.into_iter().flatten() {
        let declared =
            schema.and_then(|schema| schema.properties.iter().find(|p| p.name == attribute.name));
        if declared.is_none() && !additional_properties {
            continue;
        }
        let property = match declared {
            Some(declared) => CanonicalProperty {
                type_: declared.type_,
                required: declared.required,
                provenance: Provenance::Declared,
            },
            None => CanonicalProperty {
                type_: PropertyType::from_sql_type(&attribute.sql_type),
                required: false,
                provenance: Provenance::Derived,
            },
        };
        properties.insert(attribute.name.clone(), property);
    }

    // A declared property naming a column `attributes` doesn't report
    // (never true post-`descriptor::reconcile_schema`, but this merge stays
    // defensive rather than assuming a caller already enforced that) still
    // surfaces here, Declared, with no backend confirmation behind it.
    for property in schema.iter().flat_map(|schema| &schema.properties) {
        properties
            .entry(property.name.clone())
            .or_insert(CanonicalProperty {
                type_: property.type_,
                required: property.required,
                provenance: Provenance::Declared,
            });
    }

    Some(CanonicalSchema {
        properties,
        additional_properties,
    })
}

/// Merges `descriptor` (backend physical facts, `None` when derivation
/// failed entirely — see `CanonicalDescriptor::table`'s doc), `decl` (to
/// recover *which* of `table`/`geometry`/`pk`/`datetime` were operator
/// overrides — `merge_descriptor` itself already applied the precedence but
/// discards which side won), `schema` (`#44`), `stac` (this collection's
/// effective `stac:` settings subtree, `#36`), `capabilities` (a live
/// `Router` probe), and `geometry_profile` (`#101`, another live `Router`
/// probe — `Router::geometry_profile`, cached separately from `descriptor`)
/// into one [`CanonicalDescriptor`]. Pure — no I/O, callable from a unit
/// test with hand-built inputs; see `Router::canonical_descriptor` for the
/// async seam that gathers these six pieces.
pub fn build(
    descriptor: Option<&CollectionDescriptor>,
    decl: &CollectionDecl,
    schema: Option<&SchemaDecl>,
    stac: Option<&StacConf>,
    capabilities: CanonicalCapabilities,
    geometry_profile: Option<GeometryProfile>,
) -> CanonicalDescriptor {
    let table = descriptor.map(|d| CanonicalField {
        value: d.table.clone(),
        provenance: if decl.table.is_some() {
            Provenance::Override
        } else {
            Provenance::Derived
        },
    });
    let geometry =
        descriptor.and_then(|d| resolved_field(decl.geometry.as_deref(), d.geometry.as_deref()));
    let pk = descriptor.and_then(|d| resolved_field(decl.pk.as_deref(), d.pk.as_deref()));
    let datetime =
        descriptor.and_then(|d| resolved_field(decl.datetime.as_deref(), d.datetime.as_deref()));

    CanonicalDescriptor {
        kind: decl.kind,
        table,
        geometry,
        pk,
        datetime,
        srid: descriptor.and_then(|d| d.srid),
        projection: descriptor.and_then(|d| d.projection),
        extent: descriptor.and_then(|d| d.extent),
        row_estimate: descriptor.and_then(|d| d.row_estimate),
        schema: build_schema(descriptor.and_then(|d| d.attributes.as_deref()), schema),
        stac: stac.map(|conf| CanonicalStac {
            license: conf.license.clone(),
            keywords: conf.keywords.clone(),
            providers: conf.providers.clone(),
            assets: conf.assets.clone(),
            contacts: conf.contacts.clone(),
            lineage: conf.lineage.clone(),
        }),
        capabilities,
        geometry_profile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{FeatureSizeStats, VertexStats};
    use crate::config::{LineageProcessStepDecl, LineageSourceDecl, PropertyDecl};

    fn decl(table: Option<&str>, geometry: Option<&str>, pk: Option<&str>) -> CollectionDecl {
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
        serde_yaml::from_str(&yaml).unwrap()
    }

    fn descriptor_with_attributes(
        attributes: Option<Vec<AttributeColumn>>,
    ) -> CollectionDescriptor {
        CollectionDescriptor {
            table: "demo".to_string(),
            geometry: Some("geom".to_string()),
            pk: Some("id".to_string()),
            datetime: None,
            srid: None,
            extent: None,
            row_estimate: None,
            attributes,
            geometry_type: None,
            projection: None,
        }
    }

    fn caps() -> CanonicalCapabilities {
        CanonicalCapabilities {
            features: true,
            tiles: false,
            tiles_vector: false,
            places3d: false,
            crs_capable: false,
            cql2_conformance_classes: Some(Vec::new()),
            write: false,
            locking_conformance_classes: Some(Vec::new()),
        }
    }

    #[test]
    fn table_provenance_is_override_when_the_decl_names_one() {
        let decl = decl(Some("physical_table"), None, None);
        let descriptor = CollectionDescriptor {
            table: "physical_table".to_string(),
            ..descriptor_with_attributes(None)
        };
        let canonical = build(Some(&descriptor), &decl, None, None, caps(), None);
        let table = canonical.table.unwrap();
        assert_eq!(table.value, "physical_table");
        assert_eq!(table.provenance, Provenance::Override);
    }

    #[test]
    fn table_provenance_is_derived_when_the_decl_leaves_it_unset() {
        let decl = decl(None, None, None);
        let descriptor = CollectionDescriptor {
            table: "demo".to_string(),
            ..descriptor_with_attributes(None)
        };
        let canonical = build(Some(&descriptor), &decl, None, None, caps(), None);
        assert_eq!(canonical.table.unwrap().provenance, Provenance::Derived);
    }

    #[test]
    fn geometry_provenance_is_override_when_it_diverges_from_the_backend() {
        let decl = decl(None, Some("the_geom"), None);
        let descriptor = CollectionDescriptor {
            geometry: Some("the_geom".to_string()),
            ..descriptor_with_attributes(None)
        };
        let canonical = build(Some(&descriptor), &decl, None, None, caps(), None);
        let geometry = canonical.geometry.unwrap();
        assert_eq!(geometry.value, "the_geom");
        assert_eq!(geometry.provenance, Provenance::Override);
    }

    #[test]
    fn physical_fields_stay_none_when_neither_override_nor_derived_value_exists() {
        let decl = decl(None, None, None);
        let descriptor = CollectionDescriptor {
            geometry: None,
            pk: None,
            ..descriptor_with_attributes(None)
        };
        let canonical = build(Some(&descriptor), &decl, None, None, caps(), None);
        assert!(canonical.geometry.is_none());
        assert!(canonical.pk.is_none());
    }

    #[test]
    fn srid_is_carried_through_from_the_descriptor_with_no_provenance_concept() {
        let decl = decl(None, None, None);
        let descriptor = CollectionDescriptor {
            srid: Some(3857),
            ..descriptor_with_attributes(None)
        };
        let canonical = build(Some(&descriptor), &decl, None, None, caps(), None);
        assert_eq!(canonical.srid, Some(3857));
    }

    #[test]
    fn every_physical_and_stac_fact_is_absent_when_derivation_failed_entirely() {
        let decl = decl(None, None, None);
        let canonical = build(None, &decl, None, None, caps(), None);
        assert!(canonical.table.is_none());
        assert!(canonical.geometry.is_none());
        assert!(canonical.pk.is_none());
        assert!(canonical.datetime.is_none());
        assert!(canonical.extent.is_none());
        assert!(canonical.row_estimate.is_none());
        assert!(canonical.schema.is_none());
        assert!(canonical.stac.is_none());
    }

    #[test]
    fn stac_is_none_when_no_level_ever_declared_one() {
        let decl = decl(None, None, None);
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            None,
            caps(),
            None,
        );
        assert!(canonical.stac.is_none());
    }

    #[test]
    fn a_declared_stac_block_is_carried_through_verbatim() {
        let decl = decl(None, None, None);
        let stac = StacConf {
            license: Some("CC-BY-4.0".to_string()),
            keywords: vec!["imagery".to_string()],
            providers: vec![],
            assets: BTreeMap::new(),
            contacts: vec![],
            ..Default::default()
        };
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            Some(&stac),
            caps(),
            None,
        );
        let resolved = canonical.stac.unwrap();
        assert_eq!(resolved.license.as_deref(), Some("CC-BY-4.0"));
        assert_eq!(resolved.keywords, vec!["imagery".to_string()]);
    }

    /// `#36` slice 1: `stac.assets` reaches `CanonicalStac` the same
    /// carried-through-verbatim way `license`/`keywords` already do above —
    /// no re-shaping, no provenance concept per asset (the whole `stac`
    /// group is `Declared` by construction, see `CanonicalStac`'s own doc).
    #[test]
    fn a_declared_stac_assets_block_is_carried_through_verbatim() {
        let decl = decl(None, None, None);
        let mut assets = BTreeMap::new();
        assets.insert(
            "thumbnail".to_string(),
            AssetDecl {
                href: "https://example.com/thumb.png".to_string(),
                media_type: Some("image/png".to_string()),
                title: Some("Thumbnail".to_string()),
                roles: vec!["thumbnail".to_string()],
            },
        );
        let stac = StacConf {
            license: None,
            keywords: vec![],
            providers: vec![],
            assets,
            contacts: vec![],
            ..Default::default()
        };
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            Some(&stac),
            caps(),
            None,
        );
        let resolved = canonical.stac.unwrap();
        let thumbnail = &resolved.assets["thumbnail"];
        assert_eq!(thumbnail.href, "https://example.com/thumb.png");
        assert_eq!(thumbnail.media_type.as_deref(), Some("image/png"));
    }

    /// `#187` first slice: `stac.contacts` reaches `CanonicalStac`
    /// verbatim, exactly like `providers`/`assets` — no re-shaping, no
    /// per-contact provenance, and no invented ordering (the operator's own
    /// list order is what the ISO projection emits).
    #[test]
    fn a_declared_contacts_block_is_carried_through_verbatim() {
        let decl = decl(None, None, None);
        let stac = StacConf {
            contacts: vec![
                ContactDecl {
                    name: "Ada Lovelace".to_string(),
                    organization: Some("Example Org".to_string()),
                    email: Some("ada@example.com".to_string()),
                    role: Some("pointOfContact".to_string()),
                    url: Some("https://example.com/ada".to_string()),
                },
                ContactDecl {
                    name: "Grace Hopper".to_string(),
                    organization: None,
                    email: None,
                    role: None,
                    url: None,
                },
            ],
            ..Default::default()
        };
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            Some(&stac),
            caps(),
            None,
        );
        let resolved = canonical.stac.unwrap();
        assert_eq!(resolved.contacts, stac.contacts);
        assert_eq!(
            resolved.contacts[0].email.as_deref(),
            Some("ada@example.com")
        );
        assert!(resolved.contacts[1].organization.is_none());
    }

    /// `#50` lineage slice: `stac.lineage` reaches `CanonicalStac`
    /// verbatim, exactly like `providers`/`assets`/`contacts` — no
    /// re-shaping, no per-member provenance (the whole `stac` group is
    /// `Declared` by construction).
    #[test]
    fn a_declared_lineage_block_is_carried_through_verbatim() {
        let decl = decl(None, None, None);
        let stac = StacConf {
            lineage: Some(LineageDecl {
                statement: Some("Digitised from the 1:25000 IGM series.".to_string()),
                sources: vec![LineageSourceDecl {
                    description: "IGM 1:25000 sheet 45".to_string(),
                }],
                process_steps: vec![LineageProcessStepDecl {
                    description: "Reprojected to EPSG:4326".to_string(),
                }],
            }),
            ..Default::default()
        };
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            Some(&stac),
            caps(),
            None,
        );
        assert_eq!(canonical.stac.unwrap().lineage, stac.lineage);
    }

    /// A `stac` block that never declares lineage carries `None` — absent,
    /// never an empty placeholder block. The ISO projection relies on this
    /// to keep every undeclared collection's document byte-identical.
    #[test]
    fn an_undeclared_lineage_stays_absent() {
        let decl = decl(None, None, None);
        let stac = StacConf {
            license: Some("CC-BY-4.0".to_string()),
            ..Default::default()
        };
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            Some(&stac),
            caps(),
            None,
        );
        assert!(canonical.stac.unwrap().lineage.is_none());
    }

    /// A `stac` block that declares no contacts leaves the list empty —
    /// never a fabricated placeholder party. The projections rely on this
    /// to stay byte-identical for every deployment that never configured
    /// one.
    #[test]
    fn an_undeclared_contacts_list_stays_empty() {
        let decl = decl(None, None, None);
        let stac = StacConf {
            license: Some("CC-BY-4.0".to_string()),
            ..Default::default()
        };
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            Some(&stac),
            caps(),
            None,
        );
        assert!(canonical.stac.unwrap().contacts.is_empty());
    }

    #[test]
    fn schema_is_none_when_neither_attributes_nor_a_declared_schema_exist() {
        let decl = decl(None, None, None);
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            None,
            caps(),
            None,
        );
        assert!(canonical.schema.is_none());
    }

    #[test]
    fn a_declared_property_type_wins_over_the_sql_type_inferred_one() {
        let decl = decl(None, None, None);
        let attributes = vec![AttributeColumn {
            name: "population".to_string(),
            sql_type: "text".to_string(),
        }];
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "population".to_string(),
                type_: PropertyType::Integer,
                required: true,
            }],
            additional_properties: true,
        };
        let canonical = build(
            Some(&descriptor_with_attributes(Some(attributes))),
            &decl,
            Some(&schema),
            None,
            caps(),
            None,
        );
        let property = &canonical.schema.unwrap().properties["population"];
        assert_eq!(property.type_, PropertyType::Integer);
        assert!(property.required);
        assert_eq!(property.provenance, Provenance::Declared);
    }

    #[test]
    fn an_undeclared_attribute_is_derived_and_never_required() {
        let decl = decl(None, None, None);
        let attributes = vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }];
        let canonical = build(
            Some(&descriptor_with_attributes(Some(attributes))),
            &decl,
            None,
            None,
            caps(),
            None,
        );
        let property = &canonical.schema.unwrap().properties["name"];
        assert_eq!(property.type_, PropertyType::String);
        assert!(!property.required);
        assert_eq!(property.provenance, Provenance::Derived);
    }

    #[test]
    fn a_closed_schema_drops_undeclared_attributes_from_the_merged_view() {
        let decl = decl(None, None, None);
        let attributes = vec![
            AttributeColumn {
                name: "population".to_string(),
                sql_type: "integer".to_string(),
            },
            AttributeColumn {
                name: "name".to_string(),
                sql_type: "text".to_string(),
            },
        ];
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "population".to_string(),
                type_: PropertyType::Integer,
                required: false,
            }],
            additional_properties: false,
        };
        let canonical = build(
            Some(&descriptor_with_attributes(Some(attributes))),
            &decl,
            Some(&schema),
            None,
            caps(),
            None,
        );
        let schema_view = canonical.schema.unwrap();
        assert!(schema_view.properties.contains_key("population"));
        assert!(
            !schema_view.properties.contains_key("name"),
            "a closed schema must drop an undeclared attribute from the merged view"
        );
        assert!(!schema_view.additional_properties);
    }

    #[test]
    fn capabilities_are_carried_through_unchanged() {
        let decl = decl(None, None, None);
        let capabilities = CanonicalCapabilities {
            features: true,
            tiles: true,
            tiles_vector: true,
            places3d: true,
            crs_capable: true,
            cql2_conformance_classes: Some(vec!["basic"]),
            write: true,
            locking_conformance_classes: Some(vec!["etags"]),
        };
        let canonical = build(None, &decl, None, None, capabilities.clone(), None);
        assert_eq!(canonical.capabilities, capabilities);
    }

    // -- collection kind (`#192`) --------------------------------------------

    /// The default. Every `CollectionDecl` written before `kind` existed
    /// deserializes to `vector`, and the canonical descriptor must carry
    /// exactly that — this is what makes "the Features root's listing is
    /// unchanged for an unconfigured deployment" true rather than hoped for.
    #[test]
    fn kind_defaults_to_vector_for_a_decl_that_never_declares_one() {
        let decl = decl(None, None, None);
        assert_eq!(decl.kind, CollectionKind::Vector);
        let canonical = build(None, &decl, None, None, caps(), None);
        assert_eq!(canonical.kind, CollectionKind::Vector);
    }

    /// The projection seam: `kind` reaches the canonical descriptor verbatim
    /// from the declaration, with no derivation and no second source. Every
    /// protocol root reads it from here.
    #[test]
    fn a_declared_record_kind_reaches_the_canonical_descriptor_verbatim() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: thesaurus\ncatalog: default\nstorage: main\nkind: record\n")
                .unwrap();
        let canonical = build(
            Some(&descriptor_with_attributes(None)),
            &decl,
            None,
            None,
            caps(),
            None,
        );
        assert_eq!(canonical.kind, CollectionKind::Record);
        assert!(!canonical.kind.has_geometry());
        assert!(canonical.kind.is_record());
    }

    /// `raster` is a label for a coverage, not a synonym for "no geometry":
    /// it must keep answering `has_geometry()`, or every raster collection
    /// would silently drop out of the tiles and maps lanes.
    #[test]
    fn a_declared_raster_kind_still_has_geometry() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: dem\ncatalog: default\nstorage: main\nkind: raster\n")
                .unwrap();
        assert_eq!(decl.kind, CollectionKind::Raster);
        assert!(decl.kind.has_geometry());
        assert!(!decl.kind.is_record());
    }

    // -- geometry profile (`#101`) -------------------------------------------

    /// A collection with no computed profile — every existing caller of
    /// `build` before `#101` — must see exactly `None` here, same as every
    /// other field this struct carried before this addition.
    #[test]
    fn geometry_profile_is_none_when_no_profile_was_computed() {
        let decl = decl(None, None, None);
        let canonical = build(None, &decl, None, None, caps(), None);
        assert!(canonical.geometry_profile.is_none());
    }

    #[test]
    fn geometry_profile_is_carried_through_unchanged_when_provided() {
        let decl = decl(None, None, None);
        let profile = GeometryProfile {
            sample_size: 512,
            computed_at: std::time::SystemTime::UNIX_EPOCH,
            vertices: VertexStats {
                mean: 4.0,
                median: 4.0,
                p95: 6.0,
                max: 12,
                total_estimated: Some(4096),
            },
            vertex_density_per_area: Some(0.5),
            multi_part_fraction: 0.1,
            mean_ring_count: Some(1.0),
            feature_size: FeatureSizeStats {
                p50: Some(10.0),
                p95: Some(20.0),
                max: Some(30.0),
            },
        };
        let canonical = build(None, &decl, None, None, caps(), Some(profile));
        assert_eq!(canonical.geometry_profile, Some(profile));
    }
}
