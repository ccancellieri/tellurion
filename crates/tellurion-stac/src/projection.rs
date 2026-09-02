//! STAC `projection` extension (`proj:*`) derivation (`#36`): turns the
//! backend-known [`ProjectionFacts`] a collection's driver reported (plus
//! the SRID the derived descriptor already carries for every SQL-backed
//! vector collection) into the `proj:` fields an Item can genuinely stand
//! behind, and keeps the operator-override channel *visible* — the decision
//! recorded on `#36` (2026-08-06):
//!
//! - **Derivation is the default and needs no configuration.** Each field
//!   is emitted only where the driver genuinely knows it. A field the
//!   driver does not know is NOT emitted — not as `null`, not defaulted: an
//!   identity `proj:transform` is the exact kind of plausible-but-invented
//!   value this campaign forbids, and plausible is worse than absent.
//! - The extension URI is declared in `stac_extensions` **only if at least
//!   one `proj:` field was actually derived** — declaring while emitting
//!   nothing is the `#287` defect one document over.
//! - **An operator override wins, but a disagreement is never silent.**
//!   The override channel is the per-item STAC metadata sidecar (`#202`) —
//!   the one operator-supplied per-item passthrough this lane already has,
//!   deliberately reused rather than a second parallel channel. When the
//!   sidecar supplies a `proj:` field the driver *also* derived and the two
//!   disagree, the disagreement is logged once per collection per field —
//!   at first materialization, not per request (the sidecar lives in the
//!   collection's own storage, so boot cannot see its rows without a scan
//!   this lane never does) — naming the collection, the field, the derived
//!   value and the override. An override for a field the driver could NOT
//!   derive is silent (pure gap-filling, nothing to disagree with), and an
//!   agreeing override logs nothing (the log stays a signal, not noise).

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use tellurion_core::ProjectionFacts;

/// The one extension URI this module ever declares — the released `v1.1.0`
/// schema of `stac-extensions/projection`, verified against that repo's own
/// tags (never invented; `v1.1.0` is the release whose field set —
/// `proj:epsg`/`proj:transform`/`proj:shape` — this module emits).
pub const PROJECTION_EXTENSION_URI: &str =
    "https://stac-extensions.github.io/projection/v1.1.0/schema.json";

/// The `proj:` fields derived for one collection — built once per
/// collection per request page ([`derive_projection`]) and applied to every
/// Item on it by `mapping::to_stac_item`. Non-empty by construction:
/// "nothing derivable" is `None` at the [`derive_projection`] boundary, so
/// a `DerivedProjection` in hand is itself the proof that the extension URI
/// belongs on the document.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedProjection {
    /// `(field name, derived value)` pairs, e.g. `("proj:epsg", 4326)` —
    /// only fields genuinely known ever appear.
    pub fields: Vec<(&'static str, Value)>,
}

