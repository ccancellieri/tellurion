//! Materializes STAC `assets` entries (`#36` slice B, `#48`) from this
//! deployment's own routing capabilities: no new config, no new storage
//! concept — every href points at a lane another protocol root already
//! serves (`tellurion-tiles`' MVT/PNG/styled-PNG lanes, `tellurion-places`'
//! Glb lane). Pure mapping, no I/O: the caller (`handlers.rs`) gathers
//! `AssetCapabilities` from `Router`/`ctx.style_store` and this module only
//! shapes the result, the same I/O-vs-shape split `mapping.rs` keeps.
//!
//! ## Per-item asset records (`#221`)
//!
//! The capability-derived map above is a *per-collection* fact: every Item
//! of a tiles-capable collection advertises the same `mvt`/`png`/`glb`
//! templates. A harvested multi-item collection also has genuinely
//! per-Item assets — one scene's own COG, another's Zarr store — and those
//! live in the `"<table>_assets"` records the assets API already persists
//! (`tellurion_core::AssetRecordStore`). [`PageItemAssets`] is where the
//! two meet: the handler does ONE batched
//! `AssetRecordStore::item_assets` read for a whole page and hands the
//! records here, and this module builds each Item's final map. Still no
//! I/O in this module — the read is the caller's, the shaping is ours,
//! same split as everything above.

use std::collections::{BTreeMap, HashMap};

use tellurion_core::{
    AssetKind, AssetRecord, AssetRecordEntry, AssetState, ServerConfig, ServiceAssetsMode,
};

use crate::model::StacAsset;

const MVT_MEDIA_TYPE: &str = "application/vnd.mapbox-vector-tile";
const PNG_MEDIA_TYPE: &str = "image/png";
const GLB_MEDIA_TYPE: &str = "model/gltf-binary";

/// What this deployment can actually serve for one collection, gathered by
/// the caller (`handlers::asset_capabilities`) from the same `Router`
/// capability probes `list_collections`/`get_collection` already make —
/// never guessed or config-declared here.
pub struct AssetCapabilities {
    /// The collection's tiles lane resolves (`Router::resolve_tiles`
    /// succeeds) — gates the `mvt`/`png`/styled-PNG assets, all three of
    /// which ride that same lane (`tellurion-tiles::handlers::negotiate_format`).
    pub has_tiles: bool,
    /// The collection declares `places3d` *and* its tiles lane resolves
    /// (`tellurion-places::handlers::resolve_places3d` requires both) —
    /// gates the `glb` asset.
    pub places3d: bool,
    /// Every style id `ctx.style_store` currently knows about. Styles are a
    /// global, catalog-independent registry (`tellurion-styles`' own doc:
    /// "Style documents are global ... every root serves the same
    /// registry") — any one of them applies to any tiles-capable collection
    /// via `.../styles/{styleId}/map/tiles/...`, so listing all of them here
    /// is not an over-approximation, just a direct reflection of what that
    /// route already accepts.
    pub style_ids: Vec<String>,
    /// The collection's resolved `stac.service_assets` mode (`#220`) —
    /// whether the templated service entries below are materialized at all.
    /// Read straight off the settings chain by the caller; there is no
    /// built-in fallback here beyond [`ServiceAssetsMode`]'s own `Default`,
    /// which is the pre-`#220` behavior.
    pub service_assets: ServiceAssetsMode,
}

