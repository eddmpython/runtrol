//! Provider inventory, capability discovery, and native activity requests.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use runtrol_runtime_protocol::{
    AppScope, GetProviderCapabilitiesParams, JsonRpcId, ListModelsParams, ListNativeSessionsParams,
    MAX_NATIVE_PUBLIC_CURSOR_BYTES, MAX_PAGE_ITEMS, ProviderCapabilityAvailability,
    ProviderCapabilityObservation, ProviderCapabilityProvenance, ProviderList, ProviderUsageList,
    RuntimeErrorKind, RuntimeMethod, RuntimeModelCatalog, RuntimeModelChoice,
    RuntimeProviderCapabilities, RuntimeReasoningChoice, WatchProvidersParams,
    WatchProvidersResult,
};
use tokio::sync::watch;

use crate::Composed;
use crate::runtime_inventory::{RuntimeInventoryFailure, RuntimeSessionCatalogue};
use crate::runtime_native_sessions::{NativeCursorCodec, NativeCursorFailure};

use super::authority::authorized;
use super::connection_state::PublicState;
use super::response::{Answer, EmptyParams, inventory_failure, random_subscription_id};

pub(super) fn providers_list(
    state: &mut PublicState,
    composed: &Composed,
    providers: &ProviderList,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider list parameters are invalid",
        );
    }
    match authorized(state, composed, Some(AppScope::ProviderRead)) {
        Ok(_) => Answer::success(id, providers),
        Err(failure) => Answer::failure(id, failure),
    }
}

/// Where each account stands against its limits, from the supervisor's latest snapshot.
///
/// Answered from a snapshot the serve task publishes when a report passes, so this read costs no lock on the
/// session owner and no provider process. An empty list means nothing has reported since the Runtime started,
/// which a surface says as "no report yet" rather than as a green light.
pub(super) async fn providers_usage(
    state: &mut PublicState,
    composed: &Composed,
    usage: &ProviderUsageList,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider usage parameters are invalid",
        );
    }
    match authorized(state, composed, Some(AppScope::ProviderRead)) {
        Ok(_) => {
            composed.account_probe_wake.all().await;
            Answer::success(id, usage)
        }
        Err(failure) => Answer::failure(id, failure),
    }
}

pub(super) async fn providers_watch(
    state: &mut PublicState,
    composed: &Composed,
    updates: &watch::Sender<Arc<ProviderList>>,
    usage: &watch::Receiver<Arc<ProviderUsageList>>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<WatchProvidersParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider watch parameters are invalid",
        );
    }
    let authority = match authorized(state, composed, Some(AppScope::ProviderRead)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    composed.account_probe_wake.all().await;
    let subscription_id = match random_subscription_id() {
        Ok(subscription_id) if !subscription_id.is_empty() => subscription_id,
        Ok(_) | Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "Runtime could not allocate a provider subscription identity",
            );
        }
    };
    let provider_updates = updates.subscribe();
    let snapshot = provider_updates.borrow().as_ref().clone();
    Answer::watching_providers(
        id,
        &WatchProvidersResult {
            subscription_id,
            snapshot,
        },
        provider_updates,
        usage.clone(),
        authority,
    )
}

pub(super) async fn get_provider_capabilities(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &crate::serve::DiscoveryGates,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<GetProviderCapabilitiesParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider capability parameters are invalid",
        );
    };
    if let Err(failure) = authorized(state, composed, Some(AppScope::ProviderRead)) {
        return Answer::failure(id, failure);
    }
    let Ok(provider_id) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let discovered = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            let _lane = discovering.lane(provider_id).await.lock_owned().await;
            crate::provider_prepare::driver(composed, provider_id).await
        },
    )
    .await;
    let driver = match discovered {
        Ok(Ok(driver)) => driver,
        Ok(Err(_)) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider could not supply structural capabilities",
            );
        }
        Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::RuntimeUnavailable,
                "provider capability discovery exceeded its bounded deadline",
            );
        }
    };
    if let Err(failure) = authorized(state, composed, Some(AppScope::ProviderRead)) {
        return Answer::failure(id, failure);
    }
    Answer::success(
        id,
        &provider_capabilities(params.provider_id, driver.capabilities()),
    )
}

