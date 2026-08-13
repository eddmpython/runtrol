//! The one entry point the daemon calls about a session.
//!
//! Starting, pumping, closing and listing. They are four operations of one responsibility, so they are one module:
//! every one of them touches the same map of live sessions and the same tier bound, and splitting them would mean
//! four files sharing one invariant.
//!
//! # Why pumping is a step and not a loop
//!
//! [`SessionManager::pump_once`] reads one event, numbers it, applies it to the session's state, and returns. It does
//! not spawn a task and it does not loop, because owning the runtime is the daemon's job: the daemon decides when a
//! session is pumped and what that is raced against, and the kernel decides what one event means. Putting a task in
//! here would put a runtime configuration in the kernel, which is the one place the memory contract says it must not
//! be.
//!
//! It also makes the whole thing testable. A session is driven one event at a time against a scripted driver, and
//! every rule below is checked without a process, a socket, or a clock.
//!
//! # Why waiting on every session is one call and not one task each
//!
//! [`SessionManager::pump_any`] waits until any live session speaks, then applies that one event. A task per session
//! is the other way to do this, and it costs the thing that makes the rest of this file true: the state of every
//! session would move on a different thread from the one that reads it, so the map, the numbering and the tier bound
//! would each need a lock, and "the kernel decides what one event means" would become "some task decided, and the
//! kernel found out later".
//!
//! One caller, one owner, no locks. What it asks of a driver is written into [`Agent::next`]: a session that has
//! nothing to say yet is set aside, so being set aside must cost that driver nothing.
//!
//! # What a failure does
//!
//! It becomes session state. A protocol failure, a child that exited, a frame runtrol cannot read: each one moves the
//! session to a state the operator can see and resume from, and each one also leaves the event stream with a notice
//! saying so. A failure that was only returned to a caller would be a failure the operator never hears about.

use core::ops::Bound;
use core::pin::pin;
use core::task::Poll;
use std::collections::BTreeMap;

use runtrol_provider::{
    AbsPath, Agent, AgentCommand, ApprovalId, CloseMode, Disposition, EventBody, Level, Notice,
    NoticeCode, Opaque, OpenIntent, OptionId, Produced, Provider, ProviderError, ProviderId,
    RiskClass, SessionId, WallMs, WatchCursor, WorkspaceAccess,
};
use runtrol_security::{Caller, DeviceScope, GrantLedger, SecurityError};

use crate::events::{Published, SessionHub, SessionView};
use crate::project::{ProjectError, ProjectIdentity, WorkspaceClaim};
use crate::session::mint::Identity;
use crate::session::state::{FailureCode, Observed, SessionState};
use crate::session::tier::{Admit, HotSession, MAX_HOT, Tier};

/// A session operation could not be carried out.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// No session by that name is live.
    #[error("no live session {session}")]
    NotLive {
        /// The session that was asked about.
        session: SessionId,
    },

    /// A live session already uses the requested runtrol identifier.
    #[error("session {session} is already live")]
    AlreadyLive {
        /// The duplicate identifier.
        session: SessionId,
    },

    /// An opened process reported a different session from the intent it was given.
    #[error("opened process reports session {actual}, not requested session {expected}")]
    AgentSessionMismatch {
        /// The session in the open intent.
        expected: SessionId,
        /// The session reported by the process.
        actual: SessionId,
    },

    /// Every process slot is already live or reserved by an opening request.
    #[error("every process slot is live or reserved by a session that is opening")]
    OpeningCapacityReserved,

    /// The opening request no longer owns a process slot.
    #[error("session {session} has no pending open reservation")]
    OpenNotReserved {
        /// The session whose reservation was missing.
        session: SessionId,
    },

    /// Another opening, live, or closing process owns overlapping writable files.
    #[error("workspace {requested} overlaps {occupied}, which is reserved by session {session}")]
    WorkspaceOccupied {
        /// The new working tree.
        requested: AbsPath,
        /// The working tree already held.
        occupied: AbsPath,
        /// The session holding it.
        session: SessionId,
    },

    /// Provider opening resolved to a different workspace than the atomic reservation.
    #[error("session {session} opened workspace {opened}, not reserved workspace {reserved}")]
    WorkspaceReservationMismatch {
        /// The session being opened.
        session: SessionId,
        /// The canonical workspace that was reserved.
        reserved: AbsPath,
        /// The canonical workspace handed to the provider.
        opened: AbsPath,
    },

    /// Provider opening resolved to a different provider than the atomic process reservation.
    #[error("session {session} opened provider {opened}, not reserved provider {reserved}")]
    ProviderReservationMismatch {
        /// The session being opened.
        session: SessionId,
        /// Provider named by the process reservation.
        reserved: ProviderId,
        /// Provider handed to the attach operation.
        opened: ProviderId,
    },

    /// The project and working-tree identity could not be established safely.
    #[error(transparent)]
    Project(#[from] ProjectError),

    /// The session's agent is temporarily owned by an in-flight provider command.
    #[error("session {session} is carrying out another provider command")]
    AgentInFlight {
        /// The session whose agent is in flight.
        session: SessionId,
    },

    /// No new opaque slot generation can be minted without reusing an old one.
    #[error("process-slot reservation generation is exhausted")]
    ReservationGenerationExhausted,

    /// No new agent handoff generation can be minted without reusing an old one.
    #[error("agent handoff generation is exhausted")]
    AgentLeaseGenerationExhausted,

    /// This provider is reserved for an update and cannot start another process.
    #[error("provider {provider} is being updated")]
    ProviderUpdating {
        /// Provider whose executable tree is reserved.
        provider: ProviderId,
    },

    /// A provider process is live, opening, or closing, so its package tree cannot be changed safely.
    #[error("provider {provider} still has a live, opening, or closing process")]
    ProviderBusyForUpdate {
        /// Provider whose process prevents the update.
        provider: ProviderId,
    },

    /// Every session with a process is busy, so starting another would have to interrupt one.
    #[error(transparent)]
    NoRoom(#[from] crate::session::tier::NoRoom),

    /// The provider refused.
    ///
    /// Carried rather than flattened: the variant decides whether the operator sees "not installed", "authenticate at
    /// your machine", or "it broke", and those are three different next moves.
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// The provider has no pending approval by this name.
    #[error("approval {approval} is not pending for session {session}")]
    ApprovalNotPending {
        /// The session that was asked.
        session: SessionId,
        /// The approval that was asked.
        approval: ApprovalId,
    },

    /// The answer names an older or different view of the approval subject.
    #[error("approval {approval} subject changed before it was answered")]
    ApprovalSubjectChanged {
        /// The approval whose digest did not match.
        approval: ApprovalId,
    },

    /// The provider did not offer this option for the pending approval.
    #[error("approval {approval} did not offer option {option}")]
    ApprovalOptionNotOffered {
        /// The pending approval.
        approval: ApprovalId,
        /// The unknown option.
        option: OptionId,
    },

    /// The option exists but cannot be chosen by this answerer.
    #[error("approval {approval} option {option} is unavailable: {why}")]
    ApprovalOptionUnavailable {
        /// The pending approval.
        approval: ApprovalId,
        /// The blocked option.
        option: OptionId,
        /// Why choosing it would violate the consent or authority contract.
        why: &'static str,
    },

    /// The pending approval deadline has passed.
    #[error("approval {approval} has expired")]
    ApprovalExpired {
        /// The expired approval.
        approval: ApprovalId,
    },

    /// The caller does not hold the authority the pending approval requires.
    #[error(transparent)]
    Security(#[from] SecurityError),
}

/// One live session: its driver, its event hub, its names, and what it is doing.
struct Live {
    /// The driver.
    agent: Option<Box<dyn Agent>>,
    /// Where its events are numbered and fanned out.
    hub: SessionHub,
    /// Its two names.
    identity: Identity,
    /// Where the agent works.
    ///
    /// Kept because a listing has to say it. Which session is touching which folder is the whole of the
    /// `sessions do not trample each other` axis, and a surface that cannot show it cannot warn about it.
    workspace: AbsPath,
    /// The Core-owned working-tree identity used for writer admission.
    project: ProjectIdentity,
    /// What it is doing.
    state: SessionState,
}

/// One event, and which session produced it.
#[derive(Debug)]
pub struct Pumped {
    /// Whose event it was.
    pub session: SessionId,
    /// What was published, or `None` when that session's stream ended.
    pub published: Option<Published>,
    /// Whether a field visible in the session index changed.
    ///
    /// Content frames leave this false, so a surface watching many active sessions does not pay to rebuild its
    /// session list for conversation traffic.
    pub index_changed: bool,
}

struct Applied {
    published: Option<Published>,
    index_changed: bool,
}

/// A process that was admitted to the live session set.
#[derive(Debug)]
pub struct AttachedSession {
    /// The session that is now live.
    pub session: SessionId,
}

/// A reserved process slot, consumed when its opened process is attached.
#[derive(Debug, PartialEq, Eq)]
pub struct OpenReservation {
    session: SessionId,
    generation: u64,
}

/// Opaque proof that a closing process still occupies its bounded slot.
#[derive(Debug, PartialEq, Eq)]
pub struct ClosingReservation {
    session: SessionId,
    generation: u64,
}

/// Opaque proof that no process for one provider may start while its package tree changes.
#[derive(Debug, PartialEq, Eq)]
pub struct ProviderUpdateReservation {
    provider: ProviderId,
    generation: u64,
}

impl ProviderUpdateReservation {
    /// Provider whose process set is reserved.
    #[must_use]
    pub const fn provider(&self) -> ProviderId {
        self.provider
    }
}

impl ClosingReservation {
    /// Which session still owns the slot.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }
}

impl OpenReservation {
    /// Which session owns the slot.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReservationState {
    Opening,
    Closing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HeldReservation {
    generation: u64,
    state: ReservationState,
    project: ProjectIdentity,
    provider: Option<ProviderId>,
}

/// The result of reserving a process slot.
pub struct ReservedOpen {
    /// The reservation an attach must consume.
    pub reservation: OpenReservation,
    /// The idle process that must stop before a replacement is opened.
    pub displaced: Option<ClosingSession>,
}

/// A detached process that continues to occupy its bounded slot until cleanup finishes.
pub struct ClosingSession {
    /// The process to stop outside the session owner.
    pub agent: Box<dyn Agent>,
    /// The slot to release only after the process has stopped.
    pub reservation: ClosingReservation,
}

/// Opaque ownership proof for an agent temporarily moved out for provider I/O.
#[derive(Debug, PartialEq, Eq)]
pub struct AgentLease {
    session: SessionId,
    generation: u64,
}

/// An agent and the proof required to return it to its live session.
pub struct TakenAgent {
    /// The provider agent to use outside the session owner.
    pub agent: Box<dyn Agent>,
    /// The opaque proof used to restore or abandon it.
    pub lease: AgentLease,
}

/// A process that could not be admitted, together with the refusal.
pub struct AttachError {
    error: SessionError,
    agent: Box<dyn Agent>,
    reservation: OpenReservation,
}

impl core::fmt::Debug for AttachError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AttachError")
            .field("error", &self.error)
            .field("agent", &"open process")
            .field("reservation", &self.reservation)
            .finish()
    }
}

impl AttachError {
    /// Split the refusal from the process its caller still has to stop.
    #[must_use]
    pub fn into_parts(self) -> (SessionError, Box<dyn Agent>, OpenReservation) {
        (self.error, self.agent, self.reservation)
    }
}

/// Every session that has a process, and the rules about how many may.
pub struct SessionManager {
    /// The live ones, ordered by name so a listing is stable.
    live: BTreeMap<SessionId, Live>,
    /// Slots promised to connection tasks that have not attached their process yet.
    opening: BTreeMap<SessionId, HeldReservation>,
    /// Generation for the next opaque process-slot reservation.
    next_reservation: u64,
    /// Generation for the next temporary agent handoff.
    next_agent_lease: u64,
    /// Agents temporarily moved out for provider I/O.
    in_flight: BTreeMap<SessionId, u64>,
    /// Providers whose package trees are being changed outside the session owner.
    updating_providers: BTreeMap<ProviderId, u64>,
    /// Which session spoke last, so the next round of listening starts past it.
    ///
    /// Kept as a name rather than a position: a position would move under a session that ended, and this is asked
    /// about by exclusion, so it does not have to name a session that is still live.
    after: Option<SessionId>,
}

/// Returns an open reservation if its asynchronous convenience path is abandoned.
struct OpeningGuard<'manager> {
    manager: &'manager mut SessionManager,
    reservation: Option<OpenReservation>,
}

