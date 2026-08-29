//! What the command surface asks and what the daemon answers.
//!
//! # Every request names what it is about
//!
//! There is no current session and no selected provider. Two processes holding a notion of "the one you meant" is two
//! places for that notion to disagree, and the way it goes wrong is the worst available: a command lands on a session
//! the operator was not looking at. So every request that concerns a session carries it.
//!
//! # An event crosses as bytes that were serialized once
//!
//! [`Response::Event`] carries an event that is already encoded. The daemon encodes it once and hands the same bytes to
//! every watcher, so a session with a phone, a terminal and a window watching costs one encode rather than three. It is
//! also the last place a conversation could be re-read, and there is nothing here that could: the bytes go from a
//! provider's line to a subscriber's screen without runtrol looking inside.
//!
//! # Why the tag sits beside the content and not inside it
//!
//! Measured: an internally tagged enum cannot carry a pass-through payload at all. Putting the tag inside the content
//! forces the encoder to buffer that content into a model first so it can add a field to it, and buffering a payload is
//! exactly the re-reading this whole design exists to avoid. It fails outright rather than degrading, which is the
//! better of the two ways to find out.
//!
//! So the tag is a field beside the content. Both directions use the same shape, because two shapes on one wire is one
//! more thing for a reader to get wrong.
//!
//! # Why the error carries two flags rather than a code to look up
//!
//! A client makes exactly two decisions about a failure: whether to try again, and whether to tell the operator to go
//! to their machine. Those are on the value, so a client cannot get them wrong by branching on a number whose meaning
//! lives somewhere else.

use runtrol_provider::{
    ApprovalId, ModelCatalog, Opaque, OptionId, ProviderError, SessionId, TerminalId, WatchCursor,
    WatchGap, WorkspaceAccess,
};
use serde::{Deserialize, Serialize};

use crate::frame::WIRE_VERSION;

