//! The terminal surface on the private wire: open or join a hosted terminal, then carry its bytes both ways
//! on one connection until the CLI ends or the viewer goes.
//!
//! What this is (`docs/terminalSurface.md`): the conversation surface is the provider's own terminal
//! interface, run by the daemon on a pseudo terminal it owns, shown by any number of viewers at once. The
//! daemon side of that is [`runtrol_core::terminal::Terminal`]; this module is the wire around it. It reads
//! no byte for meaning: output is base64 of what the CLI wrote, input is base64 of what the viewer typed.
//!
//! The launch is the manifest's word (`[tui]`): the program the probe resolved, the manifest's `new` or
//! `resume` arguments, its `env` and `env_unset`. Nothing here knows a provider by name.

use std::collections::BTreeMap;
use std::sync::Arc;

use runtrol_core::terminal::{Attachment, Terminal, TerminalLaunch};
use runtrol_ipc::{Request, Response, TerminalBytes};
use runtrol_provider::{AbsPath, ProviderId, TerminalId};
use runtrol_security::Caller;

use crate::compose::Composed;
use crate::dispatch::refuse;
use crate::serve::{SurfaceConnection, write};

/// Every open terminal, by id and by the conversation it shows.
#[derive(Default)]
pub(crate) struct Terminals {
    by_id: BTreeMap<TerminalId, Open>,
    /// Which terminal shows which conversation, so a second open joins the first.
    by_conversation: BTreeMap<(ProviderId, Box<str>), TerminalId>,
}

/// One open terminal and the folder its CLI runs in.
struct Open {
    terminal: Terminal,
    /// The canonical folder, so a join is judged against the folder the conversation really runs in,
    /// never against the folder the joining request happened to name.
    workspace: AbsPath,
}

impl Terminals {
    fn get(&self, id: TerminalId) -> Option<Terminal> {
        self.by_id.get(&id).map(|open| open.terminal.clone())
    }

    /// The terminal already showing this conversation, if it runs in `workspace`.
    fn open_for(
        &self,
        provider: ProviderId,
        native: &str,
        workspace: &AbsPath,
    ) -> Option<(TerminalId, Terminal)> {
        let id = *self.by_conversation.get(&(provider, native.into()))?;
        let open = self.by_id.get(&id)?;
        (open.workspace == *workspace).then(|| (id, open.terminal.clone()))
    }

    fn insert(
        &mut self,
        id: TerminalId,
        key: Option<(ProviderId, Box<str>)>,
        terminal: Terminal,
        workspace: AbsPath,
    ) {
        self.by_id.insert(
            id,
            Open {
                terminal,
                workspace,
            },
        );
        if let Some(key) = key {
            self.by_conversation.insert(key, id);
        }
    }

    fn remove(&mut self, id: TerminalId) {
        self.by_id.remove(&id);
        self.by_conversation.retain(|_, open| *open != id);
    }

    fn len(&self) -> usize {
        self.by_id.len()
    }
}

/// Drop a terminal from the table and tell the owner, which may be waiting on it to drain.
async fn forget(composed: &Composed, id: TerminalId) {
    let open = {
        let mut terminals = composed.terminals.lock().await;
        terminals.remove(id);
        terminals.len()
    };
    composed
        .open_terminals
        .store(open, std::sync::atomic::Ordering::Release);
    composed.terminal_closed.notify_one();
}

