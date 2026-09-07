//! Private capabilities and bounded duplex receivers for observing extension input.

use runtrol_provider::ProcessIdentity;
use runtrol_runtime_protocol::{
    WatchWindowInputParams, WindowInputBinding, WindowMirrorOpenParams,
};
use tokio::sync::mpsc;

use super::{
    ConnectionToken, Registered, Registry, RuntimeErrorKind, WindowRegisterParams,
    WindowRegistration, WindowRegistry, WindowRegistryFailure, refused, watch,
};
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_terminal::owner_input::OwnerOffer;
use crate::terminal_surface::{HostedTerminal, ObservedOwner, TerminalOrigin};

pub(crate) struct InputChannel {
    subscription_id: String,
    authority: AuthorizedIntegration,
    sender: mpsc::Sender<OwnerOffer>,
}

pub(crate) struct InputTarget {
    pub(crate) binding: WindowInputBinding,
    pub(crate) shell: ProcessIdentity,
    pub(crate) authority: AuthorizedIntegration,
    pub(crate) sender: mpsc::Sender<OwnerOffer>,
}

/// The receiver owns its queue. Dropping the final receiver makes every cached sender unusable and wakes the
/// existing terminal index even when a transport task was cancelled before its normal return path.
pub(crate) struct InputReceiver {
    pub(crate) subscription_id: String,
    pub(crate) authority: AuthorizedIntegration,
    pub(crate) requests: mpsc::Receiver<OwnerOffer>,
    proof: WatchWindowInputParams,
    changes: watch::Sender<u64>,
}

impl Drop for InputReceiver {
    fn drop(&mut self) {
        self.requests.close();
        self.changes
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

pub(super) fn same_authority(left: &AuthorizedIntegration, right: &AuthorizedIntegration) -> bool {
    left.key == right.key
        && left.grant.key_generation == right.grant.key_generation
        && left.grant.grant_generation == right.grant.grant_generation
}

fn proved<'a>(registry: &'a Registry, params: &WatchWindowInputParams) -> Option<&'a Registered> {
    registry
        .windows
        .get(&params.window_session_id)
        .filter(|entry| {
            entry.descriptor.registration_generation == params.registration_generation
                && entry.owner_token == params.owner_token
                && !params.owner_token.is_empty()
        })
}

fn current_terminal(entry: &Registered, owner: &ObservedOwner) -> Option<ProcessIdentity> {
    if entry.descriptor.registration_generation != owner.registration_generation {
        return None;
    }
    let terminal = entry.descriptor.terminals.iter().find(|terminal| {
        terminal.terminal_key == owner.terminal_key
            && terminal.process_id == owner.shell_pid
            && terminal
                .command
                .as_ref()
                .is_some_and(|command| command.execution_id == owner.execution_id)
    })?;
    let shell = *entry.shells.get(&terminal.terminal_key)?;
    (Some(shell) == owner.shell_identity).then_some(shell)
}

impl WindowRegistry {
    pub(crate) async fn register_authorized(
        &self,
        connection: ConnectionToken,
        params: WindowRegisterParams,
        authority: AuthorizedIntegration,
    ) -> Result<WindowRegistration, WindowRegistryFailure> {
        self.register_inner(connection, params, Some(authority))
            .await
    }

    pub(crate) async fn watch_input(
        &self,
        authority: AuthorizedIntegration,
        params: WatchWindowInputParams,
        subscription_id: String,
        changes: watch::Sender<u64>,
    ) -> Result<InputReceiver, WindowRegistryFailure> {
        let mut registry = self.state.lock().await;
        let entry = proved(&registry, &params)
            .filter(|entry| {
                entry
                    .owner_authority
                    .as_ref()
                    .is_some_and(|registered| same_authority(registered, &authority))
            })
            .ok_or_else(|| {
                refused(
                    RuntimeErrorKind::ScopeDenied,
                    "the private owner registration proof is not current",
                )
            })?;
        if entry
            .input
            .as_ref()
            .is_some_and(|input| !input.sender.is_closed())
        {
            return Err(refused(
                RuntimeErrorKind::ControlConflict,
                "the owner already has an input receiver",
            ));
        }
        let (sender, requests) = mpsc::channel(crate::terminal_surface::MAX_HOSTED_TERMINALS);
        let entry = registry
            .windows
            .get_mut(&params.window_session_id)
            .ok_or_else(|| {
                refused(
                    RuntimeErrorKind::Internal,
                    "the proved owner registration is unavailable",
                )
            })?;
        entry.input = Some(InputChannel {
            subscription_id: subscription_id.clone(),
            authority: authority.clone(),
            sender,
        });
        changes.send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(InputReceiver {
            subscription_id,
            authority,
            requests,
            proof: params,
            changes,
        })
    }