pub(super) fn provider_capabilities(
    provider_id: runtrol_runtime_protocol::ProviderId,
    capabilities: runtrol_provider::ProviderCapabilities,
) -> RuntimeProviderCapabilities {
    RuntimeProviderCapabilities {
        provider_id,
        freshness: runtrol_runtime_protocol::CapabilityFreshness::Current,
        fresh_session: provider_capability(capabilities.fresh_session),
        resume: provider_capability(capabilities.resume),
        structured_events: provider_capability(capabilities.structured_events),
        interrupt: provider_capability(capabilities.interrupt),
        approvals: provider_capability(capabilities.approvals),
        cooling: provider_capability(capabilities.cooling),
        native_session_catalogue: provider_capability(capabilities.native_session_catalogue),
        set_model: Some(provider_capability(capabilities.set_model)),
        set_reasoning_effort: Some(provider_capability(capabilities.set_reasoning_effort)),
        native_session_delete: Some(provider_capability(capabilities.native_session_delete)),
        native_session_archive: Some(provider_capability(capabilities.native_session_archive)),
    }
}

fn provider_capability(
    capability: runtrol_provider::ProviderCapability,
) -> ProviderCapabilityObservation {
    ProviderCapabilityObservation {
        availability: match capability.state {
            runtrol_provider::ProviderCapabilityState::Available => {
                ProviderCapabilityAvailability::Available
            }
            runtrol_provider::ProviderCapabilityState::Unsupported => {
                ProviderCapabilityAvailability::Unsupported
            }
            runtrol_provider::ProviderCapabilityState::Unknown => {
                ProviderCapabilityAvailability::Unknown
            }
        },
        provenance: capability.source.map(|source| match source {
            runtrol_provider::ProviderCapabilitySource::OfficialProtocol => {
                ProviderCapabilityProvenance::OfficialProtocol
            }
            runtrol_provider::ProviderCapabilitySource::OfficialCli => {
                ProviderCapabilityProvenance::OfficialCli
            }
            runtrol_provider::ProviderCapabilitySource::DriverContract => {
                ProviderCapabilityProvenance::DriverContract
            }
        }),
        why: capability.why.map(String::from),
    }
}

