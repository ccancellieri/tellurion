//! Build-time gate for the `ui` feature: `ui_assets.rs` embeds one of this
//! crate's generated UI bundles via
//! `rust-embed`, whose own "folder does not exist" error names the folder
//! but not the fix. Failing here first, before that macro ever runs, lets
//! this crate say exactly what to run instead.

use std::env;
use std::path::Path;

fn main() {
    if env::var("CARGO_FEATURE_UI").is_err() {
        return;
    }

    let (dist_dir, build_command) = if env::var("CARGO_FEATURE_PUBLIC_DEMO").is_ok() {
        ("ui/public-demo-dist", "npm run build:public-demo")
    } else {
        ("ui/dist", "npm run build")
    };

    // Re-run if the selected bundle changes so a stale embed can't survive
    // an npm build. Cargo only tracks the directory's own mtime this way,
    // not every file inside it recursively, but Vite re-creates the directory.
    println!("cargo:rerun-if-changed={dist_dir}");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .expect("cargo always sets CARGO_MANIFEST_DIR for a build script");
    let dist_index = Path::new(&manifest_dir).join(dist_dir).join("index.html");

    if !dist_index.is_file() {
        panic!(
            "\n\nthe `ui` feature embeds a crate-local UI bundle, but {} was not found.\n\
             Build the demo UI first:\n\n    cd ui && npm ci && {}\n\n\
             then re-run this build.\n\n",
            dist_index.display(),
            build_command,
        );
    }
}
