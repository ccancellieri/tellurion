//! OGC API — Styles (OGC 20-009) requirement class URIs.
//!
//! Styles is a **long-running draft** (verified 2026-07 against
//! <https://docs.ogc.org/DRAFTS/20-009.html>) — unlike OGC API Tiles Part 1,
//! there is no approved version to conform to. These constants exist so a
//! caller can honestly name *which draft requirement classes this crate's
//! read-only surface aligns with in shape*; whether to advertise them in a
//! server's `/conformance` `conformsTo` list is that server's judgment call
//! (claiming conformance to a draft is debatable even when the shapes
//! match), so this crate does not assert them anywhere itself.
//!
//! Only the classes this crate's read-only surface aligns with are listed.
//! Write operations (`manage-styles`, `style-validation`), the SLD/CSS/JSON
//! symbology encodings (`sld-se`, `sld-10`, `sld-11`, `cscss`, `csjson`),
//! and the resource-management classes (`resources`, `manage-resources`)
//! are all out of scope for v0.2 and intentionally absent here.

/// `GET /styles`, `GET /styles/{styleId}`, `GET /styles/{styleId}/metadata`
/// — the read-only style discovery/access surface this crate implements.
pub const CONFORMANCE_STYLES_CORE: &str =
    "http://www.opengis.net/spec/ogcapi-styles-1/1.0/conf/core";

/// MapLibre Style JSON is a compatible superset of the Mapbox Style Spec
/// this requirement class names; `GET /styles/{styleId}` serves that
/// encoding natively (see [`crate::STYLE_MEDIA_TYPE`]).
pub const CONFORMANCE_MAPBOX_STYLES: &str =
    "http://www.opengis.net/spec/ogcapi-styles-1/1.0/conf/mapbox-styles";
