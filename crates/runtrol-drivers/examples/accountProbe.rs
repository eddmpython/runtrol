//! Asks one real installed CLI where the operator's account stands, through the same driver the daemon uses,
//! and prints exactly what came back with the time it took.
//!
//! A discovery probe, and then a maintenance one. Every limit surface here was found by running this against
//! the real binaries rather than by reading about them, and every one of them is a vendor's private extension
//! that will move. When a bar disappears from the sidebar, this says in one line whether the service stopped
//! answering, started answering differently, or was never asked.
//!
//! It builds the driver from the shipped manifest, so what it exercises is the declaration too: a manifest
//! whose pointers no longer match its CLI shows up here as a report with no windows.
//!
//! Usage:
//!   accountProbe                     every shipped service found on this machine
//!   accountProbe <provider-id>...    only the named ones (`claude`, `codex`, `grok`)

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use runtrol_childproc::{Containment, resolve};
use runtrol_drivers::acp::AcpProvider;
use runtrol_drivers::claude::ClaudeProvider;
use runtrol_drivers::codex::provider::CodexProvider;
use runtrol_provider::{AccountStatus, Manifest, Provider};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    let contained_by = Arc::new(Containment::without_any());
    let mut asked = 0;
    for text in runtrol_drivers::MANIFESTS {
        let manifest: Manifest = toml::from_str(text)?;
        let id = manifest.id.as_str().to_owned();
        if !wanted.is_empty() && !wanted.contains(&id) {
            continue;
        }
        asked += 1;
        report(&id, manifest, &contained_by).await;
    }
    if asked == 0 {
        return Err("no shipped service matched; try claude, codex or grok".into());
    }
    Ok(())
}

/// Build the driver this manifest declares, exactly as the daemon's composition does.
///
/// The flag sets are empty because no account surface consults them; a driver that needed one would refuse
/// here rather than answer from a probe that never ran.
fn driver_of(
    manifest: Manifest,
    contained_by: &Arc<Containment>,
) -> Result<Box<dyn Provider>, Box<dyn Error>> {
    let name = manifest
        .bin
        .names
        .first()
        .ok_or("the manifest names no executable")?;
    let program = resolve(name)?;
    let id = manifest.id;
    Ok(match manifest.kind.as_str() {
        "claude-stream-json" => Box::new(ClaudeProvider::new(
            id,
            program,
            Arc::clone(contained_by),
            manifest.models,
            manifest.account,
            BTreeSet::new(),
            BTreeMap::new(),
        )),
        "codex-app-server" => Box::new(CodexProvider::new(id, program, Arc::clone(contained_by))),
        "acp" => Box::new(AcpProvider::new(
            id,
            program,
            Arc::clone(contained_by),
            manifest.models,
            manifest.store,
            manifest.transport.argv,
            manifest.account,
        )),
        other => return Err(format!("this probe does not build a {other} driver").into()),
    })
}

/// Ask one service and print the answer, whatever it is.
#[expect(
    clippy::print_stdout,
    reason = "a discovery probe reports what it found to the operator running it"
)]
async fn report(id: &str, manifest: Manifest, contained_by: &Arc<Containment>) {
    let driver = match driver_of(manifest, contained_by) {
        Ok(driver) => driver,
        Err(error) => {
            println!("{id}: not usable on this machine ({error})");
            return;
        }
    };
    let started = Instant::now();
    let answer = driver.account().await;
    let took = started.elapsed();
    match answer {
        Err(error) => println!("{id}: the driver refused after {took:?}: {error}"),
        Ok(report) => {
            let status = match &report.status {
                AccountStatus::SignedIn => "signed in".to_owned(),
                AccountStatus::SignedOut => "signed out".to_owned(),
                AccountStatus::Unpublished { why } => format!("unpublished ({why})"),
                other => format!("{other:?}"),
            };
            println!(
                "{id}: {status} in {took:?}{}{}",
                report
                    .plan
                    .as_ref()
                    .map(|plan| format!(", plan {plan}"))
                    .unwrap_or_default(),
                report
                    .method
                    .as_ref()
                    .map(|method| format!(" via {method}"))
                    .unwrap_or_default(),
            );
            if let Some(why) = report.limits_unread.as_deref() {
                println!("    no windows: {why}");
            }
            match report.limits.as_ref() {
                None => println!("    no windows reported"),
                Some(limits) => {
                    println!(
                        "    {} window(s){}",
                        limits.windows.len(),
                        if limits.reached {
                            ", a limit is blocking"
                        } else {
                            ""
                        }
                    );
                    for window in &limits.windows {
                        println!(
                            "      {:<28} {:>5} {:>10} {}{}",
                            window.id,
                            window
                                .used_percent
                                .map_or_else(|| "-".to_owned(), |percent| format!("{percent}%")),
                            window
                                .window_minutes
                                .map_or_else(|| "-".to_owned(), |minutes| format!("{minutes}m")),
                            window
                                .scope
                                .as_deref()
                                .or(window.label.as_deref())
                                .unwrap_or(""),
                            if window.governing { " (governing)" } else { "" },
                        );
                    }
                }
            }
            if let Some(tokens) = report.tokens_today {
                println!("    {tokens} tokens today");
            }
        }
    }
}
