//! Coordinate reference systems for OGC API Features Part 2 (CRS by
//! Reference, OGC 18-058r1): the CRS identifiers this workspace advertises
//! and accepts, and the axis-order rule that makes the `crs`/`bbox-crs` query
//! parameters and the `Content-Crs` response header behave correctly.
//!
//! ## Supported CRS set
//!
//! Every collection supports exactly two CRSs (`supported_crs`): CRS84 (the
//! OGC API Features Part 1 default, always) and — when a driver's catalog
//! introspection reports one — this collection's own storage SRID, built into
//! a URI by [`epsg_uri`]. No third CRS is ever offered: there is no config
//! surface for declaring additional ones, matching this lane's "no new config
//! surface" design constraint. `resolve` is the single seam every CRS-aware
//! query parameter (`crs`, `bbox-crs`) is validated through, so a collection
//! can never be handed a CRS its own descriptor didn't advertise.
//!
//! `supported_crs` itself only asks whether a storage SRID is *known*, never
//! whether the driver reporting it can reproject into it — most drivers
//! can't (`FeatureSource::crs_capable` defaults to `false`; PostGIS is the
//! only override). A metadata endpoint advertising a collection's `crs` list
//! (Requirement 2, `/req/crs/fc-md-crs-list`) must use [`advertised_crs`]
//! instead, which folds that capability in — and one advertising `storageCrs`
//! (Requirement 4) must use [`advertised_storage_crs`], which keeps that
//! single value inside the very list `advertised_crs` published.
//!
//! ## One rule behind all three: [`can_serve`]
//!
//! [`can_serve`] answers "can this driver actually put out coordinates in
//! the CRS this request resolved to, for a collection stored at this SRID?"
//! Every other capability-aware function here is that one rule read from a
//! different side, so the three can never disagree (`#227`):
//!
//! - [`advertised_crs`] is [`supported_crs`] filtered by it — what a
//!   collection publishes is exactly what it will accept.
//! - [`advertised_storage_crs`] is Requirement 4's membership test against
//!   that filtered list.
//! - [`content_crs_uri`] names the CRS the bytes are genuinely in, which is
//!   the requested one precisely when `can_serve` said yes.
//!
//! What makes this more than bookkeeping is that a driver which cannot
//! reproject serves *its storage CRS*, not CRS84. Before `#227` the
//! `Content-Crs` header said CRS84 unconditionally whenever the request had
//! not explicitly asked for the storage CRS, so a collection stored in
//! EPSG:3857 answered with metres under a header naming degrees — the one
//! thing Part 2 gives a client to trust, saying the opposite of the truth.
//! Now such a collection advertises, and stamps, `.../EPSG/0/3857`; a client
//! that genuinely needs CRS84 asks for it and is refused by name (a 400)
//! instead of quietly plotting metres as degrees. A CRS84-equivalent storage
//! (SRID 4326, or none reported at all) is untouched in every one of these
//! functions — the overwhelmingly common deployment never moves a byte.
//!
//! ## Axis order (the classic Part 2 trap)
//!
//! `CRS84_URI` (OGC's own alias for 2D WGS 84) is defined longitude-before-
//! latitude — the order GeoJSON's own `coordinates` arrays already use, and
//! the order every response in this workspace produced before Part 2 CRS
//! support existed. `EPSG:4326` referenced *by authority* (the
//! `epsg_uri(4326)` URI) is defined latitude-before-longitude — datum-
//! identical to CRS84, opposite axis order. Since PostGIS/GeoJSON always
//! serialize a geometry's raw X,Y pair (never authority axis order), honoring
//! a request for the storage CRS when that storage SRID happens to be 4326
//! means literally swapping the two coordinate values, not reprojecting
//! anything — see `tellurion-postgis::sql`'s `ST_FlipCoordinates` usage, and
//! [`swap_bbox_axes`] for the equivalent on bbox input. This module only
//! recognizes SRID 4326 itself as latitude-before-longitude; a general
//! per-SRID axis-order table would need a full CRS registry (PROJ), which is
//! out of scope here — every CRS this crate's `supported_crs` can ever
//! advertise besides CRS84 is a single collection's own storage SRID, so this
//! narrow rule already covers every case this server can actually produce.

use crate::error::{Error, Result};

/// CRS84 (OGC API Features Part 1's default spatial-extent/geometry CRS):
/// 2D WGS 84, longitude before latitude.
pub const CRS84_URI: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";

/// OGC API — Features Part 2: CRS by Reference (18-058r1, Approved 1.0.1),
/// the standard's single conformance class. Honest only where a driver can
/// actually honour a non-default `crs`/`bbox-crs`
/// ([`FeatureSource::crs_capable`](crate::storage::FeatureSource::crs_capable)):
/// a driver that never reprojects offers exactly one CRS, the Part 1 default,
/// which is what Part 1 already required — so `crate::router::Router::
/// crs_conformance_classes` folds this class per deployment instead of any
/// static list naming it (`#217`).
pub const CRS_CONFORMANCE_CLASS: &str =
    "http://www.opengis.net/spec/ogcapi-features-2/1.0/conf/crs";

