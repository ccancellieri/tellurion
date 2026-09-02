//! `GET /collections/{collectionId}/queryables` (OGC API Features Part 3,
//! "Queryables" requirements class, `#33` follow-up): a JSON Schema document
//! naming every property a `filter` expression against this collection may
//! reference. [`queryable_property_types`] is this document's underlying
//! type map, also consumed by `params::build_queryable_filter` to bind bare
//! `?propertyName=value` query parameters (the "Queryables as Query
//! Parameters" requirements class, `#52`) — one source of truth for what a
//! queryable is, shared by both requirements classes.
//!
//! [`build_document`] reads ONLY `CanonicalDescriptor`
//! (`tellurion_core::descriptor::canonical`, `#50` convergence) — the same
//! merged schema/geometry/datetime view `tellurion-stac`'s Collection
//! mapping consumes, rather than this module separately re-merging
//! `CollectionDescriptor`/`SchemaDecl` on its own. `canonical.schema`
//! already carries the exact refinement `SchemaDecl` (`#44`) applies to the
//! derived attribute view (a declared property's own `PropertyType` in place
//! of the SQL-type-inferred one, `additional_properties: false` dropping an
//! undeclared column entirely) — this module's own job is only translating
//! that merged shape into JSON Schema properties/format pairs plus the
//! `required` array, the same rule `filter::validate` checks a filter's
//! property names against (`filter::validate_attribute_property`), so this
//! document and the filter compiler's accepted-property set can never drift
//! apart; see this module's
//! `queryable_properties_match_what_the_filter_compiler_accepts` test.

use std::collections::BTreeMap;

use serde::Serialize;

use tellurion_core::{CanonicalDescriptor, PropertyType};

/// Response media type Requirement 3 (`/req/queryables/queryables-response`)
/// mandates for the queryables document.
pub const SCHEMA_JSON_MEDIA_TYPE: &str = "application/schema+json";