/// `tenant_ext`/`catalog_ext`/`collection_ext` are external ids (`#39`) —
/// every asset href is built from them directly rather than from this
/// request's own `OriginalUri`, because these assets live on *different*
/// protocol roots (`/tiles/...`, `/3dtiles/...`) than the STAC root serving
/// this response. The fixed `/{tenant}/{protocol}/catalogs/{catalog}/...`
/// shape this assumes is `tellurion-server::app`'s own documented route
/// tree, not an assumption local to this crate.
///
/// `{tileMatrix}`/`{tileRow}`/`{tileCol}` placeholders are the exact
/// parameter names `tellurion-tiles`/`tellurion-places` already parse
/// (OGC API Tiles order — row before column); the `.mvt`/`.png`/`.glb`
/// literal suffixes are the same "suffix wins outright" content-negotiation
/// convention `tellurion_tiles::handlers::negotiate_format` and
/// `tellurion_places::handlers::glb_tile` already require, so a client
/// substituting the placeholders gets a concrete, correctly-negotiated tile
/// without needing an `Accept` header or `?f=` query parameter at all.
///
/// `#220`: every entry this builds is one of those templates, so
/// `caps.service_assets == ServiceAssetsMode::Links` short-circuits the
/// whole function — the collection's tiles/maps/3D surfaces are then
/// carried by the rel-typed capability links `tellurion-server`'s link
/// contributors emit instead, and this map is left to the literal Asset
/// Objects (`stac.assets`, and `#221`'s per-item records) that a STAC
/// client can actually dereference. `Templated` (the default) keeps every
/// byte of the pre-`#220` map.
pub fn collection_assets(
    server: &ServerConfig,
    tenant_ext: &str,
    catalog_ext: &str,
    collection_ext: &str,
    caps: &AssetCapabilities,
) -> BTreeMap<String, StacAsset> {
    let mut assets = BTreeMap::new();
    if caps.service_assets == ServiceAssetsMode::Links {
        return assets;
    }

    // `#245`: every entry below hangs off the tiles lane resolving, `glb`
    // included. Before this slice the `glb` block was a sibling of this one,
    // so the mapping described a `places3d && !has_tiles` collection — a
    // combination `tellurion_places::handlers::resolve_places3d` refuses
    // outright (it needs BOTH a `TileSource` and the `places3d`
    // declaration), and one `handlers::asset_capabilities` cannot produce
    // either, since it derives both booleans from the same `resolve_tiles`
    // call. Describing a case the route would refuse is the same overclaim
    // as a link into a resource that 404s, one layer down; the nesting makes
    // the mapping say exactly what the route says. No response byte moves:
    // the discarded combination was already unreachable through every real
    // caller.
    if caps.has_tiles {
        let tiles_root = server.public_href(&format!(
            "/{tenant_ext}/tiles/catalogs/{catalog_ext}/collections/{collection_ext}"
        ));
        let tile_template =
            format!("{tiles_root}/tiles/WebMercatorQuad/{{tileMatrix}}/{{tileRow}}/{{tileCol}}");

        assets.insert(
            "mvt".to_string(),
            StacAsset {
                href: format!("{tile_template}.mvt"),
                media_type: Some(MVT_MEDIA_TYPE.to_string()),
                title: Some("Vector tiles (MVT)".to_string()),
                description: None,
                roles: vec!["data".to_string()],
                templated: true,
            },
        );
        assets.insert(
            "png".to_string(),
            StacAsset {
                href: format!("{tile_template}.png"),
                media_type: Some(PNG_MEDIA_TYPE.to_string()),
                title: Some("Raster tiles (PNG)".to_string()),
                description: None,
                roles: vec!["visual".to_string()],
                templated: true,
            },
        );

        for style_id in &caps.style_ids {
            assets.insert(
                format!("style-{style_id}"),
                StacAsset {
                    href: format!(
                        "{tiles_root}/styles/{style_id}/map/tiles/WebMercatorQuad/{{tileMatrix}}/{{tileRow}}/{{tileCol}}.png"
                    ),
                    media_type: Some(PNG_MEDIA_TYPE.to_string()),
                    title: Some(format!("Styled raster tiles ({style_id})")),
                    description: None,
                    roles: vec!["visual".to_string()],
                    templated: true,
                },
            );
        }

        if caps.places3d {
            assets.insert(
                "glb".to_string(),
                StacAsset {
                    href: server.public_href(&format!(
                        "/{tenant_ext}/3dtiles/catalogs/{catalog_ext}/collections/{collection_ext}/3dtiles/tiles/{{tileMatrix}}/{{tileRow}}/{{tileCol}}.glb"
                    )),
                    media_type: Some(GLB_MEDIA_TYPE.to_string()),
                    title: Some("3D tiles (glTF binary)".to_string()),
                    description: None,
                    roles: vec!["data".to_string()],
                    templated: true,
                },
            );
        }
    }

    assets
}

