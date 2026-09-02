//! Items-page byte budgeting (`#184`).
//!
//! The response-shaping counterpart to `items_budget`'s refusal contract:
//! where the vertex budget rejects an over-budget page outright (a
//! correctness bound on geometry cost, enforced as a `FeatureSource`
//! decorator), the byte budget *trims* — an over-budget page is served
//! shorter, with `next_token` re-minted so paging picks the dropped tail
//! back up. That is deliberately NOT a source contract: drivers keep
//! returning whatever their own keyset paging produced, and the features
//! handler (`tellurion-features::handlers::list_items`) applies this policy
//! on the way out, only when the settings chain resolved a
//! `settings.page_max_bytes` at all (`None` — the default — means this
//! module is never called and behavior is exactly pre-`#184`).

use serde_json::Value;

use crate::storage::FeaturePage;

/// Trims `page` to the longest front-to-back feature prefix whose
/// cumulative serialized size stays within `budget` bytes — but ALWAYS
/// keeps at least the first feature, so a single oversized row is still
/// served and paging can advance past it instead of looping forever.
///
/// Per-feature cost is `serde_json::to_vec(feature).len()` — feature bytes
/// only, deliberately ignoring the surrounding FeatureCollection envelope
/// and `,` separators: the budget bounds payload *scale*, and counting the
/// few bytes of framing overhead would buy no real precision for the cost
/// of coupling this to the response's exact wire framing.
///
/// When features are dropped, `next_token` is re-minted from the last KEPT
/// feature's GeoJSON `id` (numeric ids stringified, string ids as-is) —
/// correct because every driver's own `next_token` is the pk of the last
/// returned row and every driver stamps that pk onto the feature's `id`
/// (see `tellurion-postgis::driver` and `tellurion-geopackage::driver`), so
/// the next request resumes right after the last feature actually served.
/// `number_matched` is left untouched: it counts the total match, not this
/// page. A last-kept feature without a string/numeric `id` (unreachable for
/// tellurion drivers, which always expose the pk as the id) leaves the page
/// untrimmed rather than mint a token that would silently skip rows.
///
/// A no-op on an empty page and on a page already within budget.
pub fn truncate_page_to_byte_budget(collection: &str, page: &mut FeaturePage, budget: u64) {
    if page.features_geojson.is_empty() {
        return;
    }
    let mut cumulative_bytes = 0_u64;
    let mut kept = 0_usize;
    for feature in &page.features_geojson {
        let feature_bytes = serialized_len(feature);
        // The first feature is admitted unconditionally — see the doc
        // comment: an oversized single row must still be served.
        if kept > 0 && cumulative_bytes.saturating_add(feature_bytes) > budget {
            break;
        }
        cumulative_bytes = cumulative_bytes.saturating_add(feature_bytes);
        kept += 1;
    }
    let dropped = page.features_geojson.len() - kept;
    if dropped == 0 {
        return;
    }
    let Some(token) = feature_id_token(&page.features_geojson[kept - 1]) else {
        return;
    };
    page.features_geojson.truncate(kept);
    page.next_token = Some(token);
    metrics::counter!("items_page_byte_truncated_total").increment(1);
    tracing::debug!(
        collection,
        kept,
        dropped,
        budget,
        "trimmed items page to the configured page_max_bytes budget"
    );
}

/// One feature's serialized cost in bytes. Serialization of a
/// `serde_json::Value` cannot fail in practice (every key is already a
/// string); the `unwrap_or(0)` keeps this total function anyway rather
/// than let a budget check panic a read path.
fn serialized_len(feature: &Value) -> u64 {
    serde_json::to_vec(feature)
        .map(|bytes| bytes.len() as u64)
        .unwrap_or(0)
}

