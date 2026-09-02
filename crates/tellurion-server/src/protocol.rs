//! The seven protocol families this server mounts one full API root per
//! `(tenant, catalog)` for (`#39`): features, tiles, styles, 3dtiles, stac
//! (`#36`), records (`#192`), processes (`#182`). Shared by `app.rs` (route assembly — each root is layered with
//! `Extension(Protocol)`), `landing.rs` (per-root landing page +
//! `/conformance`), and `openapi.rs` (per-root `/api` document), so the URL
//! segment name is declared exactly once.
//!
//! Also the one place a protocol is mapped onto its `settings.protocols`
//! exposure key (`#185`, [`Protocol::exposure`]), for the same reason: the
//! key an operator writes to turn a root off is that root's own URL segment,
//! and it should be spelled in exactly one file.

use tellurion_core::{CollectionKind, ProtocolExposure, ProtocolsConf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Features,
    Tiles,
    Styles,
    ThreeDTiles,
    Stac,
    /// The OGC API — Records root (`#192`).
    Records,
    /// The OGC API — Processes root (`#182`).
    Processes,
}

impl Protocol {
    /// The literal path segment under `/{tenant}/...` — matches
    /// `app::build`'s `.nest(...)` calls exactly.
    pub fn segment(self) -> &'static str {
        match self {
            Protocol::Features => "features",
            Protocol::Tiles => "tiles",
            Protocol::Styles => "styles",
            Protocol::ThreeDTiles => "3dtiles",
            Protocol::Stac => "stac",
            Protocol::Records => "records",
            Protocol::Processes => "processes",
        }
    }

    /// Human-readable name for the landing page's `title`.
    pub fn title(self) -> &'static str {
        match self {
            Protocol::Features => "OGC API Features",
            Protocol::Tiles => "OGC API Tiles",
            Protocol::Styles => "OGC API Styles",
            Protocol::ThreeDTiles => "3D Tiles",
            Protocol::Stac => "STAC API",
            Protocol::Records => "OGC API Records",
            Protocol::Processes => "OGC API Processes",
        }
    }

    /// This protocol root's own entry in a resolved exposure matrix
    /// (`#185`, `settings.protocols`). The one place a `Protocol` variant is
    /// mapped onto a `ProtocolsConf` field — exhaustive, so a protocol added
    /// to this enum cannot silently default to "exposed" without an operator
    /// ever having a key to turn it off with.
    ///
    /// `features_write` deliberately has no arm here: it is not a root of
    /// its own (no prefix disappears when it is off), it narrows the method
    /// set of paths the `features` root already serves — see
    /// `ProtocolsConf::features_write`'s own doc and
    /// `app::enforce_protocol_exposure`.
    pub fn exposure(self, protocols: &ProtocolsConf) -> ProtocolExposure {
        match self {
            Protocol::Features => protocols.features,
            Protocol::Tiles => protocols.tiles,
            Protocol::Styles => protocols.styles,
            Protocol::ThreeDTiles => protocols.three_d_tiles,
            Protocol::Stac => protocols.stac,
            Protocol::Records => protocols.records,
            Protocol::Processes => protocols.processes,
        }
    }

    /// Whether this root serves a collection of `kind` (`#192`).
    ///
    /// The one place the per-protocol collection partition is written down.
    /// Each root filters its own `/collections` listing by this same
    /// predicate, and `app::enforce_collection_kind` applies it to every
    /// request that names a collection, so a listing and a direct fetch can
    /// never disagree about whether a collection belongs to a root.
    ///
    /// - **Features, Tiles, Styles, 3D Tiles** serve geometry. A record
    ///   collection has none, so they skip it — for Tiles and 3D Tiles there
    ///   is literally nothing to render, and for Features an item with no
    ///   geometry is not a feature.
    /// - **Records** serves record collections and only those, which is what
    ///   makes OGC API — Records — Part 1: Core Requirement 37
    ///   (`/req/records-api/catalogs-response`: "only collections where the
    ///   `itemType` property ... is a string with the value `record` SHALL be
    ///   considered to be catalogs") true of this root rather than merely
    ///   asserted by it.
    /// - **STAC** serves every kind. A STAC Collection describes metadata
    ///   about a thing; whether that thing has geometry of its own does not
    ///   decide whether it can be described. Excluding record collections
    ///   here would break `#50`'s "no second catalog" principle in the other
    ///   direction — harvested, geometry-less metadata (`#191`) would become
    ///   invisible to the one surface built to describe it.
    pub fn serves_kind(self, kind: CollectionKind) -> bool {
        match self {
            Protocol::Features | Protocol::Tiles | Protocol::Styles | Protocol::ThreeDTiles => {
                kind.has_geometry()
            }
            Protocol::Records => kind.is_record(),
            Protocol::Stac => true,
            // `#182`: the Processes root serves no collection resources at
            // all — its paths are `/processes` and `/jobs`, and a job belongs
            // to a process and a catalog, never to a collection. `false` for
            // every kind is therefore the honest answer rather than a
            // placeholder: there is no `/collections/{cid}` path under this
            // root for `app::enforce_collection_kind` to ever match, so this
            // arm is never consulted at request time. It exists so that
            // adding a collection-scoped resource to this root later cannot
            // happen without someone revisiting this decision.
            Protocol::Processes => false,
        }
    }

    /// Every protocol this server mounts, in a fixed order — the tenant
    /// directory doc (`landing::tenant_directory`) walks this to list every
    /// `(protocol, catalog)` combination under a tenant.
    pub const ALL: [Protocol; 7] = [
        Protocol::Features,
        Protocol::Tiles,
        Protocol::Styles,
        Protocol::ThreeDTiles,
        Protocol::Stac,
        Protocol::Records,
        Protocol::Processes,
    ];
}

