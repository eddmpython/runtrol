//! Carrying an ordinary terminal invocation through the daemon-owned PTY.
//!
//! This module interprets neither provider output nor operator input. It performs the private greeting, opens one
//! exact local invocation, and moves bounded byte chunks in both directions. The daemon owns the process, PTY,
//! current screen, and fan-out. This command is only its first viewer.

use core::time::Duration;
use std::io;
use std::path::Path;

use runtrol_ipc::wire::{Request, Response, TerminalBytes};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::link::Unreachable;

/// How often an active local viewer checks for an actual terminal size change.
const RESIZE_INTERVAL: Duration = Duration::from_millis(200);

/// One provider command surface eligible for transparent terminal bridging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeProvider {
    /// Runtime-discovered provider identity.
    pub id: runtrol_provider::ProviderId,
    /// Manifest-declared executable candidate names.
    pub command_names: Vec<Box<str>>,
}

/// A transparent terminal bridge could not be carried.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BridgeFailure {
    /// No daemon generation could be reached.
    #[error(transparent)]
    Unreachable(#[from] Unreachable),

    /// The private local transport failed.
    #[error(transparent)]
    Transport(#[from] runtrol_ipc::transport::TransportError),

    /// Reading or drawing the invoking terminal failed.
    #[error("the invoking terminal failed: {0}")]
    Terminal(#[from] io::Error),

    /// A private frame could not be encoded or read.
    #[error("the terminal bridge received an unreadable frame: {detail}")]
    Unreadable {
        /// Serialization detail.
        detail: String,
    },

    /// The daemon stopped without completing the terminal stream.
    #[error("the runtrol daemon stopped before the hosted terminal exited")]
    NoAnswer,

    /// The daemon explicitly refused the invocation or later terminal operation.
    #[error("the runtrol daemon refused the terminal bridge: {message}")]
    Refused {
        /// Exact refusal from the daemon.
        message: Box<str>,
    },

    /// The daemon answered with a frame that cannot occur at this point in a terminal stream.
    #[error("the runtrol daemon sent {received} while opening a terminal bridge")]
    Unexpected {
        /// Safe response kind without conversation bytes.
        received: &'static str,
    },
}

/// Discover every usable provider command that can be materialized as a transparent shim.
///
/// # Errors
///
/// Returns [`BridgeFailure`] when the daemon cannot be reached, the wire cannot be agreed, or a provider identity
/// announced by the daemon is invalid.
pub async fn bridge_providers(
    address: &str,
    runtrol: &Path,
) -> Result<Vec<BridgeProvider>, BridgeFailure> {
    let mut connection = crate::link::reach(address, runtrol).await?;
    send(
        &mut connection,
        &Request::Hello {
            wire: runtrol_ipc::WIRE_VERSION,
        },
    )
    .await?;
    match receive(&mut connection).await? {
        Response::Welcome {
            wire, providers, ..
        } if wire == runtrol_ipc::WIRE_VERSION => providers
            .into_iter()
            .filter(|provider| provider.usable && !provider.terminal_commands.is_empty())
            .map(|provider| {
                let id = runtrol_provider::ProviderId::parse(&provider.id).map_err(|error| {
                    BridgeFailure::Unreadable {
                        detail: format!(
                            "the daemon announced an invalid provider identity: {error}"
                        ),
                    }
                })?;
                Ok(BridgeProvider {
                    id,
                    command_names: provider.terminal_commands,
                })
            })
            .collect(),
        Response::Failed(error) => Err(BridgeFailure::Refused {
            message: error.message,
        }),
        other => Err(BridgeFailure::Unexpected {
            received: response_kind(&other),
        }),
    }
}

/// Run one exact provider invocation as the first viewer of the daemon-owned terminal.
///
/// `arguments` are exactly the words after the provider command. `size` is called only by this active foreground
/// viewer and a resize frame is sent only when its geometry changes.
///
/// # Errors
///
/// Returns [`BridgeFailure`] when the daemon cannot be reached, refuses the invocation, sends an invalid frame, or
/// either local byte stream fails.
#[expect(
    clippy::too_many_lines,
    reason = "one local bridge loop orders exact terminal input, output, resize, exit, and refusal frames"
)]
pub async fn bridge<Size>(
    address: &str,
    runtrol: &Path,
    provider: &str,
    arguments: &[String],
    workspace: &str,
    initial_size: (u16, u16),
    mut size: Size,
) -> Result<i32, BridgeFailure>
where
    Size: FnMut() -> io::Result<(u16, u16)>,
{
    let mut connection = crate::link::reach(address, runtrol).await?;
    send(
        &mut connection,
        &Request::Hello {
            wire: runtrol_ipc::WIRE_VERSION,
        },
    )
    .await?;
    match receive(&mut connection).await? {
        Response::Welcome { wire, .. } if wire == runtrol_ipc::WIRE_VERSION => {}
        Response::Failed(error) => {
            return Err(BridgeFailure::Refused {
                message: error.message,
            });
        }
        other => {
            return Err(BridgeFailure::Unexpected {
                received: response_kind(&other),
            });
        }
    }

    send(
        &mut connection,
        &Request::TerminalOpen {
            provider: provider.into(),
            arguments: Some(
                arguments
                    .iter()
                    .map(|argument| argument.as_str().into())
                    .collect(),
            ),
            native: None,
            workspace: workspace.into(),
            cols: initial_size.0,
            rows: initial_size.1,
        },
    )
    .await?;
    let (pid, writable) = match receive(&mut connection).await? {
        Response::TerminalOpened { pid, writable, .. } => (pid, writable),
        Response::Failed(error) => {
            return Err(BridgeFailure::Refused {
                message: error.message,
            });
        }
        other => {
            return Err(BridgeFailure::Unexpected {
                received: response_kind(&other),
            });
        }
    };

    let mut output = tokio::io::stdout();
    // This title is local viewer presentation. It never enters the hosted PTY, shared screen, output ring, another
    // viewer, or the provider transcript. Showing the hosted terminal process rather than the content-named Core
    // executable keeps two simultaneous shell-launched conversations visibly distinct until the provider publishes
    // a title.
    output
        .write_all(&local_process_title(provider, pid))
        .await?;
    output.flush().await?;
    let mut input = tokio::io::stdin();
    let mut input_buffer = [0_u8; 4096];
    let mut last_size = initial_size;
    let mut resize = tokio::time::interval(RESIZE_INTERVAL);
    resize.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if !writable {
            let response = receive(&mut connection).await?;
            if let Some(code) = draw(response, &mut output).await? {
                return Ok(code);
            }
            continue;
        }
        tokio::select! {
            response = receive(&mut connection) => {
                if let Some(code) = draw(response?, &mut output).await? {
                    return Ok(code);
                }
            }
            read = input.read(&mut input_buffer) => {
                let read = read?;
                if read == 0 {
                    return Ok(0);
                }
                let bytes = input_buffer.get(..read).ok_or_else(|| BridgeFailure::Unreadable {
                    detail: "the terminal reader returned more bytes than its buffer".to_owned(),
                })?;
                send(
                    &mut connection,
                    &Request::TerminalInput {
                        bytes: TerminalBytes::from(bytes.to_vec()),
                    },
                )
                .await?;
            }
            _ = resize.tick() => {
                let current = size()?;
                if current != last_size {
                    send(
                        &mut connection,
                        &Request::TerminalResize {
                            cols: current.0,
                            rows: current.1,
                        },
                    )
                    .await?;
                    last_size = current;
                }
            }
        }
    }
}