/// The seed [`crate::router::Router::crs_conformance_classes`] folds — Part 2
/// defines exactly one class, so this is [`CRS_CONFORMANCE_CLASS`] alone,
/// spelled as a slice for the same reason `filter::CQL2_CONFORMANCE_CLASSES`
/// and `locking::LOCKING_CONFORMANCE_CLASSES` are.
pub const CRS_CONFORMANCE_CLASSES: &[&str] = &[CRS_CONFORMANCE_CLASS];

/// EPSG:4326 referenced by authority — the one SRID this module treats as
/// latitude-before-longitude. See the module doc's "Axis order" section.
const EPSG_4326_SRID: i32 = 4326;

/// Which CRS a `crs`/`bbox-crs` query parameter resolved to, once validated
/// against a collection's [`supported_crs`] (`tellurion-features`' handler is
/// the only caller of [`resolve`], before either parameter ever reaches a
/// `FeatureSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestedCrs {
    /// No `crs`/`bbox-crs` query parameter was supplied at all. Every SQL
    /// builder in `tellurion-postgis::sql` must treat this exactly like the
    /// CRS handling in this module never existed — no `ST_Transform`, no
    /// `ST_FlipCoordinates`, regardless of a collection's actual storage
    /// SRID — so a request that never asked for Part 2 CRS behavior gets
    /// byte-for-byte the same output this crate always produced.
    #[default]
    Omitted,
    /// The parameter explicitly named [`CRS84_URI`].
    Crs84,
    /// The parameter explicitly named this collection's own storage CRS
    /// (`supported_crs`'s second entry, only reachable when a storage SRID
    /// is known).
    Storage,
}

/// The EPSG "by reference" URI style Part 2's own worked examples use
/// throughout (e.g. `http://www.opengis.net/def/crs/EPSG/0/4326` for
/// SRID 4326) — the `0` version segment marks an unversioned CRS reference,
/// per OGC 18-058r1's own Recommendation 1.
pub fn epsg_uri(srid: i32) -> String {
    format!("http://www.opengis.net/def/crs/EPSG/0/{srid}")
}

/// The full list of CRS identifiers a collection advertises (Requirement 2,
/// `/req/crs/fc-md-crs-list`): [`CRS84_URI`] alone when `storage_srid` is
/// unknown, else `CRS84_URI` plus [`epsg_uri`]`(storage_srid)` — deduplicated
/// on the (currently unreachable, since no SRID this crate derives ever
/// literally equals the CRS84 URI string) chance the two coincide.
pub fn supported_crs(storage_srid: Option<i32>) -> Vec<String> {
    match storage_srid {
        Some(srid) => {
            let storage_uri = epsg_uri(srid);
            if storage_uri == CRS84_URI {
                vec![CRS84_URI.to_string()]
            } else {
                vec![CRS84_URI.to_string(), storage_uri]
            }
        }
        None => vec![CRS84_URI.to_string()],
    }
}

/// Whether a driver can actually put out a response in `resolved`'s CRS for
/// a collection stored at `storage_srid` — the single rule the collection's
/// `crs` list, its `storageCrs`, the handler's 400 gate and the
/// `Content-Crs` header are all read off (`#227`; see this module's own doc).
///
/// A `crs_capable` driver (PostGIS alone today) can do the work in every
/// case, so it answers `true` throughout. For every other driver the answer
/// is "yes, when honouring the request is a no-op":
///
/// - [`RequestedCrs::Omitted`] — nothing was requested, so there is nothing
///   to refuse. Every driver serves *something*; [`content_crs_uri`] is what
///   says truthfully which CRS that is.
/// - [`RequestedCrs::Crs84`] — a no-op exactly when the storage is already
///   CRS84-equivalent (SRID 4326, or unknown). Against a projected storage
///   it is a real coordinate transform, the same one
///   [`crs84_literals_need_transform`] already decides for a filter's
///   spatial literals: that predicate asks a question about the two *CRSs*,
///   not about literals, and this is the identical question about the same
///   pair. A driver that cannot perform it must refuse **by name** — serving
///   the request anyway means metres under a header claiming degrees, which
///   is the defect `#227` closed.
/// - [`RequestedCrs::Storage`] — a no-op for every storage SRID except 4326,
///   because a driver that never reprojects already emits its rows in the
///   storage CRS, unchanged. SRID 4326 is the exception and the classic Part
///   2 trap: EPSG:4326 by authority is latitude-before-longitude
///   ([`is_lat_lon_order`]), so honouring it means swapping every coordinate
///   pair — real work, and refused by a driver that cannot do it.
pub fn can_serve(resolved: RequestedCrs, storage_srid: Option<i32>, crs_capable: bool) -> bool {
    if crs_capable {
        return true;
    }
    match resolved {
        RequestedCrs::Omitted => true,
        RequestedCrs::Crs84 => !crs84_literals_need_transform(storage_srid),
        RequestedCrs::Storage => !storage_srid.is_some_and(is_lat_lon_order),
    }
}