impl Drop for OpeningGuard<'_> {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            self.manager.cancel_open(reservation);
        }
    }
}

impl SessionManager {
    /// A manager with nothing live.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            live: BTreeMap::new(),
            opening: BTreeMap::new(),
            next_reservation: 0,
            next_agent_lease: 0,
            in_flight: BTreeMap::new(),
            updating_providers: BTreeMap::new(),
            after: None,
        }
    }

    /// Force the next process-slot generation allocation to fail in integration tests.
    ///
    /// This fault-injection seam is not compiled unless the internal `test-support` feature is enabled.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn exhaust_reservation_generations_for_tests(&mut self) {
        self.next_reservation = u64::MAX;
    }

    /// How many sessions have a process.
    #[must_use]
    pub fn hot(&self) -> usize {
        self.live.len()
    }

    /// Whether a session is live.
    #[must_use]
    pub fn is_live(&self, session: SessionId) -> bool {
        self.live.contains_key(&session)
    }

    /// What a session is doing, if it is live.
    #[must_use]
    pub fn state(&self, session: SessionId) -> Option<&SessionState> {
        self.live.get(&session).map(|one| &one.state)
    }

    /// The provider's own name for a session, if it has announced one.
    #[must_use]
    pub fn native(&self, session: SessionId) -> Option<&str> {
        self.live
            .get(&session)?
            .identity
            .native()
            .map(AsRef::as_ref)
    }

    /// Watch a live session, beginning with the bounded recent replay window.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name.
    pub fn subscribe(
        &mut self,
        session: SessionId,
        requested: Option<WatchCursor>,
    ) -> Result<SessionView, SessionError> {
        let live = self
            .live
            .get_mut(&session)
            .ok_or(SessionError::NotLive { session })?;
        Ok(live.hub.view(requested))
    }

    /// Start a session, or continue one.
    ///
    /// Admission comes first, because a start that would have to interrupt a running turn is refused rather than
    /// resolved by force. Then the driver is asked, and only a driver that answered gets a place in the map: a session
    /// that failed to open is not a session with a process.
    ///
    /// # Errors
    ///
    /// [`SessionError::NoRoom`] when every session with a process is busy, [`SessionError::Provider`] when the driver
    /// refused. The provider's own variant is preserved, because "not installed" and "authenticate at your machine"
    /// are different next moves for the operator.
    pub async fn start(
        &mut self,
        provider: &dyn Provider,
        intent: OpenIntent,
        workspace_access: WorkspaceAccess,
    ) -> Result<SessionId, SessionError> {
        let claim = WorkspaceClaim::discover(intent.workspace.clone(), workspace_access)?;
        let ReservedOpen {
            reservation,
            displaced,
        } = self.reserve_open_for_provider(provider.id(), intent.session, claim)?;
        let mut opening = OpeningGuard {
            manager: self,
            reservation: Some(reservation),
        };
        if let Some(displaced) = displaced {
            drop(
                displaced
                    .agent
                    .close(CloseMode::Graceful { grace_ms: 0 })
                    .await,
            );
            opening.manager.release_closing(displaced.reservation);
        }
        let agent = match provider.open(intent.clone()).await {
            Ok(agent) => agent,
            Err(error) => return Err(error.into()),
        };
        let Some(reservation) = opening.reservation.take() else {
            return Err(SessionError::OpenNotReserved {
                session: intent.session,
            });
        };
        match opening
            .manager
            .attach_opened(reservation, provider.id(), &intent, agent)
        {
            Ok(attached) => {
                opening.reservation = None;
                Ok(attached.session)
            }
            Err(error) => {
                let (error, agent, reservation) = error.into_parts();
                drop(agent.close(CloseMode::Kill).await);
                opening.manager.cancel_open(reservation);
                opening.reservation = None;
                Err(error)
            }
        }
    }

    #[cfg(test)]
    async fn start_for_tests(
        &mut self,
        provider: &dyn Provider,
        intent: OpenIntent,
    ) -> Result<SessionId, SessionError> {
        self.start(provider, intent, WorkspaceAccess::Shared).await
    }

    /// Reserve one bounded process slot before a provider is asked to open it.
    ///
    /// Any displaced idle process is removed synchronously and returned. The caller must stop it before opening the
    /// replacement so the real process count never exceeds [`MAX_HOT`].
    ///
    /// # Errors
    ///
    /// Refuses a duplicate session identifier, a tier containing only busy sessions, or capacity already promised to
    /// other opening requests.
    pub fn reserve_open(
        &mut self,
        session: SessionId,
        claim: WorkspaceClaim,
    ) -> Result<ReservedOpen, SessionError> {
        self.reserve_open_inner(None, session, claim)
    }

    /// Reserve a process slot for a known provider while excluding its update lease.
    ///
    /// # Errors
    ///
    /// The same errors as [`Self::reserve_open`], plus [`SessionError::ProviderUpdating`].
    pub fn reserve_open_for_provider(
        &mut self,
        provider: ProviderId,
        session: SessionId,
        claim: WorkspaceClaim,
    ) -> Result<ReservedOpen, SessionError> {
        self.reserve_open_inner(Some(provider), session, claim)
    }

    fn reserve_open_inner(
        &mut self,
        provider: Option<ProviderId>,
        session: SessionId,
        claim: WorkspaceClaim,
    ) -> Result<ReservedOpen, SessionError> {
        if let Some(provider) = provider
            && self.updating_providers.contains_key(&provider)
        {
            return Err(SessionError::ProviderUpdating { provider });
        }
        if self.live.contains_key(&session) || self.opening.contains_key(&session) {
            return Err(SessionError::AlreadyLive { session });
        }
        if claim.access() == WorkspaceAccess::Exclusive
            && let Some((occupied_by, occupied)) = self.workspace_conflict(claim.identity())
        {
            return Err(SessionError::WorkspaceOccupied {
                requested: claim.identity().worktree().clone(),
                occupied: occupied.worktree().clone(),
                session: occupied_by,
            });
        }

        let admission = {
            let occupied = self.live.len() + self.opening.len();
            if occupied < MAX_HOT {
                Admit::Straight
            } else if self.live.len() < MAX_HOT {
                return Err(SessionError::OpeningCapacityReserved);
            } else {
                self.admit()?
            }
        };
        let generation = self.allocate_reservation_generation()?;
        let reservation = OpenReservation {
            session,
            generation,
        };
        let displaced = match admission {
            Admit::Straight => None,
            Admit::Evicting { session: victim } => {
                let closing_generation = self.allocate_reservation_generation()?;
                let Some(mut live) = self.live.remove(&victim) else {
                    return Err(SessionError::NotLive { session: victim });
                };
                let Some(agent) = live.agent.take() else {
                    self.live.insert(victim, live);
                    return Err(SessionError::AgentInFlight { session: victim });
                };
                self.opening.insert(
                    victim,
                    HeldReservation {
                        generation: closing_generation,
                        state: ReservationState::Closing,
                        project: live.project,
                        provider: Some(live.identity.provider()),
                    },
                );
                Some(ClosingSession {
                    agent,
                    reservation: ClosingReservation {
                        session: victim,
                        generation: closing_generation,
                    },
                })
            }
        };
        let project = claim.into_identity();
        self.opening.insert(
            session,
            HeldReservation {
                generation,
                state: ReservationState::Opening,
                project,
                provider,
            },
        );
        Ok(ReservedOpen {
            reservation,
            displaced,
        })
    }

    /// Reserve a process slot against a fixed non-repository workspace in internal tests.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn reserve_open_for_tests(
        &mut self,
        session: SessionId,
    ) -> Result<ReservedOpen, SessionError> {
        let workspace = AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
            .map_err(ProjectError::from)?;
        let claim = WorkspaceClaim::discover(workspace, WorkspaceAccess::Shared)?;
        self.reserve_open(session, claim)
    }

    /// Reserve one provider after proving it has no live, opening, or closing process.
    ///
    /// # Errors
    ///
    /// [`SessionError::ProviderBusyForUpdate`] when any process or reservation still names the provider, or
    /// [`SessionError::ProviderUpdating`] when another update already owns it.
    pub fn reserve_provider_update(
        &mut self,
        provider: ProviderId,
    ) -> Result<ProviderUpdateReservation, SessionError> {
        if self.updating_providers.contains_key(&provider) {
            return Err(SessionError::ProviderUpdating { provider });
        }
        if self
            .live
            .values()
            .any(|live| live.identity.provider() == provider)
            || self
                .opening
                .values()
                .any(|held| held.provider == Some(provider))
        {
            return Err(SessionError::ProviderBusyForUpdate { provider });
        }
        let generation = self.allocate_reservation_generation()?;
        self.updating_providers.insert(provider, generation);
        Ok(ProviderUpdateReservation {
            provider,
            generation,
        })
    }

    /// Release the exact provider update lease after package-manager work ends.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the opaque update ownership proof is deliberately consumed"
    )]
    pub fn release_provider_update(&mut self, reservation: ProviderUpdateReservation) {
        if self.updating_providers.get(&reservation.provider) == Some(&reservation.generation) {
            self.updating_providers.remove(&reservation.provider);
        }
    }

    /// Release a slot whose provider open was abandoned or failed.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an opaque ownership proof is deliberately consumed even though its fields are scalar"
    )]
    pub fn cancel_open(&mut self, reservation: OpenReservation) {
        let OpenReservation {
            session,
            generation,
        } = reservation;
        if held_matches(
            self.opening.get(&session),
            generation,
            ReservationState::Opening,
        ) {
            self.opening.remove(&session);
        }
    }

    /// Admit a process that a provider already opened.
    ///
    /// This operation contains no process wait. It consumes the earlier tier reservation and applies identity rules
    /// and initial state transitions synchronously.
    ///
    /// # Errors
    ///
    /// Returns both the refusal and the still-open process when the reservation, process identity or disposition does
    /// not match. The caller remains responsible for stopping that process.
    #[allow(
        clippy::result_large_err,
        reason = "the refusal must return both the live process and its opaque reservation; whether the layout crosses the lint threshold is target-dependent"
    )]
    pub fn attach_opened(
        &mut self,
        reservation: OpenReservation,
        provider: ProviderId,
        intent: &OpenIntent,
        agent: Box<dyn Agent>,
    ) -> Result<AttachedSession, AttachError> {
        let session = intent.session;
        if reservation.session != session
            || !held_matches(
                self.opening.get(&reservation.session),
                reservation.generation,
                ReservationState::Opening,
            )
        {
            return Err(AttachError {
                error: SessionError::OpenNotReserved { session },
                agent,
                reservation,
            });
        }
        if self.live.contains_key(&session) {
            return Err(AttachError {
                error: SessionError::AlreadyLive { session },
                agent,
                reservation,
            });
        }
        let actual = agent.session();
        if actual != session {
            return Err(AttachError {
                error: SessionError::AgentSessionMismatch {
                    expected: session,
                    actual,
                },
                agent,
                reservation,
            });
        }
        let Some(held) = self.opening.get(&reservation.session) else {
            return Err(AttachError {
                error: SessionError::OpenNotReserved { session },
                agent,
                reservation,
            });
        };
        if let Some(error) = provider_reservation_mismatch(held, provider, session) {
            return Err(attach_error(error, agent, reservation));
        }
        if held.project.workspace() != &intent.workspace {
            return Err(AttachError {
                error: SessionError::WorkspaceReservationMismatch {
                    session,
                    reserved: held.project.workspace().clone(),
                    opened: intent.workspace.clone(),
                },
                agent,
                reservation,
            });
        }

        let identity = match &intent.disposition {
            Disposition::Fresh => Identity::assigned(provider, session),
            // A resume already has both names. Nothing is minted, because minting would give the same conversation
            // a second identity and the operator would see it twice in one list.
            Disposition::Resume { native } => {
                let mut identity = Identity::assigned(provider, session);
                if let Ok(native) = runtrol_provider::NativeSessionId::new(native) {
                    identity.observe_native(native);
                }
                identity
            }
            other => {
                return Err(AttachError {
                    error: SessionError::Provider(ProviderError::Unsupported {
                        provider,
                        what: format!("{other:?}"),
                        why: "the kernel has no rule for that way of opening a session yet",
                    }),
                    agent,
                    reservation,
                });
            }
        };

        let workspace = intent.workspace.clone();
        let Some(held) = self.opening.remove(&reservation.session) else {
            return Err(AttachError {
                error: SessionError::OpenNotReserved { session },
                agent,
                reservation,
            });
        };
        let mut state = SessionState::new(WallMs::now());
        // Binding happened: the driver answered. Recorded through the transition table like everything else, so there
        // is still exactly one place a state may change.
        let now = WallMs::now();
        drop(state.observe(Observed::Attaching, now));
        drop(state.observe(Observed::Attached, now));

        self.live.insert(
            session,
            Live {
                agent: Some(agent),
                hub: SessionHub::new(session),
                identity,
                workspace,
                project: held.project,
                state,
            },
        );
        Ok(AttachedSession { session })
    }

    /// Read one event from a session and let it change what runtrol believes.
    ///
    /// Returns what was published, or `None` once the session's stream is over. A stream that ended has its session
    /// removed from the live map, because a session with no process is not a hot session.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name. A **provider** failure is not returned: it
    /// is promoted to session state and published as a notice, because a failure only returned to a caller is a
    /// failure the operator never hears about.
    pub async fn pump_once(
        &mut self,
        session: SessionId,
    ) -> Result<Option<Published>, SessionError> {
        let live = self
            .live
            .get_mut(&session)
            .ok_or(SessionError::NotLive { session })?;
        let agent = live
            .agent
            .as_mut()
            .ok_or(SessionError::AgentInFlight { session })?;
        let spoke = agent.next().await;
        Ok(self.apply(session, spoke).published)
    }

    /// Wait until any live session speaks, and let that event change what runtrol believes.
    ///
    /// Every live session is asked in turn, starting after the one that spoke last so that a talkative session
    /// cannot keep a quiet one from ever being heard. The first that has something ready is the one applied.
    ///
    /// # This does not finish while nothing is live
    ///
    /// With no live sessions there is nothing that could speak, so this waits. That is what a caller racing it
    /// against its listener wants: an arm that never fires rather than one that fires constantly and turns an idle
    /// daemon into a spin. Anything awaiting it alone would wait forever, which is why it is written here.
    pub async fn pump_any(&mut self) -> Pumped {
        let (session, spoke) = self.hear_one().await;
        // The next round starts after this one, which is the whole of the fairness rule.
        self.after = Some(session);
        let applied = self.apply(session, spoke);
        Pumped {
            session,
            published: applied.published,
            index_changed: applied.index_changed,
        }
    }

    /// The first live session with something ready, and what it said.
    ///
    /// Each session is asked with no waiting in between. A session that is not ready is set aside, which the
    /// [`Agent::next`] contract requires to cost it nothing, and asked again when something wakes this.
    async fn hear_one(&mut self) -> (SessionId, Option<Result<Produced, ProviderError>>) {
        let live = &mut self.live;
        let after = self.after;
        core::future::poll_fn(|cx| {
            // Everything past the one that spoke last, then everything up to and including it. Two passes over two
            // ranges rather than one rotated pass, because rotating would mean borrowing the map twice at the same
            // time. The one that spoke last is at the end of the second pass, so it is asked last and never
            // skipped. Naming it by exclusion also means it does not have to still be live.
            let past = match after {
                Some(after) => (Bound::Excluded(after), Bound::Unbounded),
                None => (Bound::Unbounded, Bound::Unbounded),
            };
            for (session, one) in live.range_mut(past) {
                let Some(agent) = one.agent.as_mut() else {
                    continue;
                };
                if let Poll::Ready(spoke) = pin!(agent.next()).poll(cx) {
                    return Poll::Ready((*session, spoke));
                }
            }
            if let Some(after) = after {
                for (session, one) in live.range_mut(..=after) {
                    let Some(agent) = one.agent.as_mut() else {
                        continue;
                    };
                    if let Poll::Ready(spoke) = pin!(agent.next()).poll(cx) {
                        return Poll::Ready((*session, spoke));
                    }
                }
            }
            Poll::Pending
        })
        .await
    }

    /// What one answer from a driver means for the session it came from.
    ///
    /// The one place an event changes anything. Both ways of pumping arrive here, so there is no second rule about
    /// what a broken stream or an ended one does.
    fn apply(
        &mut self,
        session: SessionId,
        spoke: Option<Result<Produced, ProviderError>>,
    ) -> Applied {
        // The session is here: both callers just read from it. Asked for rather than assumed, so that a future
        // caller which does not hold that guarantee gets nothing rather than a panic.
        let Some(live) = self.live.get_mut(&session) else {
            return Applied {
                published: None,
                index_changed: false,
            };
        };

        match spoke {
            Some(Ok(produced)) => {
                // The provider's own name may have arrived with this frame. The newest answer wins.
                let native_changed = if let Some(native) =
                    live.agent.as_ref().and_then(|agent| agent.native())
                    && let Ok(parsed) = runtrol_provider::NativeSessionId::new(native)
                {
                    let changed = live.identity.native() != Some(&parsed);
                    live.identity.observe_native(parsed);
                    changed
                } else {
                    false
                };

                let observed = observation_of(&produced.body);
                let lifecycle_before = live.state.lifecycle().clone();
                let stuck_before = live.state.looks_stuck();
                let published = live.hub.publish(produced.src_end, produced.body);
                if let Some(observed) = observed {
                    // A driver reporting something that cannot have happened becomes a notice rather than a panic. A
                    // supervisor that aborted on one misbehaving driver would take every other session with it.
                    if let Err(refusal) = live.state.observe(observed, published.event.at) {
                        live.hub.publish(
                            published.event.src_end,
                            notice(
                                NoticeCode::ProtocolViolation,
                                Level::Warn,
                                &refusal.to_string(),
                            ),
                        );
                    }
                }
                Applied {
                    published: Some(published),
                    index_changed: native_changed
                        || live.state.lifecycle() != &lifecycle_before
                        || live.state.looks_stuck() != stuck_before,
                }
            }

            Some(Err(error)) => {
                // Promoted to session state and said out loud, in that order. The session stays visible through its
                // metadata, and its provider-native identifier remains available for the provider's resume surface.
                let detail = error.to_string();
                let at = WallMs::now();
                drop(live.state.observe(
                    Observed::Failed {
                        code: FailureCode::Protocol,
                        detail: detail.clone(),
                    },
                    at,
                ));
                let published = live.hub.publish(
                    live.hub.src_end(),
                    notice(NoticeCode::ProtocolViolation, Level::Error, &detail),
                );
                self.live.remove(&session);
                Applied {
                    published: Some(published),
                    index_changed: true,
                }
            }

            None => {
                // The stream is over. Whether the turn that was running finished is answered by the events that came
                // before, never by this: the driver already reported an ending declared by the exit if there was one.
                let at = WallMs::now();
                drop(live.state.observe(Observed::Detached, at));
                self.live.remove(&session);
                Applied {
                    published: None,
                    index_changed: true,
                }
            }
        }
    }

    /// Send a command to a live session.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name, [`SessionError::Provider`] when the driver
    /// refused. A refusal is returned rather than swallowed: an operator who pressed something has to know it did not
    /// happen.
    pub async fn send(
        &mut self,
        session: SessionId,
        command: AgentCommand,
    ) -> Result<(), SessionError> {
        let live = self
            .live
            .get_mut(&session)
            .ok_or(SessionError::NotLive { session })?;
        let agent = live
            .agent
            .as_mut()
            .ok_or(SessionError::AgentInFlight { session })?;
        agent.send(command).await.map_err(SessionError::from)
    }

    /// Move one agent out so provider I/O can run without holding the session owner.
    ///
    /// # Errors
    ///
    /// Refuses a session that is absent or already carrying out a provider command.
    pub fn take_agent(&mut self, session: SessionId) -> Result<TakenAgent, SessionError> {
        let live = self
            .live
            .get(&session)
            .ok_or(SessionError::NotLive { session })?;
        if live.agent.is_none() {
            return Err(SessionError::AgentInFlight { session });
        }
        let generation = self.allocate_agent_lease_generation()?;
        let Some(live) = self.live.get_mut(&session) else {
            return Err(SessionError::NotLive { session });
        };
        let Some(agent) = live.agent.take() else {
            return Err(SessionError::AgentInFlight { session });
        };
        self.in_flight.insert(session, generation);
        Ok(TakenAgent {
            agent,
            lease: AgentLease {
                session,
                generation,
            },
        })
    }

    /// Restore an agent after its provider command finishes.
    ///
    /// # Errors
    ///
    /// Returns the agent when the opaque lease is stale or its session no longer accepts the handoff.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an opaque ownership proof is deliberately consumed even though its fields are scalar"
    )]
    pub fn return_agent(
        &mut self,
        lease: AgentLease,
        agent: Box<dyn Agent>,
    ) -> Result<(), Box<dyn Agent>> {
        let AgentLease {
            session,
            generation,
        } = lease;
        if self.in_flight.get(&session) != Some(&generation) {
            return Err(agent);
        }
        let Some(live) = self.live.get_mut(&session) else {
            self.in_flight.remove(&session);
            return Err(agent);
        };
        if live.agent.is_some() {
            return Err(agent);
        }
        live.agent = Some(agent);
        self.in_flight.remove(&session);
        Ok(())
    }

    /// Forget a placeholder whose externally owned agent was dropped.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an opaque ownership proof is deliberately consumed even though its fields are scalar"
    )]
    pub fn abandon_agent(&mut self, lease: AgentLease) {
        let AgentLease {
            session,
            generation,
        } = lease;
        if self.in_flight.get(&session) == Some(&generation) {
            self.in_flight.remove(&session);
            self.live.remove(&session);
        }
    }

    /// Validate an approval and move its agent out for provider I/O.
    ///
    /// # Errors
    ///
    /// Refuses the same stale, expired, unavailable or unauthorized answers as [`Self::answer_approval`], plus a
    /// session already carrying out another provider command.
    pub fn take_for_answer_approval(
        &mut self,
        caller: &Caller,
        ledger: &GrantLedger,
        session: SessionId,
        approval: ApprovalId,
        option: OptionId,
        subject_digest: [u8; 32],
    ) -> Result<(TakenAgent, AgentCommand), SessionError> {
        let live = self
            .live
            .get(&session)
            .ok_or(SessionError::NotLive { session })?;
        let agent = live
            .agent
            .as_ref()
            .ok_or(SessionError::AgentInFlight { session })?;
        let request = agent
            .approval(approval)
            .ok_or(SessionError::ApprovalNotPending { session, approval })?;
        if request.subject_digest != subject_digest {
            return Err(SessionError::ApprovalSubjectChanged { approval });
        }
        if WallMs::now() >= request.expires_at {
            return Err(SessionError::ApprovalExpired { approval });
        }
        let chosen = request
            .options
            .iter()
            .find(|candidate| candidate.id == option)
            .ok_or(SessionError::ApprovalOptionNotOffered { approval, option })?;
        let may_answer_high = caller.may(DeviceScope::ApprovalRespondHigh, ledger).is_ok();
        if let Some(why) = request
            .offerable(may_answer_high)
            .into_iter()
            .find(|offered| offered.option.id == option)
            .and_then(|offered| offered.unavailable)
        {
            return Err(SessionError::ApprovalOptionUnavailable {
                approval,
                option,
                why,
            });
        }
        let required_scope =
            if request.risk == RiskClass::High || chosen.kind.commits_beyond_this_action() {
                DeviceScope::ApprovalRespondHigh
            } else {
                DeviceScope::ApprovalRespondLow
            };
        caller.may(required_scope, ledger)?;
        let taken = self.take_agent(session)?;
        Ok((
            taken,
            AgentCommand::Answer {
                id: approval,
                option,
                subject_digest,
            },
        ))
    }

    /// Answer a provider approval after binding the choice to the request the driver still holds.
    ///
    /// Risk never arrives as input to this method. It is taken from the pending request, and a standing option raises
    /// the required authority even if the action itself is low risk. The provider-native response remains private to
    /// the driver and is removed only after [`Agent::send`] succeeds.
    ///
    /// # Errors
    ///
    /// Refuses a missing or expired approval, a stale subject digest, an option the provider did not offer, an option
    /// made unavailable by incomplete consent or authority, a missing device scope, or a provider write failure.
    pub async fn answer_approval(
        &mut self,
        caller: &Caller,
        ledger: &GrantLedger,
        session: SessionId,
        approval: ApprovalId,
        option: OptionId,
        subject_digest: [u8; 32],
    ) -> Result<(), SessionError> {
        let live = self
            .live
            .get_mut(&session)
            .ok_or(SessionError::NotLive { session })?;

        let required_scope = {
            let request = live
                .agent
                .as_ref()
                .ok_or(SessionError::AgentInFlight { session })?
                .approval(approval)
                .ok_or(SessionError::ApprovalNotPending { session, approval })?;
            if request.subject_digest != subject_digest {
                return Err(SessionError::ApprovalSubjectChanged { approval });
            }
            if WallMs::now() >= request.expires_at {
                return Err(SessionError::ApprovalExpired { approval });
            }

            let chosen = request
                .options
                .iter()
                .find(|candidate| candidate.id == option)
                .ok_or(SessionError::ApprovalOptionNotOffered { approval, option })?;
            let may_answer_high = caller.may(DeviceScope::ApprovalRespondHigh, ledger).is_ok();
            if let Some(why) = request
                .offerable(may_answer_high)
                .into_iter()
                .find(|offered| offered.option.id == option)
                .and_then(|offered| offered.unavailable)
            {
                return Err(SessionError::ApprovalOptionUnavailable {
                    approval,
                    option,
                    why,
                });
            }

            if request.risk == RiskClass::High || chosen.kind.commits_beyond_this_action() {
                DeviceScope::ApprovalRespondHigh
            } else {
                DeviceScope::ApprovalRespondLow
            }
        };

        caller.may(required_scope, ledger)?;
        live.agent
            .as_mut()
            .ok_or(SessionError::AgentInFlight { session })?
            .send(AgentCommand::Answer {
                id: approval,
                option,
                subject_digest,
            })
            .await
            .map_err(SessionError::from)
    }

    /// End a session, and hand back the driver that still has to be stopped.
    ///
    /// # Why this does not do the stopping
    ///
    /// Because stopping is a wait. A graceful close gives the process time to finish, and this is the one call that
    /// could take seconds; doing it here would hold the only owner of every session for that long, and every other
    /// session's output would stop while one of them was being closed. Waiting belongs to whoever owns the runtime,
    /// which is the daemon, and this returns as soon as the thing it does own has changed.
    ///
    /// The session is gone from this manager by the time this returns, whatever the caller then does with the driver.
    /// Its process slot remains reserved until the caller finishes cleanup and passes the returned reservation to
    /// [`SessionManager::release_closing`]. Dropping the agent rather than closing it still stops the process, because a
    /// driver holds its child that way, but the reservation must still be released explicitly.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name.
    pub fn close(&mut self, session: SessionId) -> Result<ClosingSession, SessionError> {
        let live = self
            .live
            .get(&session)
            .ok_or(SessionError::NotLive { session })?;
        if live.agent.is_none() {
            return Err(SessionError::AgentInFlight { session });
        }
        let generation = self.allocate_reservation_generation()?;
        let mut live = self
            .live
            .remove(&session)
            .ok_or(SessionError::NotLive { session })?;
        let Some(agent) = live.agent.take() else {
            self.live.insert(session, live);
            return Err(SessionError::AgentInFlight { session });
        };
        self.opening.insert(
            session,
            HeldReservation {
                generation,
                state: ReservationState::Closing,
                project: live.project,
                provider: Some(live.identity.provider()),
            },
        );
        Ok(ClosingSession {
            agent,
            reservation: ClosingReservation {
                session,
                generation,
            },
        })
    }

    /// Release a closing slot after its process cleanup completes.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an opaque ownership proof is deliberately consumed even though its fields are scalar"
    )]
    pub fn release_closing(&mut self, reservation: ClosingReservation) {
        let ClosingReservation {
            session,
            generation,
        } = reservation;
        if held_matches(
            self.opening.get(&session),
            generation,
            ReservationState::Closing,
        ) {
            self.opening.remove(&session);
        }
    }

    fn workspace_conflict(
        &self,
        requested: &ProjectIdentity,
    ) -> Option<(SessionId, &ProjectIdentity)> {
        self.live
            .iter()
            .find_map(|(session, live)| {
                requested
                    .overlaps(&live.project)
                    .then_some((*session, &live.project))
            })
            .or_else(|| {
                self.opening.iter().find_map(|(session, held)| {
                    requested
                        .overlaps(&held.project)
                        .then_some((*session, &held.project))
                })
            })
    }

    fn allocate_reservation_generation(&mut self) -> Result<u64, SessionError> {
        let generation = self.next_reservation;
        self.next_reservation = self
            .next_reservation
            .checked_add(1)
            .ok_or(SessionError::ReservationGenerationExhausted)?;
        Ok(generation)
    }

    fn allocate_agent_lease_generation(&mut self) -> Result<u64, SessionError> {
        let generation = self.next_agent_lease;
        self.next_agent_lease = self
            .next_agent_lease
            .checked_add(1)
            .ok_or(SessionError::AgentLeaseGenerationExhausted)?;
        Ok(generation)
    }

    /// Whether another session may have a process, and what gives way if so.
    fn admit(&self) -> Result<Admit, crate::session::tier::NoRoom> {
        let held: Vec<HotSession> = self
            .live
            .iter()
            .map(|(session, live)| HotSession {
                session: *session,
                last_seen: live.state.last_seen(),
                busy: live.agent.is_none() || live.state.lifecycle().turn().is_some(),
            })
            .collect();
        crate::session::tier::admit(&held)
    }

    /// Every live session, oldest name first.
    ///
    /// Only the ones with a process. The daemon joins these with runtrol's own stored session pointers. Provider
    /// transcript storage is outside this manager and is never scanned to build the list.
    pub fn live_sessions(&self) -> impl Iterator<Item = LiveSession<'_>> {
        self.live.iter().map(|(session, live)| LiveSession {
            session: *session,
            provider: live.identity.provider(),
            native: live.identity.native().map(AsRef::as_ref),
            workspace: &live.workspace,
            tier: Tier::Hot,
            state: &live.state,
        })
    }

    /// One live session by its runtrol identifier.
    #[must_use]
    pub fn live_session(&self, session: SessionId) -> Option<LiveSession<'_>> {
        let live = self.live.get(&session)?;
        Some(LiveSession {
            session,
            provider: live.identity.provider(),
            native: live.identity.native().map(AsRef::as_ref),
            workspace: &live.workspace,
            tier: Tier::Hot,
            state: &live.state,
        })
    }
}