    pub(crate) async fn input_is_current(&self, receiver: &InputReceiver) -> bool {
        let registry = self.state.lock().await;
        proved(&registry, &receiver.proof).is_some_and(|entry| {
            entry.input.as_ref().is_some_and(|input| {
                input.subscription_id == receiver.subscription_id
                    && same_authority(&input.authority, &receiver.authority)
            })
        })
    }

    /// Capture the registry's original birth stamp, never restamp an old mirror after PID reuse.
    pub(crate) async fn mirror_shell(
        &self,
        params: &WindowMirrorOpenParams,
    ) -> Option<ProcessIdentity> {
        let registry = self.state.lock().await;
        let entry = registry.windows.get(&params.window_session_id)?;
        if entry.descriptor.registration_generation != params.registration_generation
            || entry.owner_token != params.owner_token
        {
            return None;
        }
        let terminal = entry.descriptor.terminals.iter().find(|terminal| {
            terminal.terminal_key == params.terminal_key
                && terminal.process_id == params.process_id
                && terminal
                    .command
                    .as_ref()
                    .is_some_and(|command| command.execution_id == params.execution_id)
        })?;
        entry.shells.get(&terminal.terminal_key).copied()
    }

    pub(crate) async fn input_target(&self, hosted: &HostedTerminal) -> Option<InputTarget> {
        let TerminalOrigin::ObservedMirror(owner) = &hosted.origin else {
            return None;
        };
        let registry = self.state.lock().await;
        let entry = registry.windows.get(&owner.window_session_id)?;
        let shell = current_terminal(entry, owner)?;
        let input = entry
            .input
            .as_ref()
            .filter(|input| !input.sender.is_closed())?;
        let Ok(terminal_id) = hosted.id.to_string().parse() else {
            // ok: An identity that cannot be projected must never advertise or acquire an input owner.
            return None;
        };
        Some(InputTarget {
            binding: WindowInputBinding {
                window_session_id: owner.window_session_id.clone(),
                registration_generation: owner.registration_generation,
                host_generation: entry.descriptor.host_generation.clone(),
                terminal_key: owner.terminal_key.clone(),
                execution_id: owner.execution_id.clone(),
                terminal_id,
                process_id: shell.pid(),
            },
            shell,
            authority: input.authority.clone(),
            sender: input.sender.clone(),
        })
    }

    pub(crate) async fn matches_input(
        &self,
        binding: &WindowInputBinding,
        shell: ProcessIdentity,
        subscription_id: &str,
    ) -> bool {
        let registry = self.state.lock().await;
        let Some(entry) = registry.windows.get(&binding.window_session_id) else {
            return false;
        };
        entry.descriptor.registration_generation == binding.registration_generation
            && entry.descriptor.host_generation == binding.host_generation
            && entry.shells.get(&binding.terminal_key) == Some(&shell)
            && entry.input.as_ref().is_some_and(|input| {
                input.subscription_id == subscription_id && !input.sender.is_closed()
            })
            && entry.descriptor.terminals.iter().any(|terminal| {
                terminal.terminal_key == binding.terminal_key
                    && terminal.process_id == Some(binding.process_id)
                    && terminal
                        .command
                        .as_ref()
                        .is_some_and(|command| command.execution_id == binding.execution_id)
            })
    }
}
