//! Where the operator's account with one coding service stands, as that service's own surface says it.
//!
//! Account state is not conversation content: runtrol owns session identity, process state and cursors,
//! and whether the operator is signed in is a fact about the machine, not about any conversation. Every
//! word here comes from a surface the CLI publishes for exactly this question (a status command, a
//! protocol method). A driver that has no such surface says so, and a surface never guesses from files.

use serde::Serialize;

use crate::{RateLimit, Window};

/// The longest plan or method token a report may carry; a longer one is the provider's bug, not a label.
pub const MAX_ACCOUNT_TOKEN_BYTES: usize = 64;

/// Whether the operator is signed in, by the service's own word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum AccountStatus {
    /// The service said the operator is signed in.
    SignedIn,
    /// The service said nobody is signed in.
    SignedOut,
    /// This service publishes no way to ask, and this says so rather than inferring one.
    Unpublished {
        /// Why, in terms of the service's own surfaces.
        why: Box<str>,
    },
}

/// One service's account report: sign-in state, the plan it names, and any limits it reports outside a turn.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountReport {
    /// Signed in or not, or nothing to ask.
    pub status: AccountStatus,
    /// The plan or subscription token exactly as the service wrote it (`max`, `plus`, `pro`), when it names one.
    pub plan: Option<Box<str>>,
    /// How the operator is signed in, as the service names it (`claude.ai`, `chatgpt`, `apiKey`), when it says.
    pub method: Option<Box<str>>,
    /// The limit windows the service reports on request, outside any turn, when it has such a surface.
    pub limits: Option<AccountLimits>,
    /// Why no windows are here, when the service has a limits surface and the reading of it failed.
    ///
    /// Three different silences look identical without this. A service that publishes no limits surface at
    /// all is one, a signed-out account is another, and a surface that was asked and did not answer is the
    /// third, which is the only one that is runtrol's own problem to retry. Absent whenever windows are
    /// present, and absent when nothing was asked.
    pub limits_unread: Option<Box<str>>,
    /// Tokens spent today by the service's own daily count, when it publishes one.
    pub tokens_today: Option<u64>,
}

/// Limit windows read on request. The same windows a turn reports, without the turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AccountLimits {
    /// Every window the service described, shortest first.
    pub windows: Vec<Window>,
    /// A limit is blocking right now, by the service's own word.
    pub reached: bool,
}

impl AccountLimits {
    /// One probe's limit reading, with the window list bounded the same way a turn's is.
    #[must_use]
    pub fn new(windows: Vec<Window>, reached: bool) -> Self {
        let limit = RateLimit::new(windows, reached, crate::Opaque::none());
        Self {
            windows: limit.windows,
            reached,
        }
    }
}

impl AccountReport {
    /// The report of a driver whose service publishes no account surface.
    #[must_use]
    pub fn unpublished(why: &str) -> Self {
        Self {
            status: AccountStatus::Unpublished { why: why.into() },
            plan: None,
            method: None,
            limits: None,
            limits_unread: None,
            tokens_today: None,
        }
    }

    /// The limits as a turn would have reported them, for the one gauge both paths fill.
    #[must_use]
    pub fn as_rate_limit(&self) -> Option<RateLimit> {
        let limits = self.limits.as_ref()?;
        Some(RateLimit::new(
            limits.windows.clone(),
            limits.reached,
            crate::Opaque::none(),
        ))
    }
}

/// A plan or method token bounded and free of control characters, or nothing.
#[must_use]
pub fn account_token(value: Option<&str>) -> Option<Box<str>> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > MAX_ACCOUNT_TOKEN_BYTES
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_bounded_and_clean() {
        assert_eq!(account_token(Some(" max ")).as_deref(), Some("max"));
        assert_eq!(account_token(Some("")), None);
        assert_eq!(account_token(Some("a\nb")), None);
        assert_eq!(account_token(Some(&"x".repeat(65))), None);
        assert_eq!(account_token(None), None);
    }

    #[test]
    fn an_unpublished_report_carries_no_limits() {
        let report = AccountReport::unpublished("no status command");
        assert!(report.as_rate_limit().is_none());
        assert!(matches!(report.status, AccountStatus::Unpublished { .. }));
    }
}
