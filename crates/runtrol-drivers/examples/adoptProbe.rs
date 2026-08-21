//! Drives one real installed CLI through the driver layer and reports exactly what the driver answered, with
//! timing. A discovery probe for the conversation lifecycle the daemon relays: list, open (adopt), start, delete.
//! The daemon used to fold every open refusal into "outcome unknown", and this is how the real refusals were
//! read; the delete path was proved against throwaway conversations the same way.
//!
//! Usage:
//!   adoptProbe <acp|codex> <binary> list <cwd> [transport args...]
//!   adoptProbe <acp|codex> <binary> open <cwd> <native-id> [transport args...]
//!   adoptProbe <acp|codex> <binary> start <cwd> [transport args...]           (prints the new native id;
//!       with RUNTROL_PROBE_PROMPT set, sends that one prompt and waits for the turn to end, so the
//!       provider stores the conversation)
//!   adoptProbe <acp|codex> <binary> delete <cwd> <native-id> [delete-argv...]  (ACP: the CLI's own delete command)

use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use runtrol_childproc::{Containment, resolve};
use runtrol_drivers::acp::AcpProvider;
use runtrol_drivers::codex::provider::CodexProvider;
use runtrol_provider::{
    AbsPath, AgentCommand, CloseMode, ContentBlock, Disposition, EventBody, NativeSessionDeletion,
    NativeSessionId, NativeSessionQuery, OpenIntent, Provider, ProviderId, SessionId, TurnEvent,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let kind = arguments.next().ok_or("kind (acp|codex) is required")?;
    let binary = arguments.next().ok_or("the CLI executable is required")?;
    let action = arguments
        .next()
        .ok_or("list, open, start or delete is required")?;
    let workspace = arguments.next().ok_or("the folder is required")?;
    let native = if action == "open" || action == "delete" {
        Some(
            arguments
                .next()
                .ok_or("the native conversation id is required")?,
        )
    } else {
        None
    };
    let rest: Vec<Box<str>> = arguments.map(Into::into).collect();
    // For `delete` on an ACP provider the remaining arguments are the CLI's own delete command (the manifest's
    // `[sessions] delete`); for everything else they are the transport arguments.
    let (transport, delete_command): (Vec<Box<str>>, Vec<Box<str>>) = if action == "delete" {
        (vec!["agent".into(), "stdio".into()], rest)
    } else {
        (rest, Vec::new())
    };
    let provider_id = ProviderId::parse("probe")?;
    let provider: Box<dyn Provider> = match kind.as_str() {
        "acp" => Box::new(AcpProvider::new(
            provider_id,
            resolve(&binary)?,
            Arc::new(Containment::without_any()),
            runtrol_provider::ModelAliases::default(),
            runtrol_provider::SessionCatalogue {
                list: Vec::new(),
                limit_flag: None,
                delete: delete_command,
            },
            transport,
        )),
        "codex" => Box::new(CodexProvider::new(
            provider_id,
            resolve(&binary)?,
            Arc::new(Containment::without_any()),
        )),
        other => return Err(format!("unknown kind {other}").into()),
    };
    let root = AbsPath::canonicalize(&workspace)?;
    // An example, not a driver: this process owns its stdout and the operator running it reads the answer here.
    #[expect(
        clippy::print_stdout,
        reason = "a discovery probe reports what it found to the operator running it"
    )]
    {
        match (action.as_str(), native) {
            ("list", None) => {
                let started = Instant::now();
                let page = provider
                    .native_sessions(NativeSessionQuery {
                        root: Some(root),
                        cursor: None,
                        limit: 100,
                    })
                    .await?;
                println!(
                    "coverage: {:?} ({} ms)",
                    page.coverage,
                    started.elapsed().as_millis()
                );
                for session in &page.sessions {
                    println!(
                        "  {} | {:?} | {} | {}",
                        session.native.as_str(),
                        session.resume,
                        session.title.as_deref().unwrap_or("(no title)"),
                        session.cwd,
                    );
                }
            }
            ("open", Some(native)) => {
                let started = Instant::now();
                let opened = provider
                    .open(OpenIntent {
                        session: SessionId::now(),
                        workspace: root,
                        disposition: Disposition::Resume {
                            native: native.into(),
                        },
                        model: None,
                        reasoning_effort: None,
                        permission: None,
                    })
                    .await;
                match opened {
                    Ok(agent) => {
                        println!("opened in {} ms", started.elapsed().as_millis());
                        drop(agent.close(CloseMode::Kill).await);
                    }
                    Err(error) => {
                        println!("refused in {} ms: {error:?}", started.elapsed().as_millis());
                    }
                }
            }
            ("start", None) => {
                let started = Instant::now();
                let mut agent = provider
                    .open(OpenIntent {
                        session: SessionId::now(),
                        workspace: root,
                        disposition: Disposition::Fresh,
                        model: None,
                        reasoning_effort: None,
                        permission: None,
                    })
                    .await?;
                // The provider's own name for the new conversation arrives on the attachment event.
                let mut native = None;
                for _ in 0..64 {
                    match agent.next().await {
                        Some(Ok(produced)) => {
                            if let EventBody::Attached(attached) = produced.body {
                                native = Some(attached.native.as_str().to_owned());
                                break;
                            }
                        }
                        Some(Err(error)) => return Err(format!("{error:?}").into()),
                        None => break,
                    }
                }
                // One real turn when asked for, because a provider may store a conversation only once
                // something was said in it (measured on Codex 0.148: a fresh thread with no turn has no
                // rollout to list or delete).
                if let Ok(prompt) = std::env::var("RUNTROL_PROBE_PROMPT") {
                    agent
                        .send(AgentCommand::Prompt(vec![ContentBlock::Text(
                            prompt.into(),
                        )]))
                        .await?;
                    let turn_deadline = Instant::now() + std::time::Duration::from_secs(180);
                    let mut events = 0_usize;
                    loop {
                        if Instant::now() > turn_deadline {
                            println!("the turn did not end within 180 s");
                            break;
                        }
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(180),
                            agent.next(),
                        )
                        .await
                        {
                            Ok(Some(Ok(produced))) => {
                                events += 1;
                                if matches!(produced.body, EventBody::Turn(TurnEvent::Ended { .. }))
                                {
                                    println!("turn ended after {events} events");
                                    break;
                                }
                            }
                            Ok(Some(Err(error))) => return Err(format!("{error:?}").into()),
                            Ok(None) | Err(_) => break,
                        }
                    }
                }
                println!(
                    "started in {} ms: {}",
                    started.elapsed().as_millis(),
                    native.as_deref().unwrap_or("(no native id announced)")
                );
                drop(agent.close(CloseMode::Graceful { grace_ms: 2_000 }).await);
            }
            ("delete", Some(native)) => {
                let started = Instant::now();
                let deleted = provider
                    .delete_native_session(NativeSessionDeletion {
                        native: NativeSessionId::new(&native)?,
                        cwd: root,
                    })
                    .await;
                match deleted {
                    Ok(()) => println!("deleted in {} ms", started.elapsed().as_millis()),
                    Err(error) => {
                        println!("refused in {} ms: {error:?}", started.elapsed().as_millis());
                    }
                }
            }
            _ => return Err("unknown action".into()),
        }
    }
    Ok(())
}