/// What the command surface asks for.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "ask", content = "with", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Request {
    /// Open the conversation and agree on a wire format.
    ///
    /// First on every connection. A side that hears a version it does not speak refuses by name rather than reading the
    /// rest with the wrong meaning.
    Hello {
        /// The wire format the caller speaks.
        wire: u8,
    },

    /// Every session this machine has.
    List,

    /// Watch the session index.
    ///
    /// The daemon sends one current [`Response::Sessions`] snapshot after the acknowledgement, then another only
    /// when a list-visible value changes. Conversation frames never enter this stream.
    WatchSessions,

    /// Discover the current model choices for one provider.
    Models {
        /// Which provider to ask. The driver owns how discovery works.
        provider: Box<str>,
    },

    /// Inspect update ownership and registry freshness for every provider.
    ///
    /// This starts package-manager queries, so it is explicit rather than part of the greeting or session list.
    ProviderUpdates,

    /// Update one installed provider to the greatest plain release in its confirmed registry.
    ProviderUpdate {
        /// Provider whose existing package may change.
        provider: Box<str>,
    },

    /// Read the optional remote relay origin and its current connection state.
    RemoteConnection,

    /// Set or clear the relay origin from the local VS Code surface.
    RemoteConfigure {
        /// Exact lowercase HTTPS origin, or no value to disable remote connections.
        relay_origin: Option<Box<str>>,
    },

    /// Create one short-lived phone pairing QR at the local VS Code surface.
    PairingBegin,

    /// List authenticated phone proposals awaiting a local decision.
    PairingProposals,

    /// Begin one exact local presence challenge for a phone and its initial scopes.
    PairingApprovalBegin {
        /// Opaque pending proposal identity.
        proposal_id: Box<str>,
        /// Exact initial scope names selected locally.
        scopes: Vec<Box<str>>,
    },

    /// Answer and atomically spend one phone pairing challenge.
    PairingApprovalFinish {
        /// Opaque one-use local challenge identity.
        challenge_id: Box<str>,
        /// Phrase typed from the local VS Code prompt.
        answer: Box<str>,
    },

    /// Deny one authenticated phone proposal locally.
    PairingDeny {
        /// Opaque pending proposal identity.
        proposal_id: Box<str>,
    },

    /// List durable paired phones for local administration.
    Devices,

    /// Revoke one paired phone and all of its scopes locally.
    DeviceRevoke {
        /// Locally minted device identity.
        device_id: Box<str>,
    },

    /// Begin replacing one paired phone's exact scopes, workspace roots, and providers locally.
    DeviceAuthorityBegin {
        /// Locally minted device identity.
        device_id: Box<str>,
        /// Complete plain scope replacement.
        scopes: Vec<Box<str>>,
        /// Exact workspace paths selected at the PC.
        roots: Vec<Box<str>>,
        /// Exact runtime-discovered provider identities selected at the PC.
        providers: Vec<Box<str>>,
    },

    /// Answer and atomically spend one device authority replacement challenge.
    DeviceAuthorityFinish {
        /// Opaque one-use local challenge identity.
        challenge_id: Box<str>,
        /// Phrase typed from the local VS Code prompt.
        answer: Box<str>,
    },

    /// Replace or clear this authenticated phone's bodyless Web Push subscription.
    PushSubscription {
        /// Browser-issued HTTPS push capability URL, or no value to disable delivery.
        endpoint: Option<Box<str>>,
    },

    /// List bounded pending public Runtime integration enrollments for local administration.
    IntegrationEnrollments,

    /// Begin a physical-presence challenge for an exact narrowed enrollment grant.
    IntegrationApprovalBegin {
        /// Opaque pending enrollment identity.
        pending_id: Box<str>,
        /// Exact requested scopes retained by the operator.
        scopes: Vec<Box<str>>,
        /// Exact requested roots retained by the operator.
        roots: Vec<Box<str>>,
    },

    /// Answer and atomically spend one integration approval challenge.
    IntegrationApprovalFinish {
        /// Opaque one-use local challenge identity.
        challenge_id: Box<str>,
        /// Phrase typed from the local VS Code prompt.
        answer: Box<str>,
    },

    /// Approve one pending integration for the exact key that requested it.
    ///
    /// The caller signs its own pending identity, so this spends an enrollment only for whoever created it. It
    /// grants the enrollment as requested. Narrowing a grant stays a reviewed decision and keeps its phrase.
    IntegrationSelfApprove {
        /// Opaque pending enrollment identity.
        pending_id: Box<str>,
        /// Base64url Ed25519 signature over the canonical self-approval payload.
        signature: Box<str>,
    },

    /// Deny one pending integration without granting authority.
    IntegrationEnrollmentDeny {
        /// Opaque pending enrollment identity.
        pending_id: Box<str>,
    },

    /// List approved and revoked public Runtime integrations for local administration.
    Integrations,

    /// Show measured installation state and declared user-run help for one provider.
    ProviderHelp {
        /// Exact runtime-discovered provider identity.
        provider_id: Box<str>,
    },

    /// Revoke one integration and retire its current public connections on their next request.
    IntegrationRevoke {
        /// Opaque approved integration identity.
        integration_id: Box<str>,
    },

    /// Replace one active integration's exact scopes and project roots after local review.
    IntegrationGrantChange {
        /// Opaque approved integration identity.
        integration_id: Box<str>,
        /// Grant generation shown when the operator began the review.
        expected_grant_generation: u64,
        /// Complete replacement scope set.
        scopes: Vec<Box<str>>,
        /// Complete replacement project-root set.
        roots: Vec<Box<str>>,
    },

    /// List public Runtime session-forget requests awaiting one local decision.
    RuntimeForgetRequests,

    /// Confirm one exact public Runtime session-forget request at the machine.
    RuntimeForgetConfirm {
        /// Opaque one-use local confirmation identity.
        confirmation_id: Box<str>,
    },

    /// List public Runtime integration-key rotations awaiting one local decision.
    RuntimeKeyRotationRequests,

    /// Confirm one exact public Runtime integration-key rotation at the machine.
    RuntimeKeyRotationConfirm {
        /// Opaque one-use local confirmation identity.
        confirmation_id: Box<str>,
    },

    /// List public Runtime shared-writer session opens awaiting one local decision.
    RuntimeSharedOpenRequests,

    /// Confirm one exact public Runtime shared-writer session open at the machine.
    RuntimeSharedOpenConfirm {
        /// Opaque one-use local confirmation identity.
        confirmation_id: Box<str>,
    },

    /// Create or resolve one Core-owned linked worktree for an ordinary chat.
    WorkspaceIsolatePrepare {
        /// Caller-minted canonical UUID, reused after an ambiguous reply.
        request_id: Box<str>,
        /// Exact Git checkout selected locally.
        project: Box<str>,
    },

    /// List bounded Core-owned ordinary-chat worktrees for local presentation and restart recovery.
    WorkspaceIsolateList,

    /// Bind one prepared worktree to the exact Runtime session that opened in it.
    WorkspaceIsolateBind {
        /// Core-owned workspace identity returned by preparation.
        workspace_id: Box<str>,
        /// Public Runtime session identity.
        session_id: Box<str>,
        /// Exact canonical worktree returned by preparation.
        workspace: Box<str>,
    },

    /// Release one exact Core-owned worktree after its Runtime session closes or a start fails.
    WorkspaceIsolateRelease {
        /// Core-owned identity when the caller received it, otherwise absent after an ambiguous bind.
        workspace_id: Option<Box<str>>,
        /// Bound Runtime session when one was established.
        session_id: Option<Box<str>>,
        /// Exact canonical worktree observed by the caller.
        workspace: Box<str>,
    },

    /// Begin a conversation that does not exist yet.
    Start {
        /// Which CLI.
        provider: Box<str>,
        /// Where the agent works.
        workspace: Box<str>,
        /// Whether this start must own the working tree alone.
        workspace_access: WorkspaceAccess,
        /// The model to ask for, when the operator chose one.
        model: Option<Box<str>>,
        /// The permission posture to start at, when the operator chose one.
        permission: Option<Box<str>>,
    },

    /// Continue a conversation the provider already has.
    Resume {
        /// Which CLI.
        provider: Box<str>,
        /// The provider's own name for the conversation.
        native: Box<str>,
        /// Where the agent works.
        workspace: Box<str>,
        /// Whether this resumed process must own the working tree alone.
        workspace_access: WorkspaceAccess,
    },

    /// Send what the operator wrote.
    Prompt {
        /// Which session.
        session: SessionId,
        /// What they wrote, carried and never rewritten.
        text: Box<str>,
    },

    /// Give a session a short operator-owned display name, or clear it.
    ///
    /// The name is metadata only. It is never derived from conversation content and never changes the provider's
    /// own session identifier.
    Rename {
        /// Which session.
        session: SessionId,
        /// The new display name. No value clears the custom name.
        label: Option<Box<str>>,
    },

    /// Choose one option from a provider approval that is still pending.
    AnswerApproval {
        /// Which session owns the approval.
        session: SessionId,
        /// The runtrol approval identifier shown with the request.
        approval: ApprovalId,
        /// The exact provider-offered option the operator chose.
        option: OptionId,
        /// The digest shown with the subject, binding the answer to that exact content.
        subject_digest: [u8; 32],
    },

    /// Stop the turn that is running.
    ///
    /// A request, not an outcome. What ends the turn is still the provider's own word, arriving as an event.
    Interrupt {
        /// Which session.
        session: SessionId,
    },

    /// Watch a session's events.
    Watch {
        /// Which session.
        session: SessionId,
        /// The next event the caller expects, or no cursor for the bounded initial view.
        #[serde(default)]
        after: Option<WatchCursor>,
    },

    /// Open a provider's own terminal interface on a conversation, hosted on a daemon-owned pseudo terminal,
    /// and turn this connection into a view of it.
    ///
    /// If that conversation's terminal is already open, this joins it rather than starting a second process.
    /// From here on the connection carries [`Response::TerminalOutput`] down and [`Request::TerminalInput`] and
    /// [`Request::TerminalResize`] up, until [`Response::TerminalExited`] or either end goes away.
    TerminalOpen {
        /// Which CLI.
        provider: Box<str>,
        /// Exact local argv after the provider command, when this request came from the transparent execution
        /// bridge. Absent for ordinary surface opens, which use the manifest's new or resume declaration.
        #[serde(default)]
        arguments: Option<Vec<Box<str>>>,
        /// The provider's own name for the conversation to reopen, or none for a fresh one.
        #[serde(default)]
        native: Option<Box<str>>,
        /// Where the CLI works. The CLI reads trust from it, so it is never a temporary folder.
        workspace: Box<str>,
        /// This viewer's columns.
        cols: u16,
        /// This viewer's rows.
        rows: u16,
    },

    /// Join a terminal that is already open (a second viewer, such as a phone), and turn this connection into
    /// a view of it the same way [`Request::TerminalOpen`] does.
    TerminalAttach {
        /// Which terminal.
        terminal: TerminalId,
        /// This viewer's columns.
        cols: u16,
        /// This viewer's rows.
        rows: u16,
    },

    /// Bytes the viewer typed or its mouse reported, on a connection that is a terminal view.
    TerminalInput {
        /// The bytes, exactly as the viewer's terminal produced them.
        bytes: TerminalBytes,
    },

    /// The viewer's size changed, on a connection that is a terminal view.
    TerminalResize {
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },

    /// End a session.
    Close {
        /// Which session.
        session: SessionId,
        /// Stop it now rather than letting it finish.
        now: bool,
    },

    /// Stop every agent on this machine.
    ///
    /// Carries nothing, on purpose. The security posture requires this to work from anywhere with no permission at all,
    /// and a request with no arguments has nothing an attacker could aim. The worst it achieves is stopping work, which
    /// is the safe direction.
    StopEverything,

    /// A newer daemon generation has started beside this one: hand it the durable store and finish.
    ///
    /// Never refused. The daemon releases the database at once so the successor can open it, stops taking new
    /// conversations, keeps serving the turns already running, and exits by itself once none is left. The
    /// successor sends this from its own startup; nothing a person types carries it.
    Drain,

    /// Establish the successor-owned authority relay after this generation began draining.
    GenerationHandoff {
        /// Exact successor executable digest.
        successor_digest: Box<str>,
        /// Current successor grant rows. The draining generation intersects them with its frozen ceiling.
        authorities: Vec<GenerationAuthorityLine>,
        /// Current successor-owned native live claims.
        claims: Vec<GenerationLiveClaimLine>,
    },

    /// Refresh a previously established successor-owned authority and native-claim relay.
    GenerationAuthorityUpdate {
        /// Exact successor executable digest bound by the handoff.
        successor_digest: Box<str>,
        /// Complete current successor grant rows.
        authorities: Vec<GenerationAuthorityLine>,
        /// Complete current successor-owned native live claims.
        claims: Vec<GenerationLiveClaimLine>,
    },

    /// Every cross-consult direction this build knows, with its current wired state.
    ///
    /// Read-only: the state lives in the CLIs' own configuration and is asked for fresh, so there is no second
    /// place for it to go stale.
    Consult,

    /// Register `to` as a consultable MCP server inside `from`, using `from`'s own official command.
    ConsultWire {
        /// The CLI that gains a consultant.
        from: Box<str>,
        /// The CLI whose opinion becomes reachable mid-turn.
        to: Box<str>,
    },

    /// Undo [`Request::ConsultWire`] with `from`'s own removal command, restoring its configuration.
    ConsultUnwire {
        /// The CLI that loses its consultant.
        from: Box<str>,
        /// The CLI being unregistered.
        to: Box<str>,
    },

    /// Register this exact runtrol executable as an Agent Tools MCP server in every usable provider CLI.
    ///
    /// Each provider's own official registration command performs the change. No provider configuration file
    /// is read or written by runtrol.
    AgentToolsWire,

    /// Remove this exact runtrol Agent Tools MCP registration through every usable provider CLI.
    AgentToolsUnwire,
}

/// Bytes on the terminal view, in both directions: what the hosted CLI wrote, or what a viewer typed.
///
/// Base64 on the wire because the wire is JSON text and these are arbitrary bytes that must arrive exactly
/// as produced. Nothing reads them in between.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct TerminalBytes(Vec<u8>);

