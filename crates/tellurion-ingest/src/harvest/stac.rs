//! Pure STAC-API walking for `harvest stac` (`#191`): document parsing,
//! `rel=next` resolution, and STAC Item -> canonical-write-lane feature
//! mapping. Every function here is a pure transform over already-fetched
//! text — the only I/O in this module is [`CurlFetch`] at the bottom, the
//! one place a page body comes from. Orchestration (registry lookup, write
//! sink, bookmark, report) lives in `harvest.rs`.
//!
//! The harvester is deliberately a *narrow* STAC client: it follows plain
//! `GET` `rel=next` links and nothing else. Every shape it will not follow
//! (a POST/body paging link, a templated href, a non-http scheme, a
//! self-referential next link) is refused by name rather than silently
//! skipped, because a silently-skipped page is a silently-incomplete
//! harvest — and the whole point of `#191` is that a harvest is a faithful
//! replay through the canonical write lane.

use std::collections::BTreeSet;

use serde_json::Value;

/// Hard cap on one fetched document. Reuses the write lane's own buffered
/// -input limit rather than inventing a second number: a STAC page that
/// cannot fit the batch route's buffered `FeatureCollection` limit is not a
/// page this CLI is willing to stage into a batch either.
pub const MAX_DOCUMENT_BYTES: u64 = tellurion_core::DEFAULT_BATCH_MAX_BYTES;

/// One collection advertised by the source's `GET /collections`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCollection {
    /// The collection's STAC id, exactly as the source spells it.
    pub id: String,
    /// The already-resolved absolute href of this collection's own
    /// `rel=items` link, when the source advertises a plain `GET` one.
    /// `None` falls back to the STAC API spec's own
    /// `/collections/{id}/items` path (see [`items_url`]).
    pub items_href: Option<String>,
}

/// One page of items: the features it carried, plus the already-resolved
/// absolute href of its `rel=next` link (`None` = last page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemsPage {
    pub features: Vec<Value>,
    pub next: Option<String>,
}

/// One STAC Item mapped onto the shape the canonical write lane accepts,
/// plus what the mapping had to leave behind — reported, never silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedItem {
    /// A GeoJSON Feature rebuilt from scratch out of exactly the members
    /// `stage_batch_feature` reads: `type`, `id`, `geometry`, `bbox` (when
    /// the item carried one) and `properties`. Nothing rides along
    /// implicitly.
    pub feature: Value,
    /// Property names present on the item that this mapping dropped, sorted.
    pub dropped_properties: Vec<String>,
    /// How many assets the item advertised. Assets are *counted and
    /// reported*, never fetched and never written: the canonical write lane
    /// has no asset column, and `#191`'s own rule is that a remote asset is
    /// href-only (virtual) — adoption by the assets subsystem (`#93`) is a
    /// later slice, not this one.
    pub assets: u64,
}

/// Trims a source root to its canonical no-trailing-slash form and refuses
/// anything this harvester cannot walk. Only `http`/`https` are accepted:
/// the fetcher shells out to `curl`, and a `file://` "catalog" would make
/// the harvest's own provenance (which server served this?) meaningless.
pub fn normalize_root(source: &str) -> anyhow::Result<String> {
    let (scheme, rest) = split_scheme(source)
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .ok_or_else(|| {
            anyhow::anyhow!("--source must be an http(s) STAC API root URL (got '{source}')")
        })?;
    let rest = rest.trim_end_matches('/');
    let authority_end = rest.find('/').unwrap_or(rest.len());
    anyhow::ensure!(authority_end > 0, "--source '{source}' has no host");
    Ok(format!("{scheme}://{rest}"))
}

/// Parses a `GET /collections` body into the collections it advertises, in
/// document order. A collections document whose entries are not objects
/// carrying a non-empty string `id` is refused outright rather than
/// partially harvested: an unnamed collection has no target to map onto.
pub fn parse_collections(body: &str, document_url: &str) -> anyhow::Result<Vec<RemoteCollection>> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| anyhow::anyhow!("'{document_url}' is not valid JSON: {error}"))?;
    let entries = value
        .get("collections")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("'{document_url}' has no 'collections' array; is this a STAC API root?")
        })?;

    let mut collections = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    for (position, entry) in entries.iter().enumerate() {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "'{document_url}' collection #{position} has no non-empty string 'id'"
                )
            })?;
        anyhow::ensure!(
            seen.insert(id.to_string()),
            "'{document_url}' advertises collection '{id}' more than once"
        );
        let items_href = typed_link(entry.get("links"), "items", document_url)?;
        collections.push(RemoteCollection {
            id: id.to_string(),
            items_href,
        });
    }
    Ok(collections)
}

