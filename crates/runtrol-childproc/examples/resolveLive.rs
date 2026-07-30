//! Resolve the coding CLIs installed on this machine and report what was skipped.
//!
//! An example rather than a test: it depends on what is installed, so it must never gate a build. Run it to
//! see what runtrol would actually start.
//!
//! ```text
//! cargo run -p runtrol-childproc --example resolveLive
//! ```

// The workspace forbids printing to stdout, because a daemon that writes there corrupts whatever protocol
// is on the pipe. An example's whole purpose is to print for a person, and it is never linked into the
// product, so the ban does not apply and the exemption is named here rather than left implicit.
#![expect(
    clippy::print_stdout,
    reason = "an example exists to show a person its output, and is never part of the daemon"
)]

fn main() {
    for program in ["claude", "codex", "node"] {
        match runtrol_childproc::resolve(program) {
            Ok(resolved) => {
                println!("{program}:");
                println!("  runs      {}", resolved.path());
                if !resolved.leading().is_empty() {
                    println!("  with      {:?}", resolved.leading());
                }
                println!("  kind      {:?}", resolved.kind());
                println!("  skipped   {} launcher(s)", resolved.via().len());
                for skipped in resolved.via() {
                    println!("            {skipped}");
                }
                match resolved.kept_launcher() {
                    Some(reason) => println!("  kept      {reason}"),
                    None => println!("  kept      nothing"),
                }
            }
            Err(error) => println!("{program}: {error}"),
        }
        println!();
    }
}
