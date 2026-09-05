//! One caller-owned spawn operation. Discovery and Git run outside the activation lock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use runtrol_courier::wire::{Answer, MAX_FRAME_BYTES, Request};
use runtrol_ipc::Connection;
use runtrol_provider::{ProviderId, TerminalId};

use crate::Composed;
use crate::isolated_workspace::ownership::SpawnTicket;
use crate::isolated_workspace::{PreparedWorkspace, VerifiedProject, report_cleanup};
use crate::terminal_surface::{SpawnedTerminal, WorkerLaunch};

use super::Admitted;
use super::commands::refused;

pub(super) async fn serve(
    composed: &Arc<Composed>,
    admitted: Admitted,
    connection: &mut Connection,
    request: Request,
) {
    let _operation = crate::terminal_surface::TerminalOperation::begin(composed);
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut pending = None;
    let answer = {
        let running = execute(
            composed,
            admitted,
            request,
            Arc::clone(&cancelled),
            &mut pending,
        );
        tokio::pin!(running);
        tokio::select! {
            answer = &mut running => Some(answer),
            _closed = connection.recv_bounded(MAX_FRAME_BYTES) => {
                cancelled.store(true, Ordering::Release);
                // An in-progress Git operation retains its ownership through bounded completion.
                // The final launch check observes cancellation, then exact cleanup runs below.
                drop(running.await);
                None
            }
        }
    };
    if let Some(ticket) = pending
        && let Some(ended) = composed.courier_gate.cancel_spawn(&ticket).await
        && let Err(error) = composed
            .isolated_workspaces
            .lock()
            .await
            .release_terminal_if_present(&composed.containment, &ended)
            .await
    {
        report_cleanup(&error);
    }
    if let Some(answer) = answer {
        let answer = answer.unwrap_or_else(refused);
        if let Ok(bytes) = serde_json::to_vec(&answer) {
            // A committed worker stays observable if its caller leaves before receiving its identity.
            drop(tokio::time::timeout(super::serve::HELLO_WAIT, connection.send(&bytes)).await);
        }
    }
}

async fn execute(
    composed: &Arc<Composed>,
    admitted: Admitted,
    request: Request,
    cancelled: Arc<AtomicBool>,
    pending: &mut Option<SpawnTicket>,
) -> Result<Answer, String> {
    let Request::Spawn {
        provider,
        model,
        task,
        timeout_ms,
    } = request
    else {
        return Err("the command is not a spawn".to_owned());
    };
    let authority = composed
        .courier_gate
        .spawn_authority(admitted)
        .await
        .map_err(str::to_owned)?;
    let runtime = runtrol_childproc::process_identity(std::process::id())
        .ok_or("the Runtime process could not be identified")?;
    let lead: TerminalId = admitted
        .session
        .to_string()
        .parse()
        .map_err(|_| "invalid lead identity")?;
    let (hosted, ticket, initial) = {
        let terminals = composed.terminals.lock().await;
        let hosted = terminals.hosted(lead).ok_or("the lead terminal ended")?;
        let (ticket, initial) = composed
            .courier_gate
            .reserve_spawn(admitted, runtime, terminals.len(), task, timeout_ms)
            .await?;
        (hosted, ticket, initial)
    };
    *pending = Some(ticket);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let project = VerifiedProject::discover(&hosted.workspace)?;
    crate::terminal_surface::authorize_spawn_project(composed, &authority, &project)
        .map_err(|error| error.to_string())?;
    let id = ProviderId::parse(&provider).map_err(|_| "invalid provider identity")?;
    let (program, arguments) = prepare_launch(composed, id, model.as_deref(), deadline).await?;
    if cancelled.load(Ordering::Acquire) || tokio::time::Instant::now() >= deadline {
        return Err("the spawn request was cancelled or expired".to_owned());
    }
    let PreparedWorkspace {
        workspace,
        base_commit,
        workspace_identity,
    } = composed
        .isolated_workspaces
        .lock()
        .await
        .prepare_terminal(&composed.containment, &ticket, &project)
        .await?;
    let owned = Arc::new(SpawnedTerminal {
        ticket,
        project,
        workspace,
        base_commit,
        workspace_identity,
        initial_message: initial.map(|receipt| receipt.message_id),
    });
    let launch = WorkerLaunch {
        owned: Arc::clone(&owned),
        authority,
        cancelled,
        deadline,
    };
    let (terminal, _process, attachment) = crate::terminal_surface::open_worker(
        composed,
        id,
        &launch,
        program,
        arguments,
        hosted.terminal.size(),
    )
    .await
    .map_err(|error| error.to_string())?;
    drop(attachment);
    Ok(Answer::Spawned {
        session: super::session_of(terminal).map_err(|error| error.to_string())?,
        provider,
        workspace: owned.workspace.to_string(),
        base_commit: owned.base_commit.to_string(),
        spawned_by: admitted.session,
        initial,
    })
}

async fn prepare_launch(
    composed: &Composed,
    id: ProviderId,
    model: Option<&str>,
    deadline: tokio::time::Instant,
) -> Result<(runtrol_childproc::Program, Vec<String>), String> {
    let prepared = tokio::time::timeout_at(
        deadline,
        crate::provider_prepare::prepared_terminal_driver(composed, id),
    )
    .await
    .map_err(|_| "provider preparation exceeded the spawn deadline")?
    .map_err(|error| error.message().to_owned())?;
    let declared = composed
        .registry
        .get(id)
        .ok_or("the provider disappeared")?;
    let tui = declared
        .manifest
        .tui
        .as_ref()
        .ok_or("the provider declares no terminal interface")?;
    let mut arguments: Vec<String> = tui.new.iter().map(ToString::to_string).collect();
    if let Some(model) = model {
        let catalogue = tokio::time::timeout_at(
            deadline,
            crate::provider_prepare::cached_models(composed, id, &prepared),
        )
        .await
        .map_err(|_| "model discovery exceeded the spawn deadline")?
        .map_err(|error| error.to_string())?;
        if !catalogue.contains_model(model) {
            return Err("the selected model is absent from current provider discovery".to_owned());
        }
        arguments.extend(
            prepared
                .driver
                .terminal_model_arguments(model)
                .map_err(|error| error.to_string())?,
        );
    }
    let program = prepared
        .terminal_program
        .ok_or("the prepared provider has no terminal executable")?;
    Ok((program, arguments))
}