/// The URL this harvester will page items from: the collection's own
/// advertised `rel=items` link when it has a plain `GET` one, else the STAC
/// API spec's `/collections/{id}/items` path under `root`.
///
/// The fallback refuses an id that cannot be dropped into a path segment
/// verbatim. Percent-encoding one here would mean guessing which encoding
/// the source expects to read back, and a guessed id is exactly the kind of
/// invented default this CLI does not ship; an id like that must come with
/// its own `rel=items` link.
pub fn items_url(root: &str, collection: &RemoteCollection) -> anyhow::Result<String> {
    if let Some(href) = &collection.items_href {
        return Ok(href.clone());
    }
    anyhow::ensure!(
        !collection.id.is_empty()
            && collection.id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~')
            }),
        "collection id '{}' is not a bare path segment and the source advertises no 'items' link \
         for it; this harvester never invents a percent-encoding for it",
        collection.id
    );
    Ok(format!("{root}/collections/{}/items", collection.id))
}

/// Parses one items page. `page_url` is the URL that produced `body` — it
/// both names the document in every refusal and serves as the base for
/// resolving a relative `rel=next` href.
pub fn parse_items_page(body: &str, page_url: &str) -> anyhow::Result<ItemsPage> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| anyhow::anyhow!("'{page_url}' is not valid JSON: {error}"))?;
    anyhow::ensure!(
        value.get("type").and_then(Value::as_str) == Some("FeatureCollection"),
        "'{page_url}' is not a GeoJSON FeatureCollection; STAC items pages always are"
    );
    let features = value
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("'{page_url}' has no 'features' array"))?
        .clone();

    let next = typed_link(value.get("links"), "next", page_url)?;
    if let Some(next) = &next {
        anyhow::ensure!(
            next != page_url,
            "'{page_url}' advertises itself as its own 'next' page; refusing to loop"
        );
    }
    Ok(ItemsPage { features, next })
}

/// Finds the first link with `rel == relation` in an already-parsed `links`
/// array and resolves its href against `base`.
///
/// Named refusals, all of them shapes this harvester genuinely cannot
/// follow rather than shapes it merely dislikes:
///
/// - `method` other than `GET` (STAC API's POST-body paging).
/// - a `body`/`merge` member (same POST-body paging, spelled without
///   `method`).
/// - `templated: true` — there is no variable binding to fill it from.
/// - a missing or non-string `href`.
fn typed_link(links: Option<&Value>, relation: &str, base: &str) -> anyhow::Result<Option<String>> {
    let Some(links) = links else {
        return Ok(None);
    };
    let links = links
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("'{base}' has a 'links' member that is not an array"))?;
    let Some(link) = links
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some(relation))
    else {
        return Ok(None);
    };

    if let Some(method) = link.get("method").and_then(Value::as_str) {
        anyhow::ensure!(
            method.eq_ignore_ascii_case("GET"),
            "'{base}' advertises a '{relation}' link with method '{method}'; this harvester \
             follows only GET links"
        );
    }
    anyhow::ensure!(
        link.get("body").is_none() && link.get("merge").is_none(),
        "'{base}' advertises a '{relation}' link carrying a request body; this harvester follows \
         only GET links"
    );
    anyhow::ensure!(
        link.get("templated") != Some(&Value::Bool(true)),
        "'{base}' advertises a templated '{relation}' link; this harvester has nothing to fill \
         its variables from"
    );
    let href = link
        .get("href")
        .and_then(Value::as_str)
        .filter(|href| !href.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("'{base}' advertises a '{relation}' link with no non-empty 'href'")
        })?;
    resolve_href(base, href).map(Some)
}

