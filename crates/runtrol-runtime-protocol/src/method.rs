//! Closed public method vocabulary.

use core::fmt;
use core::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A public Runtime method implemented by the initial read-only boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
pub enum RuntimeMethod {
    /// Negotiate a public revision and bind the Runtime instance.
    #[serde(rename = "runtime/initialize")]
    Initialize,
    /// Finish initialization before any inventory request.
    #[serde(rename = "runtime/initialized")]
    Initialized,
    /// Server-first connection-bound authentication challenge notification.
    #[serde(rename = "runtime/challenge")]
    Challenge,
    /// Create a bounded pending local enrollment.
    #[serde(rename = "integrations/requestEnrollment")]
    IntegrationsRequestEnrollment,
    /// Read the current decision for the proved pending key.
    #[serde(rename = "integrations/watchEnrollment")]
    IntegrationsWatchEnrollment,
    /// Read the authenticated integration's current grant.
    #[serde(rename = "integrations/getGrant")]
    IntegrationsGetGrant,
    /// Replace the authenticated integration key after exact local confirmation.
    #[serde(rename = "integrations/rotateKey")]
    IntegrationsRotateKey,
    /// Read each account's latest reported position against its limits.
    #[serde(rename = "providers/usage")]
    ProvidersUsage,
    /// Read the structural provider inventory.
    #[serde(rename = "providers/list")]
    ProvidersList,
    /// Watch structural provider inventory changes without polling.
    #[serde(rename = "providers/watch")]
    ProvidersWatch,
    /// Discover structural lifecycle and event capabilities for one provider.
    #[serde(rename = "providers/getCapabilities")]
    ProvidersGetCapabilities,
    /// Discover the selected provider's current opaque model catalogue.
    #[serde(rename = "providers/listModels")]
    ProvidersListModels,
    /// Discover one root-scoped official provider-native session page.
    #[serde(rename = "providers/listNativeSessions")]
    ProvidersListNativeSessions,
    /// Name the conversations of one provider written in the last few seconds, which is how a caller knows a
    /// turn is running in a conversation this Runtime did not start.
    #[serde(rename = "providers/nativeActivity")]
    ProvidersNativeActivity,
    /// Read the Runtime-managed session catalogue.
    #[serde(rename = "sessions/list")]
    SessionsList,
    /// Watch authorized managed-session index snapshots without polling.
    #[serde(rename = "sessions/watchIndex")]
    SessionsWatchIndex,
    /// Read one exact Runtime-managed session descriptor.
    #[serde(rename = "sessions/get")]
    SessionsGet,
    /// Start one fresh provider-native session under an approved root.
    #[serde(rename = "sessions/start")]
    SessionsStart,
    /// Adopt one officially listed native session into Runtime supervision.
    #[serde(rename = "sessions/adoptNative")]
    SessionsAdoptNative,
    /// Heat one existing Runtime-managed cold session.
    #[serde(rename = "sessions/resume")]
    SessionsResume,
    /// Acquire the one renewable write lease for a live session.
    #[serde(rename = "sessions/acquireControl")]
    SessionsAcquireControl,
    /// Renew one exact control lease generation.
    #[serde(rename = "sessions/renewControl")]
    SessionsRenewControl,
    /// Voluntarily release one exact control lease generation.
    #[serde(rename = "sessions/releaseControl")]
    SessionsReleaseControl,
    /// Submit caller-owned input without rewriting or implicit retry.
    #[serde(rename = "sessions/submitInput")]
    SessionsSubmitInput,
    /// Submit caller-owned typed content blocks (text and images) without rewriting.
    #[serde(rename = "sessions/submitBlocks")]
    SessionsSubmitBlocks,
    /// Switch the answering model under the current control lease.
    #[serde(rename = "sessions/setModel")]
    SessionsSetModel,
    /// Switch the governing permission mode under the current control lease.
    #[serde(rename = "sessions/setMode")]
    SessionsSetMode,
    /// Watch the existing bounded normalized event stream.
    #[serde(rename = "sessions/watchEvents")]
    SessionsWatchEvents,
    /// Interrupt one exact controlled live session.
    #[serde(rename = "sessions/interrupt")]
    SessionsInterrupt,
    /// Release one idle hot provider process while retaining its managed pointer.
    #[serde(rename = "sessions/cool")]
    SessionsCool,
    /// Forget one cold Runtime pointer after local confirmation.
    #[serde(rename = "sessions/forget")]
    SessionsForget,
    /// Delete one provider-native conversation through the provider's own surface.
    #[serde(rename = "sessions/deleteNative")]
    SessionsDeleteNative,
    /// Archive one provider-native conversation through the provider's own surface.
    #[serde(rename = "sessions/archiveNative")]
    SessionsArchiveNative,
    /// Return the caller-visible live terminals in this Runtime generation.
    #[serde(rename = "terminals/list")]
    TerminalsList,
    /// Watch root-filtered terminal index snapshots.
    #[serde(rename = "terminals/watchIndex")]
    TerminalsWatchIndex,
    /// Open a fresh terminal or resume one authorized native conversation.
    #[serde(rename = "terminals/open")]
    TerminalsOpen,
    /// Attach one connection-bound view at current shared geometry.
    #[serde(rename = "terminals/attach")]
    TerminalsAttach,
    /// Acquire the one renewable terminal write lease.
    #[serde(rename = "terminals/acquireControl")]
    TerminalsAcquireControl,
    /// Renew one exact terminal lease generation.
    #[serde(rename = "terminals/renewControl")]
    TerminalsRenewControl,
    /// Voluntarily release one exact terminal lease generation.
    #[serde(rename = "terminals/releaseControl")]
    TerminalsReleaseControl,
    /// Write exact caller-owned bytes once under the current terminal lease.
    #[serde(rename = "terminals/write")]
    TerminalsWrite,
    /// Set bounded shared PTY geometry under the current terminal lease.
    #[serde(rename = "terminals/resize")]
    TerminalsResize,
    /// Detach only one connection-bound terminal view.
    #[serde(rename = "terminals/detach")]
    TerminalsDetach,
    /// Stop one hosted provider CLI process under the current terminal lease.
    #[serde(rename = "terminals/stop")]
    TerminalsStop,
    /// A VS Code window registers itself and its observed terminals.
    #[serde(rename = "windows/register")]
    WindowsRegister,
    /// A registered window publishes the terminals it observes now.
    #[serde(rename = "windows/update")]
    WindowsUpdate,
    /// Read every registered window.
    #[serde(rename = "windows/list")]
    WindowsList,
    /// Subscribe to the window index.
    #[serde(rename = "windows/watchIndex")]
    WindowsWatchIndex,
    /// A window opens a mirror of a terminal it observes.
    #[serde(rename = "windows/mirrorOpen")]
    WindowsMirrorOpen,
    /// A window feeds one chunk of an observed execution's raw output.
    #[serde(rename = "windows/mirrorOutput")]
    WindowsMirrorOutput,
    /// The observed execution ended or the window stops mirroring.
    #[serde(rename = "windows/mirrorEnd")]
    WindowsMirrorEnd,
    /// Read pending structured provider approvals for one controlled session.
    #[serde(rename = "approvals/listPending")]
    ApprovalsListPending,
    /// Answer one exact pending provider approval.
    #[serde(rename = "approvals/respond")]
    ApprovalsRespond,
    /// One changed managed-session index snapshot.
    #[serde(rename = "sessions/indexChanged")]
    SessionsIndexChanged,
    /// Final managed-session index subscription reason.
    #[serde(rename = "sessions/indexEnded")]
    SessionsIndexEnded,
    /// One changed provider inventory snapshot.
    #[serde(rename = "providers/changed")]
    ProvidersChanged,
    /// Final provider inventory subscription reason.
    #[serde(rename = "providers/watchEnded")]
    ProvidersWatchEnded,
    /// One changed account usage snapshot, on the same provider subscription.
    #[serde(rename = "providers/usageChanged")]
    ProvidersUsageChanged,
    /// One normalized event notification.
    #[serde(rename = "sessions/event")]
    SessionsEvent,
    /// A bounded subscription was retired after lagging.
    #[serde(rename = "sessions/lagged")]
    SessionsLagged,
    /// One changed root-filtered terminal index snapshot.
    #[serde(rename = "terminals/indexChanged")]
    TerminalsIndexChanged,
    /// Final terminal index subscription reason.
    #[serde(rename = "terminals/indexEnded")]
    TerminalsIndexEnded,
    /// One bounded exact output chunk for a terminal view.
    #[serde(rename = "terminals/output")]
    TerminalsOutput,
    /// Explicit lost-byte boundary with a replacement screen snapshot.
    #[serde(rename = "terminals/lagged")]
    TerminalsLagged,
    /// Provider process exit after preceding output drained.
    #[serde(rename = "terminals/exited")]
    TerminalsExited,
    /// The window index changed.
    #[serde(rename = "windows/indexChanged")]
    WindowsIndexChanged,
    /// The window index subscription ended.
    #[serde(rename = "windows/indexEnded")]
    WindowsIndexEnded,
    /// Stop every supervised process in the safe direction.
    #[serde(rename = "runtime/panicStop")]
    PanicStop,
}

