//! Settings inheritance (`#39`): a minimal, whitelisted chain from platform
//! down to collection. Three keys travel the chain: per-zoom tile caps, tile
//! cache TTL, and the slow-request threshold — see `config::SettingsDecl`.
//! Resolution is per key, nearest level wins: a collection that sets a key wins
//! outright; otherwise its catalog's value shows through, then its tenant's,
//! then the platform's; a key nobody ever set falls back to this module's
//! own default. Values never merge across levels — `tile_caps` is a whole
//! map replacement, not an entry-by-entry union — so "which level supplied
//! this key" is always a single, unambiguous answer.
//!
//! `Router::build` materializes one `EffectiveSettings` per collection at
//! load time (keyed by the collection's internal id) rather than walking the
//! chain on every request; see `Router::effective_settings`.
//!
//! **Named profiles (`#111`).** Any level's `SettingsDecl.profile` may name
//! one `config::ProfileDecl` by id; the resolver expands it as if the
//! profile's own keys were declared inline at that same level — an
//! explicit key at that level still wins over the profile's value for that
//! key, and the profile only ever fills a gap *within* its own level's slot
//! in the chain, so nearest-level-wins across levels is unchanged (a
//! collection-level profile still beats a catalog-level explicit key). No
//! new precedence algebra: `resolve_field` below is still the one walk,
//! just checking two candidates per level instead of one.

use std::collections::HashMap;

use serde::Serialize;

use crate::batch::BatchConfig;
use crate::config::{ColormapConf, ProtocolsConf, SettingsDecl, StacConf, ZoomCaps};

/// Fallback `cache_ttl_s` when no level in the chain ever set one. Matches
/// `cache::L2CacheAdapter`'s own default TTL magnitude
/// (`config::default_l2_ttl_s`) so an operator who sets neither sees the
/// same effective freshness window either way.
pub const DEFAULT_SETTINGS_CACHE_TTL_S: u64 = 3600;

/// Fallback slow-request threshold when no settings level declares one.
pub const DEFAULT_SLOW_REQUEST_MS: u64 = 1_000;

/// Fallback write-lane request body cap (`#91`) when no settings level
/// declares one — 1 MiB, sized for a single-feature `PUT`/`POST` body. An
/// operator who genuinely bulk-loads raises `settings.max_request_body_bytes`
/// deliberately rather than relying on this default.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 1_048_576;

/// Fallback per-tile vertex budget (`#90`) when no settings level declares
/// one — generous relative to the existing per-zoom feature cap
/// (`descriptor::heuristics::MAX_FEATURE_CAP` is 50,000 features) so an
/// ordinary tile never observes it; a collection carrying genuinely dense
/// geometry (the coastline/admin-boundary case `#90` exists for) opts into
/// a tighter `settings.tile_vertex_budget` deliberately.
pub const DEFAULT_TILE_VERTEX_BUDGET: u64 = 500_000;

/// Fallback cumulative source-geometry budget for one exact items page or
/// single-item response.
pub const DEFAULT_ITEMS_VERTEX_BUDGET: u64 = 50_000;

/// Fallback direct-upload asset size cap (assets-and-object-storage
/// proposal, first slice) when no settings level declares one — 10 MiB, an
/// order of magnitude above the write-lane default: an asset is typically a
/// thumbnail, a document, or a small raster, not a single JSON feature body.
/// An operator serving larger assets raises `settings.max_asset_bytes`
/// deliberately, the same "explicit, never a silent backend default" rule
/// `max_request_body_bytes` follows.
pub const DEFAULT_MAX_ASSET_BYTES: u64 = 10_485_760;

/// The materialized result of walking the settings chain for one collection:
/// concrete values, never `Option`, ready to consume without re-checking
/// every ancestor level again.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveSettings {
    pub tile_caps: ZoomCaps,
    pub cache_ttl_s: u64,
    pub slow_request_ms: u64,
    /// This collection's effective `stac:` config subtree (`#36`), or `None`
    /// when no level in the chain ever set one — `tellurion-stac` applies
    /// its own defaults for that case, so there is no module-level default
    /// to fall back to here the way `tile_caps`/`cache_ttl_s` have one.
    pub stac: Option<StacConf>,
    /// This collection's effective vector-tile property allowlist (`#85`).
    /// Empty when no level in the chain ever set one — the module-level
    /// default, matching pk-only projection, the same behavior every
    /// collection had before `#85`.
    pub tile_properties: Vec<String>,
    /// This collection's effective single-band colormap (`#92`), or `None`
    /// when no level in the chain ever declared one — `tellurion-cog`
    /// serves plain grayscale/RGB(A) for that case, so there is no
    /// module-level default to fall back to here either.
    pub colormap: Option<ColormapConf>,
    /// This collection's effective write-lane request body cap in bytes
    /// (`#91`), or [`DEFAULT_MAX_REQUEST_BODY_BYTES`] when no level in the
    /// chain ever declared one.
    pub max_request_body_bytes: u64,
    /// This collection's effective per-tile vertex budget (`#90`), or
    /// [`DEFAULT_TILE_VERTEX_BUDGET`] when no level in the chain ever
    /// declared one.
    pub tile_vertex_budget: u64,
    /// This collection's exact-response vertex budget, or
    /// [`DEFAULT_ITEMS_VERTEX_BUDGET`] when no level declares one.
    pub items_vertex_budget: u64,
    /// This collection's effective items-page byte budget (`#184`), or
    /// `None` when no level in the chain ever declared one. Unlike the
    /// scalar budgets above there is deliberately no module-level default
    /// to fall back to: `None` means the byte-budget lane is off and items
    /// pages pass through exactly as before `#184` — the one
    /// `EffectiveSettings` field that stays `Option` end-to-end (see
    /// `config::SettingsDecl::page_max_bytes`'s own doc).
    pub page_max_bytes: Option<u64>,
    /// This collection's effective direct-upload asset size cap in bytes
    /// (assets-and-object-storage proposal, first slice), or
    /// [`DEFAULT_MAX_ASSET_BYTES`] when no level in the chain ever declared
    /// one.
    pub max_asset_bytes: u64,
    /// This collection's effective asset media-type allow-list. Empty when
    /// no level in the chain ever set one — unrestricted, the module-level
    /// default (see `config::SettingsDecl::asset_media_types`'s own doc).
    pub asset_media_types: Vec<String>,
    /// This collection's effective batch-ingest budget/chunk-size
    /// configuration (`#114`) — always concrete, `batch::BatchConfig::
    /// default()` when no level in the chain ever declared a
    /// `settings.batch` at all. See `crate::batch`'s own module doc for why
    /// this rides the full four-level chain (unlike `admission`, which is
    /// restricted to platform/tenant).
    pub batch: BatchConfig,
    /// This node's effective protocol exposure matrix (`#185`), or `None`
    /// when no level in the chain ever declared one. Like `page_max_bytes`
    /// above (and unlike every concrete budget here) there is deliberately
    /// no module-level default to materialize into: `None` means nobody
    /// expressed an opinion, and every protocol root serves exactly as it
    /// did before `#185`. Consumers ask
    /// [`EffectiveSettings::protocols_or_default`] rather than inventing a
    /// `ProtocolsConf::default()` of their own, so "unset" and "explicitly
    /// all-enabled" stay distinguishable in `/config/effective` while
    /// behaving identically on the request path.
    pub protocols: Option<ProtocolsConf>,
}

impl EffectiveSettings {
    /// The exposure matrix to enforce for this node: whatever the chain
    /// resolved, or an all-enabled matrix when nothing declared one. The one
    /// place `None` is collapsed into behavior — see [`EffectiveSettings::
    /// protocols`] for why the field itself stays `Option`.
    pub fn protocols_or_default(&self) -> ProtocolsConf {
        self.protocols.unwrap_or_default()
    }
}

