//! Build-time gate for the `ui` feature: `ui_assets.rs` embeds `ui/dist` via
//! `rust-embed`, whose own "folder does not exist" error names the folder
//! but not the fix. Failing here first, before that macro ever runs, lets
//! this crate say exactly what to run instead.

use std::env;
use std::path::Path;

fn main() {
    // Re-run if the built assets change so a stale embed can't survive a
    // `npm run build` — cargo only tracks the directory's own mtime this
    // way, not every file inside it recursively, but that's enough to
    // catch the common case (the directory is re-created by `vite build`).
    println!("cargo:rerun-if-changed=../../ui/dist");

    if env::var("CARGO_FEATURE_UI").is_err() {
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .expect("cargo always sets CARGO_MANIFEST_DIR for a build script");
    let dist_index = Path::new(&manifest_dir).join("../../ui/dist/index.html");

    if !dist_index.is_file() {
        let build_command = if env::var("CARGO_FEATURE_PUBLIC_DEMO").is_ok() {
            "npm run build:public-demo"
        } else {
            "npm run build"
        };
        panic!(
            "\n\nthe `ui` feature embeds ui/dist, but {} was not found.\n\
             Build the demo UI first:\n\n    cd ui && npm ci && {}\n\n\
             then re-run this build.\n\n",
            dist_index.display(),
            build_command,
        );
    }
}