/// Resolves `href` against `base` — the subset of RFC 3986 reference
/// resolution a STAC link actually uses (absolute, protocol-relative,
/// root-relative, query-only, and same-directory relative references),
/// implemented here rather than pulling a URL parser into a CLI that
/// already shells out to `curl` for its only network call.
///
/// A reference naming any scheme other than http/https is refused by name:
/// the fetcher cannot dereference it, and following it silently would make
/// the harvest's provenance a lie.
pub fn resolve_href(base: &str, href: &str) -> anyhow::Result<String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        return Ok(href.to_string());
    }
    if let Some(scheme_end) = href.find("://") {
        anyhow::bail!(
            "link href '{href}' uses the '{}' scheme; this harvester dereferences only http(s)",
            &href[..scheme_end]
        );
    }
    let (scheme, rest) = split_scheme(base)
        .ok_or_else(|| anyhow::anyhow!("cannot resolve '{href}' against non-absolute '{base}'"))?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let path = &rest[authority_end..];

    if let Some(protocol_relative) = href.strip_prefix("//") {
        return Ok(format!("{scheme}://{protocol_relative}"));
    }
    if href.starts_with('/') {
        return Ok(format!("{scheme}://{authority}{href}"));
    }
    // Everything below resolves against the base's *path*, so the base's own
    // query and fragment drop out first — RFC 3986 §5.3's own composition.
    let path = path
        .split(['?', '#'])
        .next()
        .expect("split always yields at least one element");
    if href.starts_with('?') || href.starts_with('#') {
        return Ok(format!("{scheme}://{authority}{path}{href}"));
    }
    let directory = match path.rfind('/') {
        Some(index) => &path[..=index],
        None => "/",
    };
    Ok(format!("{scheme}://{authority}{directory}{href}"))
}

fn split_scheme(url: &str) -> Option<(&str, &str)> {
    let index = url.find("://")?;
    Some((&url[..index], &url[index + 3..]))
}

/// Maps one STAC Item onto the GeoJSON Feature the canonical write lane
/// stages (`tellurion_core::stage_batch_feature`), or names why it cannot.
///
/// The result is *rebuilt*, never edited in place: `links`, `collection`,
/// `stac_version`, `stac_extensions` and `assets` are all left behind on
/// purpose. A harvested item's links belong to the source server, its
/// `collection` member names the *remote* id (wrong the moment `--map`
/// renames it), and the write lane has no column for either.
///
/// `declared` is the target collection's declared property names when it
/// pins a schema. Property handling has exactly two rules, and neither
/// invents a column:
///
/// - **Schema pinned:** keep exactly the declared names the item carries;
///   every other property is dropped and reported.
/// - **No schema:** keep the scalar-valued properties (string, number,
///   bool, null) and drop the object/array-valued ones — the PostGIS write
///   lane binds a property to one column and refuses a non-scalar value
///   anyway, so dropping them here reports the loss per collection instead
///   of refusing every single item. A scalar property that has no column
///   still surfaces as that lane's own `UnwritableProperty` refusal, named
///   per item.
///
/// An asset entry with no `href` refuses the whole item: `#191` registers
/// remote assets href-only, and an asset whose href would have to be
/// invented is exactly what this CLI never does.
pub fn map_item(item: &Value, declared: Option<&BTreeSet<String>>) -> Result<MappedItem, String> {
    let object = item
        .as_object()
        .ok_or_else(|| "harvested item is not a JSON object".to_string())?;
    if object.get("type").and_then(Value::as_str) != Some("Feature") {
        return Err("harvested item's 'type' is not 'Feature'; a STAC Item always is".to_string());
    }
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            "harvested item has no non-empty string 'id'; a STAC Item id is always a string"
                .to_string()
        })?;
    let geometry = object
        .get("geometry")
        .ok_or_else(|| "harvested item is missing its 'geometry' member".to_string())?;
    let properties = match object.get("properties") {
        Some(Value::Object(map)) => map,
        Some(Value::Null) | None => {
            return Err(
                "harvested item has no 'properties' object; a STAC Item always does".to_string(),
            )
        }
        Some(_) => return Err("harvested item's 'properties' is not a JSON object".to_string()),
    };

    let assets = count_assets(object.get("assets"))?;

    let mut kept = serde_json::Map::new();
    let mut dropped = Vec::new();
    for (name, value) in properties {
        let keep = match declared {
            Some(declared) => declared.contains(name),
            None => !matches!(value, Value::Array(_) | Value::Object(_)),
        };
        if keep {
            kept.insert(name.clone(), value.clone());
        } else {
            dropped.push(name.clone());
        }
    }
    dropped.sort();

    let mut feature = serde_json::Map::new();
    feature.insert("type".to_string(), Value::String("Feature".to_string()));
    feature.insert("id".to_string(), Value::String(id.to_string()));
    feature.insert("geometry".to_string(), geometry.clone());
    if let Some(bbox) = object.get("bbox") {
        feature.insert("bbox".to_string(), bbox.clone());
    }
    feature.insert("properties".to_string(), Value::Object(kept));

    Ok(MappedItem {
        feature: Value::Object(feature),
        dropped_properties: dropped,
        assets,
    })
}

