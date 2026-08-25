//! Drives one real installed CLI through the driver layer and reports exactly what the driver answered, with
//! timing. A discovery probe for the conversation lifecycle the daemon relays: list, open (adopt), start, delete.
//! The daemon used to fold every open refusal into "outcome unknown", and this is how the real refusals were
//! read; the delete path was proved against throwaway conversations the same way; the claude store listing was
//! measured against the 266 conversations on the machine that built it.
//!
//! Usage:
//!   adoptProbe <acp|codex|claude> <binary> list [cwd] [transport args...]   (no cwd: the whole machine)
//!   adoptProbe <acp|codex|claude> <binary> open <cwd> <native-id> [transport args...]
//!   adoptProbe <acp|codex|claude> <binary> start <cwd> [transport args...]  (prints the new native id;
//!       with `RUNTROL_PROBE_PROMPT` set, sends that one prompt and waits for the turn to end, so the
//!       provider stores the conversation)
//!   adoptProbe <acp|codex> <binary> delete <cwd> <native-id> [delete-argv...]  (ACP: the CLI's own delete command)

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use runtrol_childproc::{Containment, resolve};
use runtrol_drivers::acp::AcpProvider;
use runtrol_drivers::claude::ClaudeProvider;
use runtrol_drivers::codex::provider::CodexProvider;
use runtrol_provider::{
    AbsPath, AgentCommand, CloseMode, ContentBlock, Disposition, EventBody, NativeSessionDeletion,
    NativeSessionId, NativeSessionQuery, OpenIntent, Provider, ProviderId, SessionId, TurnEvent,
};

/// How long one prompted turn may take before the probe gives up waiting.
const TURN_DEADLINE: Duration = Duration::from_mins(3);

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let kind = arguments
        .next()
        .ok_or("kind (acp|codex|claude) is required")?;
    let binary = arguments.next().ok_or("the CLI executable is required")?;
    let action = arguments
        .next()
        .ok_or("list, open, start or delete is required")?;
    let workspace = arguments.next();
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
    let provider = provider_of(&kind, &binary, transport, delete_command)?;
    let root = workspace
        .as_deref()
        .map(AbsPath::canonicalize)
        .transpose()?;
    // An example, not a driver: this process owns its stdout and the operator running it reads the answer
    // in the action functions below.
    match (action.as_str(), native) {
        ("list", None) => list(provider.as_ref(), root).await?,
        ("open", Some(native)) => {
            open(
                provider.as_ref(),
                root.ok_or("the folder is required")?,
                native,
            )
            .await;
        }
        ("start", None) => {
            start(provider.as_ref(), root.ok_or("the folder is required")?).await?;
        }
        ("delete", Some(native)) => {
            delete(
                provider.as_ref(),
                root.ok_or("the folder is required")?,
                native,
            )
            .await?;
        }
        _ => return Err("unknown action".into()),
    }
    Ok(())
}

fn provider_of(
    kind: &str,
    binary: &str,
    transport: Vec<Box<str>>,
    delete_command: Vec<Box<str>>,
) -> Result<Box<dyn Provider>, Box<dyn Error>> {
    let provider_id = ProviderId::parse("probe")?;
    let contained_by = Arc::new(Containment::without_any());
    Ok(match kind {
        "acp" => Box::new(AcpProvider::new(
            provider_id,
            resolve(binary)?,
            contained_by,
            runtrol_provider::ModelAliases::default(),
            runtrol_provider::StoreSpec {
                location: Vec::new(),
                format: None,
                list: Vec::new(),
                limit_flag: None,
                delete: delete_command,
            },
            transport,
        )),
        "codex" => Box::new(CodexProvider::new(
            provider_id,
            resolve(binary)?,
            contained_by,
        )),
        // The resume flag is declared confirmed here because this probe measures the listing, not the
        // parser; the daemon probes the flag for real before it offers a row.
        "claude" => Box::new(ClaudeProvider::new(
            provider_id,
            resolve(binary)?,
            contained_by,
            runtrol_provider::ModelAliases::default(),
            None,
            ["--resume".into()].into_iter().collect(),
            std::collections::BTreeMap::new(),
        )),
        other => return Err(format!("unknown kind {other}").into()),
    })
}

#[expect(
    clippy::print_stdout,
    reason = "a discovery probe reports what it found to the operator running it"
)]
async fn list(provider: &dyn Provider, root: Option<AbsPath>) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let mut cursor = None;
    let mut pages = 0_usize;
    let mut total = 0_usize;
    loop {
        let page = provider
            .native_sessions(NativeSessionQuery {
                root: root.clone(),
                cursor,
                limit: 100,
            })
            .await?;
        pages += 1;
        total += page.sessions.len();
        println!(
            "page {pages}: coverage {:?}, {} rows ({} ms so far)",
            page.coverage,
            page.sessions.len(),
            started.elapsed().as_millis()
        );
        for session in &page.sessions {
            println!(
                "  {} | {:?} | {} | {} | {}",
                session.native.as_str(),
                session.resume,
                session.title.as_deref().unwrap_or("(no title)"),
                session.cwd,
                session.updated_at.as_deref().unwrap_or("(no time)"),
            );
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    println!(
        "{total} rows over {pages} pages in {} ms",
        started.elapsed().as_millis()
    );
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "a discovery probe reports what it found to the operator running it"
)]
async fn open(provider: &dyn Provider, root: AbsPath, native: String) {
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
        Ok(mut agent) => {
            println!("opened in {} ms", started.elapsed().as_millis());
            // What the reopened conversation says first: the attachment and, on a provider that hands its
            // history over (Codex items, the claude store's tail), the events that history becomes. Bounded
            // by a short wait, because a resumed session says nothing more until asked.
            let quiet = Duration::from_secs(12);
            let mut kinds: Vec<String> = Vec::new();
            while let Ok(Some(Ok(produced))) = tokio::time::timeout(quiet, agent.next()).await {
                let kind = format!("{:?}", produced.body);
                let name = kind.split(['(', ' ', '{']).next().unwrap_or("?").to_owned();
                if kinds.len() < 8 {
                    let shown: String = kind.chars().take(300).collect();
                    println!("  {shown}");
                    if let EventBody::Notice(notice) = &produced.body {
                        let text: String = notice.payload.as_str().chars().take(400).collect();
                        println!("    {text}");
                    }
                }
                kinds.push(name);
                if kinds.len() >= 200 {
                    break;
                }
            }
            println!("{} events after opening: {}", kinds.len(), kinds.join(", "));
            drop(agent.close(CloseMode::Kill).await);
        }
        Err(error) => {
            println!("refused in {} ms: {error:?}", started.elapsed().as_millis());
        }
    }
}

#[expect(
    clippy::print_stdout,
    reason = "a discovery probe reports what it found to the operator running it"
)]
async fn start(provider: &dyn Provider, root: AbsPath) -> Result<(), Box<dyn Error>> {
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
        let turn_deadline = Instant::now() + TURN_DEADLINE;
        let mut events = 0_usize;
        loop {
            if Instant::now() > turn_deadline {
                println!("the turn did not end within {} s", TURN_DEADLINE.as_secs());
                break;
            }
            match tokio::time::timeout(TURN_DEADLINE, agent.next()).await {
                Ok(Some(Ok(produced))) => {
                    events += 1;
                    if matches!(produced.body, EventBody::Turn(TurnEvent::Ended { .. })) {
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
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "a discovery probe reports what it found to the operator running it"
)]
async fn delete(
    provider: &dyn Provider,
    root: AbsPath,
    native: String,
) -> Result<(), Box<dyn Error>> {
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
    Ok(())
}