/// The full list of CRS identifiers a collection advertises, capability-aware
/// (Requirement 2, `/req/crs/fc-md-crs-list`): [`supported_crs`] keeping only
/// the identifiers [`can_serve`] says this driver can genuinely put out.
/// `supported_crs` itself only ever looks at whether a storage SRID is
/// *known*, never at whether the driver reporting it can actually serve it —
/// every driver except PostGIS answers `false` from
/// `FeatureSource::crs_capable` while still reporting a real SRID (GeoParquet
/// defaults to `4326` per its own spec), so a caller that fed
/// `supported_crs`'s output straight into a Collection's `crs` metadata was
/// advertising a CRS the enforcement gate would then refuse with a 400. This
/// function is the single seam that keeps the two in sync — expressed
/// literally as "resolve each candidate the way a request would be resolved,
/// then ask the very gate the handler asks" — so whatever it advertises,
/// `resolve` plus the handler's `can_serve` check downstream is guaranteed to
/// accept, by construction rather than by two lists agreeing.
///
/// Three shapes come out of it:
///
/// - `crs_capable` (PostGIS): [`supported_crs`] entire, unchanged.
/// - A CRS84-equivalent storage (SRID 4326, or unknown) under any other
///   driver: [`CRS84_URI`] alone, unchanged — the storage URI drops out
///   because honouring it needs the axis swap `#217` already withheld it
///   for.
/// - A projected storage (`#227`): the storage URI alone. CRS84 drops out,
///   because this collection genuinely cannot be served in it — and saying
///   so is the whole point: a client that requires CRS84 gets a 400 it can
///   act on, rather than metres under a CRS84 header it cannot detect.
pub fn advertised_crs(storage_srid: Option<i32>, crs_capable: bool) -> Vec<String> {
    supported_crs(storage_srid)
        .into_iter()
        .filter(|uri| {
            resolve(Some(uri), storage_srid)
                .is_ok_and(|resolved| can_serve(resolved, storage_srid, crs_capable))
        })
        .collect()
}

/// The `storageCrs` a collection may honestly advertise (`#217`), the
/// capability-aware counterpart of [`advertised_crs`] for Part 2
/// Requirement 4 (`/req/crs/fc-md-storageCrs-valid-value`): "The value of the
/// `storageCrs` property SHALL be one of the CRS identifiers from the list …
/// found using the `crs` property."
///
/// So this returns [`epsg_uri`]`(storage_srid)` only when that exact URI is
/// one of [`advertised_crs`]'s own entries — the requirement expressed
/// literally, as a membership test against the very list the collection
/// publishes, rather than as a second capability rule that could drift from
/// it. Otherwise `None`: the property is simply omitted, the same "never
/// fabricated, never contradicting" rule `extent`/`geometryProfile` already
/// follow when the fact behind them is unknown.
///
/// Two cases produce `None` today, and both are "there is genuinely nothing
/// honest to name". An unknown storage SRID has no identifier at all. And a
/// 4326 storage under a driver that cannot reproject advertises CRS84 alone:
/// the datum coincidence does not save it, because `epsg_uri(4326)` is
/// latitude-before-longitude and a *different URI string* from
/// [`CRS84_URI`], so it is not "one of the identifiers found using `crs`"
/// however similar the two look — and pointing a client at it would earn a
/// 400 from the enforcement gate, which cannot perform that axis swap.
///
/// A **projected** storage under such a driver is the case `#227` changed:
/// its storage URI is now the collection's only advertised `crs`, because
/// that is the CRS its rows genuinely come out in, so this member reappears
/// naming it. The member and the `Content-Crs` header on every response from
/// that collection then say the same thing, which is what `#217` asked of
/// the metadata and `#227` asked of the response.
pub fn advertised_storage_crs(storage_srid: Option<i32>, crs_capable: bool) -> Option<String> {
    let uri = epsg_uri(storage_srid?);
    advertised_crs(storage_srid, crs_capable)
        .contains(&uri)
        .then_some(uri)
}

/// Validates `requested` (a raw `crs`/`bbox-crs` query parameter value, when
/// present) against `storage_srid`'s [`supported_crs`] list, the single seam
/// both parameters are resolved through. `None` (the parameter was omitted)
/// always resolves to [`RequestedCrs::Omitted`] — never an error. A value
/// naming neither [`CRS84_URI`] nor this collection's own storage CRS fails
/// with [`Error::Invalid`] (a 400 at the protocol layer, OGC API Features
/// Part 2 Requirement 11), naming the offending value and the supported set.
pub fn resolve(requested: Option<&str>, storage_srid: Option<i32>) -> Result<RequestedCrs> {
    let Some(uri) = requested else {
        return Ok(RequestedCrs::Omitted);
    };
    if uri == CRS84_URI {
        return Ok(RequestedCrs::Crs84);
    }
    if let Some(srid) = storage_srid {
        if uri == epsg_uri(srid) {
            return Ok(RequestedCrs::Storage);
        }
    }
    Err(Error::Invalid(format!(
        "unsupported crs '{uri}': this collection supports {}",
        supported_crs(storage_srid).join(", ")
    )))
}