impl Default for EffectiveSettings {
    fn default() -> Self {
        Self {
            tile_caps: ZoomCaps::default(),
            cache_ttl_s: DEFAULT_SETTINGS_CACHE_TTL_S,
            slow_request_ms: DEFAULT_SLOW_REQUEST_MS,
            stac: None,
            tile_properties: Vec::new(),
            colormap: None,
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            tile_vertex_budget: DEFAULT_TILE_VERTEX_BUDGET,
            items_vertex_budget: DEFAULT_ITEMS_VERTEX_BUDGET,
            page_max_bytes: None,
            max_asset_bytes: DEFAULT_MAX_ASSET_BYTES,
            asset_media_types: Vec::new(),
            batch: BatchConfig::default(),
            protocols: None,
        }
    }
}

/// Which level in the platform -> tenant -> catalog -> collection chain
/// declared a value, or supplied the level a resolved key's provenance
/// names (`#110`, the effective-config view). Independent of which node a
/// view was requested for — see [`SettingsProvenance`]'s own doc for how
/// "local" vs "inherited" is decided relative to the queried node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsLevel {
    Platform,
    Tenant,
    Catalog,
    Collection,
}

/// Where one [`EffectiveSettings`] field's value came from (`#110`): the
/// same nearest-level-wins walk [`resolve_effective_settings`] already
/// performs, surfaced instead of discarded. `Declared` always names the
/// literal level whose own `SettingsDecl` supplied the value — turning that
/// into the effective-config view's "local override" vs "inherited (naming
/// the level)" distinction is the *caller's* job, relative to whichever
/// node the view was requested for (a catalog's own `Declared { level:
/// Catalog }` is that catalog's local override; a collection's identical
/// `Declared { level: Catalog }` is inherited from its catalog) — see
/// `resolve_effective_settings_with_provenance`'s own doc for why that
/// relabeling is deliberately not done here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsProvenance {
    /// No level in the chain declared this key; the value is this module's
    /// own fallback default ([`EffectiveSettings::default`]).
    BuiltInDefault,
    /// `level`'s own `SettingsDecl` declared this key and won the chain.
    Declared { level: SettingsLevel },
    /// Computed by a rule other than the plain nearest-level-wins settings
    /// chain — today only `tile_caps`, when a collection's own physical
    /// `tiles.caps` block (`CollectionDecl.tiles`, a different field than
    /// `SettingsDecl.tile_caps`) wins outright over whatever the settings
    /// chain would otherwise contribute. See `Router::build_from_snapshot`,
    /// which is the only place that ever constructs this variant — nothing
    /// in this module can, since it never sees a `CollectionDecl`.
    Derived,
    /// `level`'s own `profile:` reference (`#111`, `SettingsDecl.profile`)
    /// named `profile_id`, and that profile's own fragment declared this
    /// key — `level`'s own declaration left it unset (see [`resolve_field`]'s
    /// own doc for the exact per-level order this competes with `Declared`
    /// under). `profile_id` is the one-line answer to "why does this have
    /// this value."
    Profile {
        level: SettingsLevel,
        profile_id: String,
    },
}

/// [`EffectiveSettings`], field for field, tagged with where each value
/// came from (`#110`). Produced only by
/// [`resolve_effective_settings_with_provenance`] — see that function's own
/// doc.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveSettingsProvenance {
    pub tile_caps: SettingsProvenance,
    pub cache_ttl_s: SettingsProvenance,
    pub slow_request_ms: SettingsProvenance,
    pub stac: SettingsProvenance,
    pub tile_properties: SettingsProvenance,
    pub colormap: SettingsProvenance,
    pub max_request_body_bytes: SettingsProvenance,
    pub tile_vertex_budget: SettingsProvenance,
    pub items_vertex_budget: SettingsProvenance,
    pub page_max_bytes: SettingsProvenance,
    pub max_asset_bytes: SettingsProvenance,
    pub asset_media_types: SettingsProvenance,
    pub batch: SettingsProvenance,
    pub protocols: SettingsProvenance,
}

/// One level's input to [`resolve_field`]'s walk (`#111`): the level's own
/// declaration, plus — when that declaration names a profile
/// (`SettingsDecl.profile`) the caller already resolved by id — that
/// profile's id and its own settings fragment. Built once per level by
/// [`resolve_effective_settings_with_provenance`].
struct ChainLevel<'a> {
    level: SettingsLevel,
    decl: &'a SettingsDecl,
    profile: Option<(&'a str, &'a SettingsDecl)>,
}

