//! The control lane's effective-config view (`#110`, read-only slice): a
//! `GET` returning the merged settings for any node in the platform ->
//! tenant -> catalog -> collection chain, every value tagged with
//! provenance. Answers the question an operator today can only answer by
//! reading YAML and replaying the inheritance rules in their head: "what
//! value applies here, and why."
//!
//! ```text
//! GET /config/effective                                                platform node
//! GET /{tenant}/config/effective                                       tenant node
//! GET /{tenant}/config/catalogs/{catalog}/effective                    catalog node
//! GET /{tenant}/config/catalogs/{catalog}/collections/{cid}/effective  collection node
//! GET /config/profiles                                                 named profiles (`#111`)
//! ```
//!
//! One handler serves all four `effective` mounts (`Path<HashMap<String,
//! String>>` captures whichever named segments a given route declares) —
//! the same "one domain, keyed by how deep the path went" shape
//! `tellurion-stac::asset_handlers` already uses for its own collection-
//! vs-item split. `/config/profiles` (`profiles_view`) is a fifth, simpler
//! handler: profiles have no chain of their own to resolve a node against,
//! just a flat enumeration of `AppConfig.profiles`.
//!
//! **Gating.** The platform mount sits at the top level, alongside
//! `/metrics`/`/healthz`/`/readyz` — unauthenticated, unaffected by
//! tenancy, because platform settings are not tenant data. The other three
//! mounts nest under `/{tenant}`, so they inherit `enforce_tenant_auth`
//! (`#17`) exactly like every other tenant-scoped resource on this server —
//! no bespoke gating invented for this endpoint. Settings values are
//! behavior, not secrets (env stays infrastructure-only, per this
//! project's own operational rule), which is why the platform node is safe
//! to leave open the same way `/metrics` already is.
//!
//! **Anti-drift.** Every value/provenance pair here is read from — never
//! re-derived from — `tellurion_core::resolve_effective_settings_with_
//! provenance`: the collection node reads `Router::effective_settings`/
//! `effective_settings_provenance` directly (the exact maps
//! `Router::apply_inherited_settings` overlays onto every decl a request-
//! lane driver receives); the platform/tenant/catalog nodes call the same
//! resolver function this server's `Router::build` calls, just at a
//! shallower query depth (empty `SettingsDecl`s standing in for the levels
//! below the queried node — see that function's own doc). This module only
//! ever *relabels* an already-resolved `SettingsProvenance` into this
//! view's wire vocabulary (`local_override`/`inherited`/`derived`/
//! `built_in_default`) relative to the queried node; it never decides a
//! value or which level supplied it.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use tellurion_core::{
    resolve_effective_settings_with_provenance, AppContext, BatchConfig, ColormapConf,
    ContextState, EffectiveSettings, EffectiveSettingsProvenance, ProtocolsConf, SettingsDecl,
    SettingsLevel, SettingsProvenance, StacConf, ZoomCaps,
};

use crate::app::problem_response;

