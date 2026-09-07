//! Keep the product's exact completion witness for the external Python resource gates.

#[path = "../runtimeFootprint/mod.rs"]
mod runtime_footprint;

use std::io::{BufRead as _, Write as _};
use std::path::Path;

fn main() -> Result<(), runtime_footprint::Error> {
    let args: Vec<_> = std::env::args().collect();
    let home = args
        .get(1)
        .ok_or("usage: runtimeFootprint <home> <Runtime pid>")?;
    let pid = args.get(2).ok_or("missing Runtime pid")?.parse()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let observed = runtime.block_on(async {
        tokio::time::timeout(
            runtime_footprint::STOP_WITHIN,
            runtime_footprint::Observed::open(Path::new(home), pid),
        )
        .await
    })??;
    // Read RSS once before publication to prove every exact member is inspectable.
    let resident = match observed.resident() {
        Ok(resident) => resident,
        Err(error) => {
            runtime.block_on(observed.stop())?;
            return Err(error);
        }
    };
    let members: Vec<_> = observed
        .members
        .iter()
        .map(|identity| {
            serde_json::json!({
                "pid": identity.pid(), "started": identity.started().to_string(),
            })
        })
        .collect();
    let requested = (|| -> Result<(), std::io::Error> {
        let mut output = std::io::stdout().lock();
        writeln!(
            output,
            "{}",
            serde_json::json!({"members": members, "resident": resident})
        )?;
        output.flush()?;
        // EOF and a broken output pipe also ask for cleanup. Losing the Python parent cannot strand its Runtime.
        let mut command = String::new();
        std::io::stdin().lock().read_line(&mut command)?;
        Ok(())
    })();
    runtime.block_on(observed.stop())?;
    requested?;
    println!("{}", serde_json::json!({"completed": true}));
    Ok(())
}
