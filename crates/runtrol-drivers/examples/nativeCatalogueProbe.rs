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
    if arguments.next().is_some() {
        return Err("unexpected native catalogue probe argument".into());
    }
    let provider_id = ProviderId::parse("fixture-acp")?;
    let provider = AcpProvider::new(
        provider_id,
        resolve(&fixture)?,
        Arc::new(Containment::without_any()),
        runtrol_provider::ModelAliases::default(),
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