/// Counts an item's assets, refusing one that could only be registered by
/// inventing an href. See [`map_item`]'s own doc for why a missing href is
/// fatal to the item rather than merely skipped.
fn count_assets(assets: Option<&Value>) -> Result<u64, String> {
    let Some(assets) = assets else {
        return Ok(0);
    };
    if assets.is_null() {
        return Ok(0);
    }
    let assets = assets
        .as_object()
        .ok_or_else(|| "harvested item's 'assets' is not a JSON object".to_string())?;
    for (key, asset) in assets {
        let href = asset
            .as_object()
            .and_then(|asset| asset.get("href"))
            .and_then(Value::as_str)
            .filter(|href| !href.is_empty());
        if href.is_none() {
            return Err(format!(
                "harvested item's asset '{key}' has no non-empty 'href'; harvest registers remote \
                 assets href-only and never mints one"
            ));
        }
    }
    Ok(assets.len() as u64)
}

/// The one seam a harvest reaches the network through. A trait so the page
/// walk, the bookmark and the whole report are unit-testable against an
/// in-memory catalog with no server, no fixture port and no network.
#[async_trait::async_trait]
pub trait StacFetch: Send + Sync {
    async fn get(&self, url: &str) -> anyhow::Result<String>;
}

/// Fetches over `curl`, the same "shell out rather than take an HTTP client
/// dependency" arrangement `source.rs` already uses for a remote `load`
/// source — an ingest CLI that already spawns `ogr2ogr` has no reason to
/// link a second async HTTP stack just to read JSON pages.
pub struct CurlFetch;