impl TerminalBytes {
    /// The bytes.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for TerminalBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for TerminalBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for TerminalBytes {
    /// The length only. Terminal bytes are conversation, and a debug line is not where they go.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TerminalBytes({} bytes)", self.0.len())
    }
}

impl Serialize for TerminalBytes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64ct::Encoding as _;
        serializer.serialize_str(&base64ct::Base64::encode_string(&self.0))
    }
}

impl<'de> Deserialize<'de> for TerminalBytes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use base64ct::Encoding as _;
        let text = String::deserialize(deserializer)?;
        base64ct::Base64::decode_vec(&text)
            .map(Self)
            .map_err(|error| {
                serde::de::Error::custom(format!("terminal bytes are base64: {error}"))
            })
    }
}

/// What the daemon answers.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "say", content = "with", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Response {
    /// The wire format is agreed, and here is what this build has.
    Welcome {
        /// The wire format the daemon speaks.
        wire: u8,
        /// Every provider it knows about, usable or not.
        ///
        /// Including the ones it cannot serve. An operator with a perfectly good manifest for a kind this build has no
        /// driver for should see it marked rather than wonder where it went.
        providers: Vec<ProviderLine>,
        /// Current caller authority when this is an authenticated paired-device connection.
        device: Option<Box<DeviceAuthorityLine>>,
        /// Stable VAPID application-server key on authenticated paired-device connections.
        push_public_key: Option<Box<str>>,
        /// SHA-256 of the executable this daemon is running: its generation.
        ///
        /// Added 2026-08-20. A client compares this with the build it installed to know whether it
        /// is talking to its own generation. Absent only on daemons older than that date.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_digest: Option<Box<str>>,
    },

    /// The sessions and any damaged rows the daemon could not read.
    Sessions(SessionListing),

    /// The model choices one provider can honestly offer now.
    Models(ModelCatalog),

    /// Current provider package ownership and available release status.
    ProviderUpdates(Vec<ProviderUpdateLine>),

    /// Result of one locally authorized provider update attempt.
    ProviderUpdated(ProviderUpdateResult),

    /// Current optional relay configuration and non-secret availability.
    RemoteConnection(RemoteConnection),

    /// One secret-bearing QR value returned only to the local pairing command.
    PairingInvitation(PairingInvitationLine),

    /// Authenticated phone proposals awaiting local approval.
    PairingProposals(Vec<PairingProposalLine>),

    /// One exact local pairing challenge.
    PairingApprovalChallenge {
        /// Opaque one-use challenge identity.
        challenge_id: Box<str>,
        /// Complete exact action and random phrase for local display.
        prompt: Box<str>,
    },

    /// Durable paired phones visible only at the machine.
    Devices(Vec<DeviceLine>),

    /// One exact local paired-device authority replacement challenge.
    DeviceAuthorityChallenge {
        /// Opaque one-use challenge identity.
        challenge_id: Box<str>,
        /// Complete scopes, paths, providers, and random phrase for local display.
        prompt: Box<str>,
    },

    /// Pending public Runtime integration enrollments visible only at the machine.
    IntegrationEnrollments(Vec<IntegrationEnrollmentLine>),

    /// One exact local challenge that must be typed before approval.
    IntegrationApprovalChallenge {
        /// Opaque one-use challenge identity.
        challenge_id: Box<str>,
        /// Complete exact action and random phrase for local display.
        prompt: Box<str>,
    },

    /// An enrollment became one durable integration grant.
    IntegrationApproved {
        /// Opaque approved integration identity.
        integration_id: Box<str>,
    },

    /// Approved and revoked public Runtime integrations visible only at the machine.
    Integrations(Vec<IntegrationLine>),

    /// Measured provider state and manifest-declared commands safe for local display.
    ProviderHelp(Box<ProviderHelpLine>),

    /// Public Runtime session-forget requests awaiting one local decision.
    RuntimeForgetRequests(Vec<RuntimeForgetLine>),

    /// Public Runtime integration-key rotations awaiting one local decision.
    RuntimeKeyRotationRequests(Vec<RuntimeKeyRotationLine>),

    /// Public Runtime shared-writer session opens awaiting one local decision.
    RuntimeSharedOpenRequests(Vec<RuntimeSharedOpenLine>),

    /// One exact Core-owned ordinary-chat worktree.
    IsolatedWorkspace(Box<IsolatedWorkspaceLine>),

    /// Bounded Core-owned ordinary-chat worktrees, including preserved dirty results.
    IsolatedWorkspaces(Vec<IsolatedWorkspaceLine>),

    /// Exact cleanup outcome for one Core-owned ordinary-chat worktree.
    IsolatedWorkspaceReleased(Box<IsolatedWorkspaceReleaseLine>),

    /// A session was started or resumed.
    Started {
        /// runtrol's own name for it.
        session: SessionId,
    },

    /// Done, with nothing to say about it.
    Done,

    /// A watch subscription is installed and all later answers on this connection are events.
    Watching {
        /// The first event this response stream will deliver, or `live_at` when replay is empty.
        starts_at: WatchCursor,
        /// The exact boundary between bounded replay and the installed live subscription.
        live_at: WatchCursor,
        /// The requested boundary could not be served from the bounded window.
        gap: Option<Box<WatchGap>>,
    },

    /// A session-index subscription is installed and all later answers are current session snapshots.
    WatchingSessions,

    /// One event, already encoded, with the next exact reconnect boundary.
    ///
    /// Encoded once by the daemon and handed to every watcher, so three watchers cost one encode. Also the last hop a
    /// conversation takes, and nothing here reads it.
    Event {
        /// The original provider event envelope and opaque payload.
        payload: Opaque,
        /// The first dense event not included in this response.
        next_expected: WatchCursor,
    },

    /// This watch was retired after its bounded queue filled.
    Lagged {
        /// The first dense event the watcher did not receive.
        next_expected: WatchCursor,
    },

    /// This connection is now a view of a hosted terminal. The current screen follows as the first
    /// [`Response::TerminalOutput`].
    TerminalOpened {
        /// The terminal, for a second viewer to join.
        terminal: TerminalId,
        /// The process id of the hosted CLI.
        pid: u32,
        /// Whether this originating terminal owns the one current input lease.
        #[serde(default)]
        writable: bool,
    },

    /// Bytes the hosted CLI wrote to its terminal, exactly as written.
    TerminalOutput {
        /// The bytes.
        bytes: TerminalBytes,
    },

    /// This viewer fell behind the terminal's bounded output ring. The current screen follows as the next
    /// [`Response::TerminalOutput`], and the viewer should clear before drawing it.
    ///
    /// An empty struct rather than a unit, so it carries a `with` like every other answer a surface indexes.
    TerminalLagged {},

    /// The hosted CLI ended. The connection is done after this.
    TerminalExited {
        /// Its exit code.
        code: i32,
    },

    /// Private generation handoff capabilities and the draining generation's current live claims.
    GenerationHandoff {
        /// Closed handoff capability set implemented by the draining generation.
        capabilities: GenerationHandoffCapabilities,
        /// Current draining-generation live claims for successor admission.
        claims: Vec<GenerationLiveClaimLine>,
        /// Authorization rows the draining generation recorded since the last poll, for the successor's
        /// store. Absent from a generation built before the relay, which recorded none after handover.
        #[serde(default)]
        audit: Vec<GenerationAuditLine>,
    },

    /// Every cross-consult direction, each with its current state.
    ///
    /// Answered for the status request and after a wire or unwire, so a surface renders one shape and never
    /// derives state on its own.
    Consult(Vec<ConsultLine>),

    /// It did not work.
    Failed(WireError),
}

/// Private generation handoff capabilities. This never grants public authority by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct GenerationHandoffCapabilities {
    /// The generation serves the finalized public terminal contract.
    pub public_terminal: bool,
    /// The generation can accept successor-owned grant shrink and revocation updates.
    pub authority_relay: bool,
    /// The generation can export and accept provider-native live claims.
    pub native_live_claims: bool,
}

