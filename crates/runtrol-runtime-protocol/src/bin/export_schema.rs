//! Regenerate the checked public Runtime JSON schema.

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let schema = runtrol_runtime_protocol::public_schema()?;
    let mut encoded = serde_json::to_string_pretty(&schema)?;
    encoded.push('\n');
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join(runtrol_runtime_protocol::PUBLIC_SCHEMA_NAME),
        encoded,
    )?;
    Ok(())
}