impl RuntimeMethod {
    /// The stable JSON-RPC method name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => "runtime/initialize",
            Self::Initialized => "runtime/initialized",
            Self::Challenge => "runtime/challenge",
            Self::IntegrationsRequestEnrollment => "integrations/requestEnrollment",
            Self::IntegrationsWatchEnrollment => "integrations/watchEnrollment",
            Self::IntegrationsGetGrant => "integrations/getGrant",
            Self::IntegrationsRotateKey => "integrations/rotateKey",
            Self::ProvidersUsage => "providers/usage",
            Self::ProvidersList => "providers/list",
            Self::ProvidersWatch => "providers/watch",
            Self::ProvidersGetCapabilities => "providers/getCapabilities",
            Self::ProvidersListModels => "providers/listModels",
            Self::ProvidersListNativeSessions => "providers/listNativeSessions",
            Self::ProvidersNativeActivity => "providers/nativeActivity",
            Self::SessionsList => "sessions/list",
            Self::SessionsWatchIndex => "sessions/watchIndex",
            Self::SessionsGet => "sessions/get",
            Self::SessionsStart => "sessions/start",
            Self::SessionsAdoptNative => "sessions/adoptNative",
            Self::SessionsResume => "sessions/resume",
            Self::SessionsAcquireControl => "sessions/acquireControl",
            Self::SessionsRenewControl => "sessions/renewControl",
            Self::SessionsReleaseControl => "sessions/releaseControl",
            Self::SessionsSubmitInput => "sessions/submitInput",
            Self::SessionsSubmitBlocks => "sessions/submitBlocks",
            Self::SessionsSetModel => "sessions/setModel",
            Self::SessionsSetMode => "sessions/setMode",
            Self::SessionsWatchEvents => "sessions/watchEvents",
            Self::SessionsInterrupt => "sessions/interrupt",
            Self::SessionsCool => "sessions/cool",
            Self::SessionsForget => "sessions/forget",
            Self::SessionsDeleteNative => "sessions/deleteNative",
            Self::SessionsArchiveNative => "sessions/archiveNative",
            Self::TerminalsList => "terminals/list",
            Self::TerminalsWatchIndex => "terminals/watchIndex",
            Self::TerminalsOpen => "terminals/open",
            Self::TerminalsAttach => "terminals/attach",
            Self::TerminalsAcquireControl => "terminals/acquireControl",
            Self::TerminalsRenewControl => "terminals/renewControl",
            Self::TerminalsReleaseControl => "terminals/releaseControl",
            Self::TerminalsWrite => "terminals/write",
            Self::TerminalsResize => "terminals/resize",
            Self::TerminalsDetach => "terminals/detach",
            Self::TerminalsStop => "terminals/stop",
            Self::WindowsRegister => "windows/register",
            Self::WindowsUpdate => "windows/update",
            Self::WindowsList => "windows/list",
            Self::WindowsWatchIndex => "windows/watchIndex",
            Self::WindowsMirrorOpen => "windows/mirrorOpen",
            Self::WindowsMirrorOutput => "windows/mirrorOutput",
            Self::WindowsMirrorEnd => "windows/mirrorEnd",
            Self::ApprovalsListPending => "approvals/listPending",
            Self::ApprovalsRespond => "approvals/respond",
            Self::SessionsIndexChanged => "sessions/indexChanged",
            Self::SessionsIndexEnded => "sessions/indexEnded",
            Self::ProvidersChanged => "providers/changed",
            Self::ProvidersWatchEnded => "providers/watchEnded",
            Self::ProvidersUsageChanged => "providers/usageChanged",
            Self::SessionsEvent => "sessions/event",
            Self::SessionsLagged => "sessions/lagged",
            Self::TerminalsIndexChanged => "terminals/indexChanged",
            Self::TerminalsIndexEnded => "terminals/indexEnded",
            Self::TerminalsOutput => "terminals/output",
            Self::TerminalsLagged => "terminals/lagged",
            Self::TerminalsExited => "terminals/exited",
            Self::WindowsIndexChanged => "windows/indexChanged",
            Self::WindowsIndexEnded => "windows/indexEnded",
            Self::PanicStop => "runtime/panicStop",
        }
    }
}

