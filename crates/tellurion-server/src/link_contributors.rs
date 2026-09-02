//! The wiring layer's own `LinkContributor` implementations (`#186`,
//! `#220`): capability-derived cross-protocol links, registered by name in
//! `main` into the boot-time [`LinkContributors`](tellurion_core::
//! LinkContributors) registry (`#112` model) and consumed by the protocol
//! crates' serializers through `AppContext.link_contributors`. This module
//! lives in the server crate on purpose — it is the one crate that already
//! knows every protocol root's mounting (`app.rs`'s
//! `/{tenant}/{protocol}/catalogs/{catalog}/...` route tree) *and* the
//! per-tenant exposure matrix that decides which of those roots exist
//! (`protocol.rs`), so building an href that crosses protocol roots here
//! adds no coupling the workspace didn't already have; protocol crates
//! still never import each other.
//!
//! ## A link is a promise
//!
//! Every contribution is derived from what the CURRENT `Router` actually
//! resolves, and from what the current deployment actually mounts — the
//! same resolve-time honesty the Router already enforces for handlers,
//! applied to link generation. Three independent ways a link could 404, all
//! closed by [`root_serves`] plus a per-lane capability probe:
//!
//! - the operator turned the target root off (`#185`);
//! - the target root does not serve this collection's kind (`#192`);
//! - the collection's driver has no such capability at all.
//!
//! A collection whose tiles lane doesn't resolve contributes no tiles link
//! and no stylesheet link, no stub, no dead href. Hrefs are built from the
//! `ResourceRef`'s EXTERNAL ids only (`#39`: an internal id never
//! serializes), against the same fixed route shapes each protocol crate's
//! own `router()` declares.
//!
//! ## What is deliberately NOT contributed (`#220`)
//!
//! The seam is consumed by exactly two roots today — STAC and Features —
//! and a contributor cannot know which of them is serializing the answer.
//! That rules out two surfaces `#220` lists:
//!
//! - **The Features items resource.** The OGC API Features Collection
//!   resource already links its own `/items` with the registered `items`
//!   rel (`tellurion_features::handlers::collection_summary`), so a
//!   contributed copy would be self-referential there; and in a STAC
//!   Collection the `items` rel is reserved for the STAC ItemCollection
//!   endpoint, which is a different representation of the same rows. There
//!   is no honest single link that means the right thing in both documents.
//!   `#245` supplied the STAC side of that link where it belongs — in
//!   `tellurion_stac::handlers`, which knows which root it is serializing —
//!   rather than by relaxing this refusal.
//! - **The asset-management resources.** `.../assets/{key}` and its upload
//!   lanes are mounted under the STAC root itself, and each managed asset's
//!   retrievable href already appears in that Item's own `assets` map
//!   (`tellurion_stac::assets::asset_data_href`). There is no listing
//!   resource to point a collection-level link at, and no registered
//!   relation type for one, so nothing is invented here.
//!
//! Both cases are the [`LinkContributor`] contract's "the ordinary empty
//! answer" rather than a guess.

use std::collections::BTreeSet;
use std::sync::Arc;

use tellurion_core::{
    advertised_vector_layers, ContributedLink, LinkAnchor, LinkContributor, ResourceRef, Router,
    StyleStore,
};
use tellurion_features::{PLACES3D_REL, TILESETS_MAP_REL, TILESETS_VECTOR_REL};
use tellurion_render::style_paints_any_layer;
use tellurion_styles::STYLE_MEDIA_TYPE;
use tellurion_tiles::{MAP_REL, WEB_MERCATOR_QUAD_ID};

use crate::protocol::Protocol;

/// Relation type for a templated direct-tile link. A plain token rather
/// than an absolute URI: this is the vocabulary the issue's own contract
/// names (`rel=tiles`) and the one the UI's capability-driven rendering
/// plan already recognizes (`ui/src/lib/map.ts`'s `linkByRel`), chosen over
/// minting another `tellurion.dev` extension URI for a link whose primary
/// consumer is exactly that plan.
const TILES_REL: &str = "tiles";
/// IANA-registered relation type (HTML's own) for a link to a style
/// document — the same rel `tellurion-styles`' `/styles` listing already
/// uses for each style's own stylesheet link, reused rather than invented.
const STYLESHEET_REL: &str = "stylesheet";

const MVT_MEDIA_TYPE: &str = "application/vnd.mapbox-vector-tile";
const PNG_MEDIA_TYPE: &str = "image/png";
const JSON_MEDIA_TYPE: &str = "application/json";

/// The anchors every contributor emits for: collection-level and item-level
/// documents get the same per-collection links (tiles, stylesheets and 3D
/// tilesets are facts about the collection, not any one row — see
/// `ResourceRef::item_id`'s own doc for why contributors emit item-anchored
/// links even when no item id was passed).
const ANCHORS: [LinkAnchor; 2] = [LinkAnchor::Collection, LinkAnchor::Item];

/// One link, expanded across both [`ANCHORS`]. Every contributor builds its
/// answer out of these, so "collection and item documents carry the same
/// per-collection links" is stated once rather than re-derived per lane.
fn both_anchors(
    rel: &str,
    href: String,
    media_type: &str,
    title: &str,
    templated: bool,
) -> Vec<ContributedLink> {
    ANCHORS
        .into_iter()
        .map(|anchor| ContributedLink {
            anchor,
            rel: rel.to_string(),
            href: href.clone(),
            media_type: media_type.to_string(),
            title: Some(title.to_string()),
            templated,
        })
        .collect()
}