pub(super) async fn list_models(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &crate::serve::DiscoveryGates,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<ListModelsParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "model catalogue parameters are invalid",
        );
    };
    if let Err(failure) = authorized(state, composed, Some(AppScope::ModelRead)) {
        return Answer::failure(id, failure);
    }
    let Ok(provider_id) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let discovered = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            let _lane = discovering.lane(provider_id).await.lock_owned().await;
            let prepared = crate::provider_prepare::prepared_driver(composed, provider_id)
                .await
                .map_err(|_| ())?;
            // Memoized against the exact binary for a bounded moment, so a picker opening twice
            // costs one provider spawn, not two.
            crate::provider_prepare::cached_models(composed, provider_id, &prepared)
                .await
                .map_err(|_| ())
        },
    )
    .await;
    match discovered {
        Ok(Ok(catalogue)) => Answer::success(id, &model_catalogue(catalogue)),
        Ok(Err(())) => Answer::plain(
            id,
            RuntimeErrorKind::ProviderUnavailable,
            "the selected provider could not supply a model catalogue",
        ),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "model catalogue discovery exceeded its bounded deadline",
        ),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "native discovery keeps authority, provider preparation, cursor binding, and managed-session merging explicit"
)]
pub(super) async fn list_native_sessions(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &crate::serve::DiscoveryGates,
    native_cursors: &NativeCursorCodec,
    managed: &RuntimeSessionCatalogue,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<ListNativeSessionsParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "native session catalogue parameters are invalid",
        );
    };
    if params
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.len() > MAX_NATIVE_PUBLIC_CURSOR_BYTES)
    {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the native session catalogue cursor is oversized",
        );
    }
    let authority = match authorized(state, composed, Some(AppScope::SessionNativeDiscover)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    // A named folder still has to be one this integration holds. No folder means the machine, and
    // there is nothing to authorize against: the Runtime endpoint is owner-only local, the phone
    // speaks a different wire that has no native-discovery request at all, and the managed session
    // index already made exactly this move for exactly this reason (`runtime_inventory::authorized`,
    // folderless rule in `docs/runtimeProtocol.md`). What remains bounded is what the caller is
    // shown: every returned row is re-checked below before it reaches anyone.
    let selected_root = match params.root.as_deref() {
        Some(requested) => match crate::runtime_inventory::authorized_root(&authority, requested) {
            Ok(root) => Some(root),
            Err(failure) => return inventory_failure(id, failure),
        },
        None => None,
    };
    let approved_roots = match crate::runtime_inventory::authorized_roots(&authority) {
        Ok(roots) => roots,
        Err(failure) => return inventory_failure(id, failure),
    };
    let Ok(provider) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let discovered = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            // Held only through preparation, which is the region the probe-cache atomicity argument covers.
            // The listing itself spawns a whole CLI and can take seconds; holding this mutex across it queued
            // every provider's catalogue behind whichever CLI was slowest (measured 2026-08-19: a just-opened
            // folder's conversations arrived after 13~17 s serialized, ~1.2 s not). Concurrency of the listing
            // children is bounded by the semaphore below instead.
            let prepared = {
                let _lane = discovering.lane(provider).await.lock_owned().await;
                crate::provider_prepare::prepared_driver(composed, provider)
                    .await
                    .map_err(|_| NativeDiscoveryFailure::Provider)?
            };
            let _listing = discovering
                .listing
                .acquire()
                .await
                .map_err(|_| NativeDiscoveryFailure::Provider)?;
            // A driver that cannot enumerate the machine is never handed a folderless query. It
            // would have to either invent a folder or answer one folder's worth as if it were all,
            // and the second is how a partial list comes to read as complete.
            if selected_root.is_none() && !prepared.driver.enumerates_machine() {
                return Err(NativeDiscoveryFailure::RootRequired);
            }
            let opened = params
                .cursor
                .as_deref()
                .map(|cursor| {
                    native_cursors.open(
                        &authority,
                        provider,
                        selected_root.as_ref(),
                        prepared.binary_identity,
                        cursor,
                    )
                })
                .transpose()
                .map_err(NativeDiscoveryFailure::Cursor)?;
            let catalogue = prepared
                .driver
                .native_sessions(runtrol_provider::NativeSessionQuery {
                    root: selected_root.as_ref().map(|root| root.path.clone()),
                    cursor: opened.as_ref().map(|cursor| cursor.provider_cursor.clone()),
                    limit: MAX_PAGE_ITEMS,
                })
                .await
                .map_err(|_| NativeDiscoveryFailure::Provider)?;
            let next = catalogue.next_cursor.clone();
            let mut public = crate::runtime_native_sessions::authorize_catalogue(
                native_cursors,
                &authority,
                selected_root.as_ref(),
                &approved_roots,
                managed,
                provider,
                prepared.binary_identity,
                catalogue,
            )
            .map_err(NativeDiscoveryFailure::Inventory)?;
            if let Some(next) = next {
                public.next_cursor = Some(
                    native_cursors
                        .seal(
                            &authority,
                            provider,
                            selected_root.as_ref(),
                            prepared.binary_identity,
                            &next,
                            opened.as_ref(),
                        )
                        .map_err(NativeDiscoveryFailure::Cursor)?,
                );
            }
            Ok(public)
        },
    )
    .await;
    match discovered {
        Ok(Ok(catalogue)) => Answer::success(id, &catalogue),
        Ok(Err(NativeDiscoveryFailure::Cursor(failure))) => cursor_failure(id, failure),
        Ok(Err(NativeDiscoveryFailure::Inventory(failure))) => inventory_failure(id, failure),
        Ok(Err(NativeDiscoveryFailure::Provider)) => Answer::plain(
            id,
            RuntimeErrorKind::ProviderUnavailable,
            "the selected provider could not supply a native session catalogue",
        ),
        // Named as its own failure so a caller can act on it: ask this provider per folder
        // instead. A generic refusal would leave it guessing which of the two it was.
        Ok(Err(NativeDiscoveryFailure::RootRequired)) => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "this provider lists conversations one workspace root at a time, so a root is required",
        ),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "native session discovery exceeded its bounded deadline",
        ),
    }
}

enum NativeDiscoveryFailure {
    Cursor(NativeCursorFailure),
    Inventory(RuntimeInventoryFailure),
    Provider,
    /// The caller asked about the machine and this provider only answers about a folder.
    RootRequired,
}