// -- per-item asset records (`#221`) ---------------------------------------

/// `.../assets/{key}/data` — the stable, server-computed Tellurion data
/// resource a *managed* asset's bytes are served from. The single
/// definition of that URL shape in this crate: `asset_handlers::data_href`
/// delegates here, so the href a client reads off an Item's `assets` map is
/// literally the same string the asset API's own `GET .../assets/{key}`
/// returns for that record, never a second hand-built approximation that
/// could drift.
///
/// Built from external ids (`#39`) the same way [`collection_assets`]
/// builds its own, and for the same reason — never from the request's
/// `OriginalUri`, since a managed asset's href is server-computed, not
/// client-supplied.
pub fn asset_data_href(
    server: &ServerConfig,
    tenant_ext: &str,
    catalog_ext: &str,
    collection_ext: &str,
    item_id: Option<&str>,
    key: &str,
) -> String {
    let path = match item_id {
        Some(fid) => format!(
            "/{tenant_ext}/stac/catalogs/{catalog_ext}/collections/{collection_ext}/items/{fid}/assets/{key}/data"
        ),
        None => format!(
            "/{tenant_ext}/stac/catalogs/{catalog_ext}/collections/{collection_ext}/assets/{key}/data"
        ),
    };
    server.public_href(&path)
}

/// Whether a persisted record may be advertised as a usable STAC asset
/// (`#221`'s "pending and failed managed assets are not advertised"
/// requirement).
///
/// Only [`AssetState::Available`] qualifies, and the rule lives here rather
/// than in the storage capability on purpose: `AssetRecordStore::
/// item_assets` reports every state (reconcile needs that, and so would any
/// future admin view), while a STAC `assets` entry is a promise that its
/// `href` resolves to bytes a client can fetch right now. A `pending`
/// managed asset has no bytes yet and a `failed` one never will, so
/// advertising either would put a knowingly-broken href in a public
/// document. A remote record is born available and so always passes; there
/// is no separate remote rule.
fn is_advertisable(record: &AssetRecord) -> bool {
    matches!(record.state, AssetState::Available)
}

/// One persisted item-scoped record as a STAC Asset Object.
///
/// - **Managed** → [`asset_data_href`], this deployment's own stable data
///   resource. The record's `href` column is `NULL` for a managed asset by
///   construction (registration refuses a client-supplied one), so there is
///   nothing else it could point at.
/// - **Remote** → the external `href` verbatim, exactly as registered.
///   `unwrap_or_default` only guards a row a `NULL`-href remote record
///   could produce, which registration does not allow.
///
/// `templated: false` always: both cases are literal URLs, never a
/// `{tileMatrix}`-style template — the flag exists for the capability-derived
/// tile assets alone (see [`StacAsset::templated`]).
fn record_to_stac_asset(
    server: &ServerConfig,
    tenant_ext: &str,
    catalog_ext: &str,
    collection_ext: &str,
    item_id: &str,
    key: &str,
    record: &AssetRecord,
) -> StacAsset {
    let href = match record.kind {
        AssetKind::Managed => asset_data_href(
            server,
            tenant_ext,
            catalog_ext,
            collection_ext,
            Some(item_id),
            key,
        ),
        AssetKind::Remote => record.href.clone().unwrap_or_default(),
    };
    StacAsset {
        href,
        media_type: record.media_type.clone(),
        title: record.title.clone(),
        description: record.description.clone(),
        roles: record.roles.clone(),
        templated: false,
    }
}

