//! The `cog-mosaic` manifest (`#254`): a YAML **sidecar**, authored by
//! `tellurion-ingest cog mosaic` and referenced by a `cog-mosaic` storage's
//! own locator — never a hand-written block in `config.yaml`.
//!
//! ## Why a sidecar, and why measured rather than declared
//!
//! Every provenance field here is **measured from the object itself**, never
//! transcribed by a human: [`author_mosaic_manifest`] derives each source's
//! `bbox` from that COG's own georeferencing tags (through this crate's own
//! `reader::open`, the same parse the serving path uses), its `byte_length`
//! from the file's own length, and its `sha256` from a streaming digest of
//! its bytes. A SHA-256 typed into YAML by hand is an error nobody notices
//! until the day it matters.
//!
//! The server is the other half of that arrangement and does strictly less:
//! it **validates the manifest it is given** and refuses by name if it does
//! not hold. It never authors one, never repairs one, and issues no DDL —
//! the same "authoring owns every physical-layout decision" split
//! `geopackage create-tables` and `cog author` already draw on their own
//! lanes.
//!
//! ## The bounds, all of them refusals by name
//!
//! * `version` must be exactly [`MANIFEST_VERSION`].
//! * `sources` must hold at least 1 and at most [`MAX_SOURCES`] entries.
//! * source ids must be non-empty, unique, and **listed in ascending id
//!   order** — the order is the composition order (see `mosaic.rs`), so a
//!   manifest that does not read in that order is refused rather than
//!   silently reordered: an operator must be able to read the paint order
//!   straight off the file.
//! * every `bbox` must be four finite CRS84 numbers with `minx < maxx`,
//!   `miny < maxy`, inside `[-180, 180] x [-90, 90]`.
//! * every `byte_length` must be non-zero and every `sha256` exactly 64
//!   lowercase hex characters.
//! * every `path` must be a local filesystem path. An `http(s)` locator is
//!   refused by name: verifying a source's SHA-256 means reading all of its
//!   bytes, which a ranged-read remote source cannot supply without
//!   downloading the whole object — the exact unbounded read the single-COG
//!   remote path exists to avoid. Remote mosaic sources are out of this
//!   slice's scope, named rather than half-served.
//! * two source ids may not resolve to the same canonical local file. A
//!   relative alias or symlink is refused at manifest load, before either
//!   entry can take part in composition.
//!
//! Those are the *structural* checks made while the manifest is loaded
//! (`Router::build` -> `MosaicDriverFactory::build`): document shape is
//! validated as it is parsed, then local-file identities are resolved. The
//! *content* checks — byte length, SHA-256, and the declared bbox against
//! the COG's own georeferencing — are made against the real bytes, once per
//! source, by [`verify_source`]; see `mosaic.rs` for where that runs.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::error::{CogError, Result};
use crate::reader::{self, CogSource};

/// The only manifest `version` this driver reads. A future shape change
/// bumps this and is refused by name here rather than mis-parsed.
pub const MANIFEST_VERSION: u32 = 1;

/// The issue's own bound: "validate one to 32 unique sources". A manifest
/// listing more is refused by name — never truncated, never partially
/// served. Deliberately small: this is a *bounded* deterministic mosaic, not
/// a general catalog, and the bound is what makes "read every selected
/// source or fail the tile" affordable.
pub const MAX_SOURCES: usize = 32;

/// The number of hex characters a SHA-256 digest occupies.
const SHA256_HEX_LEN: usize = 64;

/// How many bytes [`sha256_of_file`] hashes at a time. A COG is arbitrarily
/// large; the digest is streamed so verifying provenance never costs memory
/// proportional to the object.
const HASH_CHUNK_BYTES: usize = 64 * 1024;