fn cursor_failure(id: JsonRpcId, failure: NativeCursorFailure) -> Answer {
    match failure {
        NativeCursorFailure::Invalid | NativeCursorFailure::Expired => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the native session catalogue cursor is invalid, expired, or outside this context",
        ),
        NativeCursorFailure::TooManyPages => Answer::plain(
            id,
            RuntimeErrorKind::ResourceExhausted,
            "the native session catalogue exceeded its bounded page walk",
        ),
        NativeCursorFailure::Internal => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not protect the native session catalogue cursor",
        ),
    }
}

pub(super) fn model_catalogue(catalogue: runtrol_provider::ModelCatalog) -> RuntimeModelCatalog {
    match catalogue {
        runtrol_provider::ModelCatalog::Known { models } => RuntimeModelCatalog::Known {
            models: models.into_iter().map(model_choice).collect(),
        },
        runtrol_provider::ModelCatalog::Aliases {
            aliases,
            reasoning_efforts,
            why,
        } => RuntimeModelCatalog::Aliases {
            aliases: aliases.into_iter().map(String::from).collect(),
            reasoning_efforts: reasoning_efforts
                .into_iter()
                .map(reasoning_choice)
                .collect(),
            why: String::from(why),
        },
        runtrol_provider::ModelCatalog::Partial {
            aliases,
            models,
            reasoning_efforts,
            why,
        } => RuntimeModelCatalog::Partial {
            aliases: aliases.into_iter().map(String::from).collect(),
            models: models.into_iter().map(model_choice).collect(),
            reasoning_efforts: reasoning_efforts
                .into_iter()
                .map(reasoning_choice)
                .collect(),
            why: String::from(why),
        },
        runtrol_provider::ModelCatalog::Unknown { why } => RuntimeModelCatalog::Unknown {
            why: String::from(why),
        },
        runtrol_provider::ModelCatalog::Unsupported { why } => RuntimeModelCatalog::Unsupported {
            why: String::from(why),
        },
        _ => RuntimeModelCatalog::Unknown {
            why: "the provider returned catalogue coverage unsupported by this Runtime".to_owned(),
        },
    }
}

fn model_choice(choice: runtrol_provider::ModelChoice) -> RuntimeModelChoice {
    RuntimeModelChoice {
        id: String::from(choice.id),
        display_name: String::from(choice.display_name),
        description: String::from(choice.description),
        is_default: choice.is_default,
        reasoning_efforts: choice
            .reasoning_efforts
            .into_iter()
            .map(|effort| RuntimeReasoningChoice {
                id: String::from(effort.id),
                description: String::from(effort.description),
            })
            .collect(),
    }
}

fn reasoning_choice(choice: runtrol_provider::ReasoningChoice) -> RuntimeReasoningChoice {
    RuntimeReasoningChoice {
        id: String::from(choice.id),
        description: String::from(choice.description),
    }
}

pub(crate) async fn observe_native_activity(
    composed: &Arc<Composed>,
    discovering: &crate::serve::DiscoveryGates,
    provider: runtrol_provider::ProviderId,
) -> Result<Result<runtrol_provider::NativeProcessActivity, ()>, tokio::time::error::Elapsed> {
    tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            let _lane = discovering.lane(provider).await.lock_owned().await;
            if let Some(activity) = discovering.cached_native_activity(provider).await {
                return Ok(activity);
            }
            let prepared = crate::provider_prepare::prepared_driver(composed, provider).await;
            let prepared = prepared.map_err(|_| ())?;
            let activity = prepared
                .driver
                .native_process_activity()
                .await
                .map_err(|_| ())?;
            let turn_ended = discovering
                .remember_native_activity(provider, activity.clone())
                .await;
            if turn_ended {
                composed.account_probe_wake.provider(provider).await;
            }
            Ok(activity)
        },
    )
    .await
}