/// Whether `protocol`'s root can actually answer for this collection in
/// this deployment, right now — the gate every contributed link in this
/// module passes through before any capability probe is even attempted
/// (`#220`).
///
/// Reads the exact two predicates the request path itself enforces, off the
/// same `Router`, rather than keeping a second copy that could drift:
///
/// - `Router::catalog_protocols` + `Protocol::exposure` is what
///   `app::enforce_protocol_exposure` refuses on (`#185`) — a root the
///   operator turned off answers `404` for its whole prefix, so a link into
///   it is a promise this server would break on the very next request.
/// - `Router::collection_kind` + `Protocol::serves_kind` is what
///   `app::enforce_collection_kind` refuses on (`#192`) — a record
///   collection has no geometry, so the tiles/styles/3D roots do not serve
///   it and must not be linked for it.
///
/// A catalog or collection the current routing snapshot never indexed
/// answers `false`: nothing can be checked about it, and
/// [`LinkContributor`]'s own contract says a contributor that can't check
/// contributes nothing rather than guessing.
fn root_serves(router: &Router, protocol: Protocol, resource: &ResourceRef<'_>) -> bool {
    let Some(protocols) = router.catalog_protocols(resource.catalog_id) else {
        return false;
    };
    if !protocol.exposure(&protocols).is_enabled() {
        return false;
    }
    let Some(kind) = router.collection_kind(resource.collection_id) else {
        return false;
    };
    protocol.serves_kind(kind)
}

/// `/{tenant}/tiles/catalogs/{catalog}/collections/{collection}` — the
/// collection's own subtree under the Tiles root, which both the tileset
/// resources and the styled-map lane hang off (`tellurion_tiles::router`).
fn tiles_collection_root(resource: &ResourceRef<'_>) -> String {
    format!(
        "{}/{}/tiles/catalogs/{}/collections/{}",
        resource.base_url, resource.tenant, resource.catalog, resource.collection
    )
}

/// Contributes this collection's Tiles-root links, with the vector and
/// raster lanes resolved **independently** (`#220`).
///
/// Before `#220` a single `Router::resolve_tiles` probe (a `TileSource`)
/// gated everything, which left a raster-only collection — a COG or Zarr
/// store, `#37` — advertising nothing at all even though its PNG lane
/// serves perfectly well. The two probes here mirror
/// `tellurion_tiles::handlers::tile`'s own resolution order exactly:
/// `resolve_tiles` first, `resolve_raster` only if that refused.
///
/// - `TileSource` present → MVT is servable: a `tilesets-vector` link to
///   the tileset-list resource plus the templated `.mvt` tile link. PNG
///   rides the same lane (the handler rasterizes whatever MVT the source
///   produces), so the map links below are contributed too.
/// - Only `RasterSource` present → MVT is *not* servable and no
///   `tilesets-vector` link is contributed; the map links still are.
///
/// The `{tileMatrix}`/`{tileRow}`/`{tileCol}` placeholders and the
/// `.mvt`/`.png` suffix negotiation are the contract
/// `tellurion_tiles::handlers::negotiate_format` already requires; the
/// WebMercatorQuad tiling scheme is the established default, and
/// enumerating every scheme this server mounts (`#190` added
/// WorldCRS84Quad) is the tileset-list resource's job — which is precisely
/// what the two non-templated `tilesets-*` links point at.
pub struct TilesLinkContributor;

#[async_trait::async_trait]
impl LinkContributor for TilesLinkContributor {
    async fn contribute(
        &self,
        router: &Router,
        resource: &ResourceRef<'_>,
    ) -> Vec<ContributedLink> {
        if !root_serves(router, Protocol::Tiles, resource) {
            return Vec::new();
        }
        let vector = router
            .resolve_tiles(
                resource.tenant_id,
                resource.catalog_id,
                resource.collection_id,
            )
            .await
            .is_ok();
        // Probed only when the vector lane refused — the same order (and
        // the same cost for every collection that has a `TileSource`)
        // `tellurion_tiles::handlers::tile` pays.
        let raster = !vector
            && router
                .resolve_raster(
                    resource.tenant_id,
                    resource.catalog_id,
                    resource.collection_id,
                )
                .await
                .is_ok();
        if !vector && !raster {
            // No render capability at all — the ordinary empty answer,
            // never an error and never a stub (`LinkContributor`'s own
            // contract).
            return Vec::new();
        }

        let tilesets = format!("{}/tiles", tiles_collection_root(resource));
        let template =
            format!("{tilesets}/{WEB_MERCATOR_QUAD_ID}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}");

        let mut links = Vec::new();
        if vector {
            links.extend(both_anchors(
                TILESETS_VECTOR_REL,
                tilesets.clone(),
                JSON_MEDIA_TYPE,
                "Vector tilesets",
                false,
            ));
            links.extend(both_anchors(
                TILES_REL,
                format!("{template}.mvt"),
                MVT_MEDIA_TYPE,
                "Vector tiles (MVT)",
                true,
            ));
        }
        links.extend(both_anchors(
            TILESETS_MAP_REL,
            tilesets,
            JSON_MEDIA_TYPE,
            "Map tilesets",
            false,
        ));
        links.extend(both_anchors(
            TILES_REL,
            format!("{template}.png"),
            PNG_MEDIA_TYPE,
            "Raster tiles (PNG)",
            true,
        ));
        links
    }
}

/// Contributes this collection's OGC API — Maps `map` link (`#37`) — the
/// `/collections/{collectionId}/map` resource `tellurion_tiles::maps::map`
/// serves.
///
/// OGC 20-058 Requirement 46 (`/req/collection-map/desc-links`): "The OGC
/// API collection description SHALL include a link with relation type
/// `https://www.opengis.net/def/rel/ogc/1.0/map` (or `[ogc-rel:map]`) and
/// the href pointing to the map resource for this collection." The href
/// shape is Requirement 48's own (`/req/collection-map/map-operation`):
/// `GET /collections/{collectionId}/map`.
///
/// ## Gated on the routed capability, not on the route existing
///
/// A contributor of its own rather than another branch inside
/// [`TilesLinkContributor`], because the two answer different questions.
/// That one asks what the collection's `routing.tiles` lane can serve; this
/// one asks what its `routing.maps` lane can serve — a separate lane that
/// an operator can point at a different storage entirely. So the probes
/// here are the maps lane's own two, in the same order and through the same
/// calls `tellurion_tiles::maps::resolve_maps` makes for a real request:
/// `Router::resolve_maps` (a vector `TileSource`) first, then
/// `Router::resolve_maps_raster` (a `RasterSource`, `#37`) only if that
/// refused. Neither resolving means the route would 404, and a link to a
/// 404 is exactly the unverifiable promise this module refuses to make.
///
/// Before `#37` no `map` link was contributed at all — the resource existed
/// and was reachable, but nothing advertised it. Adding it under the same
/// capability probe the handler itself performs is what makes the
/// advertisement honest rather than merely present.
///
/// Deliberately NOT accompanied by a
/// `.../ogcapi-maps-1/1.0/conf/collection-map` conformance declaration,
/// which Requirement 46 belongs to: that class also carries Requirement 47
/// (`/req/collection-map/desc-crs`), "the `crs` property in the collection
/// object ... SHALL contain URI or safe CURIEs for the list of CRSs
/// supported by the server for that collection". This server's collection
/// documents do not carry a `crs` list for a raster collection, so the
/// class is not declared — the link is honoured, the class is not, and only
/// what is honoured is claimed.
pub struct MapsLinkContributor;