/// One page's worth of Item asset maps: the capability-derived map every
/// Item on the page shares, plus a fully-merged map for each Item that has
/// persisted records of its own (`#221`).
///
/// ## Why a struct rather than a map per item
///
/// [`for_item`](Self::for_item) hands back a borrowed reference to the
/// SHARED map for every item with no records — which is every item of every
/// collection that never opted in. So a page of a hundred items on an
/// un-opted-in collection allocates nothing at all here and produces
/// byte-identical Items, and only items that genuinely have records pay for
/// a merged copy.
///
/// ## Precedence: a persisted record wins
///
/// On a key present in both, the persisted record replaces the
/// capability-derived entry. Same direction as `mapping::to_stac_collection`'s
/// declared-beats-derived rule, and for the same reason: the derived map is
/// a generic template this deployment can always produce for any
/// tiles-capable collection, while a record is a deliberate per-item
/// statement an operator made through the assets API. Losing to the derived
/// entry would make a record unable to correct anything, and there would be
/// no way to express "this Item's `png` really is that COG, not the tile
/// template".
///
/// ## Scope: collection-level records never appear here
///
/// Only item-scoped records are ever in play, enforced at the storage layer
/// (`AssetRecordStore::item_assets` never returns a collection-level
/// record) rather than filtered here — the same reason `to_stac_collection`
/// documents for keeping config-declared collection assets off items:
/// flattening a collection-scoped asset onto every Item is exactly what
/// `#221` exists to stop. An entry that somehow arrived with `item_id:
/// None` is dropped rather than mis-attributed.
///
/// The converse — also projecting *collection*-scoped records into the STAC
/// Collection document — is deliberately NOT part of this: the Collection
/// document's `assets` map is still the capability-derived map plus
/// `settings.stac.assets`, exactly as before. `/collections` lists every
/// collection in one response, so reading records there would cost one
/// query per collection listed — the same N+1 shape, moved one level up —
/// and buying that needs its own batched interface over a set of
/// collections, which is a different capability from this one. Nothing here
/// forecloses it.
pub struct PageItemAssets {
    shared: BTreeMap<String, StacAsset>,
    per_item: HashMap<String, BTreeMap<String, StacAsset>>,
}

impl PageItemAssets {
    /// Folds `records` (one batched read's worth, for a whole page) onto
    /// `shared`. Pure: every I/O already happened in the caller.
    pub fn new(
        server: &ServerConfig,
        shared: BTreeMap<String, StacAsset>,
        tenant_ext: &str,
        catalog_ext: &str,
        collection_ext: &str,
        records: &[AssetRecordEntry],
    ) -> Self {
        let mut per_item: HashMap<String, BTreeMap<String, StacAsset>> = HashMap::new();
        for entry in records {
            let Some(item_id) = entry.item_id.as_deref() else {
                continue;
            };
            if !is_advertisable(&entry.record) {
                continue;
            }
            per_item
                .entry(item_id.to_string())
                .or_insert_with(|| shared.clone())
                .insert(
                    entry.key.clone(),
                    record_to_stac_asset(
                        server,
                        tenant_ext,
                        catalog_ext,
                        collection_ext,
                        item_id,
                        &entry.key,
                        &entry.record,
                    ),
                );
        }
        Self { shared, per_item }
    }