/// Which roots this deployment can serve **at all**, decided once at boot from
/// capabilities rather than per catalog from settings (`#182`).
///
/// Distinct from [`ProtocolsConf`] on purpose, because the two answer different
/// questions and an operator can only change one of them. `protocols.processes:
/// enabled` says "this catalog would like the Processes root"; this says
/// "this deployment has a durable job ledger and a runner to execute with". A
/// root needs BOTH, and when the capability is missing no setting can conjure
/// it — which is why this is not simply folded into the exposure matrix.
///
/// The one place that fact is written down, so the gate that 404s the root
/// (`app::processes_root`) and the tenant directory that decides whether to
/// link it (`landing::tenant_directory`) cannot disagree — a directory
/// advertising a prefix that answers `404` is exactly the dead link `#185`'s
/// own exposure filter exists to avoid.
#[derive(Debug, Clone, Copy)]
pub struct RootAvailability {
    /// `#182`: whether `process_lane::build` produced a lane.
    pub processes: bool,
}

impl RootAvailability {
    /// Whether `protocol`'s capability precondition is met. Every root without
    /// one is always available — its topology has never depended on anything
    /// but the exposure matrix, and must not start to.
    pub fn serves(self, protocol: Protocol) -> bool {
        match protocol {
            Protocol::Processes => self.processes,
            Protocol::Features
            | Protocol::Tiles
            | Protocol::Styles
            | Protocol::ThreeDTiles
            | Protocol::Stac
            | Protocol::Records => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capability gate applies to exactly one root and to no other.
    /// Enumerated rather than derived, the same way the collection-kind
    /// partition above is: a root that silently gained a precondition would
    /// disappear from every deployment lacking it, and the only way that can
    /// happen is if somebody edits this table on purpose.
    #[test]
    fn only_the_processes_root_has_a_capability_precondition() {
        let absent = RootAvailability { processes: false };
        let present = RootAvailability { processes: true };
        let expected_without_capability = [
            (Protocol::Features, true),
            (Protocol::Tiles, true),
            (Protocol::Styles, true),
            (Protocol::ThreeDTiles, true),
            (Protocol::Stac, true),
            (Protocol::Records, true),
            (Protocol::Processes, false),
        ];
        assert_eq!(expected_without_capability.len(), Protocol::ALL.len());
        for (protocol, available) in expected_without_capability {
            assert_eq!(
                absent.serves(protocol),
                available,
                "{protocol:?} availability with no processes capability"
            );
            assert!(
                present.serves(protocol),
                "{protocol:?} must be available once the capability is present"
            );
        }
    }

    /// The partition, stated once and checked exhaustively: every
    /// `(protocol, kind)` pair, so a future variant of either cannot be added
    /// without a deliberate decision here.
    #[test]
    fn every_protocol_serves_exactly_the_kinds_it_should() {
        use CollectionKind::{Raster, Record, Vector};
        let expected = [
            (Protocol::Features, Vector, true),
            (Protocol::Features, Raster, true),
            (Protocol::Features, Record, false),
            (Protocol::Tiles, Vector, true),
            (Protocol::Tiles, Raster, true),
            (Protocol::Tiles, Record, false),
            (Protocol::Styles, Vector, true),
            (Protocol::Styles, Raster, true),
            (Protocol::Styles, Record, false),
            (Protocol::ThreeDTiles, Vector, true),
            (Protocol::ThreeDTiles, Raster, true),
            (Protocol::ThreeDTiles, Record, false),
            (Protocol::Records, Vector, false),
            (Protocol::Records, Raster, false),
            (Protocol::Records, Record, true),
            // `#50`'s "no second catalog" principle: STAC describes
            // metadata about a thing, and whether that thing has geometry
            // of its own does not decide whether it can be described.
            (Protocol::Stac, Vector, true),
            (Protocol::Stac, Raster, true),
            (Protocol::Stac, Record, true),
            // `#182`: the Processes root has no collection resources — see
            // `serves_kind`'s own arm for why `false` here is a decision
            // rather than a placeholder.
            (Protocol::Processes, Vector, false),
            (Protocol::Processes, Raster, false),
            (Protocol::Processes, Record, false),
        ];
        for (protocol, kind, serves) in expected {
            assert_eq!(
                protocol.serves_kind(kind),
                serves,
                "{protocol:?} should {} serve {kind:?}",
                if serves { "" } else { "not" }
            );
        }
    }

    /// Every kind is served by at least one root — a kind nothing serves
    /// would be data an operator can declare and then never reach.
    #[test]
    fn no_kind_is_orphaned_by_the_partition() {
        for kind in [
            CollectionKind::Vector,
            CollectionKind::Raster,
            CollectionKind::Record,
        ] {
            assert!(
                Protocol::ALL.iter().any(|p| p.serves_kind(kind)),
                "{kind:?} is served by no protocol root at all"
            );
        }
    }

    /// Every root has a distinct URL segment and a distinct exposure key —
    /// a duplicate of either would silently make one root shadow another.
    #[test]
    fn every_protocol_has_its_own_url_segment() {
        let mut segments: Vec<&str> = Protocol::ALL.iter().map(|p| p.segment()).collect();
        segments.sort_unstable();
        let count = segments.len();
        segments.dedup();
        assert_eq!(segments.len(), count, "duplicate protocol URL segment");
    }

    /// `#192`/`#182`: the Records and Processes roots are the variants whose
    /// default exposure is `disabled` — every root that predates `#185` stays
    /// `enabled`, because for those `enabled` IS "what this deployment already
    /// did". Asserted here as well as in `tellurion_core::config` so the
    /// mapping from variant to key — the thing this module owns — is covered
    /// too, not just the default matrix itself.
    #[test]
    fn only_the_opt_in_roots_are_disabled_in_the_default_exposure_matrix() {
        let matrix = ProtocolsConf::default();
        for protocol in Protocol::ALL {
            let enabled = protocol.exposure(&matrix).is_enabled();
            let opt_in = matches!(protocol, Protocol::Records | Protocol::Processes);
            assert_eq!(enabled, !opt_in, "{protocol:?} default exposure");
        }
    }

    /// Every root maps onto a DISTINCT exposure key. Written as a mutation
    /// check rather than by naming the fields, so a new variant that
    /// accidentally reuses an existing root's key — which would make one
    /// operator switch silently turn two roots off — fails here.
    #[test]
    fn every_protocol_has_its_own_exposure_key() {
        for protocol in Protocol::ALL {
            // Start from every root ON — including the two that default off —
            // then turn this one off, and check exactly one root observes the
            // change.
            let mut matrix = ProtocolsConf {
                records: ProtocolExposure::Enabled,
                processes: ProtocolExposure::Enabled,
                ..ProtocolsConf::default()
            };
            let before: Vec<bool> = Protocol::ALL
                .iter()
                .map(|p| p.exposure(&matrix).is_enabled())
                .collect();
            match protocol {
                Protocol::Features => matrix.features = ProtocolExposure::Disabled,
                Protocol::Tiles => matrix.tiles = ProtocolExposure::Disabled,
                Protocol::Styles => matrix.styles = ProtocolExposure::Disabled,
                Protocol::ThreeDTiles => matrix.three_d_tiles = ProtocolExposure::Disabled,
                Protocol::Stac => matrix.stac = ProtocolExposure::Disabled,
                Protocol::Records => matrix.records = ProtocolExposure::Disabled,
                Protocol::Processes => matrix.processes = ProtocolExposure::Disabled,
            }
            let after: Vec<bool> = Protocol::ALL
                .iter()
                .map(|p| p.exposure(&matrix).is_enabled())
                .collect();
            let changed = before
                .iter()
                .zip(&after)
                .filter(|(before, after)| before != after)
                .count();
            assert_eq!(changed, 1, "{protocol:?} does not own a key of its own");
        }
    }
}