#[async_trait::async_trait]
impl LinkContributor for MapsLinkContributor {
    async fn contribute(
        &self,
        router: &Router,
        resource: &ResourceRef<'_>,
    ) -> Vec<ContributedLink> {
        // `/collections/{cid}/map` is mounted on the Tiles protocol root
        // (`tellurion_tiles::handlers::router`), so the root gate is that
        // root's — a deployment that turns `tiles` off answers 404 for this
        // path too, and must not advertise it.
        if !root_serves(router, Protocol::Tiles, resource) {
            return Vec::new();
        }
        let resolves = router
            .resolve_maps(
                resource.tenant_id,
                resource.catalog_id,
                resource.collection_id,
            )
            .await
            .is_ok()
            || router
                .resolve_maps_raster(
                    resource.tenant_id,
                    resource.catalog_id,
                    resource.collection_id,
                )
                .await
                .is_ok();
        if !resolves {
            return Vec::new();
        }
        both_anchors(
            MAP_REL,
            format!("{}/map", tiles_collection_root(resource)),
            PNG_MEDIA_TYPE,
            "Map",
            false,
        )
    }
}

/// Contributes the style links for a collection whose *vector* tiles lane
/// resolves — one `stylesheet` link to the style document itself, and one
/// OGC `map` link to that style's rendered-tile endpoint for this
/// collection.
///
/// ## Collection-scoped applicability (`#220`)
///
/// The style registry is global (`tellurion-styles`' own doc: every root
/// serves the same registry), but a MapLibre style document is not:
/// `tellurion_render::resolve_layer_paints` keys every layer's paint by its
/// `source-layer`, so a style whose layers name no source layer this
/// collection's tiles actually contain paints nothing at all. Before
/// `#220` every registered style was advertised for every tiles-capable
/// collection, which is how an eight-collection deployment with three
/// styles produced twenty-four links, most of them rendering blank tiles.
///
/// Applicability is therefore checked against the *real* MVT layer names,
/// through the same `TileSource::vector_layers` call (and the same
/// `external_id()` fallback for a driver that cannot report them) the
/// TileSet resource itself advertises — so the set of styles linked here is
/// exactly the set `tellurion_tiles::handlers::styled_tile` would render
/// something for. A style document that fails to load, or that names no
/// `source-layer` at all, is not advertised: an unverifiable claim is not
/// contributed.
///
/// ## Two roots, two gates
///
/// The stylesheet link targets the **Styles** root and the map link targets
/// the **Tiles** root, so each family is gated on its own root's exposure
/// (`#185`) rather than on one combined check — a deployment that serves
/// tiles but turns `styles` off still gets its rendered-map links, and vice
/// versa.
///
/// Holds its own `Arc<dyn StyleStore>` (handed over at boot, stable across
/// reloads exactly like `AppContext.style_store` itself) rather than
/// reaching through any context type — the trait's `router` parameter stays
/// the only per-request input. A `list()` failure is logged and contributes
/// nothing: links are metadata, never worth failing the response over.
pub struct StylesLinkContributor {
    style_store: Arc<dyn StyleStore>,
}

impl StylesLinkContributor {
    pub fn new(style_store: Arc<dyn StyleStore>) -> Self {
        Self { style_store }
    }

    /// Whether `style_id`'s document targets at least one of this
    /// collection's own MVT layers. A load failure or a missing document
    /// answers `false` — see this type's own doc.
    ///
    /// The predicate itself is `tellurion_render::style_paints_any_layer`
    /// (`#245`): the TileSet resource's own styled-map links apply the same
    /// rule, and the two must never disagree about which styles are worth
    /// advertising for one collection.
    fn applies_to(&self, style_id: &str, layers: &BTreeSet<String>) -> bool {
        match self.style_store.load(style_id) {
            Ok(Some(doc)) => style_paints_any_layer(&doc, layers),
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(%error, style = %style_id, "failed to load style; contributing no links for it");
                false
            }
        }
    }
}

#[async_trait::async_trait]
impl LinkContributor for StylesLinkContributor {
    async fn contribute(
        &self,
        router: &Router,
        resource: &ResourceRef<'_>,
    ) -> Vec<ContributedLink> {
        let styles_root = root_serves(router, Protocol::Styles, resource);
        let tiles_root = root_serves(router, Protocol::Tiles, resource);
        if !styles_root && !tiles_root {
            return Vec::new();
        }
        // Both link families ride the vector tiles lane: `styled_tile`
        // resolves a `TileSource` (styling changes rasterization, not which
        // lane the geometry comes from), and a stylesheet with no rendered
        // tiles to style is a stub.
        let Ok((decl, source)) = router
            .resolve_tiles(
                resource.tenant_id,
                resource.catalog_id,
                resource.collection_id,
            )
            .await
        else {
            return Vec::new();
        };
        let mut style_ids = match self.style_store.list() {
            Ok(ids) => ids,
            Err(error) => {
                tracing::warn!(%error, "failed to list styles; contributing no style links");
                return Vec::new();
            }
        };
        // `StyleStore::list` only promises "every registered id", never a
        // stable order across implementations — sorted here so a response's
        // link order is deterministic, the same re-sort the TileSet
        // resource already applies for the same reason.
        style_ids.sort();

        let layers: BTreeSet<String> = advertised_vector_layers(&decl, source.as_ref())
            .await
            .into_iter()
            .collect();
        let tiles_root_href = tiles_collection_root(resource);
        let mut links = Vec::new();
        for style_id in style_ids {
            if !self.applies_to(&style_id, &layers) {
                continue;
            }
            if styles_root {
                links.extend(both_anchors(
                    STYLESHEET_REL,
                    format!(
                        "{}/{}/styles/catalogs/{}/styles/{style_id}",
                        resource.base_url, resource.tenant, resource.catalog
                    ),
                    STYLE_MEDIA_TYPE,
                    &style_id,
                    false,
                ));
            }
            if tiles_root {
                links.extend(both_anchors(
                    MAP_REL,
                    format!(
                        "{tiles_root_href}/styles/{style_id}/map/tiles/{WEB_MERCATOR_QUAD_ID}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}.png"
                    ),
                    PNG_MEDIA_TYPE,
                    &style_id,
                    true,
                ));
            }
        }
        links
    }
}