#[async_trait::async_trait]
impl StacFetch for CurlFetch {
    /// `--max-filesize` refuses an oversized page before a byte of body is
    /// transferred *when the server advertises a length*; the length check
    /// after the fact covers a chunked response, which curl cannot
    /// pre-screen. Both are the same [`MAX_DOCUMENT_BYTES`] limit — the
    /// second is what makes the refusal unconditional, not what makes the
    /// download lazy.
    async fn get(&self, url: &str) -> anyhow::Result<String> {
        tracing::debug!(%url, "fetching STAC document");
        let output = tokio::process::Command::new("curl")
            .args([
                "-fsSL",
                "--max-filesize",
                &MAX_DOCUMENT_BYTES.to_string(),
                "-H",
                "accept: application/json, application/geo+json",
                url,
            ])
            .output()
            .await
            .map_err(|err| {
                anyhow::anyhow!("failed to invoke 'curl': {err}. Is curl installed and on PATH?")
            })?;
        anyhow::ensure!(
            output.status.success(),
            "curl failed to fetch '{url}' (exit status: {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        anyhow::ensure!(
            output.stdout.len() as u64 <= MAX_DOCUMENT_BYTES,
            "'{url}' returned more than the {MAX_DOCUMENT_BYTES}-byte document limit"
        );
        String::from_utf8(output.stdout)
            .map_err(|error| anyhow::anyhow!("'{url}' returned a non-UTF-8 body: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn normalize_root_trims_one_trailing_slash_and_refuses_a_non_http_source() {
        assert_eq!(
            normalize_root("https://example.test/stac/").unwrap(),
            "https://example.test/stac"
        );
        assert_eq!(
            normalize_root("http://example.test").unwrap(),
            "http://example.test"
        );
        let error = normalize_root("file:///tmp/catalog")
            .unwrap_err()
            .to_string();
        assert!(error.contains("http(s) STAC API root"), "{error}");
        let error = normalize_root("https://").unwrap_err().to_string();
        assert!(error.contains("has no host"), "{error}");
    }

    #[test]
    fn parse_collections_reads_ids_in_document_order_and_prefers_an_items_link() {
        let body = serde_json::json!({
            "collections": [
                {"id": "beta", "links": [
                    {"rel": "items", "href": "/other/beta/items"}
                ]},
                {"id": "alpha"}
            ]
        })
        .to_string();
        let collections =
            parse_collections(&body, "https://example.test/stac/collections").unwrap();
        assert_eq!(
            collections,
            vec![
                RemoteCollection {
                    id: "beta".to_string(),
                    items_href: Some("https://example.test/other/beta/items".to_string()),
                },
                RemoteCollection {
                    id: "alpha".to_string(),
                    items_href: None,
                },
            ]
        );
    }

    #[test]
    fn parse_collections_refuses_a_document_without_ids_or_with_a_duplicate_id() {
        let body = serde_json::json!({"collections": [{"title": "no id"}]}).to_string();
        let error = parse_collections(&body, "https://example.test/collections")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("collection #0 has no non-empty string 'id'"),
            "{error}"
        );

        let body = serde_json::json!({"collections": [{"id": "a"}, {"id": "a"}]}).to_string();
        let error = parse_collections(&body, "https://example.test/collections")
            .unwrap_err()
            .to_string();
        assert!(error.contains("more than once"), "{error}");

        let error = parse_collections("{}", "https://example.test/collections")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no 'collections' array"), "{error}");
    }

    #[test]
    fn items_url_falls_back_to_the_spec_path_and_refuses_an_unencodable_id() {
        let collection = RemoteCollection {
            id: "sentinel-2".to_string(),
            items_href: None,
        };
        assert_eq!(
            items_url("https://example.test/stac", &collection).unwrap(),
            "https://example.test/stac/collections/sentinel-2/items"
        );

        let collection = RemoteCollection {
            id: "a b/c".to_string(),
            items_href: None,
        };
        let error = items_url("https://example.test/stac", &collection)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("never invents a percent-encoding"),
            "{error}"
        );
    }

    #[test]
    fn parse_items_page_resolves_a_relative_next_link() {
        let body = serde_json::json!({
            "type": "FeatureCollection",
            "features": [],
            "links": [
                {"rel": "self", "href": "https://example.test/stac/collections/a/items"},
                {"rel": "next", "href": "?token=page-2"}
            ]
        })
        .to_string();
        let page =
            parse_items_page(&body, "https://example.test/stac/collections/a/items").unwrap();
        assert!(page.features.is_empty());
        assert_eq!(
            page.next.as_deref(),
            Some("https://example.test/stac/collections/a/items?token=page-2")
        );
    }

    #[test]
    fn parse_items_page_refuses_a_post_or_templated_next_link() {
        let post = serde_json::json!({
            "type": "FeatureCollection",
            "features": [],
            "links": [{"rel": "next", "href": "/search", "method": "POST", "body": {"token": "x"}}]
        })
        .to_string();
        let error = parse_items_page(&post, "https://example.test/items")
            .unwrap_err()
            .to_string();
        assert!(error.contains("follows only GET links"), "{error}");

        let templated = serde_json::json!({
            "type": "FeatureCollection",
            "features": [],
            "links": [{"rel": "next", "href": "/items{?token}", "templated": true}]
        })
        .to_string();
        let error = parse_items_page(&templated, "https://example.test/items")
            .unwrap_err()
            .to_string();
        assert!(error.contains("templated"), "{error}");
    }

    #[test]
    fn parse_items_page_refuses_a_self_referential_next_link() {
        let body = serde_json::json!({
            "type": "FeatureCollection",
            "features": [],
            "links": [{"rel": "next", "href": "https://example.test/items?token=1"}]
        })
        .to_string();
        let error = parse_items_page(&body, "https://example.test/items?token=1")
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing to loop"), "{error}");
    }

