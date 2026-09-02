//! Internal error type for this crate. `Unsupported`/`Open`/`Decode`/
//! Remote-read failures are structural: whether this driver
//! can serve a given source at all, decided once when its metadata is
//! parsed (`reader::CogMeta::open`) — reached either from `Router::
//! validate_catalog`'s eager boot sweep or, under `registry.validation:
//! lazy`, on this collection's first-touch. They map to `tellurion_core::
//! Error::Config`, the same "bad config fails fast, with an actionable
//! message" contract every other driver in this workspace uses for its own
//! boot-time refusals. `PixelBudgetExceeded` is the one genuinely
//! per-request refusal (`#37`) — it depends on which tile was requested —
//! and maps to `Error::Invalid` instead. `Write`/`Encode` belong to the
//! authoring path (`author.rs`) instead of serving — a CLI-context failure,
//! never reached through `StorageDriver`, but sharing this crate's one error
//! type rather than inventing a second keeps every named refusal in one
//! place.

use tellurion_http_source::SourceErrorKind;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CogError {
    #[error("environment variable '{0}' is not set")]
    MissingEnvVar(String),

    #[error("failed to open GeoTIFF '{path}': {source}")]
    Open {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to decode GeoTIFF: {0}")]
    Decode(String),

    #[error("unsupported GeoTIFF: {0}")]
    Unsupported(String),

    #[error("remote GeoTIFF read failed: {kind}")]
    RemoteRead { kind: SourceErrorKind },

    #[error("remote GeoTIFF read exceeded its operation budget")]
    RemoteOperationBudget,

    #[error(
        "requested tile needs {requested} source pixels, over this driver's budget of {budget}"
    )]
    PixelBudgetExceeded { requested: u64, budget: u64 },

    /// `#254` `cog-mosaic`: the manifest sidecar could not be read at all.
    /// Structural, like `Open` — decided once, when the storage is built.
    #[error("failed to read the mosaic manifest '{path}': {source}")]
    ManifestRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// `#254`: the manifest is not a well-formed manifest document — a YAML
    /// syntax error, a missing field, or (thanks to `deny_unknown_fields`) a
    /// field this driver does not know, which is refused rather than
    /// silently ignored.
    #[error("mosaic manifest '{path}' is not a valid manifest document: {message}")]
    ManifestParse { path: String, message: String },

    /// `#254`: the manifest parsed, but breaks one of the bounds this driver
    /// is defined by — 1..=32 unique sources in ascending id order, a
    /// well-formed CRS84 bbox, a non-zero byte length, a 64-hex-character
    /// SHA-256, a local (non-`http`) source path. Every one of those is a
    /// refusal naming the rule and the offending source. Canonical duplicate
    /// local files are reported separately by [`Self::MosaicDuplicateLocalSource`].
    #[error("mosaic manifest '{path}' is invalid: {message}")]
    ManifestInvalid { path: String, message: String },

    /// `#322`: distinct source ids resolve to the same local object. A
    /// mosaic paints source ids in order, so accepting aliases would make
    /// the same COG appear twice in that composition.
    #[error(
        "mosaic manifest '{manifest_path}' has duplicate local source '{duplicate_id}': it resolves to '{canonical_path}', already used by source '{first_id}'"
    )]
    MosaicDuplicateLocalSource {
        manifest_path: String,
        first_id: String,
        duplicate_id: String,
        canonical_path: String,
    },

    /// `#254`: a constituent COG's real bytes do not match what the manifest
    /// recorded — a byte length, a SHA-256, or a bbox that disagrees with the
    /// file's own georeferencing tags. The provenance fields are MEASURED by
    /// `ingest`; a mismatch means the object changed under the manifest (or
    /// the manifest was hand-edited), and the source is refused rather than
    /// served.
    #[error("mosaic source '{id}' failed provenance verification: {message}")]
    MosaicSourceProvenance { id: String, message: String },

    /// `#254`: reading one selected constituent COG failed. Fails the WHOLE
    /// requested tile — never a partially composed one, which would be
    /// byte-indistinguishable from legitimate transparency.
    #[error("mosaic source '{id}' could not be read, so the whole tile fails: {message}")]
    MosaicSourceRead { id: String, message: String },

    /// Authoring only: the output file couldn't be created or written.
    #[error("failed to write COG '{path}': {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Authoring only: a tile failed to Deflate-compress, or a compressed
    /// tile's byte length doesn't fit a classic (non-Big) TIFF's 32-bit
    /// offsets.
    #[error("failed to encode COG output: {0}")]
    Encode(String),
}

pub type Result<T> = std::result::Result<T, CogError>;

impl From<CogError> for tellurion_core::Error {
    fn from(error: CogError) -> Self {
        match error {
            CogError::PixelBudgetExceeded { .. } => {
                tellurion_core::Error::Invalid(error.to_string())
            }
            CogError::MissingEnvVar(_)
            | CogError::Open { .. }
            | CogError::Decode(_)
            | CogError::Unsupported(_)
            | CogError::RemoteRead { .. }
            | CogError::RemoteOperationBudget
            | CogError::ManifestRead { .. }
            | CogError::ManifestParse { .. }
            | CogError::ManifestInvalid { .. }
            | CogError::MosaicDuplicateLocalSource { .. }
            | CogError::MosaicSourceProvenance { .. }
            | CogError::Write { .. }
            | CogError::Encode(_) => tellurion_core::Error::Config(error.to_string()),
            // A constituent may disappear after the manifest's boot-time
            // provenance sweep. That is a serving failure, not a
            // client-correctable configuration or capability refusal.
            CogError::MosaicSourceRead { .. } => tellurion_core::Error::Storage(Box::new(error)),
        }
    }
}