fn held_matches(held: Option<&HeldReservation>, generation: u64, state: ReservationState) -> bool {
    held.is_some_and(|held| held.generation == generation && held.state == state)
}

fn provider_reservation_mismatch(
    held: &HeldReservation,
    opened: ProviderId,
    session: SessionId,
) -> Option<SessionError> {
    let reserved = held.provider?;
    (reserved != opened).then_some(SessionError::ProviderReservationMismatch {
        session,
        reserved,
        opened,
    })
}

fn attach_error(
    error: SessionError,
    agent: Box<dyn Agent>,
    reservation: OpenReservation,
) -> AttachError {
    AttachError {
        error,
        agent,
        reservation,
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// One session that has a process, as a listing sees it.
#[derive(Clone, Copy, Debug)]
pub struct LiveSession<'manager> {
    /// runtrol's own name.
    pub session: SessionId,
    /// Which CLI it belongs to.
    pub provider: ProviderId,
    /// The provider's own name, once it has announced one.
    pub native: Option<&'manager str>,
    /// Where the agent works.
    pub workspace: &'manager AbsPath,
    /// How much of it exists. Always the hot tier here, by definition.
    pub tier: Tier,
    /// What it is doing.
    pub state: &'manager SessionState,
}

/// A notice runtrol originates.
fn notice(code: NoticeCode, level: Level, detail: &str) -> EventBody {
    // Serialized rather than pasted, because the text can carry a provider's own words and pasting would make those
    // words into structure.
    let payload = serde_json::to_string(&serde_json::json!({"detail": detail}))
        .unwrap_or_else(|_| String::from(r#"{"detail":"unprintable"}"#));
    EventBody::Notice(Box::new(Notice {
        level,
        code,
        retryable: false,
        payload: Opaque::owned(payload),
    }))
}

/// What a published event says about the session's state, when it says anything.
///
/// Most events say nothing: content is content. The ones that matter are the turn's own frames, which the driver
/// stamped, and a detach.
fn observation_of(body: &EventBody) -> Option<Observed> {
    match body {
        EventBody::Turn(turn) => match turn {
            runtrol_provider::TurnEvent::Started { turn }
            | runtrol_provider::TurnEvent::Accepted { turn, .. } => {
                Some(Observed::TurnStarted { turn: *turn })
            }
            runtrol_provider::TurnEvent::Ended { turn, .. } => {
                Some(Observed::TurnEnded { turn: *turn })
            }
            // Blocked and resumed are about what a turn is waiting for, not about whether it is running. The state
            // machine has no transition for them because there is nothing to change.
            _ => None,
        },
        EventBody::Detached(_) => Some(Observed::Detached),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use runtrol_provider::{
        AbsPath, AgentEvent, Declarant, Produced, StopReason, TurnEvent, TurnId,
    };

    use super::*;

    fn watch_event(item: crate::events::WatchItem) -> AgentEvent {
        match item {
            crate::events::WatchItem::Event(event) => event.event().clone(),
            crate::events::WatchItem::Lagged(cursor) => panic!("unexpected lag at {cursor:?}"),
        }
    }

    /// What a scripted driver answers next.
    ///
    /// A description rather than a held answer, because a provider error carries an operating system error and cannot
    /// be cloned. Describing it also makes each test read as a sequence of events rather than as a pile of literals.
    #[derive(Clone, Copy)]
    enum Step {
        /// Something that is conversation and must not move the session.
        Content,
        /// A turn beginning.
        TurnStarted(u32),
        /// A turn ending, by the provider's own word.
        TurnEnded(u32),
        /// The provider broke its own protocol.
        Broken,
        /// The stream is over.
        End,
    }

    impl Step {
        /// What the driver hands back for this step.
        fn answer(self, provider: ProviderId) -> Option<Result<Produced, ProviderError>> {
            match self {
                Self::Content => Some(Ok(content())),
                Self::TurnStarted(index) => Some(Ok(turn_started(index))),
                Self::TurnEnded(index) => Some(Ok(turn_ended(index))),
                Self::Broken => Some(Err(ProviderError::Protocol {
                    provider,
                    doing: "reading a frame",
                    detail: "the vendor changed shape".to_owned(),
                })),
                Self::End => None,
            }
        }
    }

    /// A driver that answers from a script, so every rule can be checked without a process.
    struct Scripted {
        provider: ProviderId,
        session: SessionId,
        native: Option<String>,
        /// Answered in order. Past the end it keeps answering that the stream is over.
        script: Vec<Step>,
        at: usize,
    }

    #[async_trait]
    impl Agent for Scripted {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            self.native.as_deref()
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            let step = self.script.get(self.at).copied().unwrap_or(Step::End);
            self.at += 1;
            step.answer(self.provider)
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct Fake {
        provider: ProviderId,
        script: Vec<Step>,
        native: Option<String>,
        /// The provider is not installed, which is the refusal a start has to preserve rather than flatten.
        not_installed: bool,
    }

    #[async_trait]
    impl Provider for Fake {
        fn id(&self) -> ProviderId {
            self.provider
        }

        async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
            if self.not_installed {
                return Err(ProviderError::BinNotFound {
                    provider: self.provider,
                    searched: "nowhere".to_owned(),
                });
            }
            Ok(Box::new(Scripted {
                provider: self.provider,
                session: intent.session,
                native: self.native.clone(),
                script: self.script.clone(),
                at: 0,
            }))
        }
    }

    fn an_id() -> ProviderId {
        ProviderId::parse("example").expect("the test's own id must be valid")
    }

    fn a_fake(script: Vec<Step>) -> Fake {
        Fake {
            provider: an_id(),
            script,
            native: Some("native-name".to_owned()),
            not_installed: false,
        }
    }

    fn an_intent(session: SessionId) -> OpenIntent {
        OpenIntent {
            session,
            workspace: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
                .expect("valid"),
            disposition: Disposition::Fresh,
            model: None,
            permission: None,
        }
    }

    fn workspace(tail: &str) -> AbsPath {
        let root = if cfg!(windows) {
            r"C:\runtrol-workspace-claim"
        } else {
            "/runtrol-workspace-claim"
        };
        AbsPath::new(&format!("{root}/{tail}")).expect("valid test workspace")
    }

    fn claim(tail: &str, access: WorkspaceAccess) -> WorkspaceClaim {
        WorkspaceClaim::discover(workspace(tail), access).expect("discover test workspace")
    }

    fn intent_at(session: SessionId, workspace: AbsPath) -> OpenIntent {
        OpenIntent {
            session,
            workspace,
            disposition: Disposition::Fresh,
            model: None,
            permission: None,
        }
    }

    fn an_agent(session: SessionId) -> Box<dyn Agent> {
        Box::new(Scripted {
            provider: an_id(),
            session,
            native: Some("native-name".to_owned()),
            script: Vec::new(),
            at: 0,
        })
    }

    fn content() -> Produced {
        Produced {
            src_end: 1,
            body: EventBody::Plan {
                payload: Opaque::none(),
            },
        }
    }

    fn turn_started(index: u32) -> Produced {
        Produced {
            src_end: 2,
            body: EventBody::Turn(TurnEvent::Started {
                turn: TurnId { epoch: 0, index },
            }),
        }
    }

    fn turn_ended(index: u32) -> Produced {
        Produced {
            src_end: 3,
            body: EventBody::Turn(TurnEvent::Ended {
                turn: TurnId { epoch: 0, index },
                stop: StopReason::EndTurn,
                declared_by: Declarant::Provider,
            }),
        }
    }

    #[tokio::test]
    async fn starting_a_session_makes_it_live_and_bound() {
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();

        let started = manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("a scripted driver opens");

        assert_eq!(started, session);
        assert!(manager.is_live(session));
        assert_eq!(manager.hot(), 1);
        assert_eq!(
            manager.state(session).map(|state| state.lifecycle().name()),
            Some("idle"),
            "the driver answered, so the session is bound and not in a turn"
        );
    }

    #[test]
    fn an_exclusive_opening_claim_atomically_blocks_overlapping_workspaces() {
        let mut manager = SessionManager::new();
        let first = SessionId::now();
        let second = SessionId::now();
        let reserved = manager
            .reserve_open(first, claim("repo", WorkspaceAccess::Exclusive))
            .expect("reserve the first writer");

        assert!(matches!(
            manager.reserve_open(second, claim("repo/src", WorkspaceAccess::Exclusive)),
            Err(SessionError::WorkspaceOccupied { session, .. }) if session == first
        ));

        manager.cancel_open(reserved.reservation);
        assert!(
            manager
                .reserve_open(second, claim("repo/src", WorkspaceAccess::Exclusive))
                .is_ok(),
            "releasing the exact opening claim permits the next writer"
        );
    }

    #[test]
    fn only_an_explicit_shared_start_can_overlap_an_existing_claim() {
        let mut manager = SessionManager::new();
        manager
            .reserve_open(SessionId::now(), claim("repo", WorkspaceAccess::Exclusive))
            .expect("reserve the exclusive writer");

        assert!(
            manager
                .reserve_open(SessionId::now(), claim("repo/src", WorkspaceAccess::Shared),)
                .is_ok(),
            "the operator explicitly accepted concurrent writers"
        );
    }

    #[test]
    fn provider_update_exclusion_blocks_only_its_provider_until_released() {
        let mut manager = SessionManager::new();
        let provider = an_id();
        let other = ProviderId::parse("other").expect("the test provider id is valid");
        let update = manager
            .reserve_provider_update(provider)
            .expect("an idle provider can be reserved for update");

        assert!(matches!(
            manager.reserve_open_for_provider(
                provider,
                SessionId::now(),
                claim("same", WorkspaceAccess::Shared),
            ),
            Err(SessionError::ProviderUpdating { provider: blocked }) if blocked == provider
        ));
        let other_open = manager
            .reserve_open_for_provider(
                other,
                SessionId::now(),
                claim("other", WorkspaceAccess::Shared),
            )
            .expect("a different provider remains available");
        manager.cancel_open(other_open.reservation);

        manager.release_provider_update(update);
        assert!(
            manager
                .reserve_open_for_provider(
                    provider,
                    SessionId::now(),
                    claim("same", WorkspaceAccess::Shared),
                )
                .is_ok(),
            "releasing the exact update lease admits the provider again"
        );
    }

    #[test]
    fn every_same_provider_process_phase_blocks_an_update() {
        let mut manager = SessionManager::new();
        let provider = an_id();
        let session = SessionId::now();
        let identity = claim("provider", WorkspaceAccess::Shared);
        let intent = intent_at(session, identity.identity().workspace().clone());
        let reserved = manager
            .reserve_open_for_provider(provider, session, identity)
            .expect("reserve the provider process");

        assert!(matches!(
            manager.reserve_provider_update(provider),
            Err(SessionError::ProviderBusyForUpdate { provider: busy }) if busy == provider
        ));
        manager
            .attach_opened(reserved.reservation, provider, &intent, an_agent(session))
            .expect("attach the provider process");
        assert!(matches!(
            manager.reserve_provider_update(provider),
            Err(SessionError::ProviderBusyForUpdate { provider: busy }) if busy == provider
        ));

        let closing = manager.close(session).expect("begin provider cleanup");
        assert!(matches!(
            manager.reserve_provider_update(provider),
            Err(SessionError::ProviderBusyForUpdate { provider: busy }) if busy == provider
        ));
        manager.release_closing(closing.reservation);
        let update = manager
            .reserve_provider_update(provider)
            .expect("finished process cleanup admits the update");
        manager.release_provider_update(update);
    }

    #[test]
    fn an_opened_process_cannot_change_the_provider_it_reserved() {
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let provider = an_id();
        let other = ProviderId::parse("other").expect("the test provider id is valid");
        let identity = claim("provider", WorkspaceAccess::Shared);
        let intent = intent_at(session, identity.identity().workspace().clone());
        let reserved = manager
            .reserve_open_for_provider(provider, session, identity)
            .expect("reserve the provider process");

        let error = manager
            .attach_opened(reserved.reservation, other, &intent, an_agent(session))
            .expect_err("the provider binding changed");
        let (error, agent, reservation) = error.into_parts();
        assert!(matches!(
            error,
            SessionError::ProviderReservationMismatch {
                session: refused,
                reserved,
                opened,
            } if refused == session && reserved == provider && opened == other
        ));
        drop(agent);
        manager.cancel_open(reservation);
    }

    #[test]
    fn a_closing_process_keeps_its_workspace_until_cleanup_finishes() {
        let mut manager = SessionManager::new();
        let first = SessionId::now();
        let identity = claim("repo", WorkspaceAccess::Exclusive);
        let intent = intent_at(first, identity.identity().workspace().clone());
        let reserved = manager
            .reserve_open(first, identity)
            .expect("reserve the first writer");
        manager
            .attach_opened(reserved.reservation, an_id(), &intent, an_agent(first))
            .expect("attach the first writer");

        let closing = manager.close(first).expect("begin close");
        assert!(matches!(
            manager.reserve_open(
                SessionId::now(),
                claim("repo/src", WorkspaceAccess::Exclusive)
            ),
            Err(SessionError::WorkspaceOccupied { session, .. }) if session == first
        ));

        manager.release_closing(closing.reservation);
        assert!(
            manager
                .reserve_open(
                    SessionId::now(),
                    claim("repo/src", WorkspaceAccess::Exclusive),
                )
                .is_ok(),
            "cleanup released the closing writer identity"
        );
    }

    #[test]
    fn an_opened_process_cannot_change_the_workspace_it_reserved() {
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let reserved = manager
            .reserve_open(session, claim("repo", WorkspaceAccess::Exclusive))
            .expect("reserve the requested workspace");
        let intent = intent_at(session, workspace("other"));

        let error = manager
            .attach_opened(reserved.reservation, an_id(), &intent, an_agent(session))
            .expect_err("a changed workspace must be refused");
        let (error, agent, reservation) = error.into_parts();
        assert!(matches!(
            error,
            SessionError::WorkspaceReservationMismatch { session: refused, .. }
                if refused == session
        ));
        drop(agent);
        manager.cancel_open(reservation);
        assert!(
            manager
                .reserve_open(session, claim("other", WorkspaceAccess::Exclusive))
                .is_ok(),
            "the refused process did not leak its claim"
        );
    }

    #[tokio::test]
    async fn a_driver_that_refuses_leaves_no_session_with_a_process() {
        // A session that failed to open is not a session with a process. Leaving one in the map would have it show up
        // in a listing as running.
        let provider = Fake {
            provider: an_id(),
            script: vec![],
            native: None,
            not_installed: true,
        };
        let mut manager = SessionManager::new();
        let session = SessionId::now();

        match manager.start_for_tests(&provider, an_intent(session)).await {
            Err(SessionError::Provider(ProviderError::BinNotFound { .. })) => {}
            other => panic!("expected the provider's own variant, got {other:?}"),
        }
        assert!(!manager.is_live(session));
        assert_eq!(manager.hot(), 0);
    }

    #[tokio::test]
    async fn cancelling_the_async_start_convenience_releases_its_reservation() {
        struct NeverOpens;

        #[async_trait]
        impl Provider for NeverOpens {
            fn id(&self) -> ProviderId {
                an_id()
            }

            async fn open(&self, _intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
                core::future::pending().await
            }
        }

        let mut manager = SessionManager::new();
        let cancelled = tokio::time::timeout(
            core::time::Duration::from_millis(1),
            manager.start_for_tests(&NeverOpens, an_intent(SessionId::now())),
        )
        .await;
        assert!(cancelled.is_err(), "the provider never opens");

        for _ in 0..MAX_HOT {
            manager
                .reserve_open_for_tests(SessionId::now())
                .expect("the cancelled start returned its slot");
        }
    }

    #[test]
    fn cancelling_an_open_reservation_returns_its_bounded_slot() {
        let mut manager = SessionManager::new();
        let mut reservations = Vec::new();
        for _ in 0..MAX_HOT {
            reservations.push(
                manager
                    .reserve_open_for_tests(SessionId::now())
                    .expect("an unused bounded slot"),
            );
        }
        assert!(matches!(
            manager.reserve_open_for_tests(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        let cancelled = reservations.pop().expect("one reservation").reservation;
        manager.cancel_open(cancelled);
        assert!(manager.reserve_open_for_tests(SessionId::now()).is_ok());
    }

    #[test]
    fn a_stale_open_reservation_cannot_release_a_new_closing_lease() {
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let old = manager
            .reserve_open_for_tests(session)
            .expect("one opening lease");
        let stale = OpenReservation {
            session: old.reservation.session,
            generation: old.reservation.generation,
        };
        manager.cancel_open(old.reservation);

        let current = manager
            .reserve_open_for_tests(session)
            .expect("a new opening lease");
        let intent = an_intent(session);
        manager
            .attach_opened(current.reservation, an_id(), &intent, an_agent(session))
            .expect("the process attaches");
        let closing = manager.close(session).expect("the process starts closing");

        manager.cancel_open(stale);
        assert!(matches!(
            manager.reserve_open_for_tests(session),
            Err(SessionError::AlreadyLive { session: held }) if held == session
        ));

        drop(closing.agent);
        manager.release_closing(closing.reservation);
        assert!(manager.reserve_open_for_tests(session).is_ok());
    }

    #[test]
    fn exhausted_reservation_generations_do_not_change_admission_state() {
        let mut opening = SessionManager::new();
        opening.next_reservation = u64::MAX;
        assert!(matches!(
            opening.reserve_open_for_tests(SessionId::now()),
            Err(SessionError::ReservationGenerationExhausted)
        ));
        assert_eq!(opening.hot(), 0);
        assert!(opening.opening.is_empty());

        let mut closing = SessionManager::new();
        let session = SessionId::now();
        let reserved = closing
            .reserve_open_for_tests(session)
            .expect("one opening lease");
        closing
            .attach_opened(
                reserved.reservation,
                an_id(),
                &an_intent(session),
                an_agent(session),
            )
            .expect("the process attaches");
        closing.next_reservation = u64::MAX;
        assert!(matches!(
            closing.close(session),
            Err(SessionError::ReservationGenerationExhausted)
        ));
        assert!(closing.is_live(session), "failed close kept its process");
        assert!(closing.opening.is_empty());
    }

    #[test]
    fn an_agent_handoff_is_exclusive_and_can_be_restored_or_abandoned() {
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let reserved = manager
            .reserve_open_for_tests(session)
            .expect("one opening lease");
        manager
            .attach_opened(
                reserved.reservation,
                an_id(),
                &an_intent(session),
                an_agent(session),
            )
            .expect("the process attaches");

        let taken = manager
            .take_agent(session)
            .expect("the first command owns it");
        assert!(matches!(
            manager.take_agent(session),
            Err(SessionError::AgentInFlight { session: busy }) if busy == session
        ));
        assert!(matches!(
            manager.close(session),
            Err(SessionError::AgentInFlight { session: busy }) if busy == session
        ));
        assert!(
            manager.return_agent(taken.lease, taken.agent).is_ok(),
            "the exact lease restores its agent"
        );

        let taken = manager
            .take_agent(session)
            .expect("it can be handed out again");
        manager.abandon_agent(taken.lease);
        drop(taken.agent);
        assert!(!manager.is_live(session));
    }

    #[test]
    fn exhausted_agent_handoff_generations_leave_the_agent_attached() {
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let reserved = manager
            .reserve_open_for_tests(session)
            .expect("one opening lease");
        manager
            .attach_opened(
                reserved.reservation,
                an_id(),
                &an_intent(session),
                an_agent(session),
            )
            .expect("the process attaches");
        manager.next_agent_lease = u64::MAX;

        assert!(matches!(
            manager.take_agent(session),
            Err(SessionError::AgentLeaseGenerationExhausted)
        ));
        assert!(manager.is_live(session));
        assert!(manager.in_flight.is_empty());
        assert!(
            manager.close(session).is_ok(),
            "the agent remained attached"
        );
    }

    #[test]
    fn attach_requires_the_reserved_session_and_preserves_the_exact_identity() {
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let reserved = manager
            .reserve_open_for_tests(session)
            .expect("one bounded slot");
        let intent = an_intent(session);

        let attached = manager
            .attach_opened(reserved.reservation, an_id(), &intent, an_agent(session))
            .expect("the matching process attaches");
        assert_eq!(attached.session, session);
        let live = manager.live_session(session).expect("the session is live");
        assert_eq!(live.session, session);
        assert_eq!(live.provider, an_id());
    }

    #[test]
    fn an_agent_for_another_session_is_returned_without_becoming_live() {
        let mut manager = SessionManager::new();
        let expected = SessionId::now();
        let actual = SessionId::now();
        let reserved = manager
            .reserve_open_for_tests(expected)
            .expect("one bounded slot");
        let intent = an_intent(expected);

        let error = manager
            .attach_opened(reserved.reservation, an_id(), &intent, an_agent(actual))
            .expect_err("a mismatched process must be refused");
        let (error, agent, reservation) = error.into_parts();
        assert!(matches!(
            error,
            SessionError::AgentSessionMismatch {
                expected: named_expected,
                actual: named_actual,
            } if named_expected == expected && named_actual == actual
        ));
        assert_eq!(agent.session(), actual, "the caller gets the process back");
        manager.cancel_open(reservation);
        assert_eq!(manager.hot(), 0);
        assert!(
            manager.reserve_open_for_tests(SessionId::now()).is_ok(),
            "a rejected attach consumed and released its reservation"
        );
    }

    #[test]
    fn pairing_a_reservation_with_another_intent_releases_the_reserved_slot() {
        let mut manager = SessionManager::new();
        let reserved_for = SessionId::now();
        let intent_for = SessionId::now();
        let reserved = manager
            .reserve_open_for_tests(reserved_for)
            .expect("one bounded slot");
        let intent = an_intent(intent_for);

        let error = manager
            .attach_opened(reserved.reservation, an_id(), &intent, an_agent(intent_for))
            .expect_err("the reservation is bound to its session");
        let (error, agent, reservation) = error.into_parts();
        assert!(matches!(
            error,
            SessionError::OpenNotReserved { session } if session == intent_for
        ));
        drop(agent);
        manager.cancel_open(reservation);

        let mut held = Vec::new();
        for _ in 0..MAX_HOT {
            held.push(
                manager
                    .reserve_open_for_tests(SessionId::now())
                    .expect("the mismatched reservation was released"),
            );
        }
        assert_eq!(held.len(), MAX_HOT);
    }

    #[tokio::test]
    async fn reserving_a_full_tier_hands_out_one_idle_process_before_open() {
        let provider = a_fake(Vec::new());
        let mut manager = SessionManager::new();
        for _ in 0..MAX_HOT {
            let session = SessionId::now();
            manager
                .start_for_tests(&provider, an_intent(session))
                .await
                .expect("fills one slot");
        }

        let reserved = manager
            .reserve_open_for_tests(SessionId::now())
            .expect("one idle session gives way");
        assert!(reserved.displaced.is_some());
        assert_eq!(manager.hot(), MAX_HOT - 1);
        assert!(matches!(
            manager.reserve_open_for_tests(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));
    }

    #[tokio::test]
    async fn a_duplicate_reservation_never_replaces_the_live_session() {
        let provider = a_fake(Vec::new());
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("the original session opens");

        assert!(matches!(
            manager.reserve_open_for_tests(session),
            Err(SessionError::AlreadyLive { session: duplicate }) if duplicate == session
        ));
        assert!(manager.is_live(session));
        assert_eq!(manager.hot(), 1);
    }

    #[tokio::test]
    async fn a_turn_starting_and_ending_moves_the_session_and_nothing_else_does() {
        let provider = a_fake(vec![
            Step::Content,
            Step::TurnStarted(0),
            Step::Content,
            Step::TurnEnded(0),
        ]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");

        manager
            .pump_once(session)
            .await
            .expect("content")
            .expect("published");
        assert_eq!(
            manager.state(session).map(|state| state.lifecycle().name()),
            Some("idle"),
            "content is content"
        );

        manager
            .pump_once(session)
            .await
            .expect("a turn begins")
            .expect("published");
        assert!(
            manager
                .state(session)
                .and_then(|state| state.lifecycle().turn())
                .is_some(),
            "a turn is running"
        );

        manager
            .pump_once(session)
            .await
            .expect("content")
            .expect("published");
        assert!(
            manager
                .state(session)
                .and_then(|state| state.lifecycle().turn())
                .is_some(),
            "content during a turn leaves it running"
        );

        manager
            .pump_once(session)
            .await
            .expect("the turn ends")
            .expect("published");
        assert_eq!(
            manager.state(session).map(|state| state.lifecycle().name()),
            Some("idle")
        );
    }

    #[tokio::test]
    async fn events_reach_a_watcher_numbered_and_in_order() {
        let provider = a_fake(vec![Step::Content, Step::Content, Step::Content]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");

        let mut watcher = manager
            .subscribe(session, None)
            .expect("a live session can be watched");
        for _ in 0..3 {
            manager.pump_once(session).await.expect("published");
        }

        let mut positions = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            positions.push(watch_event(frame).seq);
        }
        assert_eq!(positions, vec![0, 1, 2], "dense and in order");
    }

    #[tokio::test]
    async fn a_provider_failure_becomes_session_state_and_is_said_out_loud() {
        // A failure only returned to a caller is a failure the operator never hears about.
        let provider = a_fake(vec![Step::Broken]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");
        let mut watcher = manager.subscribe(session, None).expect("watchable");

        let published = manager
            .pump_once(session)
            .await
            .expect("a failure is not returned as an error")
            .expect("it is published");

        match &published.event.body {
            EventBody::Notice(notice) => {
                assert_eq!(notice.code, NoticeCode::ProtocolViolation);
                assert_eq!(notice.level, Level::Error);
                assert!(notice.payload.as_str().contains("changed shape"));
            }
            other => panic!("expected a notice, got {other:?}"),
        }
        assert!(watcher.try_recv().is_some(), "and a watcher receives it");
        assert!(
            !manager.is_live(session),
            "a session whose stream broke has no process"
        );
    }

    /// A driver that never has anything ready, and never asks to be woken.
    ///
    /// Nothing waits on it alone. It is here so that a session which has nothing to say can be shown not to hold
    /// up a session that does, which is the whole reason waiting on all of them is one call.
    struct Silent(SessionId);

    #[async_trait]
    impl Agent for Silent {
        fn session(&self) -> SessionId {
            self.0
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            core::future::pending().await
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct Quiet;

    #[async_trait]
    impl Provider for Quiet {
        fn id(&self) -> ProviderId {
            an_id()
        }

        async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
            Ok(Box::new(Silent(intent.session)))
        }
    }

    #[tokio::test]
    async fn a_session_with_nothing_to_say_does_not_hold_up_one_that_does() {
        // The reason waiting on every session is one call. A supervisor that had to ask them in order would stop
        // at the first quiet one, and every other session would go unheard until that one spoke.
        let mut manager = SessionManager::new();
        let quiet = SessionId::now();
        manager
            .start_for_tests(&Quiet, an_intent(quiet))
            .await
            .expect("opens");
        let talkative = SessionId::now();
        manager
            .start_for_tests(
                &a_fake(vec![Step::Content, Step::Content]),
                an_intent(talkative),
            )
            .await
            .expect("opens");

        let first = manager.pump_any().await;
        assert_eq!(
            first.session, talkative,
            "the session with something ready is the one that is heard"
        );
        assert!(first.published.is_some());
        assert!(
            first.index_changed,
            "the first frame reveals the provider-native identifier"
        );

        let heard = manager.pump_any().await;
        assert!(heard.published.is_some());
        assert!(
            !heard.index_changed,
            "content with stable identity must not rebuild the session index"
        );
    }

    #[tokio::test]
    async fn a_lifecycle_event_marks_the_session_index_changed() {
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&a_fake(vec![Step::TurnStarted(0)]), an_intent(session))
            .await
            .expect("opens");

        let heard = manager.pump_any().await;
        assert_eq!(heard.session, session);
        assert!(heard.index_changed, "busy is visible in the session index");
    }

    #[tokio::test]
    async fn no_session_is_heard_twice_before_every_other_one_is_heard_once() {
        // Without this, a session producing a long stream keeps every other session silent for as long as it
        // talks, and an operator watching a second session sees nothing at all.
        let provider = a_fake(vec![Step::Content, Step::Content, Step::Content]);
        let mut manager = SessionManager::new();
        let mut names = Vec::new();
        for _ in 0..3 {
            let session = SessionId::now();
            manager
                .start_for_tests(&provider, an_intent(session))
                .await
                .expect("opens");
            names.push(session);
        }

        let mut heard = Vec::new();
        for _ in 0..3 {
            heard.push(manager.pump_any().await.session);
        }
        heard.sort_unstable();
        let mut expected = names.clone();
        expected.sort_unstable();
        assert_eq!(
            heard, expected,
            "every session was heard once before any was heard twice"
        );

        // And the round after that starts over rather than sticking to the last one.
        assert_eq!(
            manager.pump_any().await.session,
            *names.first().expect("one")
        );
    }

    #[tokio::test]
    async fn waiting_on_every_session_applies_the_same_rules_as_waiting_on_one() {
        // Two ways of pumping and one rule about what an event means. A stream that ends has to take its session
        // out of the live map whichever way it was heard.
        let provider = a_fake(vec![Step::End]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");

        let heard = manager.pump_any().await;
        assert_eq!(heard.session, session);
        assert!(heard.published.is_none(), "the stream is over");
        assert!(
            heard.index_changed,
            "the removed hot session changes the index"
        );
        assert!(!manager.is_live(session));
    }

    #[tokio::test]
    async fn a_stream_that_ends_takes_the_session_out_of_the_live_map() {
        let provider = a_fake(vec![Step::Content, Step::End]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");

        manager
            .pump_once(session)
            .await
            .expect("content")
            .expect("published");
        assert!(
            manager
                .pump_once(session)
                .await
                .expect("the stream ends")
                .is_none()
        );
        assert!(!manager.is_live(session));
        assert_eq!(manager.hot(), 0);
    }

    #[tokio::test]
    async fn a_driver_reporting_something_impossible_produces_a_notice_and_not_a_panic() {
        // A supervisor that aborted on one misbehaving driver would take every other session with it.
        let provider = a_fake(vec![Step::TurnEnded(9)]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");
        let mut watcher = manager.subscribe(session, None).expect("watchable");

        manager
            .pump_once(session)
            .await
            .expect("published")
            .expect("a frame");

        let mut frames = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            frames.push(watch_event(frame));
        }
        let refusal = frames
            .iter()
            .find_map(|frame| match &frame.body {
                EventBody::Notice(notice) => Some(notice),
                _ => None,
            })
            .expect("the impossible report must reach a watcher");
        assert_eq!(refusal.code, NoticeCode::ProtocolViolation);
        assert!(
            manager.is_live(session),
            "one misbehaving report does not end the session"
        );
    }

    #[tokio::test]
    async fn the_providers_own_name_is_recorded_as_soon_as_it_is_announced() {
        let provider = a_fake(vec![Step::Content]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");

        manager.pump_once(session).await.expect("published");
        assert_eq!(
            manager.native(session),
            Some("native-name"),
            "a resume needs the name the provider knows"
        );
    }

    #[tokio::test]
    async fn a_resume_does_not_give_one_conversation_a_second_identity() {
        // Minting for a resume would show the same conversation twice in one list.
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let mut intent = an_intent(session);
        intent.disposition = Disposition::Resume {
            native: "already-exists".into(),
        };

        manager
            .start_for_tests(&provider, intent)
            .await
            .expect("opens");
        assert_eq!(manager.native(session), Some("already-exists"));
    }

    #[tokio::test]
    async fn the_hot_bound_holds_and_the_forgotten_session_gives_way() {
        // A thousand sessions in a list must not mean a thousand child processes. The bound is about the operator's
        // machine: at the measured working sets of these CLIs, eight of them is gigabytes.
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let mut names = Vec::new();
        for _ in 0..crate::session::tier::MAX_HOT {
            let session = SessionId::now();
            manager
                .start_for_tests(&provider, an_intent(session))
                .await
                .expect("opens");
            names.push(session);
        }
        assert_eq!(manager.hot(), crate::session::tier::MAX_HOT);

        let extra = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(extra))
            .await
            .expect("room is made");

        assert_eq!(
            manager.hot(),
            crate::session::tier::MAX_HOT,
            "the bound held"
        );
        assert!(manager.is_live(extra));
        assert!(
            names.iter().filter(|one| manager.is_live(**one)).count()
                == crate::session::tier::MAX_HOT - 1,
            "exactly one gave way"
        );
    }

    #[tokio::test]
    async fn a_session_with_a_turn_running_is_never_the_one_that_gives_way() {
        let provider = a_fake(vec![Step::TurnStarted(0)]);
        let mut manager = SessionManager::new();
        let mut names = Vec::new();
        for _ in 0..crate::session::tier::MAX_HOT {
            let session = SessionId::now();
            manager
                .start_for_tests(&provider, an_intent(session))
                .await
                .expect("opens");
            names.push(session);
        }

        // Put the oldest into a turn, which is the one an eviction would otherwise choose.
        let oldest = *names.first().expect("one session");
        manager.pump_once(oldest).await.expect("a turn begins");

        manager
            .start_for_tests(&provider, an_intent(SessionId::now()))
            .await
            .expect("room is made from an idle session");
        assert!(
            manager.is_live(oldest),
            "the busy session was evicted despite being the oldest"
        );
    }

    #[tokio::test]
    async fn when_every_session_is_busy_a_new_one_is_refused_with_a_reason() {
        // Refused rather than resolved by force. An operator told why can wait or stop something; one whose running
        // turn was interrupted has lost work and does not know it.
        let provider = a_fake(vec![Step::TurnStarted(0)]);
        let mut manager = SessionManager::new();
        for _ in 0..crate::session::tier::MAX_HOT {
            let session = SessionId::now();
            manager
                .start_for_tests(&provider, an_intent(session))
                .await
                .expect("opens");
            manager.pump_once(session).await.expect("a turn begins");
        }

        match manager
            .start_for_tests(&provider, an_intent(SessionId::now()))
            .await
        {
            Err(SessionError::NoRoom(refusal)) => {
                assert_eq!(refusal.held, crate::session::tier::MAX_HOT);
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn closing_takes_the_process_away() {
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");

        let stopping = manager.close(session).expect("closing works");
        assert!(
            !manager.is_live(session),
            "the session is gone before anything waits for the process"
        );
        stopping
            .agent
            .close(CloseMode::Kill)
            .await
            .expect("the driver stops");
        manager.release_closing(stopping.reservation);

        match manager.close(session) {
            Err(SessionError::NotLive { session: named }) => assert_eq!(named, session),
            Ok(_) => panic!("a session that is gone cannot be closed again"),
            Err(other) => panic!("expected a refusal naming the session, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_closing_process_keeps_its_bounded_slot_until_cleanup_finishes() {
        let provider = a_fake(Vec::new());
        let mut manager = SessionManager::new();
        let mut sessions = Vec::new();
        for _ in 0..MAX_HOT {
            let session = SessionId::now();
            manager
                .start_for_tests(&provider, an_intent(session))
                .await
                .expect("fills one process slot");
            sessions.push(session);
        }

        let closing = manager
            .close(*sessions.first().expect("one live session"))
            .expect("the process is handed out for cleanup");
        assert!(matches!(
            manager.reserve_open_for_tests(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        drop(closing.agent);
        manager.release_closing(closing.reservation);
        assert!(
            manager.reserve_open_for_tests(SessionId::now()).is_ok(),
            "the slot returns only after cleanup"
        );
    }

    #[tokio::test]
    async fn closing_does_not_make_anybody_wait_for_a_process_to_stop() {
        // The reason this is not one call. A graceful close gives the process seconds to finish, and doing that here
        // would hold the only owner of every session for that long: one session being closed would stop every other
        // session's output. What this owns has changed by the time it returns.
        struct Slow(SessionId);

        #[async_trait]
        impl Agent for Slow {
            fn session(&self) -> SessionId {
                self.0
            }
            fn native(&self) -> Option<&str> {
                None
            }
            async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
                Ok(())
            }
            async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
                core::future::pending().await
            }
            async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
                core::future::pending::<()>().await;
                Ok(())
            }
        }

        struct Sluggish;

        #[async_trait]
        impl Provider for Sluggish {
            fn id(&self) -> ProviderId {
                an_id()
            }
            async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
                Ok(Box::new(Slow(intent.session)))
            }
        }

        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&Sluggish, an_intent(session))
            .await
            .expect("opens");

        // A driver that never finishes stopping. Taking it away still returns, and what it returns is the wait
        // somebody else is free to do.
        let stopping = manager.close(session).expect("the session is taken away");
        assert!(!manager.is_live(session));
        assert_eq!(manager.hot(), 0);
        drop(stopping.agent);
        manager.release_closing(stopping.reservation);
    }

    #[tokio::test]
    async fn nothing_can_be_asked_of_a_session_that_is_not_live() {
        let mut manager = SessionManager::new();
        let absent = SessionId::now();
        assert!(matches!(
            manager.pump_once(absent).await,
            Err(SessionError::NotLive { .. })
        ));
        assert!(matches!(
            manager.send(absent, AgentCommand::Interrupt).await,
            Err(SessionError::NotLive { .. })
        ));
        assert!(matches!(
            manager.subscribe(absent, None),
            Err(SessionError::NotLive { .. })
        ));
        assert_eq!(manager.state(absent), None);
    }

    #[tokio::test]
    async fn a_listing_of_live_sessions_carries_both_names_and_the_tier() {
        let provider = a_fake(vec![Step::Content]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start_for_tests(&provider, an_intent(session))
            .await
            .expect("opens");
        manager.pump_once(session).await.expect("published");

        let listed: Vec<LiveSession<'_>> = manager.live_sessions().collect();
        assert_eq!(listed.len(), 1);
        let one = listed.first().expect("one session");
        assert_eq!(one.session, session);
        assert_eq!(one.provider, an_id());
        assert_eq!(one.native, Some("native-name"));
        assert_eq!(one.tier, Tier::Hot, "a listing of live sessions is all hot");
    }

    #[test]
    fn only_the_turns_own_frames_say_anything_about_the_state() {
        // Most events are content, and content must not move a session. Getting this wrong is how a message ends a
        // turn.
        assert!(
            observation_of(&EventBody::Plan {
                payload: Opaque::none(),
            })
            .is_none()
        );
        assert!(matches!(
            observation_of(&turn_started(0).body),
            Some(Observed::TurnStarted { .. })
        ));
        assert!(matches!(
            observation_of(&turn_ended(0).body),
            Some(Observed::TurnEnded { .. })
        ));
        assert!(
            observation_of(&EventBody::Turn(TurnEvent::Blocked {
                turn: TurnId { epoch: 0, index: 0 },
                on: runtrol_provider::BlockedOn::UserInput,
            }))
            .is_none(),
            "waiting on a person is not a change of whether a turn is running"
        );
    }

    #[test]
    fn a_notice_runtrol_originates_never_pastes_text_into_structure() {
        // The detail can carry a provider's own words, and pasting would make those words into structure.
        let body = notice(NoticeCode::Other, Level::Info, r#"a "quoted" }brace{ mess"#);
        match body {
            EventBody::Notice(one) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(one.payload.as_str()).expect("readable JSON");
                assert_eq!(
                    parsed.pointer("/detail").and_then(|v| v.as_str()),
                    Some(r#"a "quoted" }brace{ mess"#)
                );
            }
            other => panic!("expected a notice, got {other:?}"),
        }
    }
}
