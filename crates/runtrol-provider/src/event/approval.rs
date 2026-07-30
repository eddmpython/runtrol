//! Asking a human, and the shape that makes it work across providers that disagree.
//!
//! # Why options rather than yes and no
//!
//! The two supported CLIs answer approvals in four different shapes between them. One takes a decision
//! word, or a structured amendment to an execution policy, or a structured amendment to a network policy.
//! Another takes a granted permission profile plus a scope, which is not a decision at all but a grant.
//! A third takes an allow-or-deny plus optionally rewritten input.
//!
//! Normalizing that into allow-or-deny throws away the amendments, the session-wide variants, the granted
//! profile, and the scope. Normalizing it into a union of all four puts a permission profile into
//! runtrol's type system, which is exactly the "let us parse a little, for convenience" that kills this
//! product.
//!
//! So: **the provider offers options, runtrol carries options, the human picks an option.** The driver
//! privately keeps what each option means on the wire and writes it back verbatim. The phone never learns
//! what a permission profile is. runtrol never learns either. Adding a fifth response shape becomes a
//! table entry rather than a type change.
//!
//! # Why consent needs content
//!
//! One CLI's file-change approval request carries **no diff**: its parameters are an item id, a turn id, a
//! thread id, a timestamp, and optionally a reason. So the driver has to join the request against the item
//! it refers to. When that join comes up empty, [`ApprovalRequest::subject_incomplete`] is set and the
//! subscriber must offer rejection only.
//!
//! That is a product rule, not an implementation detail. A phone showing "the agent wants to change some
//! files, approve?" is worse than no feature at all, because it trains the operator to tap yes.

use serde::Serialize;

use crate::event::{Opaque, ToolKind};
use crate::id::{ApprovalId, OptionId, ToolCallId, TurnId};
use crate::time::WallMs;

/// The provider is waiting for a human to choose.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    /// runtrol's identifier for this prompt.
    ///
    /// The provider's own handle is never exposed: on one CLI it is an integer scoped to a single
    /// connection, on another a request string, and neither survives a reconnect. A phone must be able to
    /// answer a prompt after its connection dropped.
    pub id: ApprovalId,
    /// The turn this belongs to, when there is one.
    pub turn: Option<TurnId>,
    /// The tool call this concerns, when there is one.
    pub tool_call: Option<ToolCallId>,
    /// What sort of thing is being approved.
    pub kind: ApprovalKind,
    /// How much saying yes commits to.
    pub risk: RiskClass,
    /// Every option the provider offered, in the provider's own order.
    ///
    /// runtrol adds none and removes none, except that it guarantees a way to say no.
    pub options: Vec<ApprovalOption>,
    /// What must be shown for consent to mean anything.
    ///
    /// Often a join of the approval request with the item it refers to, because one provider's file-change
    /// request does not carry the diff.
    pub subject: Opaque,
    /// The join failed and runtrol cannot show what would happen.
    ///
    /// When this is set the subscriber must offer rejection only. Consent to an unnamed action is not
    /// consent.
    pub subject_incomplete: bool,
    /// A hash of the subject, echoed back in the answer.
    ///
    /// Two prompts can be open at once, so an answer has to say which subject it was looking at. Hashing
    /// bytes is not reading them.
    pub subject_digest: [u8; 32],
    /// When this becomes a refusal.
    pub expires_at: WallMs,
}

