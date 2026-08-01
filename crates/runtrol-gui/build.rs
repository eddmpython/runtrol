//! Tauri's own build step, plus getting the brand into the window.
//!
//! # Why the logo is copied rather than committed beside the page
//!
//! `assets/brand/` is the source, and a second copy of a logo is a second logo: the day the mark is redrawn,
//! one of them changes and nobody notices the other. The page cannot reach outside its own root either (the
//! window's content policy allows `self` and nothing else), so the files have to be inside it.
//!
//! Copying at build time settles both. There is one source, the window always has whatever that source says
//! today, and the copies are build output rather than tracked files.

use std::path::{Path, PathBuf};

/// The brand files the window uses, and where they come from.
///
/// Two lockups rather than one because an `<img>` cannot inherit a colour: the single-file lockup works only
/// when the mark is inlined into the document, and this page loads it as an image. The brand's own guidance
/// says to use the pair with a media query, which is what the page does.
const WANTED: [&str; 2] = ["lockup-light.svg", "lockup-dark.svg"];

fn main() {
    tauri_build::build();
    bring_the_brand();
}

/// Copy the brand files this window shows into the page's own root.
fn bring_the_brand() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(repo) = crate_dir.parent().and_then(Path::parent) else {
        // Nothing to copy from, and refusing to build over it would be worse: the window still works, it just
        // shows no mark, and the missing file is visible the moment anybody opens it.
        println!("cargo:warning=cannot locate the repository root, so the brand was not copied");
        return;
    };
    let from = repo.join("assets").join("brand");
    let into = crate_dir.join("ui").join("brand");

    if let Err(error) = std::fs::create_dir_all(&into) {
        println!("cargo:warning=cannot create {}: {error}", into.display());
        return;
    }

    for name in WANTED {
        let source = from.join(name);
        // Rebuilt when the brand changes, so a redrawn mark reaches the window without anybody remembering to
        // clean anything.
        println!("cargo:rerun-if-changed={}", source.display());
        if let Err(error) = std::fs::copy(&source, into.join(name)) {
            println!("cargo:warning=cannot copy {}: {error}", source.display());
        }
    }
}