/// The URI a `Content-Crs` response header should assert for `resolved`
/// (Requirement 15/16) — the header a caller still emits even for
/// [`RequestedCrs::Omitted`], since a response is always expressed in *some*
/// CRS whether or not the request named one explicitly. `RequestedCrs::
/// Storage` with an unknown `storage_srid` (never produced by [`resolve`]
/// itself, since that combination can't resolve to `Storage` in the first
/// place) falls back to CRS84 rather than panicking.
///
/// ## Why `crs_capable` and `storage_srid` decide this, not `resolved` alone
///
/// Until `#227` the `Omitted`/`Crs84` arm returned [`CRS84_URI`]
/// unconditionally — the header asserted what the *request* had asked for
/// rather than what the response actually contained. Those are the same
/// thing only when the driver did the work:
///
/// - `Omitted` is defined as "no transform, byte-for-byte the pre-Part-2
///   output" for **every** driver including PostGIS (see
///   [`RequestedCrs::Omitted`], and `tellurion-postgis::sql::
///   reprojected_geom_expr`'s own `Omitted => geom` arm). So the coordinates
///   are in the storage CRS, full stop — CRS84 only when that storage is
///   CRS84-equivalent.
/// - `Crs84` is honoured with a real `ST_Transform` only by a `crs_capable`
///   driver. Any other driver hands back the same untouched storage
///   coordinates — which is why the handler now refuses that combination
///   ([`can_serve`]) instead of letting this function describe it.
///
/// Everything else stays exactly as it was: a CRS84-equivalent storage (SRID
/// 4326, or unknown) answers [`CRS84_URI`] on every arm, for every driver.
pub fn content_crs_uri(
    resolved: RequestedCrs,
    storage_srid: Option<i32>,
    crs_capable: bool,
) -> String {
    let storage_uri = || {
        storage_srid
            .map(epsg_uri)
            .unwrap_or_else(|| CRS84_URI.to_string())
    };
    match resolved {
        RequestedCrs::Storage => storage_uri(),
        // A driver that really reprojected into CRS84 really is serving
        // CRS84, whatever its storage SRID.
        RequestedCrs::Crs84 if crs_capable => CRS84_URI.to_string(),
        // Nothing was transformed: the bytes are in the storage CRS, and
        // that is CRS84 exactly when no transform would have been needed.
        RequestedCrs::Omitted | RequestedCrs::Crs84 => {
            if crs84_literals_need_transform(storage_srid) {
                storage_uri()
            } else {
                CRS84_URI.to_string()
            }
        }
    }
}

/// `true` when `srid`'s authority-defined axis order is latitude before
/// longitude — see the module doc's "Axis order" section for why this is
/// narrowly SRID 4326 and not a general per-SRID table.
pub fn is_lat_lon_order(srid: i32) -> bool {
    srid == EPSG_4326_SRID
}

/// `true` when reading coordinates as CRS84 against a collection stored at
/// `storage_srid` asks a driver for a real coordinate transform, rather than
/// being a no-op (`#248`).
///
/// Named for the lane that first needed it — a filter's spatial literals —
/// but the question it answers is about the two *CRSs*, not about literals:
/// "is CRS84 a different coordinate system from this collection's storage?"
/// `#227` reads the same predicate on the output side, where the transform
/// in question is the one that would turn stored geometry into CRS84
/// geometry ([`can_serve`], [`content_crs_uri`]). One predicate rather than
/// two, so the input and output lanes cannot come to different conclusions
/// about the same collection.
///
/// This is the same condition `tellurion-postgis::sql::geometry_literal_expr`
/// branches on — "is the storage SRID a different one from CRS84's?" — stated
/// once, here, so a *protocol* handler can ask it before handing a filter to a
/// driver instead of re-deriving a driver's internal rule. An unknown storage
/// SRID answers `false`: nothing is known that could make a transform
/// necessary.
///
/// ## Which requests this is about
///
/// Both of the two `filter-crs` readings that mean "these coordinates are
/// CRS84", because they are the same question about the same numbers:
///
/// - [`RequestedCrs::Crs84`] — a `filter-crs` naming CRS84 explicitly (Part 3
///   Requirement 8, `/req/filter/filter-crs-param`; `#248`).
/// - [`RequestedCrs::Omitted`] — no `filter-crs` on the wire at all, which
///   Part 3 Requirement 7 (`/req/filter/filter-crs-wgs84`) defines as
///   processing the filter's geometries in CRS84 (`#247`). Until that issue
///   this arm was not a transform anywhere in the workspace, so an omitted
///   parameter against a projected storage produced a filter geometry tagged
///   CRS84 beside a projected column — a mixed-SRID `500` from PostGIS, and a
///   silently wrong `200` from a driver that compares raw coordinates.
///
/// ## What a caller does with the answer
///
/// The caller is the one deciding whether such a filter can be served by a
/// driver that is not
/// [`FeatureSource::filter_crs_capable`](crate::storage::FeatureSource::filter_crs_capable):
/// where no transform is needed, serving it is trivially correct for every
/// driver — which is every CRS84-stored collection, including every live demo.
/// Where one is, only a driver that can transform may be handed the filter,
/// and every other must refuse it **by name** rather than evaluate the
/// literal's coordinates against a projection they were never expressed in.
pub fn crs84_literals_need_transform(storage_srid: Option<i32>) -> bool {
    matches!(storage_srid, Some(srid) if srid != EPSG_4326_SRID)
}