/// Serve one terminal request on this connection until the view ends.
///
/// Called after the scope wall admitted the request. Everything after the first answer is the view: output
/// down, input and resizes up, each inbound request judged by the same wall before it acts.
pub(crate) async fn serve(
    connection: &mut SurfaceConnection,
    composed: Arc<Composed>,
    caller: Caller,
    request: Request,
) {
    let opened = match request {
        Request::TerminalOpen {
            provider,
            native,
            workspace,
            cols,
            rows,
        } => {
            open(
                &composed,
                &provider,
                native.as_deref(),
                &workspace,
                cols,
                rows,
            )
            .await
        }
        Request::TerminalAttach {
            terminal,
            cols,
            rows,
        } => attach(&composed, terminal, cols, rows).await,
        _ => Err("not a terminal request".to_owned()),
    };
    let (id, terminal, attachment) = match opened {
        Ok(opened) => opened,
        Err(why) => {
            drop(write(connection, &refuse(&why)).await);
            return;
        }
    };
    let opened = Response::TerminalOpened {
        terminal: id,
        pid: terminal.pid(),
    };
    if write(connection, &opened).await.is_err() {
        return;
    }
    relay(connection, &composed, &caller, id, &terminal, attachment).await;
}

/// Drop the terminal from the table the moment its CLI ends, viewer or no viewer. Without this a
/// conversation whose viewer left first stayed in the table forever, kept the draining rule from ever
/// reaching zero, and answered the next open with an already-ended terminal (measured 2026-08-25).
fn forget_on_exit(composed: Arc<Composed>, id: TerminalId, terminal: &Terminal) {
    let mut exited = terminal.exited();
    tokio::spawn(async move {
        loop {
            if exited.borrow().is_some() {
                break;
            }
            if exited.changed().await.is_err() {
                break;
            }
        }
        forget(&composed, id).await;
    });
}

async fn open(
    composed: &Arc<Composed>,
    provider: &str,
    native: Option<&str>,
    workspace: &str,
    cols: u16,
    rows: u16,
) -> Result<(TerminalId, Terminal, Attachment), String> {
    let id = ProviderId::parse(provider)
        .map_err(|_| format!("{provider:?} is not a provider name runtrol accepts"))?;
    let cwd = AbsPath::canonicalize(workspace)
        .map_err(|error| format!("the workspace {workspace:?} cannot be used: {error}"))?;
    if let Some(native) = native
        && let Some((existing, terminal)) =
            composed.terminals.lock().await.open_for(id, native, &cwd)
    {
        let attachment = terminal.attach().await;
        return Ok((existing, terminal, attachment));
    }
    let declared = composed
        .registry
        .get(id)
        .ok_or_else(|| format!("no provider called {provider}"))?;
    let tui = declared
        .manifest
        .tui
        .as_ref()
        .ok_or_else(|| format!("{provider} declares no terminal interface"))?;
    let arguments: Vec<String> = match native {
        Some(native) => {
            if tui.resume.is_empty() {
                return Err(format!(
                    "{provider} publishes no way to reopen a conversation from the command line"
                ));
            }
            tui.resume
                .iter()
                .map(ToString::to_string)
                .chain(std::iter::once(native.to_owned()))
                .collect()
        }
        None => tui.new.iter().map(ToString::to_string).collect(),
    };
    let mut cache = runtrol_core::ProbeCache::open(composed.home.paths().probe_cache());
    let (program, _probed) =
        runtrol_core::probe_program(&declared.manifest, &[], &mut cache, &composed.containment)
            .await
            .map_err(|error| error.to_string())?;
    let terminal = Terminal::open(&TerminalLaunch {
        program: &program,
        arguments,
        cwd: &cwd,
        env: tui
            .env
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        env_unset: tui.env_unset.iter().map(ToString::to_string).collect(),
        size: runtrol_childproc::PtySize { cols, rows },
    })
    .map_err(|error| error.to_string())?;
    let terminal_id = TerminalId::now();
    let key = native.map(|native| (id, Box::<str>::from(native)));
    let open = {
        let mut terminals = composed.terminals.lock().await;
        terminals.insert(terminal_id, key, terminal.clone(), cwd);
        terminals.len()
    };
    composed
        .open_terminals
        .store(open, std::sync::atomic::Ordering::Release);
    forget_on_exit(Arc::clone(composed), terminal_id, &terminal);
    let attachment = terminal.attach().await;
    Ok((terminal_id, terminal, attachment))
}

