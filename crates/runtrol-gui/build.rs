//! Tauri's own build step. It reads `tauri.conf.json` and generates the context the runtime expects.

fn main() {
    tauri_build::build();
}