/// One constituent COG, exactly as the sidecar records it.
///
/// `path` is resolved relative to the **manifest's own directory** when it
/// is relative (so a manifest and its COGs move together as one directory),
/// and used as-is when absolute — see [`ManifestSource::resolve_path`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    /// Stable identity, and the composition order key (`mosaic.rs`).
    pub id: String,
    /// Local filesystem path, relative to the manifest's own directory or
    /// absolute.
    pub path: String,
    /// `[minx, miny, maxx, maxy]` in CRS84 — measured from this COG's own
    /// georeferencing tags by [`author_mosaic_manifest`], and re-checked
    /// against them by [`verify_source`].
    pub bbox: [f64; 4],
    /// The object's own byte length, measured.
    pub byte_length: u64,
    /// Lowercase hex SHA-256 of the object's own bytes, measured.
    pub sha256: String,
}

impl ManifestSource {
    /// Where this source's bytes really live, given the directory the
    /// manifest itself was read from.
    pub fn resolve_path(&self, manifest_dir: &Path) -> PathBuf {
        let declared = Path::new(&self.path);
        if declared.is_absolute() {
            declared.to_path_buf()
        } else {
            manifest_dir.join(declared)
        }
    }
}

/// The sidecar document itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MosaicManifest {
    pub version: u32,
    pub sources: Vec<ManifestSource>,
}

