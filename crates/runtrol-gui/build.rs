//! Tauri build metadata for the desktop shell.
//!
//! The frontend bundler imports the canonical brand assets directly from `assets/brand`. This build script
//! therefore has no asset-copying side effect and the packaged window contains only Vite's reproducible
//! output.

fn main() {
    tauri_build::build();
}