/// Act on what a provider says it has open by binding each conversation to the process holding it.
///
/// A fresh provider terminal starts before the provider mints its native identity. Its own cheap process
/// roster is the provider-neutral, content-free proof that binds that identity back to the exact PTY.
/// Publishing the table change makes every window replace the project placeholder with the provider title
/// without opening a second process or parsing the screen. An external process is only observed here. Its
/// terminal renderer is allocated later, when the first viewer explicitly opens that conversation.
pub(crate) async fn reconcile_native_activity(
    composed: &Arc<Composed>,
    provider: runtrol_provider::ProviderId,
    activity: &runtrol_provider::NativeProcessActivity,
) {
    // A package-manager launcher may remain as the PTY root while the provider executable that owns the native
    // conversation runs below it. Capture once per provider observation, never once per binding. If the operating
    // system refuses the ancestry view, exact root-PID binding remains available and descendant attribution closes.
    let needs_process_tree = composed
        .terminals
        .lock()
        .await
        .needs_process_tree(provider, activity);
    // The same walk answers which window owns which live process, so the focus targets are computed from this
    // capture rather than a second one. The proof is repeated when the live process set changed or the last one
    // aged out, never on every quarter-second round: a process-table capture and a window walk each round would be
    // idle work the CPU ratchet forbids.
    let shells = composed.windows.observed_shells().await;
    let live_pids: std::collections::BTreeSet<u32> = activity
        .processes
        .iter()
        .filter(|process| activity.live.contains(&process.native))
        .map(|process| process.pid)
        .collect();
    let now_ms = runtrol_provider::WallMs::now().as_millis();
    let wants_focus = !live_pids.is_empty() && {
        let proofs = composed.focus_proofs.lock().await;
        proofs.get(&provider).is_none_or(|proof| {
            proof.live_pids != live_pids
                || now_ms.saturating_sub(proof.taken_at_ms)
                    >= crate::native_focus::FOCUS_PROOF_MAX_AGE_MS
        })
    };
    let process_tree = if needs_process_tree || wants_focus {
        match runtrol_childproc::ProcessTree::capture() {
            Ok(tree) => Some(tree),
            Err(error) => {
                report_process_tree_failure(&error);
                None
            }
        }
    } else {
        None
    };
    if live_pids.is_empty() || wants_focus {
        let targets = process_tree
            .as_ref()
            .map_or_else(std::collections::BTreeMap::new, |tree| {
                let mut targets = crate::native_focus::window_targets(activity, &shells, tree);
                // A conversation no registered window owns may still sit in a terminal host with a window of its
                // own on this desktop; the nearest ancestor owning one is what a click brings forward.
                let desktop =
                    crate::native_focus::desktop_targets(activity, tree, &targets, &|chain| {
                        match runtrol_childproc::os_window::locate_window(chain, "") {
                            runtrol_childproc::os_window::Located::Found(owner) => Some(owner),
                            _ => None,
                        }
                    });
                targets.extend(desktop);
                targets
            });
        let mut focus = composed.focus_targets.lock().await;
        focus.retain(|(owner, _), _| *owner != provider);
        for (native, target) in targets {
            focus.insert((provider, native), target);
        }
        composed.focus_proofs.lock().await.insert(
            provider,
            crate::native_focus::FocusProof {
                live_pids,
                taken_at_ms: now_ms,
            },
        );
    }
    // One process can structurally own several provider conversations, as a multiplexed editor app server does. A
    // single-screen terminal cannot be assigned to an arbitrary one of them. Bind only an unambiguous process-to-
    // conversation answer; a later provider observation may narrow it.
    {
        let mut terminals = composed.terminals.lock().await;
        let processes = unambiguous_processes(activity);
        let conflicts = terminals.bind_native_processes(
            &composed.native_claims,
            process_tree.as_ref(),
            provider,
            processes
                .iter()
                .map(|process| (process.pid, process.native.as_str())),
        );
        // One process whose new conversation is already claimed elsewhere must not turn the complete provider
        // answer into an error. Its workspace batch keeps its previous bindings, independent workspaces still
        // reconcile, and the same structural observation retries after the conflicting claim ends.
        for (pid, conflict) in conflicts {
            report_binding_conflict(provider, pid, conflict);
        }
    }
}

/// Process bindings that identify one conversation rather than a multiplexed set.
///
/// A provider app server may hold several native conversations under one PID. That is valid provider ownership but
/// cannot identify which one a single-screen terminal is drawing. Returning none for that PID prevents stable sort
/// order from silently choosing and repeatedly rekeying the terminal to an arbitrary conversation.
pub(super) fn unambiguous_processes(
    activity: &runtrol_provider::NativeProcessActivity,
) -> Vec<&runtrol_provider::NativeProcessBinding> {
    let mut by_process = BTreeMap::new();
    for process in &activity.processes {
        by_process
            .entry(process.pid)
            .and_modify(
                |known: &mut Option<&runtrol_provider::NativeProcessBinding>| {
                    if known
                        .as_ref()
                        .is_some_and(|known| known.native != process.native)
                    {
                        *known = None;
                    }
                },
            )
            .or_insert(Some(process));
    }
    by_process.into_values().flatten().collect()
}

