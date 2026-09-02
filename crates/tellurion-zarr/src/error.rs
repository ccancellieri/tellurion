//! Internal error type for this crate. Structural refusals (`Unsupported`/
//! `Open`/`Decode`/`RemoteOpen`) are decided once, at metadata-parse time
//! (`reader::open`) — reached either from `Router::validate_catalog`'s eager
//! boot sweep or, under `registry.validation: lazy`, this collection's first
//! touch — and map to `tellurion_core::Error::Config`, the same "bad config
//! fails fast, with an actionable message" contract every other driver in
//! this workspace uses for its own boot-time refusals. `WindowBudgetExceeded`/
//! `DecodeBudgetExceeded` are the genuinely per-request refusals (they depend
//! on which tile was requested) and map to `Error::Invalid` instead —
//! mirrors `tellurion-cog::error::CogError`'s own structural-vs-per-request
//! split. `RemoteOpen` mirrors that same crate's own choice too: a remote
//! transport failure (an unreachable server, a non-2xx status other than a
//! chunk's `404`, a response body over its fetch's own byte budget) is
//! treated as this same structural class even when it's reached mid-request
//! (this driver re-opens/re-reads its store on every call, so a remote
//! failure can surface at any of them, not only at boot) — never
//! `Error::Invalid`, since it's never the requester's fault.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZarrError {
    #[error("failed to open Zarr store at '{path}': {source}")]
    Open {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to decode Zarr array: {0}")]
    Decode(String),

    #[error("unsupported Zarr array: {0}")]
    Unsupported(String),

    /// A remote (`http(s)`) store's request failed outright (unreachable,
    /// timed out) or answered with a status this driver doesn't treat as
    /// "chunk missing" (`404`, `store::ZarrStore::read_chunk`'s own
    /// contract) — any other non-2xx, or a response body that exceeded the
    /// fetch's own byte budget (`store::RemoteZarrSource`'s own doc). Never
    /// raised for a `404`, which is a legitimate "missing chunk" fact, not a
    /// failure.
    #[error("failed to reach remote Zarr store at '{url}': {message}")]
    RemoteOpen { url: String, message: String },

    /// A tile's clamped native-resolution read window exceeds this driver's
    /// per-request pixel budget — refused before any chunk is opened, the
    /// same "check first, never balloon" idiom `tellurion-cog`'s own
    /// `check_pixel_budget` uses.
    #[error(
        "requested tile needs a {width}x{height} native-resolution window, over this driver's budget of {budget} pixels"
    )]
    WindowBudgetExceeded {
        width: u64,
        height: u64,
        budget: u64,
    },

    /// A tile's window would require decompressing more chunk elements,
    /// summed across every chunk it touches, than this driver's per-request
    /// decode budget allows — distinct from `WindowBudgetExceeded`: a small
    /// window over a pathologically small chunk shape can still touch a huge
    /// number of chunks, each cheap on its own but unbounded in aggregate.
    #[error(
        "requested tile would decompress {elements} chunk elements across its touched chunks, over this driver's budget of {budget}"
    )]
    DecodeBudgetExceeded { elements: u64, budget: u64 },
}

pub type Result<T> = std::result::Result<T, ZarrError>;

impl From<ZarrError> for tellurion_core::Error {
    fn from(error: ZarrError) -> Self {
        match error {
            ZarrError::WindowBudgetExceeded { .. } | ZarrError::DecodeBudgetExceeded { .. } => {
                tellurion_core::Error::Invalid(error.to_string())
            }
            ZarrError::Open { .. }
            | ZarrError::Decode(_)
            | ZarrError::Unsupported(_)
            | ZarrError::RemoteOpen { .. } => tellurion_core::Error::Config(error.to_string()),
        }
    }
}