impl fmt::Display for RuntimeMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RuntimeMethod {
    type Err = UnknownMethod;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "runtime/initialize" => Ok(Self::Initialize),
            "runtime/initialized" => Ok(Self::Initialized),
            "runtime/challenge" => Ok(Self::Challenge),
            "integrations/requestEnrollment" => Ok(Self::IntegrationsRequestEnrollment),
            "integrations/watchEnrollment" => Ok(Self::IntegrationsWatchEnrollment),
            "integrations/getGrant" => Ok(Self::IntegrationsGetGrant),
            "integrations/rotateKey" => Ok(Self::IntegrationsRotateKey),
            "providers/usage" => Ok(Self::ProvidersUsage),
            "providers/list" => Ok(Self::ProvidersList),
            "providers/watch" => Ok(Self::ProvidersWatch),
            "providers/getCapabilities" => Ok(Self::ProvidersGetCapabilities),
            "providers/listModels" => Ok(Self::ProvidersListModels),
            "providers/listNativeSessions" => Ok(Self::ProvidersListNativeSessions),
            "providers/nativeActivity" => Ok(Self::ProvidersNativeActivity),
            "sessions/list" => Ok(Self::SessionsList),
            "sessions/watchIndex" => Ok(Self::SessionsWatchIndex),
            "sessions/get" => Ok(Self::SessionsGet),
            "sessions/start" => Ok(Self::SessionsStart),
            "sessions/adoptNative" => Ok(Self::SessionsAdoptNative),
            "sessions/resume" => Ok(Self::SessionsResume),
            "sessions/acquireControl" => Ok(Self::SessionsAcquireControl),
            "sessions/renewControl" => Ok(Self::SessionsRenewControl),
            "sessions/releaseControl" => Ok(Self::SessionsReleaseControl),
            "sessions/submitInput" => Ok(Self::SessionsSubmitInput),
            "sessions/submitBlocks" => Ok(Self::SessionsSubmitBlocks),
            "sessions/setModel" => Ok(Self::SessionsSetModel),
            "sessions/setMode" => Ok(Self::SessionsSetMode),
            "sessions/watchEvents" => Ok(Self::SessionsWatchEvents),
            "sessions/interrupt" => Ok(Self::SessionsInterrupt),
            "sessions/cool" => Ok(Self::SessionsCool),
            "sessions/forget" => Ok(Self::SessionsForget),
            "sessions/deleteNative" => Ok(Self::SessionsDeleteNative),
            "sessions/archiveNative" => Ok(Self::SessionsArchiveNative),
            "terminals/list" => Ok(Self::TerminalsList),
            "terminals/watchIndex" => Ok(Self::TerminalsWatchIndex),
            "terminals/open" => Ok(Self::TerminalsOpen),
            "terminals/attach" => Ok(Self::TerminalsAttach),
            "terminals/acquireControl" => Ok(Self::TerminalsAcquireControl),
            "terminals/renewControl" => Ok(Self::TerminalsRenewControl),
            "terminals/releaseControl" => Ok(Self::TerminalsReleaseControl),
            "terminals/write" => Ok(Self::TerminalsWrite),
            "terminals/resize" => Ok(Self::TerminalsResize),
            "terminals/detach" => Ok(Self::TerminalsDetach),
            "terminals/stop" => Ok(Self::TerminalsStop),
            "windows/register" => Ok(Self::WindowsRegister),
            "windows/update" => Ok(Self::WindowsUpdate),
            "windows/list" => Ok(Self::WindowsList),
            "windows/watchIndex" => Ok(Self::WindowsWatchIndex),
            "windows/mirrorOpen" => Ok(Self::WindowsMirrorOpen),
            "windows/mirrorOutput" => Ok(Self::WindowsMirrorOutput),
            "windows/mirrorEnd" => Ok(Self::WindowsMirrorEnd),
            "approvals/listPending" => Ok(Self::ApprovalsListPending),
            "approvals/respond" => Ok(Self::ApprovalsRespond),
            "sessions/indexChanged" => Ok(Self::SessionsIndexChanged),
            "sessions/indexEnded" => Ok(Self::SessionsIndexEnded),
            "providers/changed" => Ok(Self::ProvidersChanged),
            "providers/watchEnded" => Ok(Self::ProvidersWatchEnded),
            "providers/usageChanged" => Ok(Self::ProvidersUsageChanged),
            "sessions/event" => Ok(Self::SessionsEvent),
            "sessions/lagged" => Ok(Self::SessionsLagged),
            "terminals/indexChanged" => Ok(Self::TerminalsIndexChanged),
            "terminals/indexEnded" => Ok(Self::TerminalsIndexEnded),
            "terminals/output" => Ok(Self::TerminalsOutput),
            "terminals/lagged" => Ok(Self::TerminalsLagged),
            "terminals/exited" => Ok(Self::TerminalsExited),
            "windows/indexChanged" => Ok(Self::WindowsIndexChanged),
            "windows/indexEnded" => Ok(Self::WindowsIndexEnded),
            "runtime/panicStop" => Ok(Self::PanicStop),
            _ => Err(UnknownMethod(value.to_owned())),
        }
    }
}