    /// This item's asset map — its own merged one when it has records, the
    /// shared capability-derived one otherwise.
    pub fn for_item(&self, item_id: &str) -> &BTreeMap<String, StacAsset> {
        self.per_item.get(item_id).unwrap_or(&self.shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(has_tiles: bool, places3d: bool) -> AssetCapabilities {
        AssetCapabilities {
            has_tiles,
            places3d,
            style_ids: vec![],
            service_assets: ServiceAssetsMode::default(),
        }
    }

    fn server() -> ServerConfig {
        ServerConfig::default()
    }

    fn canonical_server() -> ServerConfig {
        ServerConfig {
            public_base_url: Some("https://geo.example.test/tellurion/".to_string()),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn no_capabilities_produces_no_assets() {
        let assets = collection_assets(&server(), "public", "default", "demo", &caps(false, false));
        assert!(assets.is_empty());
    }

    #[test]
    fn tiles_capability_produces_mvt_and_png_but_no_glb() {
        let assets = collection_assets(&server(), "public", "default", "demo", &caps(true, false));
        assert!(assets.contains_key("mvt"));
        assert!(assets.contains_key("png"));
        assert!(
            !assets.contains_key("glb"),
            "a tiles-only collection must not advertise a glb asset"
        );
    }

    /// `#245`: the mapping now describes exactly the route it names. A
    /// `places3d` collection whose tiles lane does NOT resolve is a
    /// combination `tellurion_places::handlers::resolve_places3d` refuses —
    /// it requires both — so no `glb` asset is materialized for it either.
    ///
    /// Supersedes the pre-`#245` test, which asserted the opposite while its
    /// own comment conceded the case was unreachable through
    /// `handlers::asset_capabilities`. That is precisely the shape of an
    /// overclaim: a described capability nothing could ask for and the route
    /// would refuse. Nothing a real caller can produce changes, because
    /// `asset_capabilities` derives both booleans from one `resolve_tiles`
    /// call and so never builds this combination in the first place.
    #[test]
    fn places3d_without_a_resolving_tiles_lane_yields_no_glb_asset_either() {
        let assets = collection_assets(&server(), "public", "default", "demo", &caps(false, true));
        assert!(
            !assets.contains_key("glb"),
            "the 3D tiles route needs a TileSource too; describing it without one \
             advertises a resource that would be refused: {assets:?}"
        );
        assert!(assets.is_empty(), "and nothing else is materialized either");
    }

    #[test]
    fn full_capabilities_produce_mvt_png_and_glb() {
        let assets = collection_assets(&server(), "public", "default", "demo", &caps(true, true));
        assert!(assets.contains_key("mvt"));
        assert!(assets.contains_key("png"));
        assert!(assets.contains_key("glb"));
    }

    #[test]
    fn mvt_and_png_hrefs_carry_the_correct_tenant_catalog_and_collection_segments() {
        let assets = collection_assets(&server(), "acme", "cat1", "roads", &caps(true, false));
        let mvt = &assets["mvt"];
        assert_eq!(
            mvt.href,
            "/acme/tiles/catalogs/cat1/collections/roads/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.mvt"
        );
        assert_eq!(
            mvt.media_type.as_deref(),
            Some("application/vnd.mapbox-vector-tile")
        );
        assert_eq!(mvt.roles, vec!["data".to_string()]);
        assert!(mvt.templated);

        let png = &assets["png"];
        assert_eq!(
            png.href,
            "/acme/tiles/catalogs/cat1/collections/roads/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.png"
        );
        assert_eq!(png.media_type.as_deref(), Some("image/png"));
        assert_eq!(png.roles, vec!["visual".to_string()]);
    }

    #[test]
    fn a_canonical_base_with_a_path_prefix_qualifies_service_templates() {
        let assets = collection_assets(
            &canonical_server(),
            "acme",
            "cat1",
            "roads",
            &caps(true, true),
        );
        assert_eq!(
            assets["mvt"].href,
            "https://geo.example.test/tellurion/acme/tiles/catalogs/cat1/collections/roads/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.mvt"
        );
        assert_eq!(
            assets["glb"].href,
            "https://geo.example.test/tellurion/acme/3dtiles/catalogs/cat1/collections/roads/3dtiles/tiles/{tileMatrix}/{tileRow}/{tileCol}.glb"
        );
    }

    #[test]
    fn glb_href_uses_the_3dtiles_root_not_the_tiles_root() {
        let assets = collection_assets(&server(), "acme", "cat1", "roads", &caps(true, true));
        let glb = &assets["glb"];
        assert_eq!(
            glb.href,
            "/acme/3dtiles/catalogs/cat1/collections/roads/3dtiles/tiles/{tileMatrix}/{tileRow}/{tileCol}.glb"
        );
        assert_eq!(glb.media_type.as_deref(), Some("model/gltf-binary"));
    }

    #[test]
    fn a_declared_style_id_produces_a_keyed_styled_asset() {
        let caps = AssetCapabilities {
            style_ids: vec!["basic".to_string()],
            ..caps(true, false)
        };
        let assets = collection_assets(&server(), "public", "default", "demo", &caps);
        let styled = assets
            .get("style-basic")
            .expect("expected a style-basic asset");
        assert_eq!(
            styled.href,
            "/public/tiles/catalogs/default/collections/demo/styles/basic/map/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.png"
        );
        assert_eq!(styled.media_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn no_declared_styles_means_no_styled_assets() {
        let assets = collection_assets(&server(), "public", "default", "demo", &caps(true, false));
        assert!(!assets.keys().any(|k| k.starts_with("style-")));
    }

    // -- `stac.service_assets` (`#220`) -----------------------------------

    /// The default is the pre-`#220` map, capability for capability. Stated
    /// as its own test so the "unconfigured deployments are byte-for-byte
    /// unchanged" guarantee is checked against the mode explicitly, not
    /// only implied by every other test in this module reading
    /// `ServiceAssetsMode::default()`.
    #[test]
    fn the_default_mode_is_templated_and_materializes_every_service_asset() {
        assert_eq!(ServiceAssetsMode::default(), ServiceAssetsMode::Templated);
        let caps = AssetCapabilities {
            style_ids: vec!["basic".to_string()],
            ..caps(true, true)
        };
        let assets = collection_assets(&server(), "public", "default", "demo", &caps);
        assert_eq!(
            assets.keys().collect::<Vec<_>>(),
            vec!["glb", "mvt", "png", "style-basic"]
        );
    }

    /// The opt-in: with the same capabilities, `links` mode materializes
    /// none of the templates. Every remaining entry in a real response then
    /// comes from `stac.assets` or a persisted record — both literal hrefs,
    /// which is the acceptance criterion "no literal asset href contains
    /// unresolved route placeholders".
    #[test]
    fn links_mode_materializes_no_templated_service_assets_at_all() {
        let caps = AssetCapabilities {
            style_ids: vec!["basic".to_string()],
            service_assets: ServiceAssetsMode::Links,
            ..caps(true, true)
        };
        let assets = collection_assets(&server(), "public", "default", "demo", &caps);
        assert!(
            assets.is_empty(),
            "links mode must leave the service surfaces to typed links: {assets:?}"
        );
    }

    /// `links` mode retires the *derived* templates only: a per-item record
    /// (`#221`) is a literal Asset Object and still lands on its item. The
    /// issue's own "source COG/Zarr/download/thumbnail objects remain STAC
    /// assets" rule, at the mapping layer.
    #[test]
    fn links_mode_still_projects_per_item_asset_records() {
        let caps = AssetCapabilities {
            service_assets: ServiceAssetsMode::Links,
            ..caps(true, false)
        };
        let assets = PageItemAssets::new(
            &server(),
            collection_assets(&server(), "public", "default", "demo", &caps),
            "public",
            "default",
            "demo",
            &[entry(
                Some("a"),
                "cog",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("https://example.test/scene-a.tif"),
                ),
            )],
        );
        assert_eq!(
            assets.for_item("a").keys().collect::<Vec<_>>(),
            vec!["cog"],
            "only the literal record survives"
        );
        assert_eq!(
            assets.for_item("a")["cog"].href,
            "https://example.test/scene-a.tif"
        );
        assert!(assets.for_item("b").is_empty());
    }

    // -- per-item asset records (`#221`) ----------------------------------

    fn record(kind: AssetKind, state: AssetState, href: Option<&str>) -> AssetRecord {
        AssetRecord {
            id: uuid::Uuid::nil(),
            kind,
            state,
            href: href.map(str::to_string),
            media_type: Some(
                "image/tiff; application=geotiff; profile=cloud-optimized".to_string(),
            ),
            title: Some("Scene COG".to_string()),
            description: Some("the scene's own cloud-optimized GeoTIFF".to_string()),
            roles: vec!["data".to_string()],
            declared_size: None,
            digest: None,
            failure_reason: None,
        }
    }

    fn entry(item_id: Option<&str>, key: &str, record: AssetRecord) -> AssetRecordEntry {
        AssetRecordEntry {
            item_id: item_id.map(str::to_string),
            key: key.to_string(),
            record,
        }
    }

    fn page(records: &[AssetRecordEntry]) -> PageItemAssets {
        PageItemAssets::new(
            &server(),
            collection_assets(&server(), "public", "default", "demo", &caps(true, false)),
            "public",
            "default",
            "demo",
            records,
        )
    }

    /// The acceptance criterion the whole slice hangs on at the mapping
    /// layer: with no records, every item's map is the shared derived one,
    /// unchanged.
    #[test]
    fn an_item_with_no_records_gets_the_shared_capability_derived_map() {
        let assets = page(&[]);
        let derived = collection_assets(&server(), "public", "default", "demo", &caps(true, false));
        assert_eq!(
            assets.for_item("a").keys().collect::<Vec<_>>(),
            derived.keys().collect::<Vec<_>>()
        );
        assert_eq!(assets.for_item("a")["mvt"].href, derived["mvt"].href);
    }

    #[test]
    fn a_remote_record_keeps_its_external_href_verbatim() {
        let assets = page(&[entry(
            Some("a"),
            "cog",
            record(
                AssetKind::Remote,
                AssetState::Available,
                Some("https://example.test/scene-a.tif"),
            ),
        )]);
        let asset = &assets.for_item("a")["cog"];
        assert_eq!(asset.href, "https://example.test/scene-a.tif");
        assert!(!asset.templated);
        assert_eq!(
            asset.description.as_deref(),
            Some("the scene's own cloud-optimized GeoTIFF")
        );
        // The derived entries are still there alongside it.
        assert!(assets.for_item("a").contains_key("mvt"));
    }

    #[test]
    fn a_managed_record_resolves_to_the_stable_tellurion_data_resource() {
        let assets = page(&[entry(
            Some("a"),
            "cog",
            record(AssetKind::Managed, AssetState::Available, None),
        )]);
        assert_eq!(
            assets.for_item("a")["cog"].href,
            "/public/stac/catalogs/default/collections/demo/items/a/assets/cog/data"
        );
    }

    #[test]
    fn a_canonical_base_qualifies_managed_assets_but_never_rewrites_remote_schemes() {
        let records = [
            entry(
                Some("a"),
                "managed",
                record(AssetKind::Managed, AssetState::Available, None),
            ),
            entry(
                Some("a"),
                "https",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("https://cdn.example.test/a.tif"),
                ),
            ),
            entry(
                Some("a"),
                "s3",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("s3://bucket/a.tif"),
                ),
            ),
            entry(
                Some("a"),
                "data",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("data:text/plain;base64,QQ=="),
                ),
            ),
            entry(
                Some("a"),
                "protocol-relative",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("//cdn.example.test/a.tif"),
                ),
            ),
        ];
        let assets = PageItemAssets::new(
            &canonical_server(),
            BTreeMap::new(),
            "public",
            "default",
            "demo",
            &records,
        );
        let item = assets.for_item("a");
        assert_eq!(
            item["managed"].href,
            "https://geo.example.test/tellurion/public/stac/catalogs/default/collections/demo/items/a/assets/managed/data"
        );
        assert_eq!(item["https"].href, "https://cdn.example.test/a.tif");
        assert_eq!(item["s3"].href, "s3://bucket/a.tif");
        assert_eq!(item["data"].href, "data:text/plain;base64,QQ==");
        assert_eq!(item["protocol-relative"].href, "//cdn.example.test/a.tif");
    }

    /// Each item's records land on that item and nowhere else — the whole
    /// point of `#221` over a single collection-level map.
    #[test]
    fn records_are_projected_onto_their_own_item_only() {
        let assets = page(&[
            entry(
                Some("a"),
                "cog",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("https://x/a"),
                ),
            ),
            entry(
                Some("b"),
                "zarr",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("https://x/b"),
                ),
            ),
        ]);
        assert_eq!(assets.for_item("a")["cog"].href, "https://x/a");
        assert!(!assets.for_item("a").contains_key("zarr"));
        assert_eq!(assets.for_item("b")["zarr"].href, "https://x/b");
        assert!(!assets.for_item("b").contains_key("cog"));
        // An item the page carried but with no records of its own.
        assert!(!assets.for_item("c").contains_key("cog"));
    }

    /// The documented collision rule: a persisted record wins over the
    /// capability-derived entry sharing its key.
    #[test]
    fn a_persisted_record_overrides_a_capability_derived_entry_of_the_same_key() {
        let assets = page(&[entry(
            Some("a"),
            "png",
            record(
                AssetKind::Remote,
                AssetState::Available,
                Some("https://example.test/scene-a-quicklook.png"),
            ),
        )]);
        assert_eq!(
            assets.for_item("a")["png"].href,
            "https://example.test/scene-a-quicklook.png"
        );
        assert!(!assets.for_item("a")["png"].templated);
        // Another item's `png` is still the derived template.
        assert!(assets.for_item("b")["png"].templated);
    }

    /// Lifecycle: a managed asset whose bytes have not arrived (or never
    /// will) is not advertised, so no public document ever carries an href
    /// this server knows would fail.
    #[test]
    fn pending_and_failed_managed_records_are_not_advertised() {
        let mut failed = record(AssetKind::Managed, AssetState::Failed, None);
        failed.failure_reason = Some("digest mismatch".to_string());
        let assets = page(&[
            entry(
                Some("a"),
                "pending-cog",
                record(AssetKind::Managed, AssetState::Pending, None),
            ),
            entry(Some("a"), "failed-cog", failed),
        ]);
        assert!(!assets.for_item("a").contains_key("pending-cog"));
        assert!(!assets.for_item("a").contains_key("failed-cog"));
        // Nothing advertisable landed, so the item keeps the shared map.
        let derived = collection_assets(&server(), "public", "default", "demo", &caps(true, false));
        assert_eq!(
            assets.for_item("a").keys().collect::<Vec<_>>(),
            derived.keys().collect::<Vec<_>>()
        );
    }

    /// A pending record alongside an available one hides only itself.
    #[test]
    fn an_unavailable_record_does_not_suppress_an_available_sibling() {
        let assets = page(&[
            entry(
                Some("a"),
                "cog",
                record(
                    AssetKind::Remote,
                    AssetState::Available,
                    Some("https://x/a"),
                ),
            ),
            entry(
                Some("a"),
                "upload",
                record(AssetKind::Managed, AssetState::Pending, None),
            ),
        ]);
        assert!(assets.for_item("a").contains_key("cog"));
        assert!(!assets.for_item("a").contains_key("upload"));
    }

    /// Defense in depth for the scope boundary: the storage capability
    /// already never returns a collection-level record, and if one arrived
    /// anyway it is dropped rather than mis-attributed to some item.
    #[test]
    fn a_collection_level_record_is_never_projected_onto_an_item() {
        let assets = page(&[entry(
            None,
            "license",
            record(
                AssetKind::Remote,
                AssetState::Available,
                Some("https://example.test/LICENSE"),
            ),
        )]);
        assert!(!assets.for_item("").contains_key("license"));
        assert!(!assets.for_item("a").contains_key("license"));
    }

    #[test]
    fn the_collection_level_data_href_has_no_items_segment() {
        assert_eq!(
            asset_data_href(&server(), "acme", "cat1", "roads", None, "thumb"),
            "/acme/stac/catalogs/cat1/collections/roads/assets/thumb/data"
        );
        assert_eq!(
            asset_data_href(&server(), "acme", "cat1", "roads", Some("f1"), "thumb",),
            "/acme/stac/catalogs/cat1/collections/roads/items/f1/assets/thumb/data"
        );
    }
}
