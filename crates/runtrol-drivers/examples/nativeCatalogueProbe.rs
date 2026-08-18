//! Drives the generic ACP native catalogue against a separately built fixture process.

use std::error::Error;
use std::sync::Arc;

use runtrol_childproc::{Containment, resolve};
use runtrol_drivers::acp::AcpProvider;
use runtrol_provider::{
    AbsPath, NativeCatalogueCoverage, NativeSessionQuery, Provider as _, ProviderId,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let fixture = arguments
        .next()
        .ok_or("the ACP fixture executable path is required")?;
    let workspace = arguments
        .next()
        .ok_or("the catalogue workspace path is required")?;
    // Any remaining arguments name the CLI's own session listing command, which is how this probe reaches a real
    // installed CLI whose protocol announces no session capability at all.
    let listing: Vec<Box<str>> = arguments.map(Into::into).collect();
    let listing_named = listing.is_empty();
    let provider_id = ProviderId::parse("fixture-acp")?;
    let provider = AcpProvider::new(
        provider_id,
        resolve(&fixture)?,
        Arc::new(Containment::without_any()),
        runtrol_provider::ModelAliases::default(),
        runtrol_provider::SessionCatalogue {
            list: listing,
            // Left unset so the probe exercises the path a CLI takes when this build does not know how to bound
            // its listing, which is the case that must never report a complete answer.
            limit_flag: None,
        },
        Vec::new(),
    );
    let root = AbsPath::canonicalize(&workspace)?;
    let first = provider
        .native_sessions(NativeSessionQuery {
            root: root.clone(),
            cursor: None,
            limit: 100,
        })
        .await?;
    // With a listing command named, this probe is pointed at a real installed CLI whose history is whatever the
    // operator happens to have. Reporting what came back is the whole point there; the fixture contract below
    // describes one deterministic fixture and cannot describe a real machine.
    if !listing_named {
        // The crate forbids stdout because a driver shares it with a child's protocol stream. This is an example
        // rather than a driver: it is the top-level process, its child owns its own pipes, and showing the
        // operator what a real installed CLI answered is the entire reason this mode exists.
        #[expect(
            clippy::print_stdout,
            reason = "a discovery probe reports what it found to the operator running it"
        )]
        {
            println!("coverage: {:?}", first.coverage);
            println!("sessions: {}", first.sessions.len());
            for session in first.sessions.iter().take(5) {
                println!(
                    "  {} | {} | {}",
                    session.native.as_str(),
                    session.title.as_deref().unwrap_or("(no title)"),
                    session.cwd,
                );
            }
        }
        return Ok(());
    }
    if !matches!(first.coverage, NativeCatalogueCoverage::Complete { .. })
        || first.sessions.len() != 1
        || first.sessions.first().map(|session| session.cwd.as_ref()) != Some(root.as_str())
    {
        return Err("the first official ACP native catalogue page is incomplete".into());
    }
    let second = provider
        .native_sessions(NativeSessionQuery {
            root,
            cursor: first.next_cursor,
            limit: 100,
        })
        .await?;
    if second.sessions.len() != 1 || second.next_cursor.is_some() {
        return Err("the second official ACP native catalogue page is incomplete".into());
    }
    Ok(())
}