impl ApprovalRequest {
    /// The options this request may be answered with, given who is answering.
    ///
    /// `may_answer_high_risk` is the answerer's authority. Options that exceed it come back marked
    /// unavailable **and still present**, because a silently shortened list misrepresents what the
    /// provider offered, and the operator has a right to see that a stronger option existed.
    ///
    /// When the subject is incomplete, only rejections are offered, whatever the authority.
    #[must_use]
    pub fn offerable(&self, may_answer_high_risk: bool) -> Vec<OfferedOption> {
        self.options
            .iter()
            .map(|option| {
                let unavailable = if self.subject_incomplete && !option.kind.is_rejection() {
                    Some("runtrol cannot show you what this would do, so it can only be refused")
                } else if option.kind.commits_beyond_this_action() && !may_answer_high_risk {
                    Some("this device may not grant a standing permission")
                } else if self.risk == RiskClass::High && !may_answer_high_risk {
                    Some("this device may not answer a high-risk request")
                } else {
                    None
                };
                OfferedOption {
                    option: option.clone(),
                    unavailable,
                }
            })
            .collect()
    }

    /// The option to use when nobody answers in time.
    ///
    /// A one-time rejection, never a standing one: an expiry must not quietly create a permanent deny
    /// rule. Returns `None` only when the provider offered no way to refuse, which the driver is required
    /// to prevent by synthesizing one.
    #[must_use]
    pub fn expiry_option(&self) -> Option<&ApprovalOption> {
        self.options
            .iter()
            .find(|option| option.kind == PermissionOptionKind::RejectOnce)
    }

    /// Whether this request can be refused at all.
    ///
    /// A prompt that cannot be declined is a trap, and a deadline that cannot answer is a hang. A driver
    /// that cannot construct a refusal for some request must not surface the request; it reports a
    /// protocol violation and interrupts the turn instead.
    #[must_use]
    pub fn can_be_refused(&self) -> bool {
        self.options.iter().any(|option| option.kind.is_rejection())
    }
}

/// One choice the provider offered.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalOption {
    /// Which option, inside this request.
    pub id: OptionId,
    /// The provider's own label, verbatim.
    ///
    /// Rendered, never parsed. The driver holds what this option means on the wire.
    pub label: Box<str>,
    /// What sort of answer it is.
    pub kind: PermissionOptionKind,
}

/// An option as offered to a particular answerer.
#[derive(Debug, Clone, Serialize)]
pub struct OfferedOption {
    /// The option itself.
    pub option: ApprovalOption,
    /// Why it cannot be chosen, when it cannot.
    ///
    /// Present rather than absent, so the answerer sees that a stronger option existed and why it is out
    /// of reach.
    pub unavailable: Option<&'static str>,
}

/// What sort of answer an option is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionOptionKind {
    /// Allow this one action.
    AllowOnce,
    /// Allow this and everything like it, from now on.
    AllowAlways,
    /// Refuse this one action.
    RejectOnce,
    /// Refuse this and everything like it, from now on.
    RejectAlways,
}

impl PermissionOptionKind {
    /// Whether this answer refuses.
    #[must_use]
    pub const fn is_rejection(&self) -> bool {
        matches!(self, Self::RejectOnce | Self::RejectAlways)
    }

    /// Whether choosing this changes policy beyond the action in front of the operator.
    ///
    /// The standing variants do. That is what makes them high risk regardless of what the action itself
    /// is: an allow-always on a harmless-looking command is a permanent rule.
    #[must_use]
    pub const fn commits_beyond_this_action(&self) -> bool {
        matches!(self, Self::AllowAlways | Self::RejectAlways)
    }
}

/// What sort of thing is being approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ApprovalKind {
    /// Running a command.
    Command,
    /// Changing files.
    FileChange,
    /// Widening what the agent is permitted to do.
    Permissions,
    /// Answering a question the provider asked.
    Elicitation,
    /// Reaching the network.
    Network,
    /// Something else.
    Other,
}

/// How much saying yes commits to.
///
/// Gates the authority a device needs to answer. Derived structurally, never from a table of tool names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RiskClass {
    /// Saying yes affects this action and nothing else.
    Low,
    /// Saying yes changes policy, runs code, or removes something.
    High,
}

