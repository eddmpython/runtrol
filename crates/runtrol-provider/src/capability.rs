//! Provider-reported structural capabilities for one exact prepared driver.

/// Whether one structural provider operation is currently usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderCapabilityState {
    /// The prepared driver has a registered implementation backed by the named official surface.
    Available,
    /// The provider or driver has no registered official surface for this operation.
    Unsupported,
    /// The answer depends on provider negotiation that has not happened in this context.
    Unknown,
}

/// Where the capability observation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderCapabilitySource {
    /// A provider-neutral protocol negotiation or stable protocol contract.
    OfficialProtocol,
    /// A provider-owned command, flag parser, or structured stream.
    OfficialCli,
    /// The driver lifecycle contract itself, such as contained process cooling.
    DriverContract,
}

/// One honest structural capability observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCapability {
    /// Current availability.
    pub state: ProviderCapabilityState,
    /// Official or structural source when one supplied the answer.
    pub source: Option<ProviderCapabilitySource>,
    /// Stable structural explanation for an unsupported or unknown answer.
    pub why: Option<Box<str>>,
}

impl ProviderCapability {
    /// Report one observed available operation.
    #[must_use]
    pub const fn available(source: ProviderCapabilitySource) -> Self {
        Self {
            state: ProviderCapabilityState::Available,
            source: Some(source),
            why: None,
        }
    }

    /// Report a missing registered official surface.
    #[must_use]
    pub fn unsupported(why: impl Into<Box<str>>) -> Self {
        Self {
            state: ProviderCapabilityState::Unsupported,
            source: None,
            why: Some(why.into()),
        }
    }

    /// Report a capability that requires later provider negotiation.
    #[must_use]
    pub fn unknown(why: impl Into<Box<str>>) -> Self {
        Self {
            state: ProviderCapabilityState::Unknown,
            source: None,
            why: Some(why.into()),
        }
    }
}

/// Structural lifecycle and event capabilities for one prepared provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// A fresh session can be opened.
    pub fresh_session: ProviderCapability,
    /// A provider-native session can be resumed.
    pub resume: ProviderCapability,
    /// Provider output is mapped to structured Runtime events.
    pub structured_events: ProviderCapability,
    /// A running turn can receive an interrupt request.
    pub interrupt: ProviderCapability,
    /// Structured provider-native approvals can be answered.
    pub approvals: ProviderCapability,
    /// A hot process can be released without deleting its provider-native session.
    pub cooling: ProviderCapability,
    /// An official provider-native session catalogue may be requested.
    pub native_session_catalogue: ProviderCapability,
    /// The answering model can be switched mid-session.
    pub set_model: ProviderCapability,
    /// The reasoning effort can be switched mid-session.
    ///
    /// Distinct from the catalogue's per-model efforts on purpose: a CLI can offer an effort at open
    /// time and refuse to change it afterwards (measured on claude 2.1.235), and a surface that only
    /// learns this from the refusal cannot say "applies from the next session" before the attempt.
    pub set_reasoning_effort: ProviderCapability,
    /// A stored provider-native conversation can be deleted through the provider's own surface.
    ///
    /// Said up front so a surface offers the act only where it exists: a CLI that publishes no way to
    /// delete what it stored (claude) is told apart from one that does (codex `thread/delete`, cline
    /// `history delete`) before anybody clicks.
    pub native_session_delete: ProviderCapability,
    /// A stored provider-native conversation can be archived through the provider's own surface.
    pub native_session_archive: ProviderCapability,
}

impl ProviderCapabilities {
    /// Honest source-compatible default for a driver that predates capability reporting.
    #[must_use]
    pub fn unknown() -> Self {
        let missing = || ProviderCapability::unknown("this driver does not report this capability");
        Self {
            fresh_session: missing(),
            resume: missing(),
            structured_events: missing(),
            interrupt: missing(),
            approvals: missing(),
            cooling: missing(),
            native_session_catalogue: missing(),
            set_model: missing(),
            set_reasoning_effort: missing(),
            native_session_delete: missing(),
            native_session_archive: missing(),
        }
    }
}