/// The paging token a kept feature stands for: its GeoJSON `id`, which
/// every tellurion driver stamps with the row's pk — string ids as-is,
/// numeric ids via `to_string` (matching how the drivers themselves
/// stringify a numeric pk into `next_token`). `None` for any other shape.
fn feature_id_token(feature: &Value) -> Option<String> {
    match feature.get("id") {
        Some(Value::String(id)) => Some(id.clone()),
        Some(Value::Number(id)) => Some(id.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn feature(id: Value, pad: usize) -> Value {
        json!({
            "type": "Feature",
            "id": id,
            "geometry": null,
            "properties": { "pad": "x".repeat(pad) }
        })
    }

    fn page(features: Vec<Value>, next_token: Option<&str>) -> FeaturePage {
        FeaturePage {
            features_geojson: features,
            number_matched: Some(99),
            next_token: next_token.map(str::to_string),
        }
    }

    #[test]
    fn under_budget_page_is_untouched_token_included() {
        let original = page(
            vec![feature(json!("a"), 0), feature(json!("b"), 0)],
            Some("b"),
        );
        let mut trimmed = original.clone();
        truncate_page_to_byte_budget("places", &mut trimmed, 1_000_000);
        assert_eq!(trimmed, original);
    }

    #[test]
    fn empty_page_is_a_no_op() {
        let original = page(vec![], None);
        let mut trimmed = original.clone();
        truncate_page_to_byte_budget("places", &mut trimmed, 1);
        assert_eq!(trimmed, original);
    }

    #[test]
    fn drops_the_tail_and_mints_the_token_from_the_last_kept_string_id() {
        let first = feature(json!("a"), 0);
        let budget = serde_json::to_vec(&first).unwrap().len() as u64;
        let mut trimmed = page(
            vec![
                first.clone(),
                feature(json!("b"), 0),
                feature(json!("c"), 0),
            ],
            Some("c"),
        );
        truncate_page_to_byte_budget("places", &mut trimmed, budget);
        assert_eq!(trimmed.features_geojson, vec![first]);
        assert_eq!(trimmed.next_token.as_deref(), Some("a"));
        // The total match count is untouched — it never meant "this page".
        assert_eq!(trimmed.number_matched, Some(99));
    }

    #[test]
    fn numeric_ids_are_stringified_into_the_token() {
        let first = feature(json!(41), 0);
        let second = feature(json!(42), 0);
        let budget = (serde_json::to_vec(&first).unwrap().len()
            + serde_json::to_vec(&second).unwrap().len()) as u64;
        let mut trimmed = page(
            vec![first.clone(), second.clone(), feature(json!(43), 200)],
            None,
        );
        truncate_page_to_byte_budget("places", &mut trimmed, budget);
        assert_eq!(trimmed.features_geojson, vec![first, second]);
        assert_eq!(trimmed.next_token.as_deref(), Some("42"));
    }

    #[test]
    fn a_single_over_budget_feature_is_still_served_and_paging_advances() {
        let oversized = feature(json!("huge"), 4_096);
        let mut trimmed = page(vec![oversized.clone(), feature(json!("b"), 0)], Some("b"));
        truncate_page_to_byte_budget("places", &mut trimmed, 1);
        assert_eq!(trimmed.features_geojson, vec![oversized]);
        assert_eq!(trimmed.next_token.as_deref(), Some("huge"));
    }

    #[test]
    fn exact_budget_boundary_keeps_the_fitting_prefix() {
        let first = feature(json!("a"), 0);
        let second = feature(json!("b"), 0);
        let both = (serde_json::to_vec(&first).unwrap().len()
            + serde_json::to_vec(&second).unwrap().len()) as u64;
        let mut untouched = page(vec![first.clone(), second.clone()], None);
        truncate_page_to_byte_budget("places", &mut untouched, both);
        assert_eq!(untouched.features_geojson.len(), 2);
        assert_eq!(untouched.next_token, None);

        let mut trimmed = page(vec![first.clone(), second], None);
        truncate_page_to_byte_budget("places", &mut trimmed, both - 1);
        assert_eq!(trimmed.features_geojson, vec![first]);
        assert_eq!(trimmed.next_token.as_deref(), Some("a"));
    }
}