/// Strips the `"<" URI-reference ">"` angle brackets a `Content-Crs` header
/// value is always wrapped in — the read-side counterpart of the exact
/// format `tellurion-features`'s `set_content_crs` already writes for the
/// *response* header (Requirement 15/16: `format!("<{uri}>")`), now needed
/// for a *request*'s own `Content-Crs` (OGC API Features Part 4,
/// Requirement 40, `/req/features/content-crs-header`). Fails with
/// [`Error::Invalid`], naming the raw value, when it isn't wrapped in
/// exactly one leading `<` and one trailing `>` around a non-empty URI — a
/// caller that sent the header but got its shape wrong is refused by name
/// rather than silently falling back to CRS84, which would defeat the
/// entire point of a header whose job is to prevent silent CRS
/// misinterpretation.
pub fn parse_content_crs_header(raw: &str) -> Result<&str> {
    raw.trim()
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .filter(|uri| !uri.is_empty())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "malformed Content-Crs header value '{raw}': expected \"<URI>\""
            ))
        })
}

/// Swaps a `[minx, miny, maxx, maxy]`-shaped bbox's coordinate pairs
/// (`[a, b, c, d]` -> `[b, a, d, c]`) — the input-side counterpart of
/// `tellurion-postgis::sql`'s `ST_FlipCoordinates` output swap. A `bbox-crs`
/// naming a latitude-before-longitude CRS ([`is_lat_lon_order`]) supplies its
/// four numbers as `[minLat, minLon, maxLat, maxLon]`; this reorders them
/// into the `[minx, miny, maxx, maxy]` (longitude-first) shape every SQL
/// envelope builder in this workspace already assumes.
pub fn swap_bbox_axes(bbox: [f64; 4]) -> [f64; 4] {
    [bbox[1], bbox[0], bbox[3], bbox[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epsg_uri_uses_the_unversioned_by_reference_style() {
        assert_eq!(epsg_uri(4326), "http://www.opengis.net/def/crs/EPSG/0/4326");
        assert_eq!(epsg_uri(3857), "http://www.opengis.net/def/crs/EPSG/0/3857");
    }

    #[test]
    fn supported_crs_is_crs84_alone_when_the_storage_srid_is_unknown() {
        assert_eq!(supported_crs(None), vec![CRS84_URI.to_string()]);
    }

    #[test]
    fn supported_crs_adds_the_storage_crs_when_the_srid_is_known() {
        assert_eq!(
            supported_crs(Some(4326)),
            vec![CRS84_URI.to_string(), epsg_uri(4326)]
        );
        assert_eq!(
            supported_crs(Some(3857)),
            vec![CRS84_URI.to_string(), epsg_uri(3857)]
        );
    }

    /// The unmoved case, and the one every live deployment is in: a storage
    /// that is already CRS84-equivalent (SRID 4326, or none reported) under
    /// a driver that cannot reproject advertises CRS84 alone, exactly as it
    /// has since `#217`. The storage URI stays out because honouring it
    /// means the EPSG:4326 axis swap such a driver cannot perform.
    #[test]
    fn advertised_crs_is_crs84_alone_for_a_crs84_storage_under_a_non_reprojecting_driver() {
        assert_eq!(advertised_crs(None, false), vec![CRS84_URI.to_string()]);
        assert_eq!(
            advertised_crs(Some(4326), false),
            vec![CRS84_URI.to_string()]
        );
    }

    /// `#227`: a **projected** storage under the same driver advertises its
    /// own storage CRS and *not* CRS84 — CRS84 is the one identifier such a
    /// collection genuinely cannot produce, since serving it would be a real
    /// coordinate transform. This is the list a client reads before choosing
    /// a `?crs=`, so it has to name what it can actually get.
    #[test]
    fn advertised_crs_is_the_storage_crs_alone_for_a_projected_non_reprojecting_collection() {
        assert_eq!(advertised_crs(Some(3857), false), vec![epsg_uri(3857)]);
        assert_eq!(advertised_crs(Some(2154), false), vec![epsg_uri(2154)]);
    }

    /// Whatever the SRID and whatever the driver, a collection is served in
    /// *some* CRS, so it always has at least one honest identifier to
    /// publish — the property that makes `advertised_crs`'s filter safe to
    /// state as a filter at all.
    #[test]
    fn advertised_crs_is_never_empty() {
        for storage_srid in [None, Some(4326), Some(3857), Some(2154)] {
            for crs_capable in [false, true] {
                assert!(
                    !advertised_crs(storage_srid, crs_capable).is_empty(),
                    "storage_srid={storage_srid:?} crs_capable={crs_capable}"
                );
            }
        }
    }

    #[test]
    fn advertised_crs_matches_supported_crs_when_the_driver_can_reproject() {
        assert_eq!(advertised_crs(None, true), supported_crs(None));
        assert_eq!(advertised_crs(Some(4326), true), supported_crs(Some(4326)));
        assert_eq!(advertised_crs(Some(3857), true), supported_crs(Some(3857)));
    }

    /// The invariant `advertised_crs` exists to guarantee, expressed
    /// directly over every `(storage_srid, crs_capable)` pair this crate can
    /// produce rather than as separate hardcoded expected lists: every URI
    /// it advertises is one [`resolve`] accepts *and* one [`can_serve`] —
    /// the very gate `tellurion-features`' handlers run before honouring a
    /// `crs`/`bbox-crs` parameter — says the driver can actually put out. A
    /// test phrased this way can't drift from the real enforcement gate the
    /// way two independently-hardcoded lists could.
    #[test]
    fn every_advertised_crs_is_accepted_by_the_enforcement_gate() {
        for storage_srid in [None, Some(4326), Some(3857), Some(2154)] {
            for crs_capable in [false, true] {
                for uri in advertised_crs(storage_srid, crs_capable) {
                    let resolved = resolve(Some(&uri), storage_srid).unwrap_or_else(|err| {
                        panic!(
                            "advertised crs '{uri}' (storage_srid={storage_srid:?}, \
                             crs_capable={crs_capable}) was rejected by resolve: {err}"
                        )
                    });
                    assert!(
                        can_serve(resolved, storage_srid, crs_capable),
                        "advertised crs '{uri}' (storage_srid={storage_srid:?}, \
                         crs_capable={crs_capable}) is one this driver cannot serve"
                    );
                }
            }
        }
    }

    /// The other half of the same invariant, which the test above cannot
    /// see: everything `can_serve` accepts must be *advertised*, or a client
    /// reading `crs` is being told less than the truth. Stated over every
    /// resolvable identifier rather than over the advertised list, so a
    /// regression that quietly narrowed the list while leaving the gate open
    /// fails here.
    #[test]
    fn every_servable_crs_is_advertised() {
        for storage_srid in [None, Some(4326), Some(3857), Some(2154)] {
            for crs_capable in [false, true] {
                let advertised = advertised_crs(storage_srid, crs_capable);
                for uri in supported_crs(storage_srid) {
                    let resolved = resolve(Some(&uri), storage_srid).expect("supported resolves");
                    if can_serve(resolved, storage_srid, crs_capable) {
                        assert!(
                            advertised.contains(&uri),
                            "'{uri}' is servable (storage_srid={storage_srid:?}, \
                             crs_capable={crs_capable}) but is not advertised"
                        );
                    }
                }
            }
        }
    }

    /// `can_serve`'s own table, spelled out — the rule every other
    /// capability-aware function here is read off, so it is worth pinning
    /// directly rather than only through its consumers.
    #[test]
    fn can_serve_admits_only_the_no_op_requests_for_a_non_reprojecting_driver() {
        // Nothing requested: never refused, for any storage.
        for storage_srid in [None, Some(4326), Some(3857)] {
            assert!(can_serve(RequestedCrs::Omitted, storage_srid, false));
        }
        // CRS84: free on a CRS84-equivalent storage, a real transform on a
        // projected one.
        assert!(can_serve(RequestedCrs::Crs84, None, false));
        assert!(can_serve(RequestedCrs::Crs84, Some(4326), false));
        assert!(!can_serve(RequestedCrs::Crs84, Some(3857), false));
        // The storage CRS: free unless it is EPSG:4326, whose authority axis
        // order needs the coordinate swap this driver cannot do.
        assert!(!can_serve(RequestedCrs::Storage, Some(4326), false));
        assert!(can_serve(RequestedCrs::Storage, Some(3857), false));
    }

    #[test]
    fn can_serve_admits_everything_for_a_reprojecting_driver() {
        for storage_srid in [None, Some(4326), Some(3857), Some(2154)] {
            for resolved in [
                RequestedCrs::Omitted,
                RequestedCrs::Crs84,
                RequestedCrs::Storage,
            ] {
                assert!(can_serve(resolved, storage_srid, true));
            }
        }
    }

    /// Part 2 Requirement 4 expressed the same way the test above expresses
    /// Requirement 2 — over every `(storage_srid, crs_capable)` pair rather
    /// than against hardcoded expectations: whatever `advertised_storage_crs`
    /// returns must be a member of the `crs` list the very same collection
    /// publishes, and it may only be absent when there is genuinely nothing
    /// honest to name.
    #[test]
    fn every_advertised_storage_crs_is_in_the_advertised_crs_list() {
        for storage_srid in [None, Some(4326), Some(3857), Some(2154)] {
            for crs_capable in [false, true] {
                let Some(uri) = advertised_storage_crs(storage_srid, crs_capable) else {
                    continue;
                };
                assert!(
                    advertised_crs(storage_srid, crs_capable).contains(&uri),
                    "advertised storageCrs '{uri}' is outside this collection's own crs list \
                     (storage_srid={storage_srid:?}, crs_capable={crs_capable})"
                );
            }
        }
    }

    /// The two `None` cases spelled out: an unknown storage SRID has nothing
    /// to name at all, and a 4326 storage under a driver that cannot
    /// reproject advertises CRS84 alone — the datum coincidence does not make
    /// `epsg_uri(4326)` the same identifier as `CRS84_URI`.
    #[test]
    fn advertised_storage_crs_is_absent_when_there_is_nothing_honest_to_name() {
        assert_eq!(advertised_storage_crs(None, true), None);
        assert_eq!(advertised_storage_crs(None, false), None);
        assert_eq!(advertised_storage_crs(Some(4326), false), None);
    }

    /// `#227`: a projected storage under a non-reprojecting driver now has
    /// something honest to name — its own storage CRS, which is both the
    /// only entry in its `crs` list and the CRS its rows genuinely come out
    /// in. `#217` had to omit the member only because the list it had to be
    /// a member of was CRS84-only.
    #[test]
    fn advertised_storage_crs_names_a_projected_storage_even_without_reprojection() {
        assert_eq!(
            advertised_storage_crs(Some(3857), false),
            Some(epsg_uri(3857))
        );
    }

    #[test]
    fn advertised_storage_crs_names_the_storage_srid_when_the_driver_can_reproject() {
        assert_eq!(
            advertised_storage_crs(Some(4326), true),
            Some(epsg_uri(4326))
        );
        assert_eq!(
            advertised_storage_crs(Some(3857), true),
            Some(epsg_uri(3857))
        );
    }

    #[test]
    fn resolve_defaults_to_omitted_when_no_parameter_was_supplied() {
        assert_eq!(resolve(None, Some(4326)).unwrap(), RequestedCrs::Omitted);
        assert_eq!(resolve(None, None).unwrap(), RequestedCrs::Omitted);
    }

    #[test]
    fn resolve_accepts_crs84_regardless_of_storage_srid() {
        assert_eq!(resolve(Some(CRS84_URI), None).unwrap(), RequestedCrs::Crs84);
        assert_eq!(
            resolve(Some(CRS84_URI), Some(3857)).unwrap(),
            RequestedCrs::Crs84
        );
    }

    #[test]
    fn resolve_accepts_the_collections_own_storage_crs() {
        assert_eq!(
            resolve(Some(&epsg_uri(3857)), Some(3857)).unwrap(),
            RequestedCrs::Storage
        );
    }

    #[test]
    fn resolve_rejects_a_crs_not_in_the_supported_set() {
        match resolve(Some(&epsg_uri(3857)), Some(4326)) {
            Err(Error::Invalid(message)) => {
                assert!(message.contains(&epsg_uri(3857)));
                assert!(message.contains(CRS84_URI));
                assert!(message.contains(&epsg_uri(4326)));
            }
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_the_storage_crs_when_the_srid_is_unknown() {
        assert!(matches!(
            resolve(Some(&epsg_uri(4326)), None),
            Err(Error::Invalid(_))
        ));
    }

    /// The unmoved case: a CRS84-equivalent storage answers CRS84 on every
    /// arm, for both driver capabilities. Every live deployment is here.
    #[test]
    fn content_crs_uri_is_crs84_for_a_crs84_equivalent_storage() {
        for storage_srid in [None, Some(4326)] {
            for crs_capable in [false, true] {
                assert_eq!(
                    content_crs_uri(RequestedCrs::Omitted, storage_srid, crs_capable),
                    CRS84_URI,
                    "storage_srid={storage_srid:?} crs_capable={crs_capable}"
                );
                assert_eq!(
                    content_crs_uri(RequestedCrs::Crs84, storage_srid, crs_capable),
                    CRS84_URI,
                    "storage_srid={storage_srid:?} crs_capable={crs_capable}"
                );
            }
        }
    }

    /// `#227`, the header that used to lie. No driver transforms anything
    /// for an omitted `crs` — that is `RequestedCrs::Omitted`'s own
    /// definition, PostGIS included — so a projected collection's default
    /// response is in its storage CRS and must say so.
    #[test]
    fn content_crs_uri_names_the_storage_crs_when_nothing_was_transformed() {
        for crs_capable in [false, true] {
            assert_eq!(
                content_crs_uri(RequestedCrs::Omitted, Some(3857), crs_capable),
                epsg_uri(3857),
                "crs_capable={crs_capable}"
            );
        }
        // A driver that cannot reproject hands back the same untouched
        // coordinates for an explicit `crs=CRS84` too. The handler refuses
        // that request outright (`can_serve`); if it ever reached here, the
        // header still must not claim degrees over metres.
        assert!(!can_serve(RequestedCrs::Crs84, Some(3857), false));
        assert_eq!(
            content_crs_uri(RequestedCrs::Crs84, Some(3857), false),
            epsg_uri(3857)
        );
    }

    /// A `crs_capable` driver asked for CRS84 explicitly really does
    /// `ST_Transform` into it, so CRS84 is the truth there.
    #[test]
    fn content_crs_uri_is_crs84_when_a_capable_driver_reprojected_into_it() {
        assert_eq!(
            content_crs_uri(RequestedCrs::Crs84, Some(3857), true),
            CRS84_URI
        );
    }

    #[test]
    fn content_crs_uri_is_the_storage_crs_when_storage_was_requested() {
        for crs_capable in [false, true] {
            assert_eq!(
                content_crs_uri(RequestedCrs::Storage, Some(3857), crs_capable),
                epsg_uri(3857)
            );
            assert_eq!(
                content_crs_uri(RequestedCrs::Storage, Some(4326), crs_capable),
                epsg_uri(4326)
            );
        }
    }

    /// The property that ties the header to the metadata: whatever
    /// `content_crs_uri` stamps for a request the gate let through is one of
    /// the identifiers the very same collection advertises. A header naming
    /// a CRS outside the `crs` list would be a fresh instance of exactly the
    /// contradiction `#217` and `#227` closed.
    #[test]
    fn every_stamped_content_crs_is_one_the_collection_advertises() {
        for storage_srid in [None, Some(4326), Some(3857), Some(2154)] {
            for crs_capable in [false, true] {
                let advertised = advertised_crs(storage_srid, crs_capable);
                for resolved in [
                    RequestedCrs::Omitted,
                    RequestedCrs::Crs84,
                    RequestedCrs::Storage,
                ] {
                    if !can_serve(resolved, storage_srid, crs_capable) {
                        continue;
                    }
                    let stamped = content_crs_uri(resolved, storage_srid, crs_capable);
                    assert!(
                        advertised.contains(&stamped),
                        "stamped Content-Crs '{stamped}' for {resolved:?} \
                         (storage_srid={storage_srid:?}, crs_capable={crs_capable}) \
                         is outside the advertised list {advertised:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn is_lat_lon_order_is_true_only_for_4326() {
        assert!(is_lat_lon_order(4326));
        assert!(!is_lat_lon_order(3857));
        assert!(!is_lat_lon_order(2154));
    }

    #[test]
    fn swap_bbox_axes_swaps_each_coordinate_pair() {
        assert_eq!(
            swap_bbox_axes([44.0, 9.0, 45.5, 10.5]),
            [9.0, 44.0, 10.5, 45.5]
        );
    }

    #[test]
    fn parse_content_crs_header_strips_the_angle_brackets() {
        assert_eq!(
            parse_content_crs_header(&format!("<{CRS84_URI}>")).unwrap(),
            CRS84_URI
        );
        assert_eq!(
            parse_content_crs_header(&format!("<{}>", epsg_uri(3857))).unwrap(),
            epsg_uri(3857)
        );
    }

    #[test]
    fn parse_content_crs_header_tolerates_surrounding_whitespace() {
        assert_eq!(
            parse_content_crs_header(&format!("  <{CRS84_URI}>  ")).unwrap(),
            CRS84_URI
        );
    }

    #[test]
    fn parse_content_crs_header_rejects_a_value_with_no_brackets_at_all() {
        assert!(matches!(
            parse_content_crs_header(CRS84_URI),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn parse_content_crs_header_rejects_a_missing_closing_bracket() {
        assert!(matches!(
            parse_content_crs_header(&format!("<{CRS84_URI}")),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn parse_content_crs_header_rejects_a_missing_opening_bracket() {
        assert!(matches!(
            parse_content_crs_header(&format!("{CRS84_URI}>")),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn parse_content_crs_header_rejects_empty_brackets() {
        assert!(matches!(
            parse_content_crs_header("<>"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn parse_content_crs_header_error_names_the_raw_value() {
        match parse_content_crs_header("not-a-crs-header") {
            Err(Error::Invalid(message)) => assert!(message.contains("not-a-crs-header")),
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }
}