pub(super) async fn native_activity(
    state: &mut PublicState,
    composed: &Arc<Composed>,
    discovering: &crate::serve::DiscoveryGates,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) =
        serde_json::from_value::<runtrol_runtime_protocol::NativeActivityParams>(params)
    else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the native activity request is not shaped as this Runtime accepts",
        );
    };
    if let Err(failure) = authorized(state, composed, Some(AppScope::SessionNativeDiscover)) {
        return Answer::failure(id, failure);
    }
    let Ok(provider) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let walked = observe_native_activity(composed, discovering, provider).await;
    match walked {
        Ok(Ok(activity)) => {
            reconcile_native_activity(composed, provider, &activity).await;
            let attachable = attachable_native_sessions(&activity);
            let focusable = focusable_native_sessions(composed, provider).await;
            Answer::success(
                id,
                &runtrol_runtime_protocol::NativeActivity {
                    provider_id: runtrol_runtime_protocol::ProviderId::new(provider.as_str()),
                    live: activity.live.iter().map(ToString::to_string).collect(),
                    attachable,
                    focusable,
                    active: activity.active.iter().map(ToString::to_string).collect(),
                },
            )
        }
        Ok(Err(())) => Answer::plain(
            id,
            RuntimeErrorKind::ProviderUnavailable,
            "the selected provider could not say what it wrote lately",
        ),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "naming what was written lately exceeded its bounded deadline",
        ),
    }
}

/// Show a live conversation where it actually runs: ask the window that owns its terminal to show it, and bring
/// that window forward.
///
/// This is the one thing Runtrol can do for a conversation it does not own, and it is deliberately not a route into
/// opening, attaching, or resuming one: nothing here touches the terminal table or the native claim registry. Like
/// a reveal, it moves a window on this machine's desktop, so only a window on this machine may ask for it.
pub(super) async fn focus_native(
    state: &mut PublicState,
    composed: &Arc<Composed>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<runtrol_runtime_protocol::NativeFocusParams>(params)
    else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the native focus request is not shaped as this Runtime accepts",
        );
    };
    if let Err(failure) = authorized(state, composed, Some(AppScope::SessionNativeDiscover)) {
        return Answer::failure(id, failure);
    }
    let Ok(provider) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the provider identity is invalid",
        );
    };
    let Some(from) = composed.windows.session_id_of(state.token()).await else {
        return Answer::plain(
            id,
            RuntimeErrorKind::PresenceRequired,
            "only a VS Code window registered on this machine can ask to focus a conversation",
        );
    };
    let target = composed
        .focus_targets
        .lock()
        .await
        .get(&(provider, params.native_session_id.clone()))
        .cloned();
    let (window_session_id, terminal_key) = match target {
        Some(crate::native_focus::FocusTarget::Window {
            window_session_id,
            terminal_key,
        }) => (window_session_id, terminal_key),
        // The terminal host's own window: brought forward directly, no window to ask and nothing to deliver.
        Some(crate::native_focus::FocusTarget::Desktop { process_ids }) => {
            let foreground = super::window_requests::bring_forward_processes(process_ids).await;
            return Answer::success(
                id,
                &runtrol_runtime_protocol::NativeFocusResult {
                    delivered: true,
                    foreground,
                },
            );
        }
        None => {
            return Answer::plain(
                id,
                RuntimeErrorKind::CapabilityUnavailable,
                "no window is proved to own that conversation's terminal",
            );
        }
    };
    let Some(result) = super::window_requests::show_at_owner(
        composed,
        Some(from),
        &window_session_id,
        &terminal_key,
    )
    .await
    else {
        return Answer::plain(
            id,
            RuntimeErrorKind::CapabilityUnavailable,
            "the window that owned that conversation's terminal is gone",
        );
    };
    Answer::success(
        id,
        &runtrol_runtime_protocol::NativeFocusResult {
            delivered: result.delivered,
            foreground: result.foreground,
        },
    )
}