/// Derives the `proj:` fields for one collection from what its driver
/// genuinely knows: `facts` is the driver's own
/// [`CatalogSource::projection`](tellurion_core::CatalogSource::projection)
/// answer (carried on the effective decl, `CollectionDecl::projection`),
/// and `srid` is the derived descriptor's SRID carrier
/// (`CollectionDecl::srid`) — the way every SQL vector backend (PostGIS,
/// GeoPackage, the memory driver's fixed CRS84) already reports its
/// geometry column's CRS, read here as an EPSG code exactly as
/// `tellurion_core::crs::epsg_uri` already does for `storageCrs`.
///
/// `facts.epsg` wins over `srid` when both are present (they cannot
/// genuinely disagree — a driver reporting both reads them from the same
/// georeferencing — but the driver-specific accessor is the more explicit
/// claim). `None` when nothing at all is known: the caller then emits no
/// `proj:` field and no extension URI, and the Item stays byte-identical to
/// what this crate served before the extension existed.
pub fn derive_projection(
    facts: Option<&ProjectionFacts>,
    srid: Option<i32>,
) -> Option<DerivedProjection> {
    let mut fields: Vec<(&'static str, Value)> = Vec::new();
    if let Some(epsg) = facts.and_then(|facts| facts.epsg).or(srid) {
        fields.push(("proj:epsg", Value::from(epsg)));
    }
    if let Some(transform) = facts.and_then(|facts| facts.transform) {
        fields.push(("proj:transform", Value::from(transform.to_vec())));
    }
    if let Some(shape) = facts.and_then(|facts| facts.shape) {
        fields.push(("proj:shape", Value::from(shape.to_vec())));
    }
    if fields.is_empty() {
        return None;
    }
    Some(DerivedProjection { fields })
}

/// `(collection external id, field)` pairs whose derived-vs-override
/// disagreement has already been logged, process-wide — the "once per
/// collection, not per request" dedup the `#36` decision requires. A plain
/// process-global rather than router-held state: the set only ever grows by
/// one short entry per genuinely disagreeing `(collection, field)` pair,
/// and a restart re-logging each disagreement once is exactly the "an
/// operator sees the mistake the first time the server starts" behavior the
/// decision describes.
static LOGGED_DISAGREEMENTS: OnceLock<Mutex<BTreeSet<(String, String)>>> = OnceLock::new();

/// Logs one derived-vs-override `proj:` disagreement — the operator's
/// sidecar value won (that is the point of the override), and this makes
/// the divergence loud instead of invisible: an operator correcting bad
/// SRID metadata in a legacy table sees a message confirming exactly that,
/// and one who pasted a stale block from another collection sees the
/// mistake at first materialization rather than months later in a client
/// bug report. Deduplicated per `(collection, field)` for the process
/// lifetime; returns whether this call actually logged (so a test can pin
/// the dedup without capturing the subscriber twice).
pub(crate) fn log_override_disagreement(
    collection: &str,
    field: &str,
    derived: &Value,
    supplied: &Value,
) -> bool {
    let registry = LOGGED_DISAGREEMENTS.get_or_init(|| Mutex::new(BTreeSet::new()));
    let mut logged = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !logged.insert((collection.to_string(), field.to_string())) {
        return false;
    }
    tracing::warn!(
        collection,
        field,
        derived = %derived,
        supplied = %supplied,
        "sidecar proj override disagrees with the driver-derived value; serving the override"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nothing_known_derives_nothing_at_all() {
        assert_eq!(derive_projection(None, None), None);
        let empty = ProjectionFacts {
            epsg: None,
            transform: None,
            shape: None,
        };
        assert_eq!(derive_projection(Some(&empty), None), None);
    }

    /// The campaign's own forbidden default, pinned: a driver that knows
    /// only its EPSG code yields exactly one field — no identity
    /// `proj:transform`, no `proj:shape`, no `null` stand-ins.
    #[test]
    fn an_underivable_field_is_not_emitted_not_even_as_null() {
        let derived = derive_projection(None, Some(4326)).expect("epsg alone derives");
        assert_eq!(derived.fields, vec![("proj:epsg", json!(4326))]);
    }

    #[test]
    fn full_raster_facts_derive_all_three_fields_in_stac_shape() {
        let facts = ProjectionFacts {
            epsg: Some(4326),
            transform: Some([0.01, 0.0, -1.28, 0.0, -0.01, 1.28]),
            shape: Some([512, 1024]),
        };
        let derived = derive_projection(Some(&facts), Some(4326)).unwrap();
        assert_eq!(
            derived.fields,
            vec![
                ("proj:epsg", json!(4326)),
                (
                    "proj:transform",
                    json!([0.01, 0.0, -1.28, 0.0, -0.01, 1.28])
                ),
                ("proj:shape", json!([512, 1024])),
            ]
        );
    }

    /// The driver's own accessor is the more explicit claim when both it
    /// and the descriptor SRID carry an EPSG code.
    #[test]
    fn driver_facts_epsg_wins_over_the_descriptor_srid() {
        let facts = ProjectionFacts {
            epsg: Some(3035),
            transform: None,
            shape: None,
        };
        let derived = derive_projection(Some(&facts), Some(4326)).unwrap();
        assert_eq!(derived.fields, vec![("proj:epsg", json!(3035))]);
    }

    #[test]
    fn a_disagreement_is_logged_once_per_collection_and_field() {
        assert!(log_override_disagreement(
            "dedup-collection",
            "proj:epsg",
            &json!(4326),
            &json!(3857)
        ));
        assert!(
            !log_override_disagreement("dedup-collection", "proj:epsg", &json!(4326), &json!(3857)),
            "second materialization of the same disagreement must not log again"
        );
        assert!(
            log_override_disagreement("dedup-collection", "proj:shape", &json!(1), &json!(2)),
            "a different field of the same collection is its own signal"
        );
    }
}