/// One complete integration authority row carried only on the owner-only generation control pipe.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct GenerationAuthorityLine {
    /// Private fixed integration store key.
    pub integration_key: [u8; 16],
    /// Stable public integration identity.
    pub integration_id: Box<str>,
    /// Ed25519 verification key used only to reject stale reconnects.
    pub public_key: [u8; 32],
    /// Exact current stable scope names.
    pub scopes: Vec<Box<str>>,
    /// Exact current canonical roots and filesystem identities.
    pub roots: Vec<GenerationAuthorityRoot>,
    /// Current key generation.
    pub key_generation: u64,
    /// Current grant generation.
    pub grant_generation: u64,
    /// Whether the successor has revoked this authority.
    pub revoked: bool,
}

/// One canonical integration root on the private generation relay.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct GenerationAuthorityRoot {
    /// Canonical approved path.
    pub path: Box<str>,
    /// Filesystem identity approved for that exact path.
    pub identity: [u8; 24],
}

/// Which provider process surface owns one exact or unresolved live claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationLiveClaimSurface {
    /// Runtime structured session process.
    Structured,
    /// Provider-faithful terminal process.
    Terminal,
}

/// One content-free provider-native live admission claim.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct GenerationLiveClaimLine {
    /// Opaque provider identity.
    pub provider_id: Box<str>,
    /// Provider-native identity when known before launch.
    pub native_session_id: Option<Box<str>>,
    /// Exact canonical workspace.
    pub workspace: Box<str>,
    /// Owning public process surface.
    pub surface: GenerationLiveClaimSurface,
    /// Surface-local owner identity with no content.
    pub owner_id: Box<str>,
}

/// One authorization row a draining generation kept for the successor that owns the store.
///
/// The private store row, field for field, so the successor appends it as if it had recorded it. Content-free
/// like the row: opaque identities and stable machine reasons, never caller text.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct GenerationAuditLine {
    /// Decision time.
    pub occurred_at_ms: u64,
    /// Private fixed integration store key of the authenticated integration, when there was one.
    pub integration_key: Option<[u8; 16]>,
    /// Signing-key generation used for the request.
    pub key_generation: Option<u64>,
    /// Stable public or private administration method name.
    pub method: Box<str>,
    /// Stable required app scope, when the method has one.
    pub scope: Option<Box<str>>,
    /// Opaque approved project identity.
    pub project: Option<Box<str>>,
    /// Opaque Runtime session identity.
    pub session: Option<Box<str>>,
    /// UUIDv7 mutation identity.
    pub request_id: Option<Box<str>>,
    /// Structural decision.
    pub outcome: GenerationAuditOutcome,
    /// Stable machine reason.
    pub reason: Box<str>,
}

/// The structural decision of one relayed authorization row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationAuditOutcome {
    /// The operation entered evaluation after structural parsing.
    Attempted,
    /// Authority checks and the operation succeeded.
    Allowed,
    /// The operation was refused with the recorded machine reason.
    Denied,
}

/// One cross-consult direction, as a surface shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsultLine {
    /// The CLI that would gain a consultant.
    pub from: Box<str>,
    /// The CLI whose opinion would become reachable.
    pub to: Box<str>,
    /// Where this direction stands.
    pub state: ConsultState,
    /// Why, when the state needs a sentence: the measured absence for an unsupported direction, or the
    /// CLI's own words when its answer could not be trusted.
    pub why: Option<Box<str>>,
}

/// Where one cross-consult direction stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsultState {
    /// The registration exists in the `from` CLI's own configuration.
    Wired,
    /// It does not.
    Unwired,
    /// This direction cannot be wired, and `why` says what was measured.
    Unsupported,
}

/// The small fixed edges around an already encoded event payload.
///
/// Keeping the provider-sized payload as its own slice lets the transport write it without allocating a second full
/// response and then a third framed copy. This type lives beside [`Response`] so the split wire spelling has one owner.
#[derive(Debug)]
pub struct EventResponseEdges {
    suffix: Vec<u8>,
}

impl EventResponseEdges {
    /// Bytes before the raw event payload.
    #[must_use]
    pub const fn prefix(&self) -> &'static [u8] {
        br#"{"say":"event","with":{"payload":"#
    }

    /// Bytes after the raw event payload.
    #[must_use]
    pub fn suffix(&self) -> &[u8] {
        &self.suffix
    }
}

/// Encode only the cursor-sized suffix of an event response.
///
/// # Errors
///
/// When this build cannot serialize its own reconnect cursor.
pub fn event_response_edges(
    next_expected: WatchCursor,
) -> Result<EventResponseEdges, serde_json::Error> {
    let cursor = serde_json::to_vec(&next_expected)?;
    let mut suffix = Vec::with_capacity(cursor.len() + 20);
    suffix.extend_from_slice(br#","next_expected":"#);
    suffix.extend_from_slice(&cursor);
    suffix.extend_from_slice(b"}}");
    Ok(EventResponseEdges { suffix })
}

/// One provider, as a listing shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderLine {
    /// Its identifier.
    pub id: Box<str>,
    /// What to call it in front of a person.
    pub display_name: Box<str>,
    /// Whether a session can be started for it.
    pub usable: bool,
    /// Why not, when it cannot.
    ///
    /// A sentence rather than a flag, because "this build has no driver for that protocol" and "nothing declares that
    /// kind" send the operator in different directions.
    pub why_not: Option<Box<str>>,
    /// Bare executable names whose interactive invocation can enter the transparent terminal bridge.
    ///
    /// Runtime-discovered from the provider manifest. Empty when the provider has no terminal surface.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminal_commands: Vec<Box<str>>,
}

/// One pending public Runtime integration proposal for local presentation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IntegrationEnrollmentLine {
    /// Opaque pending identity.
    pub pending_id: Box<str>,
    /// Safe client display name.
    pub client_name: Box<str>,
    /// Safe client version text.
    pub client_version: Box<str>,
    /// Consumer installed-instance identity.
    pub client_instance_id: Box<str>,
    /// Short public-key fingerprint for operator comparison.
    pub key_fingerprint: Box<str>,
    /// Hexadecimal digest of the exact enrollment manifest.
    pub manifest_digest: Box<str>,
    /// Exact stable requested scopes.
    pub scopes: Vec<Box<str>>,
    /// Exact requested project paths before local canonicalization.
    pub roots: Vec<Box<str>>,
    /// Expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// One provider's measured local state and declarative setup assistance.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderHelpLine {
    /// Runtime-discovered provider identity.
    pub provider_id: Box<str>,
    /// Provider or manifest supplied display name.
    pub display_name: Box<str>,
    /// `usable`, `missing`, or `unavailable`.
    pub installation_state: Box<str>,
    /// Provider-owned version text when measured.
    pub version: Option<Box<str>>,
    /// Safe structural reason when the provider is not usable.
    pub why: Option<Box<str>>,
    /// Provider-declared sign-in command for the operator to run.
    pub sign_in: Option<Box<str>>,
    /// Provider-declared diagnosis command for the operator to run.
    pub diagnose: Option<Box<str>>,
    /// Provider-declared installation command for the operator to run.
    pub install: Option<Box<str>>,
}

/// One durable public Runtime integration grant for local presentation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IntegrationLine {
    /// Opaque integration identity.
    pub integration_id: Box<str>,
    /// Operator-approved label.
    pub label: Box<str>,
    /// Consumer installed-instance identity.
    pub client_instance_id: Box<str>,
    /// Exact current scopes.
    pub scopes: Vec<Box<str>>,
    /// Every scope this Runtime revision permits the local operator to grant.
    pub available_scopes: Vec<Box<str>>,
    /// Canonical current roots.
    pub roots: Vec<Box<str>>,
    /// Current public-key generation.
    pub key_generation: u64,
    /// Grant generation changed by narrowing or revocation.
    pub grant_generation: u64,
    /// Whether this grant is revoked.
    pub revoked: bool,
}