/// Live conversations a registered window can show, as the last observation proved.
///
/// This is a separate answer from `attachable`, and deliberately so: a window can show a terminal it observes
/// whether or not anything can be mirrored or attached from it, and a row that says so is telling the truth about
/// the one thing Runtrol can actually do for it.
async fn focusable_native_sessions(
    composed: &Arc<Composed>,
    provider: runtrol_provider::ProviderId,
) -> Vec<String> {
    composed
        .focus_targets
        .lock()
        .await
        .keys()
        .filter(|(owner, _)| *owner == provider)
        .map(|(_, native)| native.clone())
        .collect()
}

/// Live conversations for which the exact owning process publishes a safe terminal route.
///
/// The route itself remains daemon-private. Public clients need only the provider-qualified native identity
/// so they can offer `terminals/open` without guessing a process, command, or provider capability.
///
/// The only route is the provider's own attachment: a console another terminal host owns is never joined (an
/// arbitrary external terminal is focus-only, `PLAN-02`), so `attachable` and `focusable` never name the same
/// conversation for the same reason.
pub(super) fn attachable_native_sessions(
    activity: &runtrol_provider::NativeProcessActivity,
) -> Vec<String> {
    activity
        .processes
        .iter()
        .filter(|process| {
            activity.live.contains(&process.native)
                && matches!(
                    process.terminal_access,
                    runtrol_provider::NativeTerminalAccess::Official { .. }
                )
        })
        .map(|process| process.native.to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The answer after the provider agreed, with Runtrol's own bookkeeping done.
///
/// A deleted conversation must not linger as a nameless Runtrol pointer: the pointer names nothing and
/// the operator can neither open it nor delete it again. Measured 2026-08-25: two such rows sat in the
/// sidebar after two deletions. An archive keeps its pointers; the conversation still exists.
#[expect(
    clippy::print_stderr,
    reason = "a detached inventory refresh has no waiting request to answer; stderr is the daemon's existing operational failure channel"
)]
pub(super) fn schedule_provider_inventory_refresh(
    providers: watch::Sender<Arc<ProviderList>>,
    composed: Arc<Composed>,
) {
    drop(tokio::spawn(async move {
        match crate::runtime_inventory::providers_in_background(composed).await {
            Ok(Some(next)) => {
                let next = Arc::new(next);
                providers.send_if_modified(|current| {
                    if current.as_ref() == next.as_ref() {
                        return false;
                    }
                    *current = next;
                    true
                });
            }
            Ok(None) => {}
            Err(error) => eprintln!("{error}"),
        }
    }));
}

pub(super) const fn method_needs_provider_refresh(method: RuntimeMethod) -> bool {
    matches!(
        method,
        RuntimeMethod::ProvidersList
            | RuntimeMethod::ProvidersWatch
            | RuntimeMethod::ProvidersGetCapabilities
            | RuntimeMethod::ProvidersListModels
            | RuntimeMethod::ProvidersListNativeSessions
            // Not the activity observation. It arrives four times a second per service from every window,
            // and each one past the recheck floor rebuilt the whole inventory (a walk of PATH and PATHEXT
            // per service) in the background, every second, for as long as a window was open. On Windows
            // that walk is what made every other answer slow (refresh p95 tens to hundreds of ms on CI,
            // 2026-08-29). A roster read discovers no executable; the requests that can are listed here.
            | RuntimeMethod::SessionsStart
            | RuntimeMethod::SessionsAdoptNative
            | RuntimeMethod::SessionsResume
            | RuntimeMethod::TerminalsOpen
    )
}

/// A live process whose new conversation another live claim already holds.
///
/// Said on the daemon's error stream and nowhere else: the answer this round still goes out with every
/// other process bound, and the same process is offered again next round until the other claim ends.
#[expect(
    clippy::print_stderr,
    reason = "the daemon's error stream is the only surface a background observation has; failing the whole answer froze every icon of the service (2026-08-29)"
)]
fn report_binding_conflict(
    provider: runtrol_provider::ProviderId,
    pid: u32,
    conflict: crate::native_claims::TerminalClaimError,
) {
    eprintln!(
        "runtrol: process {pid} of {} names a conversation another live claim holds: {conflict}",
        provider.as_str()
    );
}

/// Report the optional ancestry surface failing once rather than on every provider observation.
#[expect(
    clippy::print_stderr,
    reason = "the daemon's error stream is the existing operational failure channel for a background provider observation"
)]
fn report_process_tree_failure(error: &runtrol_childproc::ProcessTreeError) {
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if !REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("runtrol: provider process ancestry is unavailable: {error}");
    }
}
