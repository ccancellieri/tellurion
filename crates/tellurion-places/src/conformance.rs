//! Link relation/media-type metadata for the 3D Tiles surface.
//!
//! No conformance-class URIs are exported here. OGC API — 3D GeoVolumes
//! (OGC 22-001), the standard that would define `/collections/{cid}/3dtiles`
//! as a formal conformance class, is still a candidate draft with no
//! approved class to cite (re-verify before ever changing this). 3D Tiles
//! 1.1 itself (OGC 22-025) is an approved community standard, but it is a
//! content-format spec, not an OGC API — it has no `/conformance`-style
//! class URIs to advertise either. So the server aggregates these as plain
//! link metadata, not as entries in a `conformsTo` list.

/// Link relation used to point at a collection's 3D Tiles tileset. Not a
/// registered IANA or OGC relation type — Tellurion's own descriptive rel,
/// chosen so a client can discover the endpoint without this crate implying
/// conformance it doesn't have.
pub const REL_3D_TILES: &str = "3d-tiles";

/// Media type of the `tileset.json` document served at
/// `/collections/{cid}/3dtiles`.
pub const MEDIA_TYPE_TILESET: &str = "application/json";

/// Media type of glTF binary (.glb) tile content.
pub const MEDIA_TYPE_GLB: &str = "model/gltf-binary";

/// Media type of `.subtree` implicit-tiling availability documents. Not an
/// IANA-registered media type — 3D Tiles 1.1 (OGC 22-025) defines the
/// binary subtree file format but does not register or recommend a media
/// type for it. `application/octet-stream` is the honest choice for an
/// opaque binary format with no dedicated registration.
pub const MEDIA_TYPE_SUBTREE: &str = "application/octet-stream";