/// One public Runtime session-forget request safe for local presentation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeForgetLine {
    /// Opaque one-use local confirmation identity.
    pub confirmation_id: Box<str>,
    /// Integration asking to remove the Runtime pointer.
    pub integration_id: Box<str>,
    /// Operator-approved integration label.
    pub integration_label: Box<str>,
    /// Exact Runtime session pointer that would be removed.
    pub session_id: Box<str>,
    /// Expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// One public Runtime integration-key rotation safe for local presentation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeKeyRotationLine {
    /// Opaque one-use local confirmation identity.
    pub confirmation_id: Box<str>,
    /// Integration asking to replace its public key.
    pub integration_id: Box<str>,
    /// Operator-approved integration label.
    pub integration_label: Box<str>,
    /// Exact key generation the request will replace.
    pub current_key_generation: u64,
    /// Short fingerprint of the proposed replacement key.
    pub new_key_fingerprint: Box<str>,
    /// Expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// One public Runtime shared-writer session open safe for local presentation.
///
/// A second writer in a working tree is the one open the public Runtime will not grant on its own: it
/// waits here for the person at the machine, who sees which operation, which coding service and which
/// folder before saying yes.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeSharedOpenLine {
    /// Opaque one-use local confirmation identity.
    pub confirmation_id: Box<str>,
    /// Integration asking to open the session.
    pub integration_id: Box<str>,
    /// Operator-approved integration label.
    pub integration_label: Box<str>,
    /// The public session open method asked for (start, native adoption, or resume).
    pub operation: Box<str>,
    /// Coding service the session would run.
    pub provider_id: Box<str>,
    /// Working tree the session would share with the writers already there.
    pub workspace: Box<str>,
    /// Expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// One Core-owned linked worktree for an ordinary chat.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IsolatedWorkspaceLine {
    /// Stable Core-owned workspace identity.
    pub workspace_id: Box<str>,
    /// Exact canonical base checkout selected by the operator.
    pub project: Box<str>,
    /// Exact canonical linked worktree.
    pub workspace: Box<str>,
    /// Frozen Git commit used to create the linked worktree.
    pub base_commit: Box<str>,
    /// `creating`, `ready`, `bound`, `preservedDirty`, or `released`.
    pub state: Box<str>,
    /// Bound public Runtime session, when one was established.
    pub session_id: Option<Box<str>>,
}

/// Cleanup outcome for one Core-owned ordinary-chat worktree.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IsolatedWorkspaceReleaseLine {
    /// Stable Core-owned workspace identity.
    pub workspace_id: Box<str>,
    /// Exact worktree that was removed or preserved.
    pub workspace: Box<str>,
    /// `removed`, `preservedDirty`, or `alreadyRemoved`.
    pub outcome: Box<str>,
}

/// Optional relay origin and current non-secret availability.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoteConnection {
    /// Exact configured HTTPS origin, or no value when the relay is disabled.
    pub relay_origin: Option<Box<str>>,
    /// Current closed state.
    pub state: RemoteConnectionState,
    /// Failure boundary only when [`RemoteConnectionState::Offline`].
    pub stage: Option<RemoteConnectionStage>,
}

/// A secret-bearing PWA pairing URL.
///
/// Serialization deliberately emits a string for the VS Code surface, while diagnostics redact it so a debug path
/// cannot copy the relay credential or one-time Noise secret into logs.
#[derive(Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub struct PairingUrl(Box<str>);

impl PairingUrl {
    /// Wrap one URL created by the daemon's local pairing coordinator.
    #[must_use]
    pub fn new(url: impl Into<Box<str>>) -> Self {
        Self(url.into())
    }

    /// Borrow the value for a local QR renderer.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for PairingUrl {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("PairingUrl(..)")
    }
}

/// One short-lived local QR invitation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PairingInvitationLine {
    /// PWA route with all sensitive pairing material confined to the URL fragment.
    pub pairing_url: PairingUrl,
    /// Exclusive Unix millisecond expiry.
    pub expires_at_ms: u64,
    /// Short PC static-key fingerprint for comparison after pairing.
    pub pc_key_fingerprint: Box<str>,
}

/// One PSK-authenticated phone waiting for the operator at the PC.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PairingProposalLine {
    /// Opaque proposal identity.
    pub proposal_id: Box<str>,
    /// Validated device name.
    pub name: Box<str>,
    /// Validated platform label.
    pub platform: Box<str>,
    /// Short authenticated static-key fingerprint.
    pub key_fingerprint: Box<str>,
    /// Every plain device scope this build permits the local operator to select.
    pub available_scopes: Vec<Box<str>>,
}

/// One durable paired phone without credentials or conversation data.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceLine {
    /// Locally minted device identity.
    pub device_id: Box<str>,
    /// Operator-approved name.
    pub name: Box<str>,
    /// Operator-approved platform.
    pub platform: Box<str>,
    /// Short pinned Noise-key fingerprint.
    pub key_fingerprint: Box<str>,
    /// Exact current scope names.
    pub scopes: Vec<Box<str>>,
    /// Every plain device scope this build permits the local operator to select.
    pub available_scopes: Vec<Box<str>>,
    /// Canonical workspace paths currently approved for this device.
    pub roots: Vec<Box<str>>,
    /// Runtime-discovered provider identities currently approved for this device.
    pub providers: Vec<Box<str>>,
    /// Pairing time in Unix milliseconds.
    pub paired_at_ms: u64,
}

/// Current authority disclosed only to the paired device that owns it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceAuthorityLine {
    /// Exact current plain scope names.
    pub scopes: Vec<Box<str>>,
    /// Canonical workspace roots currently approved for this device.
    pub roots: Vec<Box<str>>,
    /// Runtime-discovered provider identities currently approved for this device.
    pub providers: Vec<Box<str>>,
}

/// Closed remote connection state rendered without parsing prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteConnectionState {
    /// No relay origin is configured.
    Disabled,
    /// DNS, registration, ticket, TLS, or WebSocket setup is in progress.
    Connecting,
    /// The PC relay WebSocket is ready to authenticate phones.
    Online,
    /// The last attempt failed and the bounded retry supervisor remains active.
    Offline,
}

/// Non-secret boundary at which the last relay attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteConnectionStage {
    /// DNS resolution or exact destination admission.
    Discovery,
    /// Idempotent route registration.
    Registration,
    /// Ticket, TLS, or WebSocket connection.
    Connection,
    /// Established ciphertext exchange.
    Exchange,
}

/// One provider's independently verified update state.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderUpdateLine {
    /// Provider identifier from the runtime registry.
    pub provider: Box<str>,
    /// Current closed state.
    pub state: ProviderUpdateState,
    /// Discovered package identifier when ownership is confirmed.
    pub package: Option<Box<str>>,
    /// Installed semantic version when ownership is confirmed.
    pub installed: Option<Box<str>>,
    /// Greatest plain registry release when newer than the installed copy.
    pub target: Option<Box<str>>,
    /// Greatest earlier plain release when the registry proves one.
    pub rollback: Option<Box<str>>,
    /// Bounded runtrol-owned explanation when the state is not actionable.
    pub why: Option<Box<str>>,
}

/// Provider update status rendered by every surface without parsing prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderUpdateState {
    /// The installed release is the greatest plain release in its confirmed registry.
    Current,
    /// A newer plain release exists in the confirmed registry.
    Available,
    /// The provider owns the update mechanism and runtrol only observes it.
    ObserveOnly,
    /// No provider invocation is installed.
    NotInstalled,
    /// Ownership or registry evidence is absent or contradictory.
    Unconfirmed,
}

/// Closed provider update result.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderUpdateResult {
    /// Provider whose package was inspected or changed.
    pub provider: Box<str>,
    /// Exact bounded outcome.
    pub outcome: ProviderUpdateOutcome,
    /// Version installed before the attempt.
    pub from: Box<str>,
    /// Version requested from the confirmed registry.
    pub to: Box<str>,
    /// Runtrol-owned explanation when a rollback was necessary.
    pub why: Option<Box<str>>,
}

