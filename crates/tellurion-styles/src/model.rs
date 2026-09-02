//! Response DTOs for the OGC API — Styles read-only surface. Kept local to
//! this crate rather than imported from `tellurion-features` so
//! `tellurion-styles` has no dependency on another protocol crate — the
//! same independence every protocol crate keeps from every other one.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub href: String,
    pub rel: String,
    #[serde(rename = "type")]
    pub media_type: String,
}

impl Link {
    pub fn new(
        href: impl Into<String>,
        rel: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            href: href.into(),
            rel: rel.into(),
            media_type: media_type.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StyleSummary {
    pub id: String,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StylesListResponse {
    pub styles: Vec<StyleSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StylesheetRef {
    pub link: Link,
    /// Always `true` in v0.2: the only stylesheet encoding this crate
    /// serves is the document's own native MapLibre Style JSON (no
    /// transcoding to SLD/CSS/JSON symbology).
    pub native: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerRef {
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StyleMetadataResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub stylesheets: Vec<StylesheetRef>,
    pub layers: Vec<LayerRef>,
}