    #[test]
    fn parse_items_page_refuses_a_document_that_is_not_a_feature_collection() {
        let error = parse_items_page(r#"{"type":"Feature"}"#, "https://example.test/items")
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a GeoJSON FeatureCollection"), "{error}");
    }

    #[test]
    fn resolve_href_covers_the_reference_shapes_a_stac_link_actually_uses() {
        let base = "https://example.test/stac/collections/a/items?token=1";
        assert_eq!(
            resolve_href(base, "https://other.test/x").unwrap(),
            "https://other.test/x"
        );
        assert_eq!(
            resolve_href(base, "//other.test/x").unwrap(),
            "https://other.test/x"
        );
        assert_eq!(
            resolve_href(base, "/root/x").unwrap(),
            "https://example.test/root/x"
        );
        assert_eq!(
            resolve_href(base, "?token=2").unwrap(),
            "https://example.test/stac/collections/a/items?token=2"
        );
        assert_eq!(
            resolve_href(base, "items2").unwrap(),
            "https://example.test/stac/collections/a/items2"
        );
        let error = resolve_href(base, "ftp://example.test/x")
            .unwrap_err()
            .to_string();
        assert!(error.contains("dereferences only http(s)"), "{error}");
    }

    #[test]
    fn map_item_keeps_only_the_declared_properties_when_the_target_pins_a_schema() {
        let item = serde_json::json!({
            "type": "Feature",
            "stac_version": "1.1.0",
            "id": "item-1",
            "collection": "remote",
            "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
            "bbox": [1.0, 2.0, 1.0, 2.0],
            "properties": {"datetime": "2026-01-01T00:00:00Z", "eo:cloud_cover": 3, "extra": "x"},
            "links": [{"rel": "self", "href": "https://example.test/items/item-1"}],
            "assets": {"data": {"href": "https://example.test/data.tif"}}
        });
        let mapped = map_item(&item, Some(&declared(["datetime"].as_slice()))).unwrap();
        assert_eq!(
            mapped.feature,
            serde_json::json!({
                "type": "Feature",
                "id": "item-1",
                "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
                "bbox": [1.0, 2.0, 1.0, 2.0],
                "properties": {"datetime": "2026-01-01T00:00:00Z"}
            })
        );
        assert_eq!(mapped.dropped_properties, vec!["eo:cloud_cover", "extra"]);
        assert_eq!(mapped.assets, 1);
    }

    #[test]
    fn map_item_drops_non_scalar_properties_when_the_target_pins_no_schema() {
        let item = serde_json::json!({
            "type": "Feature",
            "id": "item-2",
            "geometry": null,
            "properties": {
                "datetime": "2026-01-01T00:00:00Z",
                "count": 4,
                "flag": true,
                "missing": null,
                "proj:transform": [1, 2, 3],
                "raster:bands": {"nodata": 0}
            }
        });
        let mapped = map_item(&item, None).unwrap();
        assert_eq!(
            mapped.feature["properties"],
            serde_json::json!({
                "datetime": "2026-01-01T00:00:00Z",
                "count": 4,
                "flag": true,
                "missing": null
            })
        );
        assert_eq!(
            mapped.dropped_properties,
            vec!["proj:transform", "raster:bands"]
        );
        assert_eq!(mapped.feature["geometry"], Value::Null);
        assert_eq!(mapped.assets, 0);
    }

    #[test]
    fn map_item_names_every_shape_it_refuses() {
        let cases = [
            (serde_json::json!([]), "not a JSON object"),
            (
                serde_json::json!({"type": "Item", "id": "x"}),
                "'type' is not 'Feature'",
            ),
            (
                serde_json::json!({"type": "Feature", "id": 7, "geometry": null, "properties": {}}),
                "no non-empty string 'id'",
            ),
            (
                serde_json::json!({"type": "Feature", "id": "x", "properties": {}}),
                "missing its 'geometry' member",
            ),
            (
                serde_json::json!({"type": "Feature", "id": "x", "geometry": null}),
                "no 'properties' object",
            ),
            (
                serde_json::json!({
                    "type": "Feature", "id": "x", "geometry": null, "properties": {},
                    "assets": {"data": {"type": "image/tiff"}}
                }),
                "never mints one",
            ),
        ];
        for (item, expected) in cases {
            let error = map_item(&item, None).unwrap_err();
            assert!(error.contains(expected), "{error} does not name {expected}");
        }
    }
}