impl RiskClass {
    /// Classify a request from its structure.
    ///
    /// Structural on purpose. A tool-name table would be the hardcoding the discovery rule forbids, and it
    /// would misclassify the first tool a vendor renamed. The inputs here are all things the provider
    /// declared about its own request: what sort of approval it is, what kind of tool it concerns, and
    /// whether any offered option creates a standing rule.
    #[must_use]
    pub fn classify(
        kind: ApprovalKind,
        tool: Option<ToolKind>,
        options: &[ApprovalOption],
    ) -> Self {
        // A standing permission outlives the action it was granted for, so it is high risk whatever the
        // action was.
        if options
            .iter()
            .any(|option| option.kind == PermissionOptionKind::AllowAlways)
        {
            return Self::High;
        }
        // Widening permissions and reaching the network are high risk by what they are.
        if matches!(
            kind,
            ApprovalKind::Permissions | ApprovalKind::Network | ApprovalKind::Command
        ) {
            return Self::High;
        }
        // Running, deleting, editing, and moving change the machine.
        if tool.is_some_and(|kind| kind.is_dangerous()) {
            return Self::High;
        }
        Self::Low
    }
}

/// A pending approval is no longer pending, and nobody here answered it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum WithdrawnReason {
    /// The provider cancelled it.
    ProviderCancelled,
    /// Another client answered it.
    ///
    /// Possible on a shared provider daemon, which multiplexes several sessions and tells every client
    /// when one of them resolves a request. The other CLI's per-session process cannot do this, and
    /// handling both the same way keeps the asymmetry invisible to a subscriber.
    ResolvedElsewhere,
    /// The deadline passed and runtrol refused on the operator's behalf.
    Expired,
    /// The turn ended or the process died while the prompt was open.
    TurnGone,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: u32, kind: PermissionOptionKind, label: &str) -> ApprovalOption {
        ApprovalOption {
            id: OptionId(id),
            label: label.into(),
            kind,
        }
    }

    fn request(options: Vec<ApprovalOption>, incomplete: bool, risk: RiskClass) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalId::now(),
            turn: Some(TurnId::first(0)),
            tool_call: None,
            kind: ApprovalKind::FileChange,
            risk,
            options,
            subject: Opaque::owned(r#"{"changes":[{"path":"src/main.rs"}]}"#.to_owned()),
            subject_incomplete: incomplete,
            subject_digest: [0_u8; 32],
            expires_at: WallMs::now().plus_millis(90_000),
        }
    }

    fn allow_and_deny() -> Vec<ApprovalOption> {
        vec![
            option(0, PermissionOptionKind::AllowOnce, "Yes"),
            option(1, PermissionOptionKind::AllowAlways, "Yes, and always"),
            option(2, PermissionOptionKind::RejectOnce, "No"),
        ]
    }

    #[test]
    fn a_request_without_a_refusal_is_a_trap() {
        // A prompt that cannot be declined is a hang with a button on it. The driver is required to
        // synthesize a refusal, and this is the check that says whether it did.
        let trap = request(
            vec![option(0, PermissionOptionKind::AllowOnce, "Yes")],
            false,
            RiskClass::Low,
        );
        assert!(!trap.can_be_refused());
        assert!(trap.expiry_option().is_none());

        let sound = request(allow_and_deny(), false, RiskClass::Low);
        assert!(sound.can_be_refused());
    }

    #[test]
    fn expiry_never_creates_a_standing_refusal() {
        // Nobody answering must not quietly install a permanent deny rule.
        let with_both = request(
            vec![
                option(0, PermissionOptionKind::AllowOnce, "Yes"),
                option(1, PermissionOptionKind::RejectAlways, "No, and always"),
                option(2, PermissionOptionKind::RejectOnce, "No"),
            ],
            false,
            RiskClass::Low,
        );
        let chosen = with_both
            .expiry_option()
            .expect("a one-time refusal exists");
        assert_eq!(chosen.kind, PermissionOptionKind::RejectOnce);
    }

    #[test]
    fn an_incomplete_subject_leaves_only_refusals() {
        // "The agent wants to change some files, approve?" is worse than no feature, because it teaches
        // the operator to tap yes.
        let blind = request(allow_and_deny(), true, RiskClass::Low);
        let offered = blind.offerable(true);
        for entry in &offered {
            if entry.option.kind.is_rejection() {
                assert!(entry.unavailable.is_none(), "refusing is always available");
            } else {
                assert!(
                    entry.unavailable.is_some(),
                    "approving a subject runtrol cannot show must be blocked"
                );
            }
        }
    }

    #[test]
    fn a_low_authority_answerer_still_sees_every_option() {
        // A shortened list is a lie about what the provider offered. The operator has a right to know a
        // stronger option existed and why it is out of reach.
        let high = request(allow_and_deny(), false, RiskClass::High);
        let offered = high.offerable(false);
        assert_eq!(offered.len(), high.options.len(), "no option may be hidden");
        assert!(
            offered
                .iter()
                .all(|entry| entry.unavailable.is_some() || entry.option.kind.is_rejection()),
            "every blocked option carries a reason"
        );
        let always = offered
            .iter()
            .find(|entry| entry.option.kind == PermissionOptionKind::AllowAlways)
            .expect("the standing option is still listed");
        assert!(always.unavailable.is_some());
    }

    #[test]
    fn a_high_authority_answerer_can_use_everything() {
        let high = request(allow_and_deny(), false, RiskClass::High);
        let offered = high.offerable(true);
        assert!(offered.iter().all(|entry| entry.unavailable.is_none()));
    }

    #[test]
    fn a_standing_option_makes_any_request_high_risk() {
        // The action may be trivial. The permanent rule it installs is not.
        let with_standing = vec![
            option(0, PermissionOptionKind::AllowAlways, "Always"),
            option(1, PermissionOptionKind::RejectOnce, "No"),
        ];
        assert_eq!(
            RiskClass::classify(ApprovalKind::Other, Some(ToolKind::Read), &with_standing),
            RiskClass::High
        );
    }

    #[test]
    fn a_plain_read_with_one_time_options_is_low_risk() {
        let once = vec![
            option(0, PermissionOptionKind::AllowOnce, "Yes"),
            option(1, PermissionOptionKind::RejectOnce, "No"),
        ];
        assert_eq!(
            RiskClass::classify(ApprovalKind::Other, Some(ToolKind::Read), &once),
            RiskClass::Low
        );
    }

    #[test]
    fn commands_permissions_and_network_are_high_risk_by_what_they_are() {
        let once = vec![
            option(0, PermissionOptionKind::AllowOnce, "Yes"),
            option(1, PermissionOptionKind::RejectOnce, "No"),
        ];
        for kind in [
            ApprovalKind::Command,
            ApprovalKind::Permissions,
            ApprovalKind::Network,
        ] {
            assert_eq!(
                RiskClass::classify(kind, None, &once),
                RiskClass::High,
                "{kind:?} must be high risk"
            );
        }
    }

    #[test]
    fn a_dangerous_tool_kind_raises_the_class() {
        let once = vec![
            option(0, PermissionOptionKind::AllowOnce, "Yes"),
            option(1, PermissionOptionKind::RejectOnce, "No"),
        ];
        assert_eq!(
            RiskClass::classify(ApprovalKind::FileChange, Some(ToolKind::Delete), &once),
            RiskClass::High
        );
        assert_eq!(
            RiskClass::classify(ApprovalKind::FileChange, Some(ToolKind::Read), &once),
            RiskClass::Low
        );
    }

    #[test]
    fn a_subject_never_reaches_a_log_line() {
        // The subject is a diff or a command line. Both are content.
        let secret = request(allow_and_deny(), false, RiskClass::Low);
        let printed = format!("{secret:?}");
        assert!(!printed.contains("main.rs"), "leaked: {printed}");
    }
}