fn local_process_title(provider: &str, pid: u32) -> Vec<u8> {
    format!("\x1b]0;Runtrol {provider} [{pid}]\x07").into_bytes()
}

async fn draw(
    response: Response,
    output: &mut tokio::io::Stdout,
) -> Result<Option<i32>, BridgeFailure> {
    match response {
        Response::TerminalOutput { bytes } => {
            output.write_all(bytes.as_ref()).await?;
            output.flush().await?;
            Ok(None)
        }
        Response::TerminalLagged {} => {
            output.write_all(b"\x1b[2J\x1b[H").await?;
            output.flush().await?;
            Ok(None)
        }
        Response::TerminalExited { code } => Ok(Some(code)),
        Response::Failed(error) => Err(BridgeFailure::Refused {
            message: error.message,
        }),
        other => Err(BridgeFailure::Unexpected {
            received: response_kind(&other),
        }),
    }
}

async fn send(
    connection: &mut runtrol_ipc::transport::Connection,
    request: &Request,
) -> Result<(), BridgeFailure> {
    let frame = serde_json::to_vec(request).map_err(|error| BridgeFailure::Unreadable {
        detail: error.to_string(),
    })?;
    connection.send(&frame).await?;
    Ok(())
}

async fn receive(
    connection: &mut runtrol_ipc::transport::Connection,
) -> Result<Response, BridgeFailure> {
    let frame = connection.recv().await?.ok_or(BridgeFailure::NoAnswer)?;
    serde_json::from_slice(&frame).map_err(|error| BridgeFailure::Unreadable {
        detail: error.to_string(),
    })
}

fn response_kind(response: &Response) -> &'static str {
    match response {
        Response::Welcome { .. } => "a second welcome",
        Response::TerminalOpened { .. } => "a second terminal-open acknowledgement",
        Response::TerminalOutput { .. } => "terminal output before open acknowledgement",
        Response::TerminalLagged {} => "a lag marker before open acknowledgement",
        Response::TerminalExited { .. } => "a terminal exit before open acknowledgement",
        Response::Failed(_) => "a refusal",
        _ => "a non-terminal response",
    }
}

#[cfg(test)]
mod tests {
    use super::local_process_title;

    #[test]
    fn local_bridge_title_names_the_hosted_process_not_the_shared_core_binary() {
        assert_eq!(
            local_process_title("codex", 30_996),
            b"\x1b]0;Runtrol codex [30996]\x07"
        );
    }
}