impl MosaicManifest {
    /// Parses and structurally validates a manifest read from `path`.
    /// `path` is carried only so every refusal names the file the operator
    /// has to go and fix.
    pub fn parse(path: &Path, yaml: &str) -> Result<Self> {
        let manifest: MosaicManifest =
            serde_yaml::from_str(yaml).map_err(|error| CogError::ManifestParse {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    /// Reads and parses the manifest at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let yaml = std::fs::read_to_string(path).map_err(|source| CogError::ManifestRead {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(path, &yaml)
    }

    /// Every structural bound in this module's own doc, each a refusal by
    /// name. Pure: no I/O, so a test can exercise each rule against a
    /// literal document.
    pub fn validate(&self, path: &Path) -> Result<()> {
        let refuse = |message: String| -> CogError {
            CogError::ManifestInvalid {
                path: path.display().to_string(),
                message,
            }
        };

        if self.version != MANIFEST_VERSION {
            return Err(refuse(format!(
                "manifest version {} is not supported; this driver reads version {MANIFEST_VERSION}",
                self.version
            )));
        }
        if self.sources.is_empty() {
            return Err(refuse(format!(
                "manifest lists no sources; a mosaic manifest must list 1..={MAX_SOURCES}"
            )));
        }
        if self.sources.len() > MAX_SOURCES {
            return Err(refuse(format!(
                "manifest lists {} sources, over this driver's bound of {MAX_SOURCES}",
                self.sources.len()
            )));
        }

        let mut previous: Option<&str> = None;
        for source in &self.sources {
            if source.id.trim().is_empty() {
                return Err(refuse("a source has an empty id".to_string()));
            }
            match previous {
                Some(prev) if prev == source.id => {
                    return Err(refuse(format!("duplicate source id '{}'", source.id)));
                }
                Some(prev) if prev > source.id.as_str() => {
                    return Err(refuse(format!(
                        "source '{}' is listed after '{prev}'; a mosaic manifest's sources must \
                         be listed in ascending id order, because that order IS the composition \
                         order (later ids paint over earlier ones)",
                        source.id
                    )));
                }
                _ => {}
            }
            previous = Some(&source.id);

            validate_bbox(&source.id, source.bbox).map_err(refuse)?;

            if source.byte_length == 0 {
                return Err(refuse(format!(
                    "source '{}' declares byte_length 0; a COG is never zero bytes",
                    source.id
                )));
            }
            if source.sha256.len() != SHA256_HEX_LEN
                || !source
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(refuse(format!(
                    "source '{}' declares sha256 '{}', which is not {SHA256_HEX_LEN} lowercase \
                     hex characters",
                    source.id, source.sha256
                )));
            }
            if source.path.trim().is_empty() {
                return Err(refuse(format!("source '{}' has an empty path", source.id)));
            }
            if is_http_locator(&source.path) {
                return Err(refuse(format!(
                    "source '{}' has an http(s) locator '{}'; a mosaic source must be a local \
                     file, because verifying its declared sha256 means reading all of its bytes \
                     — which a ranged-read remote source cannot supply without downloading the \
                     whole object. Remote mosaic sources are out of this driver's scope",
                    source.id, source.path
                )));
            }
        }

        // A duplicate id that is NOT adjacent cannot exist once the list is
        // known to be ascending, but proving that here rather than reasoning
        // about it keeps the bound honest if the ordering rule ever moves.
        let mut ids: Vec<&str> = self.sources.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        if ids.len() != before {
            return Err(refuse(
                "the manifest lists the same source id more than once".to_string(),
            ));
        }
        Ok(())
    }
}

fn is_http_locator(path: &str) -> bool {
    path.get(.."http://".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || path
            .get(.."https://".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

/// `[minx, miny, maxx, maxy]`, CRS84, finite, non-degenerate, in range.
fn validate_bbox(id: &str, bbox: [f64; 4]) -> std::result::Result<(), String> {
    if !bbox.iter().all(|v| v.is_finite()) {
        return Err(format!(
            "source '{id}' has a malformed bbox {bbox:?}: every value must be finite"
        ));
    }
    let [minx, miny, maxx, maxy] = bbox;
    if minx >= maxx || miny >= maxy {
        return Err(format!(
            "source '{id}' has a malformed bbox {bbox:?}: expected [minx, miny, maxx, maxy] with \
             minx < maxx and miny < maxy"
        ));
    }
    if !(-180.0..=180.0).contains(&minx)
        || !(-180.0..=180.0).contains(&maxx)
        || !(-90.0..=90.0).contains(&miny)
        || !(-90.0..=90.0).contains(&maxy)
    {
        return Err(format!(
            "source '{id}' has a malformed bbox {bbox:?}: CRS84 bounds are [-180, 180] on \
             longitude and [-90, 90] on latitude"
        ));
    }
    Ok(())
}

/// Streaming SHA-256 over the file at `path`, hex-encoded, plus its byte
/// length — the two measured provenance fields, taken in one pass. Bounded
/// memory ([`HASH_CHUNK_BYTES`] at a time) regardless of the object's size.
pub fn sha256_of_file(path: &Path) -> Result<(String, u64)> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path).map_err(|source| CogError::Open {
        path: path.display().to_string(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|source| CogError::Open {
            path: path.display().to_string(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(SHA256_HEX_LEN);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok((hex, total))
}

/// How closely a manifest's declared bbox must match the COG's own
/// georeferencing for [`verify_source`] to accept it. The manifest is
/// authored from that very transform through `serde_yaml`'s own round-trip
/// of an `f64`, so a match is exact in practice; the tolerance exists only
/// so a last-digit formatting difference is never mistaken for tampering.
const BBOX_TOLERANCE_DEG: f64 = 1e-9;

/// Content verification for one source, against its real bytes: declared
/// `byte_length` and `sha256` must match what is on disk, and the declared
/// `bbox` must match the COG's own georeferencing tags. Returns the parsed
/// [`reader::CogMeta`] so the caller never pays for a second open.
///
/// Blocking by design — every caller runs it on the blocking pool, the same
/// way every other read in this crate does.
pub fn verify_source(source: &ManifestSource, path: &Path) -> Result<reader::CogMeta> {
    let (digest, length) = sha256_of_file(path)?;
    if length != source.byte_length {
        return Err(CogError::MosaicSourceProvenance {
            id: source.id.clone(),
            message: format!(
                "manifest declares byte_length {}, but '{}' is {length} bytes",
                source.byte_length,
                path.display()
            ),
        });
    }
    if digest != source.sha256 {
        return Err(CogError::MosaicSourceProvenance {
            id: source.id.clone(),
            message: format!(
                "manifest declares sha256 {}, but '{}' hashes to {digest}",
                source.sha256,
                path.display()
            ),
        });
    }
    let meta = reader::open(&CogSource::Local(path.to_path_buf()))?;
    if !bbox_matches(source.bbox, meta.extent_crs84) {
        return Err(CogError::MosaicSourceProvenance {
            id: source.id.clone(),
            message: format!(
                "manifest declares bbox {:?}, but '{}' georeferences itself to {:?}; a mosaic \
                 manifest's bboxes are MEASURED from each source's own tags, never declared by \
                 hand",
                source.bbox,
                path.display(),
                meta.extent_crs84
            ),
        });
    }
    Ok(meta)
}

fn bbox_matches(declared: [f64; 4], measured: [f64; 4]) -> bool {
    declared
        .iter()
        .zip(measured.iter())
        .all(|(a, b)| (a - b).abs() <= BBOX_TOLERANCE_DEG)
}

/// What [`author_mosaic_manifest`] wrote, for the CLI to print.
#[derive(Debug, Clone, PartialEq)]
pub struct MosaicAuthorReport {
    pub manifest_path: PathBuf,
    pub sources: Vec<ManifestSource>,
    /// The union of every source's own measured bbox, CRS84.
    pub union_bbox: [f64; 4],
}

/// Authors the sidecar: opens every `inputs` COG through this crate's own
/// reader, measures its bbox/length/digest, sorts by id, validates the
/// result against the very same [`MosaicManifest::validate`] the server will
/// apply, and writes the YAML.
///
/// The id of a source is its file stem. A repeated stem is refused by name
/// rather than silently deduplicated — two files with the same stem in one
/// mosaic is an operator mistake, not a shorthand.
pub fn author_mosaic_manifest(inputs: &[PathBuf], output: &Path) -> Result<MosaicAuthorReport> {
    if inputs.is_empty() {
        return Err(CogError::ManifestInvalid {
            path: output.display().to_string(),
            message: format!("no --source given; a mosaic manifest must list 1..={MAX_SOURCES}"),
        });
    }
    if inputs.len() > MAX_SOURCES {
        return Err(CogError::ManifestInvalid {
            path: output.display().to_string(),
            message: format!(
                "{} --source arguments given, over this driver's bound of {MAX_SOURCES}",
                inputs.len()
            ),
        });
    }

    let output_dir = output.parent().unwrap_or_else(|| Path::new("."));
    let canonical_output_dir = std::fs::canonicalize(output_dir).unwrap_or_else(|_| {
        if output_dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            output_dir.to_path_buf()
        }
    });

    let mut sources = Vec::with_capacity(inputs.len());
    for input in inputs {
        let id = input
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
            .ok_or_else(|| CogError::ManifestInvalid {
                path: output.display().to_string(),
                message: format!(
                    "'{}' has no usable file stem to take a source id from",
                    input.display()
                ),
            })?;
        let (sha256, byte_length) = sha256_of_file(input)?;
        let meta = reader::open(&CogSource::Local(input.clone()))?;
        sources.push(ManifestSource {
            id,
            path: relative_to(input, &canonical_output_dir),
            bbox: meta.extent_crs84,
            byte_length,
            sha256,
        });
    }
    sources.sort_by(|a, b| a.id.cmp(&b.id));

    let manifest = MosaicManifest {
        version: MANIFEST_VERSION,
        sources,
    };
    // Authored documents go through the SAME validation the server applies,
    // so `ingest` can never emit a manifest the server would refuse.
    manifest.validate(output)?;

    let union_bbox =
        manifest
            .sources
            .iter()
            .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |acc, source| {
                [
                    acc[0].min(source.bbox[0]),
                    acc[1].min(source.bbox[1]),
                    acc[2].max(source.bbox[2]),
                    acc[3].max(source.bbox[3]),
                ]
            });

    let yaml = serde_yaml::to_string(&manifest).map_err(|error| {
        CogError::Encode(format!("failed to serialize the mosaic manifest: {error}"))
    })?;
    let document = format!(
        "# Tellurion COG mosaic manifest, authored by `tellurion-ingest cog mosaic`.\n\
         # Every bbox/byte_length/sha256 below was MEASURED from the source object\n\
         # itself -- do not edit them by hand; re-run the command instead.\n\
         # Sources are listed in ascending id order, which IS the composition order:\n\
         # a later id paints over an earlier one wherever its own pixel is opaque.\n\
         {yaml}"
    );
    std::fs::write(output, document).map_err(|source| CogError::Write {
        path: output.display().to_string(),
        source,
    })?;

    Ok(MosaicAuthorReport {
        manifest_path: output.to_path_buf(),
        sources: manifest.sources,
        union_bbox,
    })
}

/// `input` expressed relative to `base` when it sits under it, absolute
/// otherwise — so a manifest written beside its COGs stays relocatable as a
/// directory, while one written elsewhere still resolves.
fn relative_to(input: &Path, base: &Path) -> String {
    let canonical_input = std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    match canonical_input.strip_prefix(base) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => canonical_input.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str) -> ManifestSource {
        ManifestSource {
            id: id.to_string(),
            path: format!("{id}.tif"),
            bbox: [-1.0, -1.0, 1.0, 1.0],
            byte_length: 1302,
            sha256: "a".repeat(64),
        }
    }

    fn manifest(sources: Vec<ManifestSource>) -> MosaicManifest {
        MosaicManifest {
            version: MANIFEST_VERSION,
            sources,
        }
    }

    fn refusal(manifest: &MosaicManifest) -> String {
        match manifest.validate(Path::new("/tmp/m.yaml")) {
            Err(CogError::ManifestInvalid { message, .. }) => message,
            other => panic!("expected ManifestInvalid, got {other:?}"),
        }
    }

    #[test]
    fn one_source_is_the_smallest_accepted_manifest() {
        assert!(manifest(vec![source("a")])
            .validate(Path::new("/tmp/m.yaml"))
            .is_ok());
    }

    #[test]
    fn thirty_two_sources_are_accepted_and_thirty_three_are_refused_by_name() {
        let ids: Vec<String> = (0..MAX_SOURCES).map(|i| format!("s{i:03}")).collect();
        let at_bound = manifest(ids.iter().map(|id| source(id)).collect());
        assert!(
            at_bound.validate(Path::new("/tmp/m.yaml")).is_ok(),
            "exactly {MAX_SOURCES} sources is inside the bound"
        );

        let mut over = at_bound.clone();
        over.sources.push(source("s999"));
        let message = refusal(&over);
        assert!(
            message.contains("33 sources") && message.contains(&MAX_SOURCES.to_string()),
            "the refusal must name the count and the bound: {message}"
        );
    }

    #[test]
    fn an_empty_source_list_is_refused_by_name() {
        let message = refusal(&manifest(vec![]));
        assert!(message.contains("no sources"), "{message}");
    }

    #[test]
    fn a_duplicate_source_id_is_refused_by_name() {
        let message = refusal(&manifest(vec![source("a"), source("a")]));
        assert!(message.contains("duplicate source id 'a'"), "{message}");
    }

    #[test]
    fn sources_out_of_ascending_id_order_are_refused_by_name() {
        let message = refusal(&manifest(vec![source("b"), source("a")]));
        assert!(
            message.contains("ascending id order") && message.contains("composition order"),
            "{message}"
        );
    }

    #[test]
    fn a_malformed_bbox_is_refused_by_name() {
        for bad in [
            [1.0, -1.0, -1.0, 1.0],     // minx >= maxx
            [-1.0, 1.0, 1.0, -1.0],     // miny >= maxy
            [-181.0, -1.0, 1.0, 1.0],   // outside CRS84 longitude
            [-1.0, -91.0, 1.0, 1.0],    // outside CRS84 latitude
            [f64::NAN, -1.0, 1.0, 1.0], // not finite
        ] {
            let mut s = source("a");
            s.bbox = bad;
            let message = refusal(&manifest(vec![s]));
            assert!(
                message.contains("malformed bbox"),
                "bbox {bad:?} should be refused by name: {message}"
            );
        }
    }

    #[test]
    fn a_malformed_sha256_is_refused_by_name() {
        for bad in ["", "abc", &"A".repeat(64), &"z".repeat(64), &"a".repeat(63)] {
            let mut s = source("a");
            s.sha256 = bad.to_string();
            let message = refusal(&manifest(vec![s]));
            assert!(
                message.contains("lowercase hex"),
                "sha256 '{bad}' should be refused by name: {message}"
            );
        }
    }

    #[test]
    fn a_zero_byte_length_is_refused_by_name() {
        let mut s = source("a");
        s.byte_length = 0;
        let message = refusal(&manifest(vec![s]));
        assert!(message.contains("byte_length 0"), "{message}");
    }

    #[test]
    fn a_remote_source_locator_is_refused_by_name() {
        for locator in [
            "http://example.invalid/a.tif",
            "https://example.invalid/a.tif",
            "HTTPS://example.invalid/a.tif",
        ] {
            let mut s = source("a");
            s.path = locator.to_string();
            let message = refusal(&manifest(vec![s]));
            assert!(
                message.contains("local file") && message.contains("sha256"),
                "{message}"
            );
        }
    }

    #[test]
    fn an_unsupported_manifest_version_is_refused_by_name() {
        let mut m = manifest(vec![source("a")]);
        m.version = 2;
        let message = refusal(&m);
        assert!(message.contains("version 2 is not supported"), "{message}");
    }

    #[test]
    fn an_unknown_manifest_field_is_refused_rather_than_ignored() {
        let yaml = "version: 1\nsources:\n  - id: a\n    path: a.tif\n    \
                    bbox: [-1.0, -1.0, 1.0, 1.0]\n    byte_length: 10\n    \
                    sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n    \
                    priority: 7\n";
        match MosaicManifest::parse(Path::new("/tmp/m.yaml"), yaml) {
            Err(CogError::ManifestParse { message, .. }) => {
                assert!(message.contains("priority"), "{message}");
            }
            other => panic!("expected ManifestParse, got {other:?}"),
        }
    }

    #[test]
    fn a_relative_source_path_resolves_against_the_manifests_own_directory() {
        let s = source("a");
        assert_eq!(
            s.resolve_path(Path::new("/data/mosaic")),
            PathBuf::from("/data/mosaic/a.tif")
        );
    }

    #[test]
    fn an_absolute_source_path_is_used_as_is() {
        let mut s = source("a");
        s.path = "/elsewhere/a.tif".to_string();
        assert_eq!(
            s.resolve_path(Path::new("/data/mosaic")),
            PathBuf::from("/elsewhere/a.tif")
        );
    }

    #[test]
    fn sha256_of_file_matches_the_known_digest_of_an_empty_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").unwrap();
        let (digest, length) = sha256_of_file(&path).unwrap();
        assert_eq!(length, 0);
        assert_eq!(
            digest,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// The digest is streamed in [`HASH_CHUNK_BYTES`] blocks — an input
    /// several blocks long must hash exactly as a single-shot digest of the
    /// same bytes, or the chunk loop has an off-by-one nobody would notice
    /// on a small fixture.
    #[test]
    fn sha256_of_file_streams_a_multi_chunk_input_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let bytes: Vec<u8> = (0..HASH_CHUNK_BYTES * 3 + 17)
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&path, &bytes).unwrap();

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let (digest, length) = sha256_of_file(&path).unwrap();
        assert_eq!(length, bytes.len() as u64);
        assert_eq!(digest, expected);
    }
}