/// What one provider update did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderUpdateOutcome {
    /// The installed release was already current, so no package command ran.
    AlreadyCurrent,
    /// The requested release was installed and independently rediscovered.
    Updated,
    /// Verification failed and the exact previous release was restored.
    RolledBack,
}

/// A session listing that can report one damaged row without hiding every readable row.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SessionListing {
    /// Readable sessions, oldest first.
    pub sessions: Vec<SessionLine>,
    /// Named storage failures that were skipped.
    pub warnings: Vec<Box<str>>,
    /// Where each account stands against its limits, by each service's own latest report, in service order.
    ///
    /// On the index because the index is what a surface already watches: a phone draws the account's
    /// position from the same push that moves its session rows, with no second request and no clock.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage: Vec<UsageLine>,
}

/// One service's latest account position, as a listing shows it.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct UsageLine {
    /// Which CLI's account.
    pub provider: Box<str>,
    /// A limit is blocking right now, by the service's own word.
    pub reached: bool,
    /// Every limit window the service described, shortest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<UsageWindowLine>,
    /// Tokens spent today by the service's own daily count, when it publishes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_today: Option<u64>,
    /// When the report arrived, unix milliseconds.
    pub at_ms: u64,
}

/// One limit window, as far as the service described it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct UsageWindowLine {
    /// Stable identity within one service's report, as that service names the window.
    pub id: Box<str>,
    /// What the service calls this window for a person to read, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Box<str>>,
    /// What this limit is scoped to, when the service scopes it to one model or surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Box<str>>,
    /// The service says this window is the one governing right now.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub governing: bool,
    /// How full the window is, as a percentage, when the service says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<u8>,
    /// When it resets, unix milliseconds, when the service says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
    /// How long the window is, in minutes, when the service says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
}

/// One session, as a listing shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionLine {
    /// runtrol's own name.
    pub session: SessionId,
    /// Which CLI it belongs to.
    pub provider: Box<str>,
    /// The provider's own name, once it has announced one.
    pub native: Option<Box<str>>,
    /// A short name the operator gave this session.
    ///
    /// Absent until explicitly named. A surface derives a provider and workspace fallback without reading the
    /// conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Box<str>>,
    /// Where the agent works.
    ///
    /// On the wire because a surface has to be able to say which session is touching which folder, which is
    /// the whole of the `sessions do not trample each other` axis. It is deliberately **not** on the terminal
    /// listing: that surface is whitespace-splittable by contract, and a path can contain spaces.
    pub workspace: Box<str>,
    /// How much of it exists: whether it has a process right now.
    pub hot: bool,
    /// What it is doing, in one word.
    pub doing: Box<str>,
    /// What a running turn is waiting for, when it cannot continue by itself.
    ///
    /// This is bounded session metadata already observed by Core. It carries no approval identifier, subject, or
    /// conversation content. A phone uses the distinction to go to a person wait without treating account quota as
    /// something the operator can answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<SessionWaiting>,
    /// It has gone quiet, and the turn is still running.
    ///
    /// Both halves matter. A subscriber shows "this looks stuck" and offers to stop it; what it must not show is a
    /// completion runtrol invented.
    pub looks_stuck: bool,
}

/// Why a running session cannot continue by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionWaiting {
    /// A person has to answer an approval or a provider-native input request.
    Person,
    /// An account limit has to lapse. There is no useful operator action.
    Quota,
}

/// Why something did not work.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WireError {
    /// What went wrong, in words an operator reads.
    pub message: Box<str>,
    /// Trying again could plausibly work without anybody intervening.
    pub retryable: bool,
    /// The operator has to do something at their own machine.
    ///
    /// The one honest answer a phone can give. Authentication in particular is unfixable from anywhere else, because
    /// runtrol carries no credential and a remote caller has no way to supply one.
    pub needs_the_operator: bool,
}

impl WireError {
    /// Turn a provider failure into what goes on the wire.
    ///
    /// The only place that mapping happens. Two codes for one failure would mean clients branching on the wrong one,
    /// and a retryable failure being treated as fatal.
    #[must_use]
    pub fn from_provider(error: &ProviderError) -> Self {
        Self {
            message: error.to_string().into(),
            retryable: error.retryable(),
            needs_the_operator: error.needs_operator_at_the_machine(),
        }
    }

    /// A failure that is nobody's fault but has to be reported.
    #[must_use]
    pub fn plain(message: &str) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            needs_the_operator: false,
        }
    }
}