/// Walks `chain` (nearest level first); at each level, `get` is tried
/// against the level's own declaration first and, only if that leaves the
/// key unset, against the level's named profile (`#111`) if it has one —
/// so an explicit key at a level always wins over that same level's own
/// profile, while a profile referenced at a nearer level still wins over an
/// explicit key at a farther one (the per-level check happens entirely
/// before the walk ever moves to the next level). Returns the first value
/// found paired with which level (and, for a profile-sourced value, which
/// profile) supplied it — or `default` paired with
/// [`SettingsProvenance::BuiltInDefault`] when nothing in the chain
/// declares the key. The one place
/// `resolve_effective_settings_with_provenance` decides "declared vs
/// profile vs default, and which level," so every field below resolves
/// through it rather than repeating the walk with its own inline logic.
fn resolve_field<T: Clone>(
    chain: &[ChainLevel<'_>; 4],
    default: T,
    get: impl Fn(&SettingsDecl) -> Option<T>,
) -> (T, SettingsProvenance) {
    for entry in chain {
        if let Some(value) = get(entry.decl) {
            return (value, SettingsProvenance::Declared { level: entry.level });
        }
        if let Some((profile_id, profile_decl)) = entry.profile {
            if let Some(value) = get(profile_decl) {
                return (
                    value,
                    SettingsProvenance::Profile {
                        level: entry.level,
                        profile_id: profile_id.to_string(),
                    },
                );
            }
        }
    }
    (default, SettingsProvenance::BuiltInDefault)
}

/// Resolves one node's effective settings from the four-level chain,
/// nearest first: `collection`, `catalog`, `tenant`, `platform`. Each
/// whitelisted key independently takes the first `Some` it finds walking
/// that order (a level's own named profile filling a gap of its own before
/// the walk moves on, `#111` — see [`resolve_field`]'s own doc); a key none
/// of the four levels or their profiles set falls back to
/// [`EffectiveSettings::default`]. `profiles` maps a profile id to its own
/// settings fragment — pass an empty map for a caller with no `profiles:`
/// block to expand. Thin wrapper over
/// [`resolve_effective_settings_with_provenance`] that discards the
/// provenance half — see that function's own doc.
pub fn resolve_effective_settings(
    collection: &SettingsDecl,
    catalog: &SettingsDecl,
    tenant: &SettingsDecl,
    platform: &SettingsDecl,
    profiles: &HashMap<&str, &SettingsDecl>,
) -> EffectiveSettings {
    resolve_effective_settings_with_provenance(collection, catalog, tenant, platform, profiles).0
}

/// Same resolution [`resolve_effective_settings`] performs, plus per-key
/// provenance (`#110`): which level's own declaration (or the module
/// default) supplied each field. `resolve_effective_settings` is a thin
/// wrapper over this function, not a sibling implementation — there is
/// exactly one place this chain is ever walked, so a caller that needs
/// provenance (the control lane's effective-config view,
/// `tellurion-server::config_view`) can never see a value that disagrees
/// with what the request lanes actually apply (`Router::build_from_snapshot`
/// calls this directly and stores both halves — see
/// `Router::effective_settings`/`Router::effective_settings_provenance`).
///
/// Callers resolving a node other than a real collection (a platform,
/// tenant, or catalog view with no collection in play) pass
/// `SettingsDecl::default()` for `collection` (and, for a tenant or
/// platform view, `catalog` too) — an empty declaration can never win the
/// chain, so the nearest non-empty level found is always at or above the
/// node actually being queried.
///
/// `profiles` (`#111`) maps a profile id to its own settings fragment — the
/// caller's own lookup of every `SettingsDecl.profile` reference it might
/// need, keyed by id; pass an empty map when nothing in the chain can name
/// one (or to resolve as if profiles didn't exist at all). Each of
/// `collection`/`catalog`/`tenant`/`platform` may itself name at most one
/// profile via its own `profile` field — expansion happens inside
/// [`resolve_field`]'s walk, never by pre-merging a profile's keys into the
/// `SettingsDecl` passed in here, so provenance can still tell "this
/// level's own key" apart from "this level's profile's key."
pub fn resolve_effective_settings_with_provenance<'a>(
    collection: &'a SettingsDecl,
    catalog: &'a SettingsDecl,
    tenant: &'a SettingsDecl,
    platform: &'a SettingsDecl,
    profiles: &HashMap<&'a str, &'a SettingsDecl>,
) -> (EffectiveSettings, EffectiveSettingsProvenance) {
    let profile_of = |decl: &'a SettingsDecl| -> Option<(&'a str, &'a SettingsDecl)> {
        let profile_id = decl.profile.as_deref()?;
        profiles
            .get(profile_id)
            .map(|settings| (profile_id, *settings))
    };
    let chain: [ChainLevel<'a>; 4] = [
        ChainLevel {
            level: SettingsLevel::Collection,
            decl: collection,
            profile: profile_of(collection),
        },
        ChainLevel {
            level: SettingsLevel::Catalog,
            decl: catalog,
            profile: profile_of(catalog),
        },
        ChainLevel {
            level: SettingsLevel::Tenant,
            decl: tenant,
            profile: profile_of(tenant),
        },
        ChainLevel {
            level: SettingsLevel::Platform,
            decl: platform,
            profile: profile_of(platform),
        },
    ];
    let defaults = EffectiveSettings::default();

    let (tile_caps, tile_caps_provenance) =
        resolve_field(&chain, defaults.tile_caps, |decl| decl.tile_caps.clone());
    let (cache_ttl_s, cache_ttl_s_provenance) =
        resolve_field(&chain, defaults.cache_ttl_s, |decl| decl.cache_ttl_s);
    let (slow_request_ms, slow_request_ms_provenance) =
        resolve_field(&chain, defaults.slow_request_ms, |decl| {
            decl.slow_request_ms
        });
    let (stac, stac_provenance) =
        resolve_field(&chain, defaults.stac, |decl| decl.stac.clone().map(Some));
    let (tile_properties, tile_properties_provenance) =
        resolve_field(&chain, defaults.tile_properties, |decl| {
            decl.tile_properties.clone()
        });
    let (colormap, colormap_provenance) = resolve_field(&chain, defaults.colormap, |decl| {
        decl.colormap.clone().map(Some)
    });
    let (max_request_body_bytes, max_request_body_bytes_provenance) =
        resolve_field(&chain, defaults.max_request_body_bytes, |decl| {
            decl.max_request_body_bytes
        });
    let (tile_vertex_budget, tile_vertex_budget_provenance) =
        resolve_field(&chain, defaults.tile_vertex_budget, |decl| {
            decl.tile_vertex_budget
        });
    let (items_vertex_budget, items_vertex_budget_provenance) =
        resolve_field(&chain, defaults.items_vertex_budget, |decl| {
            decl.items_vertex_budget
        });
    // `page_max_bytes` (`#184`) resolves like `colormap`/`stac` above — an
    // `Option` value with no built-in default, `.map(Some)` lifting a
    // declared level into the walk — because `None` is a real effective
    // outcome (the byte-budget lane off), not a gap to fill.
    let (page_max_bytes, page_max_bytes_provenance) =
        resolve_field(&chain, defaults.page_max_bytes, |decl| {
            decl.page_max_bytes.map(Some)
        });
    let (max_asset_bytes, max_asset_bytes_provenance) =
        resolve_field(&chain, defaults.max_asset_bytes, |decl| {
            decl.max_asset_bytes
        });
    let (asset_media_types, asset_media_types_provenance) =
        resolve_field(&chain, defaults.asset_media_types, |decl| {
            decl.asset_media_types.clone()
        });
    // `batch` (`#114`) resolves its declared `Option<BatchDecl>` through the
    // same nearest-level-wins, whole-value-replaces walk `colormap` uses
    // just above, then materializes the winning (or absent) declaration
    // into a concrete `BatchConfig` via `BatchDecl::resolve` — see that
    // type's own doc for why an unset field within the winning declaration
    // falls back to this module's default rather than a different level's
    // value for that one field.
    let (batch_decl, batch_provenance) =
        resolve_field(&chain, None, |decl| decl.batch.clone().map(Some));
    let batch = batch_decl.unwrap_or_default().resolve();
    // `protocols` (`#185`) resolves exactly like `page_max_bytes` above:
    // an `Option` value with no built-in default, `.map(Some)` lifting a
    // declared level into the walk, because "nobody declared one" is a real
    // effective outcome (every root served) rather than a gap to fill.
    // Whole-value replacement — a nearer level's block replaces a farther
    // one's outright, never merging key by key.
    let (protocols, protocols_provenance) =
        resolve_field(&chain, defaults.protocols, |decl| decl.protocols.map(Some));

    (
        EffectiveSettings {
            tile_caps,
            cache_ttl_s,
            slow_request_ms,
            stac,
            tile_properties,
            colormap,
            max_request_body_bytes,
            tile_vertex_budget,
            items_vertex_budget,
            page_max_bytes,
            max_asset_bytes,
            asset_media_types,
            batch,
            protocols,
        },
        EffectiveSettingsProvenance {
            tile_caps: tile_caps_provenance,
            cache_ttl_s: cache_ttl_s_provenance,
            slow_request_ms: slow_request_ms_provenance,
            stac: stac_provenance,
            tile_properties: tile_properties_provenance,
            colormap: colormap_provenance,
            max_request_body_bytes: max_request_body_bytes_provenance,
            tile_vertex_budget: tile_vertex_budget_provenance,
            items_vertex_budget: items_vertex_budget_provenance,
            page_max_bytes: page_max_bytes_provenance,
            max_asset_bytes: max_asset_bytes_provenance,
            asset_media_types: asset_media_types_provenance,
            batch: batch_provenance,
            protocols: protocols_provenance,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContactDecl, ProtocolExposure};
    use std::collections::BTreeMap;

    fn caps(pairs: &[(u8, u64)]) -> ZoomCaps {
        ZoomCaps(pairs.iter().copied().collect::<BTreeMap<_, _>>())
    }

    fn settings(tile_caps: Option<ZoomCaps>, cache_ttl_s: Option<u64>) -> SettingsDecl {
        SettingsDecl {
            tile_caps,
            cache_ttl_s,
            ..Default::default()
        }
    }

    fn empty() -> SettingsDecl {
        SettingsDecl::default()
    }

    fn stac(license: &str) -> StacConf {
        StacConf {
            license: Some(license.to_string()),
            ..Default::default()
        }
    }

    fn settings_with_stac(stac: Option<StacConf>) -> SettingsDecl {
        SettingsDecl {
            stac,
            ..Default::default()
        }
    }

    fn settings_with_slow_request_ms(slow_request_ms: Option<u64>) -> SettingsDecl {
        SettingsDecl {
            slow_request_ms,
            ..Default::default()
        }
    }

    fn settings_with_tile_properties(tile_properties: Option<Vec<String>>) -> SettingsDecl {
        SettingsDecl {
            tile_properties,
            ..Default::default()
        }
    }

    fn props(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn collection_level_wins_when_it_sets_the_key() {
        let collection = settings(Some(caps(&[(0, 10)])), Some(5));
        let catalog = settings(Some(caps(&[(0, 99)])), Some(99));
        let tenant = settings(Some(caps(&[(0, 99)])), Some(99));
        let platform = settings(Some(caps(&[(0, 99)])), Some(99));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_caps, caps(&[(0, 10)]));
        assert_eq!(effective.cache_ttl_s, 5);
    }

    #[test]
    fn falls_through_to_catalog_when_collection_says_nothing() {
        let collection = empty();
        let catalog = settings(Some(caps(&[(0, 20)])), Some(20));
        let tenant = settings(Some(caps(&[(0, 99)])), Some(99));
        let platform = settings(Some(caps(&[(0, 99)])), Some(99));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_caps, caps(&[(0, 20)]));
        assert_eq!(effective.cache_ttl_s, 20);
    }

    #[test]
    fn falls_through_to_tenant_when_collection_and_catalog_say_nothing() {
        let collection = empty();
        let catalog = empty();
        let tenant = settings(Some(caps(&[(0, 30)])), Some(30));
        let platform = settings(Some(caps(&[(0, 99)])), Some(99));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_caps, caps(&[(0, 30)]));
        assert_eq!(effective.cache_ttl_s, 30);
    }

    #[test]
    fn falls_through_to_platform_when_only_it_sets_the_key() {
        let collection = empty();
        let catalog = empty();
        let tenant = empty();
        let platform = settings(Some(caps(&[(0, 40)])), Some(40));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_caps, caps(&[(0, 40)]));
        assert_eq!(effective.cache_ttl_s, 40);
    }

    #[test]
    fn falls_back_to_the_module_default_when_nothing_in_the_chain_sets_the_key() {
        let (collection, catalog, tenant, platform) = (empty(), empty(), empty(), empty());
        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective, EffectiveSettings::default());
        assert_eq!(effective.cache_ttl_s, DEFAULT_SETTINGS_CACHE_TTL_S);
    }

    /// Each key resolves independently — a collection can win on one key
    /// while falling through on the other.
    #[test]
    fn keys_resolve_independently_not_as_a_whole_record() {
        let collection = settings(Some(caps(&[(0, 10)])), None);
        let catalog = settings(None, Some(50));
        let tenant = empty();
        let platform = empty();

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(
            effective.tile_caps,
            caps(&[(0, 10)]),
            "collection's own caps win"
        );
        assert_eq!(
            effective.cache_ttl_s, 50,
            "cache_ttl_s falls through to catalog since the collection left it unset"
        );
    }

    /// "Maps replace whole" — a lower level's `tile_caps` never merges
    /// entry-by-entry with a higher level's; the winning level's map is
    /// taken exactly as declared, even if a higher level covered more zooms.
    #[test]
    fn tile_caps_replace_whole_never_merge_across_levels() {
        let collection = settings(Some(caps(&[(5, 1000)])), None);
        let catalog = settings(Some(caps(&[(0, 10), (10, 20)])), None);

        let effective =
            resolve_effective_settings(&collection, &catalog, &empty(), &empty(), &HashMap::new());
        assert_eq!(
            effective.tile_caps,
            caps(&[(5, 1000)]),
            "the collection's single-zoom map must not pick up the catalog's z0/z10 entries"
        );
    }

    #[test]
    fn slow_request_ms_uses_the_nearest_declared_level() {
        let platform = settings_with_slow_request_ms(Some(4_000));
        let tenant = settings_with_slow_request_ms(Some(3_000));
        let catalog = settings_with_slow_request_ms(Some(2_000));
        let collection = settings_with_slow_request_ms(Some(1_000));

        assert_eq!(
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new())
                .slow_request_ms,
            1_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &catalog, &tenant, &platform, &HashMap::new())
                .slow_request_ms,
            2_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &tenant, &platform, &HashMap::new())
                .slow_request_ms,
            3_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new())
                .slow_request_ms,
            4_000
        );
    }

    #[test]
    fn slow_request_ms_defaults_when_no_level_declares_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.slow_request_ms, DEFAULT_SLOW_REQUEST_MS);
    }

    fn settings_with_max_request_body_bytes(max_request_body_bytes: Option<u64>) -> SettingsDecl {
        SettingsDecl {
            max_request_body_bytes,
            ..Default::default()
        }
    }

    #[test]
    fn max_request_body_bytes_uses_the_nearest_declared_level() {
        let platform = settings_with_max_request_body_bytes(Some(4_000));
        let tenant = settings_with_max_request_body_bytes(Some(3_000));
        let catalog = settings_with_max_request_body_bytes(Some(2_000));
        let collection = settings_with_max_request_body_bytes(Some(1_000));

        assert_eq!(
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new())
                .max_request_body_bytes,
            1_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &catalog, &tenant, &platform, &HashMap::new())
                .max_request_body_bytes,
            2_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &tenant, &platform, &HashMap::new())
                .max_request_body_bytes,
            3_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new())
                .max_request_body_bytes,
            4_000
        );
    }

    #[test]
    fn max_request_body_bytes_defaults_when_no_level_declares_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert_eq!(
            effective.max_request_body_bytes,
            DEFAULT_MAX_REQUEST_BODY_BYTES
        );
    }

    fn settings_with_tile_vertex_budget(tile_vertex_budget: Option<u64>) -> SettingsDecl {
        SettingsDecl {
            tile_vertex_budget,
            ..Default::default()
        }
    }

    fn settings_with_max_asset_bytes(max_asset_bytes: Option<u64>) -> SettingsDecl {
        SettingsDecl {
            max_asset_bytes,
            ..Default::default()
        }
    }

    #[test]
    fn tile_vertex_budget_uses_the_nearest_declared_level() {
        let platform = settings_with_tile_vertex_budget(Some(4_000));
        let tenant = settings_with_tile_vertex_budget(Some(3_000));
        let catalog = settings_with_tile_vertex_budget(Some(2_000));
        let collection = settings_with_tile_vertex_budget(Some(1_000));

        assert_eq!(
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new())
                .tile_vertex_budget,
            1_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &catalog, &tenant, &platform, &HashMap::new())
                .tile_vertex_budget,
            2_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &tenant, &platform, &HashMap::new())
                .tile_vertex_budget,
            3_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new())
                .tile_vertex_budget,
            4_000
        );
    }

    #[test]
    fn max_asset_bytes_uses_the_nearest_declared_level() {
        let platform = settings_with_max_asset_bytes(Some(4_000));
        let collection = settings_with_max_asset_bytes(Some(1_000));
        assert_eq!(
            resolve_effective_settings(&collection, &empty(), &empty(), &platform, &HashMap::new())
                .max_asset_bytes,
            1_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new())
                .max_asset_bytes,
            4_000
        );
    }

    #[test]
    fn tile_vertex_budget_defaults_when_no_level_declares_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.tile_vertex_budget, DEFAULT_TILE_VERTEX_BUDGET);
    }

    fn settings_with_items_vertex_budget(items_vertex_budget: Option<u64>) -> SettingsDecl {
        SettingsDecl {
            items_vertex_budget,
            ..Default::default()
        }
    }

    #[test]
    fn items_vertex_budget_uses_the_nearest_declared_level() {
        let platform = settings_with_items_vertex_budget(Some(40_000));
        let tenant = settings_with_items_vertex_budget(Some(30_000));
        let catalog = settings_with_items_vertex_budget(Some(20_000));
        let collection = settings_with_items_vertex_budget(Some(10_000));

        assert_eq!(
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new())
                .items_vertex_budget,
            10_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &catalog, &tenant, &platform, &HashMap::new())
                .items_vertex_budget,
            20_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &tenant, &platform, &HashMap::new())
                .items_vertex_budget,
            30_000
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new())
                .items_vertex_budget,
            40_000
        );
    }

    #[test]
    fn items_vertex_budget_defaults_when_no_level_declares_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.items_vertex_budget, DEFAULT_ITEMS_VERTEX_BUDGET);
    }

    #[test]
    fn items_vertex_budget_reports_profile_provenance() {
        let profile = settings_with_items_vertex_budget(Some(12_345));
        let collection = settings_with_profile("exact-items");
        let profiles = profile_map(&[("exact-items", &profile)]);

        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &collection,
            &empty(),
            &empty(),
            &empty(),
            &profiles,
        );
        assert_eq!(effective.items_vertex_budget, 12_345);
        assert_eq!(
            provenance.items_vertex_budget,
            SettingsProvenance::Profile {
                level: SettingsLevel::Collection,
                profile_id: "exact-items".to_string(),
            }
        );
    }

    fn settings_with_page_max_bytes(page_max_bytes: Option<u64>) -> SettingsDecl {
        SettingsDecl {
            page_max_bytes,
            ..Default::default()
        }
    }

    /// `#184`: same nearest-level-wins walk as `items_vertex_budget` above —
    /// in particular a collection-level `page_max_bytes` wins over a
    /// platform-level one.
    #[test]
    fn page_max_bytes_uses_the_nearest_declared_level() {
        let platform = settings_with_page_max_bytes(Some(4_000_000));
        let tenant = settings_with_page_max_bytes(Some(3_000_000));
        let catalog = settings_with_page_max_bytes(Some(2_000_000));
        let collection = settings_with_page_max_bytes(Some(1_000_000));

        assert_eq!(
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new())
                .page_max_bytes,
            Some(1_000_000)
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &catalog, &tenant, &platform, &HashMap::new())
                .page_max_bytes,
            Some(2_000_000)
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &tenant, &platform, &HashMap::new())
                .page_max_bytes,
            Some(3_000_000)
        );
        assert_eq!(
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new())
                .page_max_bytes,
            Some(4_000_000)
        );
    }

    /// `#184`: no built-in default — an undeclared `page_max_bytes` resolves
    /// to `None` (lane off) with `BuiltInDefault` provenance, never to some
    /// fabricated number.
    #[test]
    fn page_max_bytes_stays_none_when_no_level_declares_it() {
        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &empty(),
            &empty(),
            &empty(),
            &empty(),
            &HashMap::new(),
        );
        assert_eq!(effective.page_max_bytes, None);
        assert_eq!(
            provenance.page_max_bytes,
            SettingsProvenance::BuiltInDefault
        );
    }

    fn settings_with_protocols(protocols: ProtocolsConf) -> SettingsDecl {
        SettingsDecl {
            protocols: Some(protocols),
            ..Default::default()
        }
    }

    /// `#185`: no built-in default — an undeclared `protocols` resolves to
    /// `None` (every root served) with `BuiltInDefault` provenance, the same
    /// `page_max_bytes` precedent, never to a fabricated all-enabled matrix
    /// that would be indistinguishable from an operator writing one down.
    #[test]
    fn protocols_stays_none_when_no_level_declares_it() {
        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &empty(),
            &empty(),
            &empty(),
            &empty(),
            &HashMap::new(),
        );
        assert_eq!(effective.protocols, None);
        assert_eq!(provenance.protocols, SettingsProvenance::BuiltInDefault);
        // ...and every protocol is exposed for that case.
        let matrix = effective.protocols_or_default();
        assert!(matrix.features.is_enabled());
        assert!(matrix.features_write.is_enabled());
        assert!(matrix.tiles.is_enabled());
        assert!(matrix.styles.is_enabled());
        assert!(matrix.three_d_tiles.is_enabled());
        assert!(matrix.stac.is_enabled());
    }

    /// `#185`: nearest level wins and the whole block replaces — a tenant
    /// that disables `tiles` does NOT keep the platform's `stac: disabled`,
    /// exactly like `tile_caps`/`asset_media_types` above.
    #[test]
    fn protocols_replaces_the_whole_matrix_never_merges_across_levels() {
        let platform = settings_with_protocols(ProtocolsConf {
            stac: ProtocolExposure::Disabled,
            ..Default::default()
        });
        let tenant = settings_with_protocols(ProtocolsConf {
            tiles: ProtocolExposure::Disabled,
            ..Default::default()
        });

        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &empty(),
            &empty(),
            &tenant,
            &platform,
            &HashMap::new(),
        );
        let matrix = effective.protocols.expect("the tenant declared a matrix");
        assert_eq!(matrix.tiles, ProtocolExposure::Disabled);
        assert_eq!(matrix.stac, ProtocolExposure::Enabled);
        assert_eq!(
            provenance.protocols,
            SettingsProvenance::Declared {
                level: SettingsLevel::Tenant
            }
        );

        // With the tenant silent, the platform's block shows through whole.
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new());
        let matrix = effective.protocols.expect("the platform declared a matrix");
        assert_eq!(matrix.stac, ProtocolExposure::Disabled);
        assert_eq!(matrix.tiles, ProtocolExposure::Enabled);
    }

    /// `#185`: `features_write` is its own key — turning the write lane off
    /// leaves the `features` root itself exposed.
    #[test]
    fn protocols_carries_the_write_lane_independently_of_the_features_root() {
        let catalog = settings_with_protocols(ProtocolsConf {
            features_write: ProtocolExposure::Disabled,
            ..Default::default()
        });
        let effective =
            resolve_effective_settings(&empty(), &catalog, &empty(), &empty(), &HashMap::new());
        let matrix = effective.protocols_or_default();
        assert!(matrix.features.is_enabled());
        assert!(!matrix.features_write.is_enabled());
    }

    /// `#185` rides the same profile expansion (`#111`) every other key does.
    #[test]
    fn protocols_can_come_from_a_named_profile() {
        let profile = settings_with_protocols(ProtocolsConf {
            styles: ProtocolExposure::Disabled,
            ..Default::default()
        });
        let profiles = HashMap::from([("read-only-ish", &profile)]);
        let tenant = SettingsDecl {
            profile: Some("read-only-ish".to_string()),
            ..Default::default()
        };
        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &empty(),
            &empty(),
            &tenant,
            &empty(),
            &profiles,
        );
        assert_eq!(
            effective.protocols_or_default().styles,
            ProtocolExposure::Disabled
        );
        assert_eq!(
            provenance.protocols,
            SettingsProvenance::Profile {
                level: SettingsLevel::Tenant,
                profile_id: "read-only-ish".to_string(),
            }
        );
    }

    #[test]
    fn max_asset_bytes_defaults_when_no_level_declares_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.max_asset_bytes, DEFAULT_MAX_ASSET_BYTES);
    }

    #[test]
    fn asset_media_types_replaces_the_whole_list_never_merges_across_levels() {
        let collection = SettingsDecl {
            asset_media_types: Some(props(&["image/png"])),
            ..Default::default()
        };
        let catalog = SettingsDecl {
            asset_media_types: Some(props(&["image/png", "image/tiff"])),
            ..Default::default()
        };
        let effective =
            resolve_effective_settings(&collection, &catalog, &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.asset_media_types, props(&["image/png"]));
    }

    #[test]
    fn asset_media_types_is_empty_when_nothing_in_the_chain_sets_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert!(effective.asset_media_types.is_empty());
    }

    // -- `batch:` whitelist inheritance (`#114`) -----------------------------
    //
    // Resolves through the same nearest-level-wins, whole-value-replaces
    // chain as `colormap`/`stac` above, but (unlike either of those) always
    // materializes to a concrete `BatchConfig` via `BatchDecl::resolve`
    // rather than staying `None`.

    fn settings_with_batch(batch: Option<crate::batch::BatchDecl>) -> SettingsDecl {
        SettingsDecl {
            batch,
            ..Default::default()
        }
    }

    #[test]
    fn batch_collection_level_wins_when_it_sets_the_key() {
        let collection = settings_with_batch(Some(crate::batch::BatchDecl {
            max_bytes: Some(1_000),
            ..Default::default()
        }));
        let platform = settings_with_batch(Some(crate::batch::BatchDecl {
            max_bytes: Some(9_999),
            ..Default::default()
        }));

        let effective =
            resolve_effective_settings(&collection, &empty(), &empty(), &platform, &HashMap::new());
        assert_eq!(effective.batch.max_bytes, 1_000);
    }

    #[test]
    fn batch_falls_through_to_platform_when_only_it_sets_the_key() {
        let platform = settings_with_batch(Some(crate::batch::BatchDecl {
            max_items: Some(42),
            ..Default::default()
        }));

        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new());
        assert_eq!(effective.batch.max_items, 42);
    }

    #[test]
    fn batch_defaults_to_the_module_config_when_nothing_in_the_chain_sets_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.batch, crate::batch::BatchConfig::default());
    }

    /// "The whole value replaces" — a declaring level's own unset fields
    /// fall back to `BatchDecl::resolve`'s module defaults, never to a
    /// farther level's value for that one field, even though the farther
    /// level did set it.
    #[test]
    fn batch_never_merges_fields_across_levels() {
        let collection = settings_with_batch(Some(crate::batch::BatchDecl {
            max_bytes: Some(1_000),
            max_items: None,
            chunk_items: None,
        }));
        let platform = settings_with_batch(Some(crate::batch::BatchDecl {
            max_bytes: None,
            max_items: Some(50),
            chunk_items: Some(10),
        }));

        let effective =
            resolve_effective_settings(&collection, &empty(), &empty(), &platform, &HashMap::new());
        assert_eq!(effective.batch.max_bytes, 1_000);
        assert_eq!(
            effective.batch.max_items,
            crate::batch::DEFAULT_BATCH_MAX_ITEMS,
            "the collection's own declaration won outright, so its unset \
             max_items must fall back to this module's default rather than \
             picking up the platform's 50"
        );
        assert_eq!(
            effective.batch.chunk_items,
            crate::batch::DEFAULT_BATCH_CHUNK_ITEMS
        );
    }

    // -- `stac:` whitelist inheritance (`#36`) -------------------------------
    //
    // `stac` resolves through the exact same nearest-level-wins chain as
    // `tile_caps`/`cache_ttl_s` above — these four tests exercise it at each
    // of the four levels the way the tile-caps tests already do, so a future
    // change to the chain's precedence can't silently regress `stac` while
    // the scalar-key tests above stay green.

    #[test]
    fn stac_collection_level_wins_when_it_sets_the_key() {
        let collection = settings_with_stac(Some(stac("collection-license")));
        let catalog = settings_with_stac(Some(stac("catalog-license")));
        let tenant = settings_with_stac(Some(stac("tenant-license")));
        let platform = settings_with_stac(Some(stac("platform-license")));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(
            effective.stac.and_then(|s| s.license),
            Some("collection-license".to_string())
        );
    }

    #[test]
    fn stac_falls_through_to_catalog_when_collection_says_nothing() {
        let collection = empty();
        let catalog = settings_with_stac(Some(stac("catalog-license")));
        let tenant = settings_with_stac(Some(stac("tenant-license")));
        let platform = settings_with_stac(Some(stac("platform-license")));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(
            effective.stac.and_then(|s| s.license),
            Some("catalog-license".to_string())
        );
    }

    #[test]
    fn stac_falls_through_to_tenant_when_collection_and_catalog_say_nothing() {
        let collection = empty();
        let catalog = empty();
        let tenant = settings_with_stac(Some(stac("tenant-license")));
        let platform = settings_with_stac(Some(stac("platform-license")));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(
            effective.stac.and_then(|s| s.license),
            Some("tenant-license".to_string())
        );
    }

    #[test]
    fn stac_falls_through_to_platform_when_only_it_sets_the_key() {
        let collection = empty();
        let catalog = empty();
        let tenant = empty();
        let platform = settings_with_stac(Some(stac("platform-license")));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(
            effective.stac.and_then(|s| s.license),
            Some("platform-license".to_string())
        );
    }

    /// No level in the chain ever declaring a `stac:` subtree resolves to
    /// `None`, not a fabricated default — `stac` has no module-level default
    /// the way `tile_caps`/`cache_ttl_s` do (see `EffectiveSettings::default`).
    #[test]
    fn stac_is_none_when_nothing_in_the_chain_sets_it() {
        let (collection, catalog, tenant, platform) = (empty(), empty(), empty(), empty());
        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.stac, None);
    }

    /// "Whole subtree replaces" — a lower level's `stac:` block never merges
    /// field-by-field with a higher level's; the winning level's block is
    /// taken exactly as declared, even if a higher level set more fields.
    #[test]
    fn stac_replaces_the_whole_subtree_never_merges_across_levels() {
        let collection = settings_with_stac(Some(StacConf {
            license: Some("collection-license".to_string()),
            ..Default::default()
        }));
        let catalog = settings_with_stac(Some(StacConf {
            keywords: vec!["catalog-keyword".to_string()],
            ..Default::default()
        }));

        let effective =
            resolve_effective_settings(&collection, &catalog, &empty(), &empty(), &HashMap::new());
        let resolved = effective.stac.unwrap();
        assert_eq!(resolved.license.as_deref(), Some("collection-license"));
        assert!(
            resolved.keywords.is_empty(),
            "the collection's stac block must not pick up the catalog's keywords"
        );
    }

    /// `#187`: `contacts` rides the same whole-value replacement as every
    /// other `stac:` field — extending `StacConf` rather than adding a
    /// second settings key is precisely what buys this for free, with no
    /// new provenance or finality plumbing to keep in step.
    #[test]
    fn stac_contacts_follow_the_same_whole_subtree_replacement_rule() {
        let contact = ContactDecl {
            name: "Ada Lovelace".to_string(),
            organization: None,
            email: None,
            role: None,
            url: None,
        };
        let catalog = settings_with_stac(Some(StacConf {
            contacts: vec![contact.clone()],
            ..Default::default()
        }));

        // Nothing below the catalog declares `stac:` at all -> inherited.
        let effective =
            resolve_effective_settings(&empty(), &catalog, &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.stac.unwrap().contacts, vec![contact]);

        // A collection that declares its own `stac:` block replaces the
        // catalog's outright, contacts included — silence is a declaration
        // of "no contacts", not an inheritance request.
        let collection = settings_with_stac(Some(StacConf {
            license: Some("CC-BY-4.0".to_string()),
            ..Default::default()
        }));
        let effective =
            resolve_effective_settings(&collection, &catalog, &empty(), &empty(), &HashMap::new());
        assert!(effective.stac.unwrap().contacts.is_empty());
    }

    // -- `tile_properties:` whitelist inheritance (`#85`) --------------------
    //
    // Same nearest-level-wins chain as `tile_caps`/`stac` above — the same
    // four-level sweep, plus the module default and the whole-list-replaces
    // rule.

    #[test]
    fn tile_properties_collection_level_wins_when_it_sets_the_key() {
        let collection = settings_with_tile_properties(Some(props(&["a"])));
        let catalog = settings_with_tile_properties(Some(props(&["b"])));
        let tenant = settings_with_tile_properties(Some(props(&["c"])));
        let platform = settings_with_tile_properties(Some(props(&["d"])));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_properties, props(&["a"]));
    }

    #[test]
    fn tile_properties_falls_through_to_catalog_when_collection_says_nothing() {
        let collection = empty();
        let catalog = settings_with_tile_properties(Some(props(&["b"])));
        let tenant = settings_with_tile_properties(Some(props(&["c"])));
        let platform = settings_with_tile_properties(Some(props(&["d"])));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_properties, props(&["b"]));
    }

    #[test]
    fn tile_properties_falls_through_to_tenant_when_collection_and_catalog_say_nothing() {
        let collection = empty();
        let catalog = empty();
        let tenant = settings_with_tile_properties(Some(props(&["c"])));
        let platform = settings_with_tile_properties(Some(props(&["d"])));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_properties, props(&["c"]));
    }

    #[test]
    fn tile_properties_falls_through_to_platform_when_only_it_sets_the_key() {
        let collection = empty();
        let catalog = empty();
        let tenant = empty();
        let platform = settings_with_tile_properties(Some(props(&["d"])));

        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert_eq!(effective.tile_properties, props(&["d"]));
    }

    /// No level in the chain ever declaring `tile_properties` resolves to
    /// empty — pk-only, the same behavior every collection had before `#85`.
    #[test]
    fn tile_properties_is_empty_when_nothing_in_the_chain_sets_it() {
        let (collection, catalog, tenant, platform) = (empty(), empty(), empty(), empty());
        let effective =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        assert!(effective.tile_properties.is_empty());
    }

    /// "The whole list replaces" — a lower level's `tile_properties` never
    /// merges entry-by-entry with a higher level's; the winning level's list
    /// is taken exactly as declared, even if a higher level named more
    /// columns.
    #[test]
    fn tile_properties_replaces_the_whole_list_never_merges_across_levels() {
        let collection = settings_with_tile_properties(Some(props(&["name"])));
        let catalog = settings_with_tile_properties(Some(props(&["name", "pop", "class"])));

        let effective =
            resolve_effective_settings(&collection, &catalog, &empty(), &empty(), &HashMap::new());
        assert_eq!(
            effective.tile_properties,
            props(&["name"]),
            "the collection's single-entry list must not pick up the catalog's other entries"
        );
    }

    // -- `colormap:` whitelist inheritance (`#92`) ---------------------------
    //
    // Resolves through the exact same nearest-level-wins, whole-value-
    // replaces chain as `stac` above.

    fn ramp(min: f64) -> ColormapConf {
        ColormapConf::Ramp {
            ramp: crate::config::ColorRamp::Grayscale,
            min,
            max: 255.0,
        }
    }

    fn settings_with_colormap(colormap: Option<ColormapConf>) -> SettingsDecl {
        SettingsDecl {
            colormap,
            ..Default::default()
        }
    }

    #[test]
    fn colormap_collection_level_wins_when_it_sets_the_key() {
        let collection = settings_with_colormap(Some(ramp(1.0)));
        let catalog = settings_with_colormap(Some(ramp(2.0)));

        let effective =
            resolve_effective_settings(&collection, &catalog, &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.colormap, Some(ramp(1.0)));
    }

    #[test]
    fn colormap_falls_through_to_platform_when_only_it_sets_the_key() {
        let platform = settings_with_colormap(Some(ramp(3.0)));

        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &platform, &HashMap::new());
        assert_eq!(effective.colormap, Some(ramp(3.0)));
    }

    /// No level in the chain ever declaring a `colormap:` resolves to
    /// `None`, not a fabricated default — same "no module-level default"
    /// shape as `stac`.
    #[test]
    fn colormap_is_none_when_nothing_in_the_chain_sets_it() {
        let effective =
            resolve_effective_settings(&empty(), &empty(), &empty(), &empty(), &HashMap::new());
        assert_eq!(effective.colormap, None);
    }

    // -- provenance (`#110`, `resolve_effective_settings_with_provenance`) --
    //
    // `resolve_effective_settings` is a thin wrapper discarding the second
    // element these tests check — so every assertion here about *values*
    // duplicates a case already covered above, and the point is entirely
    // the provenance half.

    #[test]
    fn provenance_names_the_declaring_level_for_each_of_the_four_levels() {
        let collection = settings(Some(caps(&[(0, 10)])), None);
        let catalog = settings(None, Some(20));
        let tenant = settings_with_slow_request_ms(Some(30));
        let platform = settings_with_max_request_body_bytes(Some(40));

        let (_, provenance) = resolve_effective_settings_with_provenance(
            &collection,
            &catalog,
            &tenant,
            &platform,
            &HashMap::new(),
        );

        assert_eq!(
            provenance.tile_caps,
            SettingsProvenance::Declared {
                level: SettingsLevel::Collection
            }
        );
        assert_eq!(
            provenance.cache_ttl_s,
            SettingsProvenance::Declared {
                level: SettingsLevel::Catalog
            }
        );
        assert_eq!(
            provenance.slow_request_ms,
            SettingsProvenance::Declared {
                level: SettingsLevel::Tenant
            }
        );
        assert_eq!(
            provenance.max_request_body_bytes,
            SettingsProvenance::Declared {
                level: SettingsLevel::Platform
            }
        );
    }

    #[test]
    fn provenance_is_built_in_default_when_nothing_in_the_chain_declares_the_key() {
        let (_, provenance) = resolve_effective_settings_with_provenance(
            &empty(),
            &empty(),
            &empty(),
            &empty(),
            &HashMap::new(),
        );
        assert_eq!(provenance.tile_caps, SettingsProvenance::BuiltInDefault);
        assert_eq!(provenance.cache_ttl_s, SettingsProvenance::BuiltInDefault);
        assert_eq!(
            provenance.slow_request_ms,
            SettingsProvenance::BuiltInDefault
        );
        assert_eq!(provenance.stac, SettingsProvenance::BuiltInDefault);
        assert_eq!(
            provenance.tile_properties,
            SettingsProvenance::BuiltInDefault
        );
        assert_eq!(provenance.colormap, SettingsProvenance::BuiltInDefault);
        assert_eq!(
            provenance.max_request_body_bytes,
            SettingsProvenance::BuiltInDefault
        );
        assert_eq!(
            provenance.tile_vertex_budget,
            SettingsProvenance::BuiltInDefault
        );
        assert_eq!(
            provenance.max_asset_bytes,
            SettingsProvenance::BuiltInDefault
        );
        assert_eq!(
            provenance.asset_media_types,
            SettingsProvenance::BuiltInDefault
        );
        assert_eq!(provenance.batch, SettingsProvenance::BuiltInDefault);
    }

    /// A key set identically at two levels still names the *nearest* one —
    /// provenance follows the same nearest-wins precedence as the value
    /// itself, never the outermost level that happens to agree.
    #[test]
    fn provenance_names_the_nearest_level_even_when_a_farther_level_also_declares_the_key() {
        let collection = empty();
        let catalog = settings_with_slow_request_ms(Some(2_000));
        let tenant = settings_with_slow_request_ms(Some(2_000));
        let platform = settings_with_slow_request_ms(Some(2_000));

        let (_, provenance) = resolve_effective_settings_with_provenance(
            &collection,
            &catalog,
            &tenant,
            &platform,
            &HashMap::new(),
        );
        assert_eq!(
            provenance.slow_request_ms,
            SettingsProvenance::Declared {
                level: SettingsLevel::Catalog
            }
        );
    }

    /// `resolve_effective_settings` must always agree with the value half of
    /// `resolve_effective_settings_with_provenance` — the whole point of the
    /// refactor (`#110`) is that there is exactly one resolution, not two
    /// that could drift apart.
    #[test]
    fn resolve_effective_settings_agrees_with_the_value_half_of_the_provenance_variant() {
        let collection = settings(Some(caps(&[(0, 10)])), Some(5));
        let catalog = settings_with_stac(Some(stac("catalog-license")));
        let tenant = settings_with_tile_properties(Some(props(&["name"])));
        let platform = settings_with_max_asset_bytes(Some(9_999));

        let via_plain =
            resolve_effective_settings(&collection, &catalog, &tenant, &platform, &HashMap::new());
        let (via_provenance, _) = resolve_effective_settings_with_provenance(
            &collection,
            &catalog,
            &tenant,
            &platform,
            &HashMap::new(),
        );
        assert_eq!(via_plain, via_provenance);
    }

    // -- named profile expansion (`#111`) ------------------------------------
    //
    // A profile is just another `SettingsDecl` fragment, referenced by id
    // from `SettingsDecl.profile` at any single level. Expansion happens
    // inside `resolve_field`'s own per-level check, never by pre-merging the
    // profile's keys into the referencing level's `SettingsDecl` — these
    // tests exercise that resolver behavior directly, the same level-by-level
    // shape as every test above, plus the profile lookup map.

    fn settings_with_profile(profile: &str) -> SettingsDecl {
        SettingsDecl {
            profile: Some(profile.to_string()),
            ..Default::default()
        }
    }

    fn profile_map<'a>(
        entries: &[(&'a str, &'a SettingsDecl)],
    ) -> HashMap<&'a str, &'a SettingsDecl> {
        entries.iter().copied().collect()
    }

    #[test]
    fn a_profile_fills_the_gap_when_its_referencing_level_declares_nothing_of_its_own() {
        let heavy_raster = settings(None, Some(99));
        let collection = settings_with_profile("heavy-raster");
        let profiles = profile_map(&[("heavy-raster", &heavy_raster)]);

        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &collection,
            &empty(),
            &empty(),
            &empty(),
            &profiles,
        );
        assert_eq!(effective.cache_ttl_s, 99);
        assert_eq!(
            provenance.cache_ttl_s,
            SettingsProvenance::Profile {
                level: SettingsLevel::Collection,
                profile_id: "heavy-raster".to_string(),
            }
        );
    }

    /// An explicit key at a level always wins over that same level's own
    /// profile — the profile only ever fills a gap, never overrides.
    #[test]
    fn an_explicit_key_beats_its_own_levels_profile() {
        let heavy_raster = settings(None, Some(99));
        let collection = SettingsDecl {
            cache_ttl_s: Some(5),
            profile: Some("heavy-raster".to_string()),
            ..Default::default()
        };
        let profiles = profile_map(&[("heavy-raster", &heavy_raster)]);

        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &collection,
            &empty(),
            &empty(),
            &empty(),
            &profiles,
        );
        assert_eq!(effective.cache_ttl_s, 5);
        assert_eq!(
            provenance.cache_ttl_s,
            SettingsProvenance::Declared {
                level: SettingsLevel::Collection
            }
        );
    }

    /// Nearest-level-wins is unchanged: a collection's own profile still
    /// beats an explicit key declared at the catalog, even though the
    /// collection itself never declares the key directly.
    #[test]
    fn a_collection_level_profile_beats_a_catalog_level_explicit_key() {
        let heavy_raster = settings(None, Some(7));
        let collection = settings_with_profile("heavy-raster");
        let catalog = settings(None, Some(20));
        let profiles = profile_map(&[("heavy-raster", &heavy_raster)]);

        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &collection,
            &catalog,
            &empty(),
            &empty(),
            &profiles,
        );
        assert_eq!(effective.cache_ttl_s, 7);
        assert_eq!(
            provenance.cache_ttl_s,
            SettingsProvenance::Profile {
                level: SettingsLevel::Collection,
                profile_id: "heavy-raster".to_string(),
            }
        );
    }

    /// A profile referenced at a farther level (platform) still falls
    /// through normally when every nearer level (including that level's own
    /// explicit keys) leaves the gap open.
    #[test]
    fn a_platform_level_profile_falls_through_like_any_other_platform_value() {
        let baseline = settings_with_max_asset_bytes(Some(4_096));
        let platform = settings_with_profile("baseline");
        let profiles = profile_map(&[("baseline", &baseline)]);

        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &empty(),
            &empty(),
            &empty(),
            &platform,
            &profiles,
        );
        assert_eq!(effective.max_asset_bytes, 4_096);
        assert_eq!(
            provenance.max_asset_bytes,
            SettingsProvenance::Profile {
                level: SettingsLevel::Platform,
                profile_id: "baseline".to_string(),
            }
        );
    }

    /// A `profile:` reference the caller's `profiles` map doesn't contain
    /// contributes nothing — the walk simply continues as if that level
    /// named no profile at all. `AppConfig::validate` refuses a dangling
    /// profile reference at config load (`config.rs`), so this only ever
    /// matters as this resolver's own defensive behavior, not a state a
    /// validated config can reach.
    #[test]
    fn a_profile_reference_absent_from_the_profiles_map_contributes_nothing() {
        let collection = settings_with_profile("does-not-exist");
        let platform = settings(None, Some(42));

        let effective =
            resolve_effective_settings(&collection, &empty(), &empty(), &platform, &HashMap::new());
        assert_eq!(effective.cache_ttl_s, 42);
    }

    // -- `final_keys` metadata is inert to resolution (`#110`) -------------
    //
    // Enforcement of `SettingsDecl::final_keys` lives entirely in
    // `config::validate_settings_finality` — this resolver never reads the
    // field at all. These tests pin that: a `SettingsDecl` carrying
    // `final_keys` resolves identically to one that doesn't, proving the
    // presence of finality metadata can never silently change what value or
    // provenance a real request lane observes.

    #[test]
    fn final_keys_metadata_does_not_change_which_value_or_provenance_resolves() {
        let platform = SettingsDecl {
            tile_vertex_budget: Some(500_000),
            final_keys: vec!["tile_vertex_budget".to_string()],
            ..Default::default()
        };
        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &empty(),
            &empty(),
            &empty(),
            &platform,
            &HashMap::new(),
        );
        assert_eq!(effective.tile_vertex_budget, 500_000);
        assert_eq!(
            provenance.tile_vertex_budget,
            SettingsProvenance::Declared {
                level: SettingsLevel::Platform
            },
            "declaring a key final must not change which level provenance names"
        );
    }

    /// A key neither the chain's own declarations nor any of their profiles
    /// ever set still falls back to the module default, tagged
    /// `BuiltInDefault` — profiles don't change the bottom of the chain.
    #[test]
    fn built_in_default_stays_below_a_profile_that_never_sets_the_key() {
        let heavy_raster = settings_with_stac(Some(stac("heavy-raster-license")));
        let collection = settings_with_profile("heavy-raster");
        let profiles = profile_map(&[("heavy-raster", &heavy_raster)]);

        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &collection,
            &empty(),
            &empty(),
            &empty(),
            &profiles,
        );
        assert_eq!(effective.cache_ttl_s, DEFAULT_SETTINGS_CACHE_TTL_S);
        assert_eq!(provenance.cache_ttl_s, SettingsProvenance::BuiltInDefault);
        // The profile's own key still resolves, proving the map wiring is
        // exercised and not merely absent for this field.
        assert_eq!(
            effective.stac.and_then(|s| s.license),
            Some("heavy-raster-license".to_string())
        );
    }
}