async fn attach(
    composed: &Composed,
    id: TerminalId,
    cols: u16,
    rows: u16,
) -> Result<(TerminalId, Terminal, Attachment), String> {
    let terminal = composed
        .terminals
        .lock()
        .await
        .get(id)
        .ok_or_else(|| format!("no open terminal {id}"))?;
    // The newest viewer's size wins. Two viewers of different sizes see the same screen at the smaller one's
    // mercy, which is how a shared terminal has always worked.
    terminal
        .resize(runtrol_childproc::PtySize { cols, rows })
        .await
        .map_err(|error| error.to_string())?;
    let attachment = terminal.attach().await;
    Ok((id, terminal, attachment))
}

/// Carry the view: output down, input and resizes up, until the CLI ends or the viewer goes.
async fn relay(
    connection: &mut SurfaceConnection,
    composed: &Composed,
    caller: &Caller,
    id: TerminalId,
    terminal: &Terminal,
    mut attachment: Attachment,
) {
    if write(
        connection,
        &Response::TerminalOutput {
            bytes: TerminalBytes::from(attachment.snapshot.to_vec()),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    // Copied out: the watch's borrow guard must not live across the write below.
    let already_exited: Option<i32> = *attachment.exited.borrow();
    if let Some(code) = already_exited {
        drop(write(connection, &Response::TerminalExited { code }).await);
        forget(composed, id).await;
        return;
    }
    loop {
        tokio::select! {
            output = attachment.live.recv() => {
                let response = match output {
                    Ok(chunk) => Response::TerminalOutput { bytes: TerminalBytes::from(chunk.to_vec()) },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Re-attach from the screen: the ring is bounded on purpose, and the screen model
                        // holds everything a viewer needs to catch up.
                        let fresh = terminal.attach().await;
                        attachment.live = fresh.live;
                        if write(connection, &Response::TerminalLagged {}).await.is_err() {
                            return;
                        }
                        Response::TerminalOutput { bytes: TerminalBytes::from(fresh.snapshot.to_vec()) }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                };
                if write(connection, &response).await.is_err() {
                    return;
                }
            }
            changed = attachment.exited.changed() => {
                if changed.is_err() {
                    return;
                }
                let exited: Option<i32> = *attachment.exited.borrow();
                if let Some(code) = exited {
                    drop(write(connection, &Response::TerminalExited { code }).await);
                    forget(composed, id).await;
                    return;
                }
            }
            inbound = connection.recv() => {
                let Ok(Some(frame)) = inbound else { return };
                let request = match serde_json::from_slice::<Request>(&frame) {
                    Ok(request) => request,
                    Err(error) => {
                        drop(write(connection, &refuse(&error.to_string())).await);
                        return;
                    }
                };
                // The wall, on every inbound request of the view: a grant that may read a screen is not a
                // grant that may type into it, and the first request's admission says nothing about these.
                if let Err(refusal) = crate::scope::allowed_with_authority(
                    caller,
                    &request,
                    &composed.device_authority,
                ) {
                    drop(write(connection, &refuse(&refusal.to_string())).await);
                    return;
                }
                match request {
                    Request::TerminalInput { bytes } => {
                        if let Err(error) = terminal.input(bytes.as_ref()).await {
                            drop(write(connection, &refuse(&error.to_string())).await);
                            return;
                        }
                    }
                    Request::TerminalResize { cols, rows } => {
                        if let Err(error) = terminal.resize(runtrol_childproc::PtySize { cols, rows }).await {
                            drop(write(connection, &refuse(&error.to_string())).await);
                            return;
                        }
                    }
                    // A view carries terminal traffic only; anything else ends it, said out loud.
                    _ => {
                        drop(write(connection, &refuse("this connection is a terminal view")).await);
                        return;
                    }
                }
            }
        }
    }
}