const JSON_SCHEMA_DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// Every spatial queryable advertises this `format`: `CollectionDescriptor`
/// does not carry the physical geometry type (`PhysicalCollection::
/// geometry_type` is derived but never threaded through to the descriptor,
/// `#19`), so the geometry column always uses the spec's explicit
/// "any geometry type" wildcard (Requirement 3E) rather than guessing one.
const GEOMETRY_FORMAT_ANY: &str = "geometry-any";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryablesDocument {
    #[serde(rename = "$schema")]
    pub schema: &'static str,
    /// The request URI without query parameters (Requirement 3, "the URI of
    /// the resource without query parameters") — a path only, matching how
    /// every other link this crate emits stays scheme/host-free (see
    /// `params::items_href`'s doc comment: absolute scheme/host is the
    /// server crate's concern).
    #[serde(rename = "$id")]
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub properties: BTreeMap<String, PropertySchema>,
    /// Names of every property the declared schema (`#44`) marks
    /// `required: true`. Empty — and omitted from the serialized document —
    /// for an undeclared collection, matching the derived-only document
    /// this crate produced before `#44`. Sorted for deterministic output.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

/// One queryable's schema. Untagged so each variant serializes as a plain
/// JSON object (no `Geometry`/`Scalar` wrapper key) — the two shapes are
/// mutually exclusive by construction (Requirement 3B: a spatial property
/// never has a `type` member; Requirement 3C: every other property must).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PropertySchema {
    /// A spatial queryable (Requirements 3B/3E): `format: geometry-*`,
    /// deliberately no `type` or `$ref` member.
    Geometry { title: String, format: &'static str },
    /// Every other queryable: a JSON Schema `type`, plus a `format` for
    /// date/date-time columns (Recommendation 1).
    Scalar {
        title: String,
        #[serde(rename = "type")]
        type_: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<&'static str>,
    },
}

/// Builds the queryables document for `collection_id`, whose merged
/// metadata is `canonical` (`tellurion_core::descriptor::canonical`, `#50`).
/// `id` is the request's `$id` value (already query-string-free — see
/// `QueryablesDocument::id`'s doc comment). `canonical.schema` being `None`
/// (no declared schema and no derivable attributes) produces the same
/// derived-only, no-`required` document this function always did before
/// `#44`/`#50` existed.
pub fn build_document(
    canonical: &CanonicalDescriptor,
    collection_id: &str,
    id: String,
) -> QueryablesDocument {
    let mut required: Vec<String> = canonical
        .schema
        .as_ref()
        .map(|schema| {
            schema
                .properties
                .iter()
                .filter(|(_, property)| property.required)
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
    required.sort();

    QueryablesDocument {
        schema: JSON_SCHEMA_DRAFT,
        id,
        title: collection_id.to_string(),
        type_: "object",
        properties: queryable_properties(canonical),
        required,
    }
}

/// Every *simple-valued* queryable's underlying [`PropertyType`], keyed by
/// property name — the one source of truth [`queryable_properties`] (this
/// document's own JSON Schema shapes) and `params::build_queryable_filter`
/// (bare `?propertyName=value` equality predicates, OGC API Features Part 3
/// Requirement 4, `/req/queryables-query-parameters/parameters`, `#52`) both
/// read instead of separately re-deriving "what's a queryable" from
/// `canonical.schema`/`.datetime`, so the queryables document and the
/// query-parameter mechanism can never quietly disagree on which properties
/// exist or what type each one is.
///
/// Deliberately excludes the geometry column: Requirement 4C scopes the
/// query-parameter mechanism to "every queryable ... that has a simple value
/// (string, number, integer or boolean)", and geometry has no [`PropertyType`]
/// at all (see that type's own doc comment) — [`queryable_properties`] adds
/// it back separately as a [`PropertySchema::Geometry`], which is exactly
/// the shape Requirement 4C's carve-out describes.
///
/// Same set `filter::validate` accepts against the same collection under the
/// identical rule (`#44`): `canonical.schema`'s already-merged property set
/// — which already excludes anything a closed (`additional_properties:
/// false`) declared schema drops, exactly as
/// `filter::validate_attribute_property` excludes it there — plus the
/// datetime column, always included regardless of `canonical.schema` (see
/// `tellurion_core::filter`'s "Property validation" docs).
pub fn queryable_property_types(canonical: &CanonicalDescriptor) -> BTreeMap<String, PropertyType> {
    let mut types = BTreeMap::new();

    if let Some(schema) = &canonical.schema {
        for (name, property) in &schema.properties {
            types.insert(name.clone(), property.type_);
        }
    }

    // The real driver's attribute schema already includes the datetime
    // column (only the geometry column is excluded, see
    // `tellurion-postgis`'s `ATTRIBUTE_SCHEMA_SQL`), so this is normally a
    // no-op; it only fires for a hand-built descriptor whose `datetime`
    // has no matching attribute column — still a property `filter::validate`
    // accepts, so it must still appear here, unconditionally on `canonical.schema`.
    if let Some(datetime) = &canonical.datetime {
        types
            .entry(datetime.value.clone())
            .or_insert(PropertyType::DateTime);
    }

    types
}

/// Every property `filter::validate` accepts against the same collection
/// under the identical rule (`#44`), rendered as a JSON Schema
/// [`PropertySchema`]: [`queryable_property_types`]'s scalar properties plus
/// the geometry column, which — having no [`PropertyType`] — is added here
/// directly rather than through that shared map. Insertion order here
/// doesn't matter for correctness (`BTreeMap` sorts on serialization), only
/// that the *set* of keys and each entry's shape match.
fn queryable_properties(canonical: &CanonicalDescriptor) -> BTreeMap<String, PropertySchema> {
    let mut properties: BTreeMap<String, PropertySchema> = queryable_property_types(canonical)
        .into_iter()
        .map(|(name, type_)| {
            let (json_type, format) = type_.json_schema_shape();
            let title = name.clone();
            (
                name,
                PropertySchema::Scalar {
                    title,
                    type_: json_type,
                    format,
                },
            )
        })
        .collect();

    // Always wins over any schema entry of the same name: the geometry
    // column is never a plain scalar, and the real driver already excludes
    // it from the attribute schema query, so this never actually overwrites
    // anything in practice. Unconditional on `canonical.schema` too — a
    // declared schema never enumerates the geometry column (`#44`,
    // `tellurion_core::descriptor::reconcile_schema` refuses that at boot-or-first-touch).
    if let Some(geometry) = &canonical.geometry {
        properties.insert(
            geometry.value.clone(),
            PropertySchema::Geometry {
                title: geometry.value.clone(),
                format: GEOMETRY_FORMAT_ANY,
            },
        );
    }

    properties
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::descriptor::canonical;
    use tellurion_core::filter::{self, Filter};
    use tellurion_core::{
        AttributeColumn, CollectionDecl, CollectionDescriptor, PropertyDecl, PropertyType,
        SchemaDecl,
    };

    /// No physical field overrides — every merged `canonical_for` fixture
    /// below is built from a bare `demo` collection, so provenance (never
    /// asserted on by this module's own tests; that's `descriptor::
    /// canonical`'s job) always comes out `Derived`.
    fn bare_decl() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }

    /// Merges `descriptor`/`schema` into a `CanonicalDescriptor` the same
    /// way `Router::canonical_descriptor` would, minus the live capability
    /// probe (irrelevant to this module's own property/required-array
    /// assertions) — the one seam this module's tests build their input
    /// through, so a fixture only needs a `CollectionDescriptor` plus an
    /// optional `SchemaDecl`, same as before `#50`.
    fn canonical_for(
        descriptor: &CollectionDescriptor,
        schema: Option<&SchemaDecl>,
    ) -> CanonicalDescriptor {
        canonical::build(
            Some(descriptor),
            &bare_decl(),
            schema,
            None,
            tellurion_core::CanonicalCapabilities::default(),
            None,
        )
    }

    /// Mirrors `tellurion_core::filter`'s own test fixture: a datetime
    /// column deliberately absent from `attributes`, so both the "derived
    /// from the attribute schema" and the "fallback for a bare descriptor
    /// datetime" paths in `queryable_properties` are exercised together.
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
                AttributeColumn {
                    name: "active".to_string(),
                    sql_type: "boolean".to_string(),
                },
                AttributeColumn {
                    name: "price".to_string(),
                    sql_type: "double precision".to_string(),
                },
            ]),
            geometry_type: None,
            projection: None,
        }
    }

    #[test]
    fn document_top_level_shape() {
        let canonical = canonical_for(&descriptor(), None);
        let doc = build_document(
            &canonical,
            "demo",
            "/collections/demo/queryables".to_string(),
        );
        assert_eq!(doc.schema, "https://json-schema.org/draft/2020-12/schema");
        assert_eq!(doc.id, "/collections/demo/queryables");
        assert_eq!(doc.title, "demo");
        assert_eq!(doc.type_, "object");
        assert!(
            doc.required.is_empty(),
            "an undeclared collection has no 'required' array"
        );
    }

    #[test]
    fn maps_every_known_sql_type_to_its_json_schema_shape() {
        let canonical = canonical_for(&descriptor(), None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(
            doc.properties["name"],
            PropertySchema::Scalar {
                title: "name".to_string(),
                type_: "string",
                format: None,
            }
        );
        assert_eq!(
            doc.properties["population"],
            PropertySchema::Scalar {
                title: "population".to_string(),
                type_: "integer",
                format: None,
            }
        );
        assert_eq!(
            doc.properties["active"],
            PropertySchema::Scalar {
                title: "active".to_string(),
                type_: "boolean",
                format: None,
            }
        );
        assert_eq!(
            doc.properties["price"],
            PropertySchema::Scalar {
                title: "price".to_string(),
                type_: "number",
                format: None,
            }
        );
    }

    /// A single-attribute descriptor fixture — one undeclared column of
    /// `sql_type`, no geometry/datetime/pk — isolating exactly the
    /// SQL-type-inference path `canonical::build_schema` exercises for a
    /// `Provenance::Derived` property.
    fn descriptor_with_one_attribute(name: &str, sql_type: &str) -> CollectionDescriptor {
        CollectionDescriptor {
            table: "demo".to_string(),
            geometry: None,
            pk: None,
            datetime: None,
            srid: None,
            extent: None,
            row_estimate: None,
            attributes: Some(vec![AttributeColumn {
                name: name.to_string(),
                sql_type: sql_type.to_string(),
            }]),
            geometry_type: None,
            projection: None,
        }
    }

    #[test]
    fn an_unrecognized_sql_type_falls_back_to_string() {
        let canonical = canonical_for(&descriptor_with_one_attribute("payload", "jsonb"), None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(
            doc.properties["payload"],
            PropertySchema::Scalar {
                title: "payload".to_string(),
                type_: "string",
                format: None,
            }
        );
    }

    #[test]
    fn a_date_column_gets_the_date_format() {
        let canonical = canonical_for(&descriptor_with_one_attribute("valid_on", "date"), None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(
            doc.properties["valid_on"],
            PropertySchema::Scalar {
                title: "valid_on".to_string(),
                type_: "string",
                format: Some("date"),
            }
        );
    }

    #[test]
    fn a_timestamp_column_gets_the_date_time_format() {
        let canonical = canonical_for(
            &descriptor_with_one_attribute("observed_at", "timestamp with time zone"),
            None,
        );
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(
            doc.properties["observed_at"],
            PropertySchema::Scalar {
                title: "observed_at".to_string(),
                type_: "string",
                format: Some("date-time"),
            }
        );
    }

    #[test]
    fn the_datetime_column_falls_back_to_date_time_format_when_absent_from_attributes() {
        // `descriptor()`'s `datetime` ("observed_at") is deliberately not in
        // `attributes` — the fallback branch in `queryable_properties`.
        let canonical = canonical_for(&descriptor(), None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(
            doc.properties["observed_at"],
            PropertySchema::Scalar {
                title: "observed_at".to_string(),
                type_: "string",
                format: Some("date-time"),
            }
        );
    }

    #[test]
    fn the_geometry_column_uses_the_format_geometry_idiom_with_no_type_member() {
        let canonical = canonical_for(&descriptor(), None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(
            doc.properties["geom"],
            PropertySchema::Geometry {
                title: "geom".to_string(),
                format: "geometry-any",
            }
        );
        // Serialized form must have no "type"/"$ref" member on this entry
        // (Requirement 3B) — asserting on the `Value` catches a future
        // accidental `#[serde(flatten)]`/derive change the typed equality
        // check above wouldn't.
        let value = serde_json::to_value(&doc.properties["geom"]).unwrap();
        assert!(value.get("type").is_none());
        assert!(value.get("$ref").is_none());
        assert_eq!(value["format"], "geometry-any");
    }

    #[test]
    fn serialized_document_omits_format_when_absent() {
        let canonical = canonical_for(&descriptor(), None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        let value = serde_json::to_value(&doc.properties["name"]).unwrap();
        assert!(
            value.get("format").is_none(),
            "a plain string property must not serialize a null 'format' key"
        );
    }

    /// The pinning test the drift concern calls for: every property this
    /// module exposes must be one `tellurion_core::filter::validate` actually
    /// accepts against the same descriptor, and every property
    /// `filter::validate` accepts must appear here — computed independently
    /// (straight from `descriptor.attributes`/`.geometry`/`.datetime`, not by
    /// calling `queryable_properties` again) so a future edit to either side
    /// alone breaks this test. No-schema regression guard (`#44`): passing
    /// `None` here exercises exactly the derived-only behavior this module
    /// had before declared schemas existed.
    #[test]
    fn queryable_properties_match_what_the_filter_compiler_accepts() {
        let descriptor = descriptor();
        let canonical = canonical_for(&descriptor, None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());

        for name in doc.properties.keys() {
            let probe = Filter::IsNull {
                property: name.clone(),
                negated: false,
            };
            assert!(
                filter::validate(&probe, &descriptor, None).is_ok(),
                "queryable '{name}' must be a property the filter compiler accepts"
            );
        }

        let mut expected: Vec<String> = descriptor
            .attributes
            .iter()
            .flatten()
            .map(|a| a.name.clone())
            .collect();
        if let Some(geometry) = &descriptor.geometry {
            if !expected.contains(geometry) {
                expected.push(geometry.clone());
            }
        }
        if let Some(datetime) = &descriptor.datetime {
            if !expected.contains(datetime) {
                expected.push(datetime.clone());
            }
        }
        expected.sort();

        let mut actual: Vec<String> = doc.properties.keys().cloned().collect();
        actual.sort();

        assert_eq!(
            actual, expected,
            "queryables property set must exactly match filter::validate's accepted properties"
        );
    }

    /// Same drift-pinning property as
    /// `queryable_properties_match_what_the_filter_compiler_accepts`, now
    /// exercised with a closed declared schema (`additional_properties:
    /// false`, `#44`): the document must drop to exactly the schema's own
    /// declared properties plus geometry/datetime, matching what
    /// `filter::validate` accepts under that same schema.
    #[test]
    fn queryable_properties_match_the_filter_compiler_under_a_closed_declared_schema() {
        let descriptor = descriptor();
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "population".to_string(),
                type_: PropertyType::Integer,
                required: false,
            }],
            additional_properties: false,
        };
        let canonical = canonical_for(&descriptor, Some(&schema));
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());

        for name in doc.properties.keys() {
            let probe = Filter::IsNull {
                property: name.clone(),
                negated: false,
            };
            assert!(
                filter::validate(&probe, &descriptor, Some(&schema)).is_ok(),
                "queryable '{name}' must be a property the filter compiler accepts under this schema"
            );
        }

        // Every attribute *not* declared by the schema must be rejected by
        // the filter compiler under this schema — and absent from the
        // document — proving the two sides agree in both directions.
        for attribute in descriptor.attributes.iter().flatten() {
            if attribute.name == "population" {
                continue;
            }
            assert!(
                !doc.properties.contains_key(&attribute.name),
                "'{}' is not declared by the closed schema, so it must not appear",
                attribute.name
            );
            let probe = Filter::IsNull {
                property: attribute.name.clone(),
                negated: false,
            };
            assert!(filter::validate(&probe, &descriptor, Some(&schema)).is_err());
        }

        let mut actual: Vec<&str> = doc.properties.keys().map(String::as_str).collect();
        actual.sort();
        assert_eq!(actual, vec!["geom", "observed_at", "population"]);
    }

    #[test]
    fn a_declared_property_type_overrides_the_sql_type_derived_one() {
        // `name`'s backend SQL type is "text" (-> "string" by inference);
        // declaring it "integer" here proves the declaration wins, not the
        // inferred shape — the whole point of "types ... refine the derived
        // view".
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "name".to_string(),
                type_: PropertyType::Integer,
                required: false,
            }],
            additional_properties: true,
        };
        let canonical = canonical_for(&descriptor(), Some(&schema));
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(
            doc.properties["name"],
            PropertySchema::Scalar {
                title: "name".to_string(),
                type_: "integer",
                format: None,
            }
        );
    }

    #[test]
    fn required_declared_properties_populate_the_top_level_required_array() {
        let schema = SchemaDecl {
            properties: vec![
                PropertyDecl {
                    name: "population".to_string(),
                    type_: PropertyType::Integer,
                    required: true,
                },
                PropertyDecl {
                    name: "name".to_string(),
                    type_: PropertyType::String,
                    required: false,
                },
            ],
            additional_properties: true,
        };
        let canonical = canonical_for(&descriptor(), Some(&schema));
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        assert_eq!(doc.required, vec!["population".to_string()]);

        let value = serde_json::to_value(&doc).unwrap();
        assert_eq!(value["required"], serde_json::json!(["population"]));
    }

    #[test]
    fn an_undeclared_collection_serializes_with_no_required_member_at_all() {
        let canonical = canonical_for(&descriptor(), None);
        let doc = build_document(&canonical, "demo", "irrelevant".to_string());
        let value = serde_json::to_value(&doc).unwrap();
        assert!(
            value.get("required").is_none(),
            "an undeclared collection's document must omit 'required' entirely, not serialize an empty array"
        );
    }

    #[test]
    fn a_collection_with_no_derivable_attributes_still_produces_a_valid_document() {
        let descriptor = CollectionDescriptor {
            table: "demo".to_string(),
            geometry: None,
            pk: None,
            datetime: None,
            srid: None,
            extent: None,
            row_estimate: None,
            attributes: None,
            geometry_type: None,
            projection: None,
        };
        let canonical = canonical_for(&descriptor, None);
        let doc = build_document(
            &canonical,
            "demo",
            "/collections/demo/queryables".to_string(),
        );
        assert!(doc.properties.is_empty());
    }
}