/// Whether a hello agrees with this build.
///
/// # Errors
///
/// The version this build speaks, when they differ. A caller turns that into a message naming both, because an operator
/// whose two processes disagree needs to know which one is behind.
pub const fn agree(theirs: u8) -> Result<u8, u8> {
    if theirs == WIRE_VERSION {
        Ok(WIRE_VERSION)
    } else {
        Err(WIRE_VERSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_request(request: &Request) -> Request {
        let encoded = serde_json::to_string(request).expect("a request is writable");
        serde_json::from_str(&encoded).expect("and readable")
    }

    #[test]
    fn every_request_about_a_session_carries_which_one() {
        // No current session and no selected provider. Two processes holding "the one you meant" is two places for that
        // to disagree, and the way it goes wrong is a command landing on a session the operator was not looking at.
        let session = SessionId::now();
        let approval = ApprovalId::now();
        let about = [
            Request::Prompt {
                session,
                text: "do the thing".into(),
            },
            Request::Interrupt { session },
            Request::AnswerApproval {
                session,
                approval,
                option: OptionId(0),
                subject_digest: [1; 32],
            },
            Request::Watch {
                session,
                after: None,
            },
            Request::Close {
                session,
                now: false,
            },
        ];
        for request in about {
            let encoded = serde_json::to_string(&request).expect("writable");
            assert!(
                encoded.contains(&session.to_string()),
                "a request about a session must name it: {encoded}"
            );
        }
    }

    #[test]
    fn stopping_everything_carries_nothing_that_could_be_aimed() {
        // The one capability the security posture requires to work from anywhere with no permission. A request with no
        // arguments has nothing an attacker can point at, and the worst it achieves is stopping work.
        let encoded = serde_json::to_string(&Request::StopEverything).expect("writable");
        let parsed: serde_json::Value = serde_json::from_str(&encoded).expect("readable");
        let object = parsed.as_object().expect("an object");
        assert_eq!(object.len(), 1, "the tag and nothing else: {encoded}");
        assert_eq!(
            object.get("ask").and_then(|v| v.as_str()),
            Some("stopEverything")
        );
    }

    #[test]
    fn a_handoff_answer_carries_audit_rows_and_one_from_before_the_relay_still_reads() {
        let line = GenerationAuditLine {
            occurred_at_ms: 1_700_000_000_000,
            integration_key: Some([9; 16]),
            key_generation: Some(2),
            method: "terminals/attach".into(),
            scope: Some("session.input.write".into()),
            project: None,
            session: Some("sess_7".into()),
            request_id: None,
            outcome: GenerationAuditOutcome::Allowed,
            reason: "allowed".into(),
        };
        let answer = Response::GenerationHandoff {
            capabilities: GenerationHandoffCapabilities {
                public_terminal: true,
                authority_relay: true,
                native_live_claims: true,
            },
            claims: Vec::new(),
            audit: vec![line.clone()],
        };
        let encoded = serde_json::to_string(&answer).expect("writable");
        match serde_json::from_str::<Response>(&encoded).expect("readable") {
            Response::GenerationHandoff { audit, .. } => assert_eq!(audit, vec![line]),
            other => panic!("expected the handoff answer back, got {other:?}"),
        }

        // A draining generation built before the relay answers without the field. It recorded nothing after
        // handover, and the successor must keep reading its claims rather than counting it as a miss.
        let older = r#"{"say":"generationHandoff","with":{"capabilities":{"public_terminal":true,"authority_relay":true,"native_live_claims":true},"claims":[]}}"#;
        match serde_json::from_str::<Response>(older).expect("an older answer still reads") {
            Response::GenerationHandoff { audit, .. } => assert!(audit.is_empty()),
            other => panic!("expected the handoff answer, got {other:?}"),
        }
    }

    #[test]
    fn terminal_bytes_cross_the_wire_exactly_and_the_lagged_answer_carries_a_payload() {
        // Every byte value, including the ones JSON cannot carry raw, comes back as it went.
        let bytes: Vec<u8> = (0..=255u8).collect();
        let encoded = serde_json::to_string(&Response::TerminalOutput {
            bytes: TerminalBytes::from(bytes.clone()),
        })
        .expect("writable");
        assert!(encoded.starts_with(r#"{"say":"terminalOutput","with":{"bytes":""#));
        match serde_json::from_str::<Response>(&encoded).expect("readable") {
            Response::TerminalOutput { bytes: back } => assert_eq!(back.as_ref(), &bytes[..]),
            other => panic!("expected terminal output, got {other:?}"),
        }
        let input = serde_json::to_string(&Request::TerminalInput {
            bytes: TerminalBytes::from(b"\x1b[<0;4;9M".to_vec()),
        })
        .expect("writable");
        match serde_json::from_str::<Request>(&input).expect("readable") {
            Request::TerminalInput { bytes } => assert_eq!(bytes.as_ref(), b"\x1b[<0;4;9M"),
            other => panic!("expected terminal input, got {other:?}"),
        }
        // Surfaces index every answer's `with`; a payload-free answer would have none.
        assert_eq!(
            serde_json::to_string(&Response::TerminalLagged {}).expect("writable"),
            r#"{"say":"terminalLagged","with":{}}"#
        );
        assert!(
            !format!("{:?}", TerminalBytes::from(b"secret".to_vec())).contains("secret"),
            "terminal bytes are conversation and never appear in a debug line"
        );
    }

    #[test]
    fn watching_sessions_is_a_payload_free_subscription_boundary() {
        assert_eq!(
            serde_json::to_string(&Request::WatchSessions).expect("writable"),
            r#"{"ask":"watchSessions"}"#
        );
        assert_eq!(
            serde_json::to_string(&Response::WatchingSessions).expect("writable"),
            r#"{"say":"watchingSessions"}"#
        );
    }

    #[test]
    fn a_request_reads_back_as_what_it_was() {
        let session = SessionId::now();
        let approval = ApprovalId::now();
        for request in [
            Request::Hello { wire: WIRE_VERSION },
            Request::List,
            Request::WatchSessions,
            Request::Models {
                provider: "claude".into(),
            },
            Request::RemoteConnection,
            Request::RemoteConfigure {
                relay_origin: Some("https://relay.example.com".into()),
            },
            Request::DeviceAuthorityBegin {
                device_id: "018f0000-0000-7000-8000-000000000000".into(),
                scopes: vec!["session.start".into()],
                roots: vec!["/work".into()],
                providers: vec!["claude".into()],
            },
            Request::DeviceAuthorityFinish {
                challenge_id: "dac_example".into(),
                answer: "typed phrase here".into(),
            },
            Request::PushSubscription {
                endpoint: Some("https://fcm.googleapis.com/fcm/send/example".into()),
            },
            Request::Start {
                provider: "claude".into(),
                workspace: "/work".into(),
                workspace_access: WorkspaceAccess::Exclusive,
                model: Some("haiku".into()),
                permission: None,
            },
            Request::Resume {
                provider: "claude".into(),
                native: "some-name".into(),
                workspace: "/work".into(),
                workspace_access: WorkspaceAccess::Exclusive,
            },
            Request::Prompt {
                session,
                text: "hello".into(),
            },
            Request::Rename {
                session,
                label: Some("release repair".into()),
            },
            Request::AnswerApproval {
                session,
                approval,
                option: OptionId(2),
                subject_digest: [3; 32],
            },
            Request::StopEverything,
            Request::Consult,
            Request::ConsultWire {
                from: "claude".into(),
                to: "codex".into(),
            },
            Request::ConsultUnwire {
                from: "claude".into(),
                to: "codex".into(),
            },
            Request::AgentToolsWire,
            Request::AgentToolsUnwire,
        ] {
            let back = round_trip_request(&request);
            assert_eq!(
                core::mem::discriminant(&back),
                core::mem::discriminant(&request),
                "a request changed shape crossing the wire"
            );
        }
    }

    #[test]
    fn watch_reads_an_absent_or_exact_next_expected_cursor() {
        let session = SessionId::now();
        let old_shape = format!(r#"{{"ask":"watch","with":{{"session":"{session}"}}}}"#);
        match serde_json::from_str::<Request>(&old_shape)
            .expect("an omitted cursor remains readable")
        {
            Request::Watch { after: None, .. } => {}
            other => panic!("expected an initial watch, got {other:?}"),
        }

        let after = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 7,
            seq: 91,
        };
        let encoded = serde_json::to_string(&Request::Watch {
            session,
            after: Some(after),
        })
        .expect("writable");
        match serde_json::from_str::<Request>(&encoded).expect("readable") {
            Request::Watch {
                session: read_session,
                after: Some(read_after),
            } => {
                assert_eq!(read_session, session);
                assert_eq!(read_after, after);
            }
            other => panic!("expected a cursor watch, got {other:?}"),
        }
    }

    #[test]
    fn a_prompt_carries_what_the_operator_wrote_and_nothing_added() {
        let written = "first line\nsecond line";
        let request = Request::Prompt {
            session: SessionId::now(),
            text: written.into(),
        };
        match round_trip_request(&request) {
            Request::Prompt { text, .. } => assert_eq!(&*text, written),
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    #[test]
    fn an_approval_answer_keeps_the_exact_subject_binding() {
        let session = SessionId::now();
        let approval = ApprovalId::now();
        let digest = core::array::from_fn(|index| u8::try_from(index).expect("the index fits"));
        let request = Request::AnswerApproval {
            session,
            approval,
            option: OptionId(17),
            subject_digest: digest,
        };

        match round_trip_request(&request) {
            Request::AnswerApproval {
                session: read_session,
                approval: read_approval,
                option,
                subject_digest,
            } => {
                assert_eq!(read_session, session);
                assert_eq!(read_approval, approval);
                assert_eq!(option, OptionId(17));
                assert_eq!(subject_digest, digest);
            }
            other => panic!("expected an approval answer, got {other:?}"),
        }
    }

    #[test]
    fn a_pass_through_payload_survives_the_tagged_envelope() {
        // Measured: with the tag inside the content this does not encode at all. The encoder has to buffer the content
        // into a model so it can add a field to it, and buffering a payload is the re-reading the design avoids. The
        // failure is loud rather than quiet, which is the better way to find out.
        let payload = r#"{"z":1,"a":[2,3],"nested":{"k":"v"}}"#;
        let next_expected = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 2,
            seq: 9,
        };
        let encoded = serde_json::to_string(&Response::Event {
            payload: Opaque::owned(payload.to_owned()),
            next_expected,
        })
        .expect("a tag beside the content lets a payload through");
        assert!(encoded.contains(payload), "byte for byte: {encoded}");

        let back: Response = serde_json::from_str(&encoded).expect("and it reads back");
        match back {
            Response::Event {
                payload: read,
                next_expected: read_next,
            } => {
                assert_eq!(read.as_str(), payload);
                assert_eq!(read_next, next_expected);
            }
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn an_event_crosses_as_bytes_nobody_re_reads() {
        // The last hop a conversation takes. Encoded once by the daemon and handed to every watcher, so three watchers
        // cost one encode rather than three.
        let event =
            Opaque::owned(r#"{"event":"agentMessageChunk","content":{"text":"hello"}}"#.to_owned());
        let response = Response::Event {
            payload: event,
            next_expected: WatchCursor {
                stream: runtrol_provider::StreamId::now(),
                epoch: 0,
                seq: 1,
            },
        };
        let encoded = serde_json::to_string(&response).expect("writable");

        assert!(
            encoded.contains(r#""text":"hello""#),
            "the payload has to arrive unaltered: {encoded}"
        );
        let printed = format!("{response:?}");
        assert!(
            !printed.contains("hello"),
            "and it must not reach a log line: {printed}"
        );
    }

    #[test]
    fn split_event_edges_are_the_exact_response_wire_shape() {
        let payload = Opaque::owned(r#"{"text":"kept raw"}"#.to_owned());
        let next_expected = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 4,
            seq: 19,
        };
        let whole = serde_json::to_vec(&Response::Event {
            payload: payload.clone(),
            next_expected,
        })
        .expect("writable");
        let edges = event_response_edges(next_expected).expect("cursor is writable");
        let split = [edges.prefix(), payload.as_str().as_bytes(), edges.suffix()].concat();

        assert_eq!(split, whole);
        assert!(matches!(
            serde_json::from_slice::<Response>(&split).expect("readable"),
            Response::Event { .. }
        ));
    }

    #[test]
    fn watch_acknowledgements_and_lag_controls_round_trip_every_cursor() {
        let requested = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 3,
            seq: 8,
        };
        let live_at = WatchCursor {
            seq: 21,
            ..requested
        };
        let watching = Response::Watching {
            starts_at: live_at,
            live_at,
            gap: Some(Box::new(WatchGap { requested, live_at })),
        };
        match serde_json::from_slice::<Response>(
            &serde_json::to_vec(&watching).expect("watch acknowledgement is writable"),
        )
        .expect("watch acknowledgement is readable")
        {
            Response::Watching {
                starts_at,
                live_at: read_live,
                gap: Some(gap),
            } => {
                assert_eq!(starts_at, live_at);
                assert_eq!(read_live, live_at);
                assert_eq!(*gap, WatchGap { requested, live_at });
            }
            other => panic!("expected a watch acknowledgement, got {other:?}"),
        }

        let lagged = Response::Lagged {
            next_expected: requested,
        };
        match serde_json::from_slice::<Response>(
            &serde_json::to_vec(&lagged).expect("lag control is writable"),
        )
        .expect("lag control is readable")
        {
            Response::Lagged { next_expected } => assert_eq!(next_expected, requested),
            other => panic!("expected a lag control, got {other:?}"),
        }
    }

    #[test]
    fn a_consult_answer_carries_every_direction_with_its_state_and_reason() {
        // One shape for status and for the answer to a wire, so a surface never derives state on its own.
        let response = Response::Consult(vec![
            ConsultLine {
                from: "claude".into(),
                to: "codex".into(),
                state: ConsultState::Wired,
                why: None,
            },
            ConsultLine {
                from: "codex".into(),
                to: "claude".into(),
                state: ConsultState::Unsupported,
                why: Some("measured absent".into()),
            },
        ]);
        let encoded = serde_json::to_string(&response).expect("writable");
        assert!(encoded.contains("unsupported"), "{encoded}");
        match serde_json::from_str::<Response>(&encoded).expect("readable") {
            Response::Consult(lines) => {
                assert_eq!(lines.len(), 2);
                let unsupported = lines
                    .iter()
                    .find(|line| line.state == ConsultState::Unsupported)
                    .expect("the unsupported direction survives the wire");
                assert!(unsupported.why.is_some(), "and it says why");
            }
            other => panic!("expected a consult answer, got {other:?}"),
        }
    }

    #[test]
    fn a_provider_that_cannot_be_served_is_listed_with_the_reason() {
        // An operator with a perfectly good manifest for a kind this build has no driver for should see it marked, not
        // wonder where it went.
        let response = Response::Welcome {
            wire: WIRE_VERSION,
            providers: vec![
                ProviderLine {
                    id: "claude".into(),
                    display_name: "Claude Code".into(),
                    usable: true,
                    why_not: None,
                    terminal_commands: vec!["claude".into(), "claude.cmd".into()],
                },
                ProviderLine {
                    id: "something".into(),
                    display_name: "Something Else".into(),
                    usable: false,
                    why_not: Some("this build has no driver for that protocol".into()),
                    terminal_commands: Vec::new(),
                },
            ],
            device: None,
            push_public_key: None,
            build_digest: None,
        };
        let encoded = serde_json::to_string(&response).expect("writable");
        let back: Response = serde_json::from_str(&encoded).expect("readable");
        match back {
            Response::Welcome { providers, .. } => {
                assert_eq!(providers.len(), 2, "both are listed");
                let unusable = providers
                    .iter()
                    .find(|one| !one.usable)
                    .expect("the unusable one is there");
                assert!(unusable.why_not.is_some(), "and it says why");
            }
            other => panic!("expected a welcome, got {other:?}"),
        }
    }

    #[test]
    fn a_session_that_looks_stuck_still_reports_its_turn_as_running() {
        // Both halves matter. A surface shows "this looks stuck" and offers to stop it; what it must not show is a
        // completion runtrol invented.
        let line = SessionLine {
            session: SessionId::now(),
            provider: "claude".into(),
            native: Some("some-name".into()),
            label: Some("Fix release".into()),
            workspace: r"C:\work\dartlab".into(),
            hot: true,
            doing: "busy".into(),
            waiting_on: Some(SessionWaiting::Person),
            looks_stuck: true,
        };
        let encoded = serde_json::to_string(&line).expect("writable");
        let back: SessionLine = serde_json::from_str(&encoded).expect("readable");
        assert!(back.looks_stuck);
        assert_eq!(&*back.doing, "busy", "quiet is not finished");
        assert_eq!(back.waiting_on, Some(SessionWaiting::Person));

        let mut previous: serde_json::Value = serde_json::from_str(&encoded).expect("JSON");
        previous
            .as_object_mut()
            .expect("a session line is an object")
            .remove("label");
        previous
            .as_object_mut()
            .expect("a session line is an object")
            .remove("waiting_on");
        let compatible: SessionLine =
            serde_json::from_value(previous).expect("the additive field stays compatible");
        assert_eq!(compatible.label, None);
        assert_eq!(compatible.waiting_on, None);
    }

    #[test]
    fn a_failure_carries_the_two_decisions_a_client_makes() {
        // Whether to try again, and whether to send the operator to their machine. On the value, so a client cannot get
        // them wrong by branching on a number whose meaning lives somewhere else.
        let authentication = ProviderError::AuthRequired {
            provider: runtrol_provider::ProviderId::parse("claude").expect("valid"),
            how: "run the login command".to_owned(),
        };
        let wire = WireError::from_provider(&authentication);
        assert!(
            wire.needs_the_operator,
            "authentication is unfixable remotely"
        );
        assert!(wire.message.contains("login"), "{}", wire.message);

        let plain = WireError::plain("nothing is listening");
        assert!(!plain.retryable);
        assert!(!plain.needs_the_operator);
    }

    #[test]
    fn a_failure_that_could_work_next_time_says_so() {
        // A retryable failure treated as fatal is a session the operator gives up on for no reason.
        let transient = ProviderError::Spawn {
            provider: runtrol_provider::ProviderId::parse("claude").expect("valid"),
            program: "claude".to_owned(),
            source: std::io::Error::other("temporarily unavailable"),
        };
        let wire = WireError::from_provider(&transient);
        assert_eq!(wire.retryable, transient.retryable());
    }

    #[test]
    fn a_hello_that_does_not_agree_answers_with_what_this_build_speaks() {
        assert_eq!(agree(WIRE_VERSION), Ok(WIRE_VERSION));
        assert_eq!(agree(WIRE_VERSION + 1), Err(WIRE_VERSION));
        assert_eq!(agree(0), Err(WIRE_VERSION));
    }

    #[test]
    fn pairing_urls_cross_the_wire_but_are_redacted_from_diagnostics() {
        let secret = "https://example.invalid/runtrol/app/#pair=relay-secret";
        let response = Response::PairingInvitation(PairingInvitationLine {
            pairing_url: PairingUrl::new(secret),
            expires_at_ms: 1_767_225_600_000,
            pc_key_fingerprint: "public-key".into(),
        });
        let diagnostic = format!("{response:?}");
        assert!(diagnostic.contains("PairingUrl(..)"));
        assert!(!diagnostic.contains("relay-secret"));
        let encoded = serde_json::to_string(&response).expect("wire serialization");
        assert!(encoded.contains(secret));
    }
}
