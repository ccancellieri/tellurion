//! `cog author` and `cog mosaic`: this lane's two authoring commands.
//!
//! `cog author`: converts a plain, single-resolution GeoTIFF into a
//! serving-optimized COG (tiled, Deflate-compressed, with an overview
//! pyramid) via `tellurion_cog::author_cog`. This lane's counterpart to
//! `geopackage create-tables`: authoring owns every physical-layout
//! decision here too — same philosophy as the vector side, just no database
//! involved, since a COG's own file bytes ARE the physical layout.
//!
//! `cog mosaic` (`#254`) is the same philosophy one level up: it authors the
//! manifest SIDECAR a `cog-mosaic` storage is pointed at, by scanning the
//! constituent COGs and MEASURING each one's bbox (from its own
//! georeferencing tags), byte length (from the object) and SHA-256 (from the
//! bytes themselves). Nothing here is declared by hand — a SHA-256
//! transcribed into YAML by a human is an error nobody notices until the day
//! it matters. The server never authors or repairs a manifest; it only
//! validates the one it is given, and refuses by name if it does not hold.
//! See `tellurion_cog::manifest`'s own module doc for the full schema and
//! every bound.

use std::path::PathBuf;

use tellurion_cog::{AuthorOptions, ResampleMode};
use tellurion_core::CollectionDecl;

pub struct AuthorArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    pub tile_size: u32,
    pub resample: ResampleMode,
    pub collection: String,
    pub catalog: String,
    pub storage: String,
}

pub async fn author(args: AuthorArgs) -> anyhow::Result<()> {
    let options = AuthorOptions {
        tile_size: args.tile_size,
        resample: args.resample,
    };
    let report = tellurion_cog::author_cog(&args.input, &args.output, &options)?;

    println!(
        "wrote {} ({} bytes, {} level(s)):",
        args.output.display(),
        report.output_bytes,
        report.level_dims.len()
    );
    for (index, (width, height)) in report.level_dims.iter().enumerate() {
        let label = if index == 0 {
            "main".to_string()
        } else {
            format!("overview {index}")
        };
        println!("  {label}: {width}x{height}");
    }
    println!();
    println!(
        "{}",
        collection_snippet(&args.collection, &args.catalog, &args.storage)?
    );

    Ok(())
}

/// Renders the `storages:`/`collections:` YAML an operator pastes into
/// their config to serve the authored file, in the exact shape
/// `AppConfig` expects — same convention `seed`/`load`/`geopackage seed`
/// already follow. Field values go in through `CollectionDecl`'s own
/// struct fields, never string-interpolated into the YAML text, so a
/// `--collection` id containing YAML-special characters can't corrupt the
/// snippet.
fn collection_snippet(collection: &str, catalog: &str, storage: &str) -> anyhow::Result<String> {
    let mut decl: CollectionDecl = serde_yaml::from_str(
        "id: placeholder\ncatalog: placeholder\nstorage: placeholder\n\
         tiles: { minzoom: 0, maxzoom: 14, caps: {} }\n",
    )?;
    decl.id = collection.to_string();
    decl.catalog = catalog.to_string();
    decl.storage = storage.to_string();
    let collections = crate::yaml_snippet::render_collection_snippet(decl)?;
    Ok(format!(
        "storages:\n  - id: {storage}\n    driver: cog\n    url_env: TELLURION_COG_PATH\n\n{collections}"
    ))
}

pub struct MosaicArgs {
    pub sources: Vec<PathBuf>,
    pub output: PathBuf,
    pub collection: String,
    pub catalog: String,
    pub storage: String,
}

pub async fn mosaic(args: MosaicArgs) -> anyhow::Result<()> {
    let report = tellurion_cog::author_mosaic_manifest(&args.sources, &args.output)?;

    println!(
        "wrote {} ({} source(s)):",
        report.manifest_path.display(),
        report.sources.len()
    );
    // Printed in the manifest's own order, which IS the composition order:
    // a later id paints over an earlier one wherever it is opaque.
    for source in &report.sources {
        println!(
            "  {} -> {} ({} bytes, sha256 {}, bbox {:?})",
            source.id, source.path, source.byte_length, source.sha256, source.bbox
        );
    }
    println!("  union bbox: {:?}", report.union_bbox);
    println!();
    println!(
        "{}",
        mosaic_collection_snippet(&args.collection, &args.catalog, &args.storage)?
    );

    Ok(())
}

/// The `cog-mosaic` counterpart of [`collection_snippet`] — same convention,
/// same `CollectionDecl` round trip (never string interpolation of operator
/// input), just the mosaic driver name and a locator that names the MANIFEST
/// rather than a GeoTIFF.
fn mosaic_collection_snippet(
    collection: &str,
    catalog: &str,
    storage: &str,
) -> anyhow::Result<String> {
    let mut decl: CollectionDecl = serde_yaml::from_str(
        "id: placeholder\ncatalog: placeholder\nstorage: placeholder\n\
         tiles: { minzoom: 0, maxzoom: 14, caps: {} }\n",
    )?;
    decl.id = collection.to_string();
    decl.catalog = catalog.to_string();
    decl.storage = storage.to_string();
    let collections = crate::yaml_snippet::render_collection_snippet(decl)?;
    Ok(format!(
        "storages:\n  - id: {storage}\n    driver: {}\n    url_env: TELLURION_COG_MOSAIC_MANIFEST\n\n{collections}",
        tellurion_cog::MOSAIC_DRIVER_NAME
    ))
}