/// Contributes the 3D Tiles tileset link for a collection the 3D root can
/// actually serve (`#220`): the `tileset.json` resource
/// `tellurion_places::handlers::tileset` answers, which carries its own
/// content URIs — so a client reaches the glTF-binary tiles through the
/// standard 3D Tiles discovery path rather than through a templated href.
///
/// Gated on exactly what `tellurion_places::handlers::resolve_places3d`
/// requires: the tiles lane resolving to a `TileSource` *and* the
/// collection declaring `places3d`. `Router::resolve_volume` deliberately
/// does not appear here — a driver-wide `VolumeSource` answer changes how
/// the glTF is built, never whether the route answers, so gating on it
/// would either suppress a link that works (footprint+height extrusion) or
/// advertise one that 404s (a solid-geometry driver with no `places3d`
/// block).
pub struct Places3dLinkContributor;

#[async_trait::async_trait]
impl LinkContributor for Places3dLinkContributor {
    async fn contribute(
        &self,
        router: &Router,
        resource: &ResourceRef<'_>,
    ) -> Vec<ContributedLink> {
        if !root_serves(router, Protocol::ThreeDTiles, resource) {
            return Vec::new();
        }
        let Ok((decl, _source)) = router
            .resolve_tiles(
                resource.tenant_id,
                resource.catalog_id,
                resource.collection_id,
            )
            .await
        else {
            return Vec::new();
        };
        if decl.places3d.is_none() {
            return Vec::new();
        }
        both_anchors(
            PLACES3D_REL,
            format!(
                "{}/{}/3dtiles/catalogs/{}/collections/{}/3dtiles",
                resource.base_url, resource.tenant, resource.catalog, resource.collection
            ),
            JSON_MEDIA_TYPE,
            "3D Tiles tileset",
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tellurion_core::{
        AppConfig, CatalogSource, CollectionDecl, DriverFactory, FeaturePage, FeatureSource,
        Filter, ItemsQuery, PhysicalCollection, RasterSource, RasterWindow, Registry,
        Result as CoreResult, StorageDecl, StorageDriver, TileCoord, TileSource,
    };

    struct EmptyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for EmptyCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![])
        }
    }

    struct FakeBackend;

    /// The single feature the end-to-end tests below fetch as a STAC Item.
    fn demo_feature() -> serde_json::Value {
        serde_json::json!({
            "type": "Feature",
            "id": "f1",
            "geometry": null,
            "properties": {}
        })
    }

    #[async_trait::async_trait]
    impl FeatureSource for FakeBackend {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> CoreResult<FeaturePage> {
            Ok(FeaturePage {
                features_geojson: vec![demo_feature()],
                number_matched: Some(1),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            id: &str,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok((id == "f1").then(demo_feature))
        }
    }

    #[async_trait::async_trait]
    impl TileSource for FakeBackend {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<bytes::Bytes>> {
            Ok(None)
        }
    }

    /// A raster-only backend — the COG/Zarr shape (`#37`): a `RasterSource`
    /// and no `TileSource` at all, which is exactly the case `#220` says
    /// was represented inconsistently.
    struct RasterBackend;

    #[async_trait::async_trait]
    impl RasterSource for RasterBackend {
        async fn raster_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
        ) -> CoreResult<Option<RasterWindow>> {
            Ok(None)
        }
    }

    /// Advertises features + vector tiles — the shape a PostGIS-backed
    /// collection has in production.
    struct TileCapableDriver;

    impl StorageDriver for TileCapableDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeBackend))
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::new(FakeBackend))
        }
    }

    /// Advertises features only — a collection with no render lane to link.
    struct FeaturesOnlyDriver;

    impl StorageDriver for FeaturesOnlyDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeBackend))
        }
    }

    /// Advertises the raster lane only.
    struct RasterOnlyDriver;

    impl StorageDriver for RasterOnlyDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
            Some(Arc::new(RasterBackend))
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Lanes {
        Vector,
        FeaturesOnly,
        Raster,
    }

    struct TestFactory {
        lanes: Lanes,
    }

    impl DriverFactory for TestFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(match self.lanes {
                Lanes::Vector => Arc::new(TileCapableDriver) as Arc<dyn StorageDriver>,
                Lanes::FeaturesOnly => Arc::new(FeaturesOnlyDriver),
                Lanes::Raster => Arc::new(RasterOnlyDriver),
            })
        }
    }

    /// The one config every test in this module boots from. `collection` is
    /// spliced into the single collection's declaration (`kind:`,
    /// `places3d:`) and `exposure` into the catalog's own `settings:` block
    /// — the level `Router::catalog_protocols` materializes the `#185`
    /// matrix from, which is exactly where an operator writes it.
    fn config_yaml(collection: &str, exposure: &str) -> String {
        let catalog_settings = if exposure.is_empty() {
            String::new()
        } else {
            format!("    settings:\n      protocols: {{ {exposure} }}\n")
        };
        format!(
            r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs:
  - id: default
    tenant: public
{catalog_settings}
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
{collection}
"#
        )
    }

    fn router_with(lanes: Lanes, collection: &str, exposure: &str) -> Router {
        let config: AppConfig = serde_yaml::from_str(&config_yaml(collection, exposure)).unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory { lanes }));
        Router::build(&config, &registry).unwrap()
    }

    fn test_router(tiles: bool) -> Router {
        router_with(
            if tiles {
                Lanes::Vector
            } else {
                Lanes::FeaturesOnly
            },
            "",
            "",
        )
    }

    /// The one `places3d` declaration the 3D tests splice in.
    const PLACES3D: &str = "    places3d: { height_property: h }\n";

    fn resource<'a>() -> ResourceRef<'a> {
        ResourceRef {
            tenant: "acme",
            catalog: "maps",
            collection: "roads",
            item_id: None,
            base_url: "",
            tenant_id: "public",
            catalog_id: "default",
            collection_id: "demo",
        }
    }

    struct FailingStyleStore;

    impl StyleStore for FailingStyleStore {
        fn load(&self, _id: &str) -> CoreResult<Option<serde_json::Value>> {
            Err(tellurion_core::Error::Storage("boom".into()))
        }

        fn list(&self) -> CoreResult<Vec<String>> {
            Err(tellurion_core::Error::Storage("boom".into()))
        }
    }

    /// A style document targeting `source_layer`.
    fn style_doc(source_layer: &str) -> serde_json::Value {
        serde_json::json!({
            "version": 8,
            "layers": [
                { "id": "fill", "type": "fill", "source-layer": source_layer }
            ]
        })
    }

    /// Every style in this store targets the collection's own MVT layer
    /// (`demo`, the fallback `external_id()` the fake `TileSource` produces),
    /// so applicability is satisfied and the tests below exercise the other
    /// gates.
    struct FixedStyleStore(Vec<&'static str>);

    impl StyleStore for FixedStyleStore {
        fn load(&self, id: &str) -> CoreResult<Option<serde_json::Value>> {
            Ok(self.0.contains(&id).then(|| style_doc("demo")))
        }

        fn list(&self) -> CoreResult<Vec<String>> {
            Ok(self.0.iter().map(|s| s.to_string()).collect())
        }
    }

    /// Maps style id -> the one `source-layer` its document names, so a test
    /// can register a style that targets some *other* collection.
    struct TargetedStyleStore(Vec<(&'static str, &'static str)>);

    impl StyleStore for TargetedStyleStore {
        fn load(&self, id: &str) -> CoreResult<Option<serde_json::Value>> {
            Ok(self
                .0
                .iter()
                .find(|(style, _)| *style == id)
                .map(|(_, layer)| style_doc(layer)))
        }

        fn list(&self) -> CoreResult<Vec<String>> {
            Ok(self.0.iter().map(|(style, _)| style.to_string()).collect())
        }
    }

    fn rels(links: &[ContributedLink]) -> BTreeSet<&str> {
        links.iter().map(|l| l.rel.as_str()).collect()
    }

    fn hrefs_with_rel<'a>(links: &'a [ContributedLink], rel: &str) -> Vec<&'a str> {
        links
            .iter()
            .filter(|l| l.rel == rel && l.anchor == LinkAnchor::Collection)
            .map(|l| l.href.as_str())
            .collect()
    }

    // -- the exposure/kind gate (`#220`'s central hazard) -------------------

    /// The regression this slice exists to prevent: a root the operator
    /// turned off (`#185`) contributes NOTHING, even though every driver
    /// capability the link would have described is still present. Without
    /// the gate this collection contributes tiles, map and stylesheet links
    /// into a prefix that answers `404`.
    #[tokio::test]
    async fn a_disabled_tiles_root_contributes_no_tiles_map_or_stylesheet_link() {
        let router = router_with(Lanes::Vector, "", "tiles: disabled");
        // Sanity: the capability itself is intact, so the emptiness below
        // can only come from the exposure gate.
        assert!(router
            .resolve_tiles("public", "default", "demo")
            .await
            .is_ok());

        assert!(TilesLinkContributor
            .contribute(&router, &resource())
            .await
            .is_empty());

        let styles = StylesLinkContributor::new(Arc::new(FixedStyleStore(vec!["basic"])));
        let style_links = styles.contribute(&router, &resource()).await;
        assert_eq!(
            rels(&style_links),
            BTreeSet::from([STYLESHEET_REL]),
            "the stylesheet link survives (the styles root is untouched); \
             the rendered-map link must not, it lives on the disabled root"
        );
    }

    /// The mirror case: `styles` off, `tiles` on. Each family is gated on
    /// its own root, so exactly one of the two disappears.
    #[tokio::test]
    async fn a_disabled_styles_root_drops_only_the_stylesheet_links() {
        let router = router_with(Lanes::Vector, "", "styles: disabled");
        let styles = StylesLinkContributor::new(Arc::new(FixedStyleStore(vec!["basic"])));
        let links = styles.contribute(&router, &resource()).await;
        assert_eq!(rels(&links), BTreeSet::from([MAP_REL]));
        // And the tiles root itself is unaffected.
        assert!(!TilesLinkContributor
            .contribute(&router, &resource())
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn a_disabled_3dtiles_root_contributes_no_tileset_link() {
        let enabled = router_with(Lanes::Vector, PLACES3D, "");
        assert!(!Places3dLinkContributor
            .contribute(&enabled, &resource())
            .await
            .is_empty());

        let disabled = router_with(Lanes::Vector, PLACES3D, "3dtiles: disabled");
        assert!(
            Places3dLinkContributor
                .contribute(&disabled, &resource())
                .await
                .is_empty(),
            "a link into a root the operator switched off is a dangling promise"
        );
    }

    /// `#192`: a record collection has no geometry, so the tiles/styles/3D
    /// roots do not serve it — `app::enforce_collection_kind` refuses those
    /// requests, and nothing is contributed for them either. Proven with a
    /// driver that *does* advertise a `TileSource`, so only the kind
    /// partition can be producing the emptiness.
    #[tokio::test]
    async fn a_record_collection_contributes_no_render_links_even_with_a_tile_source() {
        let router = router_with(Lanes::Vector, "    kind: record\n", "");
        assert!(router
            .resolve_tiles("public", "default", "demo")
            .await
            .is_ok());

        assert!(TilesLinkContributor
            .contribute(&router, &resource())
            .await
            .is_empty());
        assert!(
            StylesLinkContributor::new(Arc::new(FixedStyleStore(vec!["basic"])))
                .contribute(&router, &resource())
                .await
                .is_empty()
        );
    }

    /// A `ResourceRef` naming ids the current routing snapshot never indexed
    /// cannot be checked at all, so nothing is claimed for it.
    #[tokio::test]
    async fn an_unindexed_collection_contributes_nothing() {
        let router = test_router(true);
        let unknown = ResourceRef {
            collection_id: "never-indexed",
            ..resource()
        };
        assert!(TilesLinkContributor
            .contribute(&router, &unknown)
            .await
            .is_empty());
        let unknown_catalog = ResourceRef {
            catalog_id: "never-indexed",
            ..resource()
        };
        assert!(TilesLinkContributor
            .contribute(&router, &unknown_catalog)
            .await
            .is_empty());
    }

    // -- `#37`: the OGC API Maps `map` link ---------------------------------

    /// GATE 4: the `map` link is contributed exactly when the maps lane
    /// resolves — for a vector collection through `Router::resolve_maps`,
    /// and for a raster-only collection (COG/Zarr, no `TileSource` at all)
    /// through `Router::resolve_maps_raster`.
    ///
    /// Both cases in one test on purpose: the load-bearing claim is that
    /// the SAME link appears for both, which is only checkable by looking
    /// at both. OGC 20-058 Requirement 46 fixes the rel and Requirement 48
    /// the href.
    #[tokio::test]
    async fn the_map_link_is_contributed_for_a_vector_and_for_a_raster_collection() {
        for lanes in [Lanes::Vector, Lanes::Raster] {
            let router = router_with(lanes, "", "");
            let links = MapsLinkContributor.contribute(&router, &resource()).await;
            assert_eq!(rels(&links), BTreeSet::from([MAP_REL]));
            assert_eq!(
                hrefs_with_rel(&links, MAP_REL),
                vec!["/acme/tiles/catalogs/maps/collections/roads/map"],
                "Requirement 48: GET /collections/{{collectionId}}/map"
            );
            // Both anchors, like every other per-collection link here: a map
            // is a fact about the collection, not about any one row.
            for anchor in ANCHORS {
                assert_eq!(links.iter().filter(|l| l.anchor == anchor).count(), 1);
            }
            assert!(
                links.iter().all(|link| !link.templated),
                "the map resource takes query parameters, not path placeholders — a \
                 templated link would tell a client to substitute something"
            );
            assert!(links.iter().all(|link| link.media_type == PNG_MEDIA_TYPE));
        }
    }

    /// GATE 4, the other half: a collection whose maps lane resolves to
    /// NEITHER capability gets no `map` link. The route 404s for it, and a
    /// link to a 404 is exactly the unverifiable promise this module
    /// refuses to make — the link-level twin of never declaring a
    /// conformance class you do not honour.
    #[tokio::test]
    async fn a_collection_with_neither_maps_capability_gets_no_map_link() {
        let router = router_with(Lanes::FeaturesOnly, "", "");
        assert!(router
            .resolve_maps("public", "default", "demo")
            .await
            .is_err());
        assert!(router
            .resolve_maps_raster("public", "default", "demo")
            .await
            .is_err());
        assert!(
            MapsLinkContributor
                .contribute(&router, &resource())
                .await
                .is_empty(),
            "a features-only collection has no map resource, so nothing may advertise one"
        );
    }

    /// The `map` resource is mounted on the Tiles root, so an operator who
    /// switches that root off silences this link too — the same `#185` gate
    /// every other link in this module passes through.
    #[tokio::test]
    async fn a_disabled_tiles_root_contributes_no_map_link() {
        let router = router_with(Lanes::Raster, "", "tiles: disabled");
        assert!(MapsLinkContributor
            .contribute(&router, &resource())
            .await
            .is_empty());
    }

    // -- tiles: vector and raster resolved independently --------------------

    #[tokio::test]
    async fn a_vector_collection_gets_both_tileset_rels_and_both_tile_templates() {
        let links = TilesLinkContributor
            .contribute(&test_router(true), &resource())
            .await;

        assert_eq!(
            rels(&links),
            BTreeSet::from([TILES_REL, TILESETS_VECTOR_REL, TILESETS_MAP_REL])
        );
        for anchor in ANCHORS {
            assert_eq!(
                links.iter().filter(|l| l.anchor == anchor).count(),
                4,
                "tilesets-vector + mvt + tilesets-map + png, per anchor"
            );
        }
        // External ids only in the href — never the internal
        // `public`/`default`/`demo` trio the router was probed with.
        assert_eq!(
            hrefs_with_rel(&links, TILESETS_VECTOR_REL),
            vec!["/acme/tiles/catalogs/maps/collections/roads/tiles"]
        );
        let mut tiles = hrefs_with_rel(&links, TILES_REL);
        tiles.sort_unstable();
        assert_eq!(
            tiles,
            vec![
                "/acme/tiles/catalogs/maps/collections/roads/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.mvt",
                "/acme/tiles/catalogs/maps/collections/roads/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.png",
            ]
        );
        assert!(links
            .iter()
            .filter(|l| l.rel == TILES_REL)
            .all(|l| l.templated));
        assert!(links
            .iter()
            .filter(|l| l.rel != TILES_REL)
            .all(|l| !l.templated && l.media_type == JSON_MEDIA_TYPE));
    }

    /// `#220`'s raster-only case: a COG/Zarr collection has no `TileSource`,
    /// so before this slice it contributed nothing at all. It must now get
    /// the PNG lane it genuinely serves — and must NOT get the MVT lane it
    /// does not.
    #[tokio::test]
    async fn a_raster_only_collection_gets_the_map_links_and_no_vector_ones() {
        let router = router_with(Lanes::Raster, "", "");
        assert!(router
            .resolve_tiles("public", "default", "demo")
            .await
            .is_err());
        assert!(router
            .resolve_raster("public", "default", "demo")
            .await
            .is_ok());

        let links = TilesLinkContributor.contribute(&router, &resource()).await;
        assert_eq!(rels(&links), BTreeSet::from([TILES_REL, TILESETS_MAP_REL]));
        assert_eq!(
            hrefs_with_rel(&links, TILES_REL),
            vec![
                "/acme/tiles/catalogs/maps/collections/roads/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.png"
            ],
            "a raster collection has no MVT to advertise"
        );
    }

    #[tokio::test]
    async fn tiles_contributor_contributes_nothing_for_a_features_only_collection() {
        let links = TilesLinkContributor
            .contribute(&test_router(false), &resource())
            .await;
        assert!(links.is_empty(), "no capability means no link, no stub");
    }

    #[tokio::test]
    async fn tiles_contributor_applies_the_base_url_prefix() {
        let resource = ResourceRef {
            base_url: "https://example.test",
            ..resource()
        };
        let links = TilesLinkContributor
            .contribute(&test_router(true), &resource)
            .await;
        assert!(!links.is_empty());
        assert!(links
            .iter()
            .all(|l| l.href.starts_with("https://example.test/acme/")));
    }

    // -- styles: collection-scoped applicability ----------------------------

    #[tokio::test]
    async fn styles_contributor_emits_a_stylesheet_and_a_map_link_per_applicable_style() {
        let contributor =
            StylesLinkContributor::new(Arc::new(FixedStyleStore(vec!["basic", "dark"])));
        let links = contributor
            .contribute(&test_router(true), &resource())
            .await;

        assert_eq!(links.len(), 8, "2 styles x 2 rels x 2 anchors");
        let sheets = hrefs_with_rel(&links, STYLESHEET_REL);
        assert_eq!(
            sheets,
            vec![
                "/acme/styles/catalogs/maps/styles/basic",
                "/acme/styles/catalogs/maps/styles/dark",
            ],
            "sorted, so link order never depends on the store's listing order"
        );
        assert!(links
            .iter()
            .filter(|l| l.rel == STYLESHEET_REL)
            .all(|l| !l.templated && l.media_type == STYLE_MEDIA_TYPE));
        assert_eq!(
            hrefs_with_rel(&links, MAP_REL),
            vec![
                "/acme/tiles/catalogs/maps/collections/roads/styles/basic/map/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.png",
                "/acme/tiles/catalogs/maps/collections/roads/styles/dark/map/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.png",
            ]
        );
        assert!(links
            .iter()
            .filter(|l| l.rel == MAP_REL)
            .all(|l| l.templated && l.media_type == PNG_MEDIA_TYPE));
    }

    /// `#220`: "do not advertise every global style for every collection".
    /// `other` targets a source layer this collection's tiles never carry,
    /// so it paints nothing and is not advertised; `mine` targets the
    /// collection's own MVT layer name and is.
    #[tokio::test]
    async fn a_style_that_targets_another_collections_layer_is_not_advertised() {
        let contributor = StylesLinkContributor::new(Arc::new(TargetedStyleStore(vec![
            ("mine", "demo"),
            ("other", "somebody-elses-layer"),
        ])));
        let links = contributor
            .contribute(&test_router(true), &resource())
            .await;
        assert_eq!(
            hrefs_with_rel(&links, STYLESHEET_REL),
            vec!["/acme/styles/catalogs/maps/styles/mine"]
        );
        assert!(links.iter().all(|l| l.title.as_deref() == Some("mine")));
    }

    /// A style whose document names no `source-layer` at all paints nothing
    /// on any collection — advertising it would be a link to a blank tile.
    #[tokio::test]
    async fn a_style_with_no_source_layer_is_not_advertised() {
        struct BackgroundOnly;
        impl StyleStore for BackgroundOnly {
            fn load(&self, _id: &str) -> CoreResult<Option<serde_json::Value>> {
                Ok(Some(serde_json::json!({
                    "version": 8,
                    "layers": [ { "id": "bg", "type": "background" } ]
                })))
            }
            fn list(&self) -> CoreResult<Vec<String>> {
                Ok(vec!["bg-only".to_string()])
            }
        }
        let links = StylesLinkContributor::new(Arc::new(BackgroundOnly))
            .contribute(&test_router(true), &resource())
            .await;
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn styles_contributor_contributes_nothing_without_the_vector_tiles_capability() {
        let contributor = StylesLinkContributor::new(Arc::new(FixedStyleStore(vec!["basic"])));
        assert!(contributor
            .contribute(&test_router(false), &resource())
            .await
            .is_empty());
        // A raster collection has no MVT layers for a style to target
        // either — `styled_tile` itself resolves a `TileSource`.
        assert!(contributor
            .contribute(&router_with(Lanes::Raster, "", ""), &resource())
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn styles_contributor_contributes_nothing_when_no_styles_are_registered() {
        let contributor = StylesLinkContributor::new(Arc::new(FixedStyleStore(vec![])));
        let links = contributor
            .contribute(&test_router(true), &resource())
            .await;
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn a_style_store_failure_degrades_to_no_links_not_an_error() {
        let contributor = StylesLinkContributor::new(Arc::new(FailingStyleStore));
        let links = contributor
            .contribute(&test_router(true), &resource())
            .await;
        assert!(links.is_empty());
    }

    // -- 3D tiles ------------------------------------------------------------

    #[tokio::test]
    async fn places3d_link_needs_both_the_tiles_lane_and_the_declaration() {
        // Declared + tiles-capable: linked.
        let links = Places3dLinkContributor
            .contribute(&router_with(Lanes::Vector, PLACES3D, ""), &resource())
            .await;
        assert_eq!(
            hrefs_with_rel(&links, PLACES3D_REL),
            vec!["/acme/3dtiles/catalogs/maps/collections/roads/3dtiles"]
        );
        assert!(links.iter().all(|l| !l.templated));

        // Tiles-capable but undeclared: `resolve_places3d` would refuse.
        assert!(Places3dLinkContributor
            .contribute(&test_router(true), &resource())
            .await
            .is_empty());

        // Declared but no tiles lane: `resolve_places3d` would refuse too.
        assert!(Places3dLinkContributor
            .contribute(&router_with(Lanes::FeaturesOnly, PLACES3D, ""), &resource())
            .await
            .is_empty());
    }

    // -- end-to-end through the real app route tree -------------------------
    //
    // With the three production contributors registered exactly as `main`
    // registers them, STAC collection and item responses carry the typed
    // capability links, and the Features collection response carries them
    // too — without duplicating the sibling links it already builds itself.

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use tower::ServiceExt;

    use tellurion_core::{
        AppContext, LinkContributors, MokaTileCache, Resolver, StaticResolver, TileCache,
    };

    fn integration_app(collection: &str, exposure: &str) -> axum::Router {
        let config: AppConfig = serde_yaml::from_str(&config_yaml(collection, exposure)).unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            lanes: Lanes::Vector,
        }));
        let router = Router::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FixedStyleStore(vec!["basic"]));

        // Exactly `main`'s own registration, against this test's stores.
        let mut contributors = LinkContributors::new();
        contributors.register("3dtiles", Arc::new(Places3dLinkContributor));
        contributors.register(
            "styles",
            Arc::new(StylesLinkContributor::new(Arc::clone(&style_store))),
        );
        contributors.register("tiles", Arc::new(TilesLinkContributor));

        let ctx = Arc::new(
            AppContext::new(config, router, resolver, None, cache, style_store)
                .with_link_contributors(contributors),
        );
        crate::app::build(ctx, PrometheusBuilder::new().build_recorder().handle(), 60)
    }

    async fn get_json(app: &axum::Router, path: &str) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::OK,
            "GET {path} must serve"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn links_with_rel<'a>(links: &'a serde_json::Value, rel: &str) -> Vec<&'a serde_json::Value> {
        links
            .as_array()
            .expect("links array present")
            .iter()
            .filter(|l| l["rel"] == rel)
            .collect()
    }

    #[tokio::test]
    async fn stac_collection_response_carries_every_typed_capability_link() {
        let app = integration_app(PLACES3D, "");
        let body = get_json(&app, "/public/stac/catalogs/default/collections/demo").await;

        let tiles = links_with_rel(&body["links"], TILES_REL);
        assert_eq!(
            tiles.len(),
            2,
            "mvt + png once each — Item-anchored duplicates must be filtered out"
        );
        assert!(tiles.iter().all(|l| l["templated"] == true));
        assert!(tiles.iter().any(|l| l["href"]
            == "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.mvt"));

        for rel in [TILESETS_VECTOR_REL, TILESETS_MAP_REL] {
            let found = links_with_rel(&body["links"], rel);
            assert_eq!(found.len(), 1, "{rel}");
            assert_eq!(
                found[0]["href"],
                "/public/tiles/catalogs/default/collections/demo/tiles"
            );
            assert!(
                found[0].get("templated").is_none(),
                "{rel} is dereferenceable"
            );
        }

        let stylesheets = links_with_rel(&body["links"], STYLESHEET_REL);
        assert_eq!(stylesheets.len(), 1);
        assert_eq!(
            stylesheets[0]["href"],
            "/public/styles/catalogs/default/styles/basic"
        );
        assert_eq!(stylesheets[0]["type"], STYLE_MEDIA_TYPE);

        let maps = links_with_rel(&body["links"], MAP_REL);
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0]["title"], "basic");

        let three_d = links_with_rel(&body["links"], PLACES3D_REL);
        assert_eq!(three_d.len(), 1);
        assert_eq!(
            three_d[0]["href"],
            "/public/3dtiles/catalogs/default/collections/demo/3dtiles"
        );
    }

    /// The whole-response proof of the hazard: with `tiles` and `3dtiles`
    /// switched off for the catalog, the STAC document a client actually
    /// receives carries no link into either prefix — while the stylesheet
    /// link, whose own root is still exposed, survives.
    #[tokio::test]
    async fn a_stac_document_never_links_into_a_root_the_operator_switched_off() {
        let app = integration_app(PLACES3D, "tiles: disabled, 3dtiles: disabled");
        let body = get_json(&app, "/public/stac/catalogs/default/collections/demo").await;
        let links = body["links"].as_array().unwrap();
        for rel in [
            TILES_REL,
            TILESETS_VECTOR_REL,
            TILESETS_MAP_REL,
            MAP_REL,
            PLACES3D_REL,
        ] {
            assert!(
                links.iter().all(|l| l["rel"] != rel),
                "{rel} must not appear: {body}"
            );
        }
        assert!(links.iter().any(|l| l["rel"] == STYLESHEET_REL));
        // And the prefix really is gone, which is what made those links a
        // broken promise in the first place.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/public/tiles/catalogs/default/collections/demo/tiles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stac_item_responses_carry_the_same_per_collection_links() {
        let app = integration_app("", "");

        // Single item.
        let item = get_json(
            &app,
            "/public/stac/catalogs/default/collections/demo/items/f1",
        )
        .await;
        assert_eq!(links_with_rel(&item["links"], TILES_REL).len(), 2);
        assert_eq!(links_with_rel(&item["links"], STYLESHEET_REL).len(), 1);
        assert_eq!(links_with_rel(&item["links"], MAP_REL).len(), 1);

        // Items page: every feature carries the same per-collection links.
        let page = get_json(&app, "/public/stac/catalogs/default/collections/demo/items").await;
        let feature = &page["features"][0];
        assert_eq!(links_with_rel(&feature["links"], TILES_REL).len(), 2);
        assert_eq!(links_with_rel(&feature["links"], STYLESHEET_REL).len(), 1);

        // /search items are the same item shape — no "except /search" gap.
        let search = get_json(&app, "/public/stac/catalogs/default/search").await;
        let found = &search["features"][0];
        assert_eq!(links_with_rel(&found["links"], TILES_REL).len(), 2);
        assert_eq!(links_with_rel(&found["links"], STYLESHEET_REL).len(), 1);
    }

    /// `#220`: the Features Collection resource already builds its own
    /// `tilesets-vector`/`tilesets-map`/3D sibling links (`#49`), and a
    /// contributor names the same resources under the same registered rels.
    /// The merge must not state either claim twice.
    #[tokio::test]
    async fn features_collection_response_carries_contributed_links_without_duplicating_its_own() {
        let app = integration_app(PLACES3D, "");
        let body = get_json(&app, "/public/features/catalogs/default/collections/demo").await;

        for rel in [TILESETS_VECTOR_REL, TILESETS_MAP_REL, PLACES3D_REL] {
            assert_eq!(
                links_with_rel(&body["links"], rel).len(),
                1,
                "{rel} must appear exactly once: {body}"
            );
        }
        assert_eq!(links_with_rel(&body["links"], TILES_REL).len(), 2);
        assert_eq!(links_with_rel(&body["links"], STYLESHEET_REL).len(), 1);
        assert_eq!(links_with_rel(&body["links"], MAP_REL).len(), 1);
    }
}