/// An incoming method name outside the public table.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown public Runtime method {0:?}")]
pub struct UnknownMethod(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_public_method_round_trips_through_its_stable_name() {
        for method in [
            RuntimeMethod::Initialize,
            RuntimeMethod::Initialized,
            RuntimeMethod::Challenge,
            RuntimeMethod::IntegrationsRequestEnrollment,
            RuntimeMethod::IntegrationsWatchEnrollment,
            RuntimeMethod::IntegrationsGetGrant,
            RuntimeMethod::IntegrationsRotateKey,
            RuntimeMethod::ProvidersList,
            RuntimeMethod::ProvidersWatch,
            RuntimeMethod::ProvidersGetCapabilities,
            RuntimeMethod::ProvidersListModels,
            RuntimeMethod::ProvidersListNativeSessions,
            RuntimeMethod::ProvidersNativeActivity,
            RuntimeMethod::SessionsList,
            RuntimeMethod::SessionsWatchIndex,
            RuntimeMethod::SessionsGet,
            RuntimeMethod::SessionsStart,
            RuntimeMethod::SessionsAdoptNative,
            RuntimeMethod::SessionsResume,
            RuntimeMethod::SessionsAcquireControl,
            RuntimeMethod::SessionsRenewControl,
            RuntimeMethod::SessionsReleaseControl,
            RuntimeMethod::SessionsSubmitInput,
            RuntimeMethod::SessionsSubmitBlocks,
            RuntimeMethod::SessionsSetModel,
            RuntimeMethod::SessionsSetMode,
            RuntimeMethod::SessionsWatchEvents,
            RuntimeMethod::SessionsInterrupt,
            RuntimeMethod::SessionsCool,
            RuntimeMethod::SessionsForget,
            RuntimeMethod::SessionsDeleteNative,
            RuntimeMethod::SessionsArchiveNative,
            RuntimeMethod::TerminalsList,
            RuntimeMethod::TerminalsWatchIndex,
            RuntimeMethod::TerminalsOpen,
            RuntimeMethod::TerminalsAttach,
            RuntimeMethod::TerminalsAcquireControl,
            RuntimeMethod::TerminalsRenewControl,
            RuntimeMethod::TerminalsReleaseControl,
            RuntimeMethod::TerminalsWrite,
            RuntimeMethod::TerminalsResize,
            RuntimeMethod::TerminalsDetach,
            RuntimeMethod::TerminalsStop,
            RuntimeMethod::WindowsRegister,
            RuntimeMethod::WindowsUpdate,
            RuntimeMethod::WindowsList,
            RuntimeMethod::WindowsWatchIndex,
            RuntimeMethod::WindowsMirrorOpen,
            RuntimeMethod::WindowsMirrorOutput,
            RuntimeMethod::WindowsMirrorEnd,
            RuntimeMethod::ApprovalsListPending,
            RuntimeMethod::ApprovalsRespond,
            RuntimeMethod::SessionsIndexChanged,
            RuntimeMethod::SessionsIndexEnded,
            RuntimeMethod::ProvidersChanged,
            RuntimeMethod::ProvidersWatchEnded,
            RuntimeMethod::ProvidersUsageChanged,
            RuntimeMethod::SessionsEvent,
            RuntimeMethod::SessionsLagged,
            RuntimeMethod::TerminalsIndexChanged,
            RuntimeMethod::TerminalsIndexEnded,
            RuntimeMethod::TerminalsOutput,
            RuntimeMethod::TerminalsLagged,
            RuntimeMethod::TerminalsExited,
            RuntimeMethod::WindowsIndexChanged,
            RuntimeMethod::WindowsIndexEnded,
            RuntimeMethod::PanicStop,
        ] {
            assert_eq!(method.as_str().parse(), Ok(method));
        }
        assert!("private/control".parse::<RuntimeMethod>().is_err());
    }
}