/// This view's own provenance vocabulary — the issue's exact naming
/// (`built-in default | derived | inherited (naming the level) | local
/// override`), plus `profile` (`#111`, naming both the level and the
/// profile id) for a value a named profile supplied — derived by
/// [`to_wire_provenance`] from a [`SettingsProvenance`] relative to the
/// node being viewed. Never constructed any other way, so it can never
/// disagree with what `SettingsProvenance` actually says.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireProvenance {
    BuiltInDefault,
    Derived,
    Inherited {
        level: WireLevel,
    },
    LocalOverride,
    /// A named profile referenced at `level` supplied this value —
    /// `profile_id` is the one-line "why does this have this value"
    /// answer the issue asks the effective-config view to give. `level`
    /// follows the same local-vs-ancestor convention `Inherited` already
    /// uses: it names whichever level's own `profile:` reference pulled
    /// the value in, which may or may not be the queried node itself.
    Profile {
        level: WireLevel,
        profile_id: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireLevel {
    Platform,
    Tenant,
    Catalog,
    Collection,
}

impl From<SettingsLevel> for WireLevel {
    fn from(level: SettingsLevel) -> Self {
        match level {
            SettingsLevel::Platform => WireLevel::Platform,
            SettingsLevel::Tenant => WireLevel::Tenant,
            SettingsLevel::Catalog => WireLevel::Catalog,
            SettingsLevel::Collection => WireLevel::Collection,
        }
    }
}

/// Relabels `core` relative to `self_level` (the node this view was
/// requested for): the queried node's own declaration is a `local_
/// override`; a strict ancestor's is `inherited`, naming it; a rule outside
/// the settings chain entirely (only `tile_caps`, see `SettingsProvenance::
/// Derived`'s own doc) is `derived`; a named profile's own fragment
/// (`#111`) is `profile`, naming both the referencing level and the
/// profile id; nothing in the chain is `built_in_default`. Exhaustive
/// match, no fallback branch — a level cannot exist without being one of
/// these five.
fn to_wire_provenance(core: SettingsProvenance, self_level: SettingsLevel) -> WireProvenance {
    match core {
        SettingsProvenance::BuiltInDefault => WireProvenance::BuiltInDefault,
        SettingsProvenance::Derived => WireProvenance::Derived,
        SettingsProvenance::Declared { level } if level == self_level => {
            WireProvenance::LocalOverride
        }
        SettingsProvenance::Declared { level } => WireProvenance::Inherited {
            level: level.into(),
        },
        SettingsProvenance::Profile { level, profile_id } => WireProvenance::Profile {
            level: level.into(),
            profile_id,
        },
    }
}

#[derive(Debug, Serialize)]
struct ValueWithProvenance<T: Serialize> {
    value: T,
    provenance: WireProvenance,
}

#[derive(Debug, Serialize)]
struct SettingsView {
    tile_caps: ValueWithProvenance<ZoomCaps>,
    cache_ttl_s: ValueWithProvenance<u64>,
    slow_request_ms: ValueWithProvenance<u64>,
    stac: ValueWithProvenance<Option<StacConf>>,
    tile_properties: ValueWithProvenance<Vec<String>>,
    colormap: ValueWithProvenance<Option<ColormapConf>>,
    max_request_body_bytes: ValueWithProvenance<u64>,
    tile_vertex_budget: ValueWithProvenance<u64>,
    items_vertex_budget: ValueWithProvenance<u64>,
    /// `#184`: the one budget whose effective value is itself optional —
    /// `null` here is a real answer (byte budgeting off), not a missing
    /// field, mirroring `stac`/`colormap` above.
    page_max_bytes: ValueWithProvenance<Option<u64>>,
    max_asset_bytes: ValueWithProvenance<u64>,
    asset_media_types: ValueWithProvenance<Vec<String>>,
    batch: ValueWithProvenance<BatchConfig>,
    /// `#185`: the protocol exposure matrix. `null` is a real answer, the
    /// same way `page_max_bytes` above is — it means no level in this chain
    /// ever declared one, so every protocol root is served; it is not a
    /// missing field, and it is deliberately distinguishable from an
    /// explicit all-`enabled` block an operator wrote down. This is the
    /// endpoint that answers "why is this protocol off here": the value
    /// plus the level that supplied it.
    protocols: ValueWithProvenance<Option<ProtocolsConf>>,
}

/// Zips `effective`/`provenance` (always resolved together — see this
/// module's doc) into the wire shape, relabeling each field's provenance
/// relative to `self_level`.
fn build_settings_view(
    effective: &EffectiveSettings,
    provenance: &EffectiveSettingsProvenance,
    self_level: SettingsLevel,
) -> SettingsView {
    SettingsView {
        tile_caps: ValueWithProvenance {
            value: effective.tile_caps.clone(),
            provenance: to_wire_provenance(provenance.tile_caps.clone(), self_level),
        },
        cache_ttl_s: ValueWithProvenance {
            value: effective.cache_ttl_s,
            provenance: to_wire_provenance(provenance.cache_ttl_s.clone(), self_level),
        },
        slow_request_ms: ValueWithProvenance {
            value: effective.slow_request_ms,
            provenance: to_wire_provenance(provenance.slow_request_ms.clone(), self_level),
        },
        stac: ValueWithProvenance {
            value: effective.stac.clone(),
            provenance: to_wire_provenance(provenance.stac.clone(), self_level),
        },
        tile_properties: ValueWithProvenance {
            value: effective.tile_properties.clone(),
            provenance: to_wire_provenance(provenance.tile_properties.clone(), self_level),
        },
        colormap: ValueWithProvenance {
            value: effective.colormap.clone(),
            provenance: to_wire_provenance(provenance.colormap.clone(), self_level),
        },
        max_request_body_bytes: ValueWithProvenance {
            value: effective.max_request_body_bytes,
            provenance: to_wire_provenance(provenance.max_request_body_bytes.clone(), self_level),
        },
        tile_vertex_budget: ValueWithProvenance {
            value: effective.tile_vertex_budget,
            provenance: to_wire_provenance(provenance.tile_vertex_budget.clone(), self_level),
        },
        items_vertex_budget: ValueWithProvenance {
            value: effective.items_vertex_budget,
            provenance: to_wire_provenance(provenance.items_vertex_budget.clone(), self_level),
        },
        page_max_bytes: ValueWithProvenance {
            value: effective.page_max_bytes,
            provenance: to_wire_provenance(provenance.page_max_bytes.clone(), self_level),
        },
        max_asset_bytes: ValueWithProvenance {
            value: effective.max_asset_bytes,
            provenance: to_wire_provenance(provenance.max_asset_bytes.clone(), self_level),
        },
        asset_media_types: ValueWithProvenance {
            value: effective.asset_media_types.clone(),
            provenance: to_wire_provenance(provenance.asset_media_types.clone(), self_level),
        },
        batch: ValueWithProvenance {
            value: effective.batch,
            provenance: to_wire_provenance(provenance.batch.clone(), self_level),
        },
        protocols: ValueWithProvenance {
            value: effective.protocols,
            provenance: to_wire_provenance(provenance.protocols.clone(), self_level),
        },
    }
}

#[derive(Debug, Serialize)]
struct NodeRef {
    level: WireLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    collection: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EffectiveConfigView {
    node: NodeRef,
    settings: SettingsView,
}

pub(crate) fn platform_effective_config_view(state: &ContextState) -> EffectiveConfigView {
    let profiles_by_id: HashMap<&str, &SettingsDecl> = state
        .config
        .profiles
        .iter()
        .map(|profile| (profile.id.as_str(), &profile.settings))
        .collect();
    let (effective, provenance) = resolve_effective_settings_with_provenance(
        &SettingsDecl::default(),
        &SettingsDecl::default(),
        &SettingsDecl::default(),
        &state.config.settings,
        &profiles_by_id,
    );
    EffectiveConfigView {
        node: NodeRef {
            level: WireLevel::Platform,
            tenant: None,
            catalog: None,
            collection: None,
        },
        settings: build_settings_view(&effective, &provenance, SettingsLevel::Platform),
    }
}

fn ok_json(view: EffectiveConfigView) -> Response {
    (StatusCode::OK, Json(view)).into_response()
}

/// `GET .../config/effective` — one handler for all four node depths (see
/// this module's own doc). `params` holds only whichever of `tenant`/
/// `catalog`/`collection` the matched route actually captures; deeper
/// segments are trusted to imply the shallower ones already resolved
/// (a route can never capture `collection` without `tenant`/`catalog`
/// alongside it).
pub async fn effective_config_view(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
) -> Response {
    let state = ctx.current();
    let Some(tenant_ext) = params.get("tenant") else {
        return ok_json(platform_effective_config_view(&state));
    };

    // Named profiles (`#111`) — looked up once per request here, the same
    // lookup `Router::build_from_snapshot` performs once per reload; the
    // tenant/catalog nodes below resolve directly through it, while the
    // collection node reads `Router`'s own already-expanded maps
    // (see this module's own "anti-drift" doc).
    let profiles_by_id: HashMap<&str, &SettingsDecl> = state
        .config
        .profiles
        .iter()
        .map(|profile| (profile.id.as_str(), &profile.settings))
        .collect();

    let Ok(tenant_id) = state.resolver.resolve_tenant(tenant_ext).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(tenant_decl) = state.tenants.iter().find(|t| t.id == tenant_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Some(catalog_ext) = params.get("catalog") else {
        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &SettingsDecl::default(),
            &SettingsDecl::default(),
            &tenant_decl.settings,
            &state.config.settings,
            &profiles_by_id,
        );
        return ok_json(EffectiveConfigView {
            node: NodeRef {
                level: WireLevel::Tenant,
                tenant: Some(tenant_ext.clone()),
                catalog: None,
                collection: None,
            },
            settings: build_settings_view(&effective, &provenance, SettingsLevel::Tenant),
        });
    };

    let catalog_decl = match state.registry.catalog(&tenant_id, catalog_ext).await {
        Ok(Some(decl)) => decl,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(%error, tenant = tenant_ext.as_str(), catalog = catalog_ext.as_str(), "effective-config view: failed to read catalog");
            return problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "failed to read the catalog declaration",
            );
        }
    };

    let Some(collection_ext) = params.get("collection") else {
        let (effective, provenance) = resolve_effective_settings_with_provenance(
            &SettingsDecl::default(),
            &catalog_decl.settings,
            &tenant_decl.settings,
            &state.config.settings,
            &profiles_by_id,
        );
        return ok_json(EffectiveConfigView {
            node: NodeRef {
                level: WireLevel::Catalog,
                tenant: Some(tenant_ext.clone()),
                catalog: Some(catalog_ext.clone()),
                collection: None,
            },
            settings: build_settings_view(&effective, &provenance, SettingsLevel::Catalog),
        });
    };

    let collection_decl = match state
        .registry
        .collection(&catalog_decl.id, collection_ext)
        .await
    {
        Ok(Some(decl)) => decl,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(%error, tenant = tenant_ext.as_str(), catalog = catalog_ext.as_str(), collection = collection_ext.as_str(), "effective-config view: failed to read collection");
            return problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "failed to read the collection declaration",
            );
        }
    };

    // Read straight off `Router`'s own materialized maps — the exact
    // values/provenance `Router::apply_inherited_settings` already overlays
    // onto every decl a request-lane driver receives (see this module's own
    // doc). `None` here means the registry knows this collection but the
    // currently-served `Router` snapshot does not — only possible under the
    // relational backend, in the window between a registry write and the
    // next debounced reload (`#39`'s reload pipeline); reporting that
    // honestly as "not currently routed" is this read-only slice's whole
    // answer to that window — a measured staleness bound is `#110`'s
    // change-propagation slice, not this one.
    let (Some(effective), Some(provenance)) = (
        state.router.effective_settings(&collection_decl.id),
        state
            .router
            .effective_settings_provenance(&collection_decl.id),
    ) else {
        return problem_response(
            StatusCode::NOT_FOUND,
            "NotCurrentlyRouted",
            "this collection is registered but not part of the currently served routing snapshot",
        );
    };

    ok_json(EffectiveConfigView {
        node: NodeRef {
            level: WireLevel::Collection,
            tenant: Some(tenant_ext.clone()),
            catalog: Some(catalog_ext.clone()),
            collection: Some(collection_ext.clone()),
        },
        settings: build_settings_view(effective, provenance, SettingsLevel::Collection),
    })
}

/// One `profiles:` entry (`#111`) as this read-only view reports it: its id
/// and its raw declared fragment — the same `SettingsDecl` shape a
/// `ProfileDecl` carries, `Option`s and all, not a resolved
/// `EffectiveSettings`. A profile has no chain to walk on its own, so there
/// is no provenance to attach here; `profile:<id>` provenance only shows up
/// on the *consuming* node's own effective-config view (`build_settings_view`
/// above), once a chain actually expands this fragment somewhere.
#[derive(Debug, Serialize)]
struct ProfileSummary {
    id: String,
    settings: SettingsDecl,
}

#[derive(Debug, Serialize)]
struct ProfilesView {
    profiles: Vec<ProfileSummary>,
}

/// `GET /config/profiles` (`#111`) — every named profile this deployment
/// declares, id and contents, read straight off `AppConfig.profiles`. No
/// tenant scoping, no auth: profiles are plain config data, the same
/// "settings values are behavior, not secrets" reasoning the platform
/// `/config/effective` mount above already documents, and mounted the same
/// way — top level, alongside it.
pub async fn profiles_view(State(ctx): State<Arc<AppContext>>) -> Response {
    let state = ctx.current();
    let profiles = state
        .config
        .profiles
        .iter()
        .map(|profile| ProfileSummary {
            id: profile.id.clone(),
            settings: profile.settings.clone(),
        })
        .collect();
    (StatusCode::OK, Json(ProfilesView { profiles })).into_response()
}
