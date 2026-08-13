//! Provider-native approval choices, kept beside the connection that can answer them.
//!
//! The public event carries opaque option identifiers. This module privately retains the JSON-RPC request id and
//! the exact provider response behind each option, so choosing one never teaches the supervisor or a phone what a
//! provider-specific policy amendment means.

use core::time::Duration;
use std::collections::{BTreeMap, VecDeque};

use bytes::Bytes;
use runtrol_provider::{
    ApprovalId, ApprovalKind, ApprovalOption, ApprovalRequest, Opaque, OptionId,
    PermissionOptionKind, RiskClass, ToolCallId, ToolKind, TurnId, WallMs,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest as _, Sha256};

use crate::codex::bound::DECLINE_RESULT;
use crate::framing::jsonrpc::RequestId;

/// How many item payloads may be retained for an approval join.
const MAX_ITEM_SUBJECTS: usize = 32;
/// How many provider decisions one approval may offer.
const MAX_OPTIONS: usize = 16;
/// How many provider questions one session may leave pending.
const MAX_PENDING_APPROVALS: usize = 32;
/// A remote round trip has this long before the fail-closed answer is due.
const APPROVAL_WINDOW_MS: u64 = 90_000;

const COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";
const FILE_APPROVAL: &str = "item/fileChange/requestApproval";
const ITEM_STARTED: &str = "item/started";
const FILE_PATCH_UPDATED: &str = "item/fileChange/patchUpdated";

#[derive(Debug, thiserror::Error)]
pub(super) enum ApprovalBuildError {
    #[error("the approval method has no binding")]
    UnboundMethod,
    #[error("the approval carried no parameters")]
    MissingParameters,
    #[error("the approval parameters were not readable")]
    UnreadableParameters,
    #[error("the approval offered more options than runtrol will retain")]
    TooManyOptions,
    #[error("the approval offered a decision shape this driver cannot classify safely")]
    UnknownDecision,
    #[error("the session already holds the maximum number of pending approvals")]
    TooManyPending,
    #[error("the approval is no longer pending")]
    NotPending,
    #[error("the approval subject changed")]
    SubjectChanged,
    #[error("the approval option is not backed by a provider response")]
    OptionMissing,
}

/// What has to go back to the provider for one chosen option.
pub(super) struct NativeAnswer {
    pub(super) approval: ApprovalId,
    pub(super) request: RequestId,
    pub(super) result: String,
}

struct PendingApproval {
    request: ApprovalRequest,
    native_request: RequestId,
    answers: BTreeMap<OptionId, String>,
    expiry_option: OptionId,
}

struct NativeOptions {
    offered: Vec<ApprovalOption>,
    answers: BTreeMap<OptionId, String>,
    expiry_option: OptionId,
}

struct ItemSubject {
    item: Box<str>,
    payload: Bytes,
}

/// Pending provider questions and the bounded item state needed to show file changes honestly.
#[derive(Default)]
pub(super) struct ApprovalBook {
    pending: BTreeMap<ApprovalId, PendingApproval>,
    items: VecDeque<ItemSubject>,
}

impl ApprovalBook {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn get(&self, approval: ApprovalId) -> Option<&ApprovalRequest> {
        self.pending.get(&approval).map(|pending| &pending.request)
    }

    pub(super) fn all(&self) -> Vec<&ApprovalRequest> {
        self.pending
            .values()
            .map(|pending| &pending.request)
            .collect()
    }

    /// Retain a shared reference to the newest item payload that can make a later file approval meaningful.
    pub(super) fn observe(&mut self, method: &str, params: &Bytes) {
        if !matches!(method, ITEM_STARTED | FILE_PATCH_UPDATED) {
            return;
        }
        let Ok(observed) = serde_json::from_slice::<ObservedItem<'_>>(params) else {
            return;
        };
        let item = observed
            .item_id
            .or_else(|| observed.item.and_then(|nested| nested.id));
        let Some(item) = item else {
            return;
        };

        self.items.retain(|held| &*held.item != item);
        if self.items.len() == MAX_ITEM_SUBJECTS {
            drop(self.items.pop_front());
        }
        self.items.push_back(ItemSubject {
            item: item.into(),
            payload: params.clone(),
        });
    }

    pub(super) fn clear_items(&mut self) {
        self.items.clear();
    }

    pub(super) fn open(
        &mut self,
        native_request: RequestId,
        method: &str,
        params: Option<Bytes>,
        turn: Option<TurnId>,
    ) -> Result<ApprovalRequest, ApprovalBuildError> {
        if self.pending.len() >= MAX_PENDING_APPROVALS {
            return Err(ApprovalBuildError::TooManyPending);
        }
        let params = params.ok_or(ApprovalBuildError::MissingParameters)?;
        let parsed = serde_json::from_slice::<ApprovalParams<'_>>(&params)
            .map_err(|_| ApprovalBuildError::UnreadableParameters)?;
        let kind = match method {
            COMMAND_APPROVAL => ApprovalKind::Command,
            FILE_APPROVAL => ApprovalKind::FileChange,
            _ => return Err(ApprovalBuildError::UnboundMethod),
        };

        let raw_decisions = decisions(method, parsed.available_decisions.as_deref());
        let native_options = options(raw_decisions)?;
        let tool = match kind {
            ApprovalKind::Command => Some(ToolKind::Execute),
            ApprovalKind::FileChange => Some(ToolKind::Edit),
            _ => None,
        };

        let joined = if kind == ApprovalKind::FileChange {
            self.items
                .iter()
                .rev()
                .find(|held| &*held.item == parsed.item_id)
                .map(|held| held.payload.clone())
        } else {
            None
        };
        let subject_bytes = joined.clone().unwrap_or_else(|| params.clone());
        let subject = opaque(&subject_bytes)?;
        let subject_incomplete = match kind {
            ApprovalKind::Command => {
                parsed.command.is_none_or(str::is_empty)
                    && parsed.network_approval_context.is_none()
            }
            ApprovalKind::FileChange => joined.is_none(),
            _ => true,
        };
        let subject_digest: [u8; 32] = Sha256::digest(subject.as_str().as_bytes()).into();
        let approval = ApprovalId::now();
        #[expect(
            clippy::manual_ok_err,
            reason = "an unusable optional provider item id removes correlation but must not discard the approval"
        )]
        let tool_call = match ToolCallId::new(parsed.item_id) {
            Ok(id) => Some(id),
            Err(_) => None,
        };
        let request = ApprovalRequest {
            id: approval,
            turn,
            tool_call,
            kind,
            risk: RiskClass::classify(kind, tool, &native_options.offered),
            options: native_options.offered,
            subject,
            subject_incomplete,
            subject_digest,
            expires_at: WallMs::now().plus_millis(APPROVAL_WINDOW_MS),
        };
        self.pending.insert(
            approval,
            PendingApproval {
                request: request.clone(),
                native_request,
                answers: native_options.answers,
                expiry_option: native_options.expiry_option,
            },
        );
        Ok(request)
    }

    pub(super) fn answer(
        &self,
        approval: ApprovalId,
        option: OptionId,
        subject_digest: [u8; 32],
    ) -> Result<NativeAnswer, ApprovalBuildError> {
        let pending = self
            .pending
            .get(&approval)
            .ok_or(ApprovalBuildError::NotPending)?;
        if pending.request.subject_digest != subject_digest {
            return Err(ApprovalBuildError::SubjectChanged);
        }
        let result = pending
            .answers
            .get(&option)
            .cloned()
            .ok_or(ApprovalBuildError::OptionMissing)?;
        Ok(NativeAnswer {
            approval,
            request: pending.native_request.clone(),
            result,
        })
    }

    pub(super) fn due(&self, now: WallMs) -> Option<NativeAnswer> {
        self.pending
            .values()
            .filter(|pending| pending.request.expires_at <= now)
            .min_by_key(|pending| pending.request.expires_at)
            .and_then(expiry_answer)
    }

    pub(super) fn wait(&self, now: WallMs) -> Option<Duration> {
        self.pending
            .values()
            .map(|pending| pending.request.expires_at)
            .min()
            .map(|expires| Duration::from_millis(now.millis_until(expires).unwrap_or_default()))
    }

    pub(super) fn complete(&mut self, approval: ApprovalId) {
        drop(self.pending.remove(&approval));
    }

    pub(super) fn rejections(&self) -> Vec<NativeAnswer> {
        self.pending.values().filter_map(expiry_answer).collect()
    }
}

fn expiry_answer(pending: &PendingApproval) -> Option<NativeAnswer> {
    pending
        .answers
        .get(&pending.expiry_option)
        .map(|result| NativeAnswer {
            approval: pending.request.id,
            request: pending.native_request.clone(),
            result: result.clone(),
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalParams<'line> {
    item_id: &'line str,
    #[serde(default)]
    command: Option<&'line str>,
    #[serde(default, borrow)]
    available_decisions: Option<Vec<&'line RawValue>>,
    #[serde(default, borrow)]
    network_approval_context: Option<&'line RawValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObservedItem<'line> {
    #[serde(default)]
    item_id: Option<&'line str>,
    #[serde(default)]
    item: Option<NestedItem<'line>>,
}

#[derive(Clone, Copy, Deserialize)]
struct NestedItem<'line> {
    #[serde(default)]
    id: Option<&'line str>,
}

fn decisions(method: &str, offered: Option<&[&RawValue]>) -> Vec<String> {
    if let Some(offered) = offered.filter(|offered| !offered.is_empty()) {
        return offered
            .iter()
            .map(|decision| decision.get().to_owned())
            .collect();
    }
    if matches!(method, COMMAND_APPROVAL | FILE_APPROVAL) {
        return ["accept", "acceptForSession", "decline", "cancel"]
            .into_iter()
            .map(|decision| format!(r#""{decision}""#))
            .collect();
    }
    Vec::new()
}

fn options(mut decisions: Vec<String>) -> Result<NativeOptions, ApprovalBuildError> {
    if decisions.len() >= MAX_OPTIONS {
        return Err(ApprovalBuildError::TooManyOptions);
    }
    let has_decline = decisions
        .iter()
        .any(|decision| decision_token(decision).is_some_and(|token| token == "decline"));
    if !has_decline {
        decisions.push(r#""decline""#.to_owned());
    }

    let mut offered = Vec::with_capacity(decisions.len());
    let mut answers = BTreeMap::new();
    let mut expiry_option = None;
    for (index, native) in decisions.into_iter().enumerate() {
        let option =
            OptionId(u32::try_from(index).map_err(|_| ApprovalBuildError::TooManyOptions)?);
        let (label, kind) = classify(&native)?;
        if decision_token(&native).is_some_and(|token| token == "decline") {
            expiry_option = Some(option);
        }
        offered.push(ApprovalOption {
            id: option,
            label,
            kind,
        });
        let result = if decision_token(&native).is_some_and(|token| token == "decline") {
            DECLINE_RESULT.to_owned()
        } else {
            format!(r#"{{"decision":{native}}}"#)
        };
        answers.insert(option, result);
    }
    let expiry_option = expiry_option.ok_or(ApprovalBuildError::UnknownDecision)?;
    Ok(NativeOptions {
        offered,
        answers,
        expiry_option,
    })
}

fn classify(native: &str) -> Result<(Box<str>, PermissionOptionKind), ApprovalBuildError> {
    if let Some(token) = decision_token(native) {
        let kind = match token {
            "accept" => PermissionOptionKind::AllowOnce,
            "acceptForSession" => PermissionOptionKind::AllowAlways,
            "decline" | "cancel" => PermissionOptionKind::RejectOnce,
            _ => return Err(ApprovalBuildError::UnknownDecision),
        };
        return Ok((token.into(), kind));
    }

    let value = serde_json::from_str::<serde_json::Value>(native)
        .map_err(|_| ApprovalBuildError::UnknownDecision)?;
    let object = value
        .as_object()
        .ok_or(ApprovalBuildError::UnknownDecision)?;
    let label = if object.contains_key("acceptWithExecpolicyAmendment") {
        "acceptWithExecpolicyAmendment"
    } else if object.contains_key("applyNetworkPolicyAmendment") {
        "applyNetworkPolicyAmendment"
    } else {
        return Err(ApprovalBuildError::UnknownDecision);
    };
    Ok((label.into(), PermissionOptionKind::AllowAlways))
}

fn decision_token(native: &str) -> Option<&str> {
    let Ok(token) = serde_json::from_str::<&str>(native) else {
        return None;
    };
    Some(token)
}

fn opaque(bytes: &Bytes) -> Result<Opaque, ApprovalBuildError> {
    let text = core::str::from_utf8(bytes).map_err(|_| ApprovalBuildError::UnreadableParameters)?;
    Opaque::borrowed_from(bytes, text).ok_or(ApprovalBuildError::UnreadableParameters)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(text: &str) -> Bytes {
        Bytes::copy_from_slice(text.as_bytes())
    }

    #[test]
    fn command_options_keep_the_exact_provider_response() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                RequestId::Number(41),
                COMMAND_APPROVAL,
                Some(params(
                    r#"{"threadId":"t","turnId":"u","itemId":"i","startedAtMs":1,"command":"cargo test","availableDecisions":["accept","acceptForSession","decline"]}"#,
                )),
                Some(TurnId::first(0)),
            )
            .expect("the provider shape is bound");

        assert_eq!(request.kind, ApprovalKind::Command);
        assert_eq!(request.risk, RiskClass::High);
        assert_eq!(request.options.len(), 3);
        assert!(!request.subject_incomplete);
        let native = book
            .answer(request.id, OptionId(1), request.subject_digest)
            .expect("the exact option is pending");
        assert_eq!(native.request, RequestId::Number(41));
        assert_eq!(native.result, r#"{"decision":"acceptForSession"}"#);
    }

    #[test]
    fn a_file_approval_uses_the_bounded_patch_join() {
        let mut book = ApprovalBook::new();
        book.observe(
            FILE_PATCH_UPDATED,
            &params(
                r#"{"threadId":"t","turnId":"u","itemId":"i","changes":[{"path":"src/main.rs","diff":"+safe"}]}"#,
            ),
        );
        let request = book
            .open(
                RequestId::Number(42),
                FILE_APPROVAL,
                Some(params(
                    r#"{"threadId":"t","turnId":"u","itemId":"i","startedAtMs":1}"#,
                )),
                None,
            )
            .expect("the joined request is bound");

        assert!(!request.subject_incomplete);
        assert!(request.subject.as_str().contains("src/main.rs"));
        assert!(request.subject.as_str().contains("+safe"));
    }

    #[test]
    fn a_missing_file_join_leaves_only_rejection_available() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                RequestId::Number(43),
                FILE_APPROVAL,
                Some(params(
                    r#"{"threadId":"t","turnId":"u","itemId":"missing","startedAtMs":1}"#,
                )),
                None,
            )
            .expect("the request can still be refused");

        assert!(request.subject_incomplete);
        for offered in request.offerable(true) {
            if offered.option.kind.is_rejection() {
                assert!(offered.unavailable.is_none());
            } else {
                assert!(offered.unavailable.is_some());
            }
        }
    }

    #[test]
    fn an_unknown_provider_decision_fails_closed() {
        let mut book = ApprovalBook::new();
        let result = book.open(
            RequestId::Number(44),
            COMMAND_APPROVAL,
            Some(params(
                r#"{"threadId":"t","turnId":"u","itemId":"i","startedAtMs":1,"command":"x","availableDecisions":["vendorNewChoice"]}"#,
            )),
            None,
        );
        assert!(matches!(result, Err(ApprovalBuildError::UnknownDecision)));
    }

    #[test]
    fn expiry_uses_decline_and_removing_it_makes_the_answer_stale() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                RequestId::Number(45),
                COMMAND_APPROVAL,
                Some(params(
                    r#"{"threadId":"t","turnId":"u","itemId":"i","startedAtMs":1,"command":"x","availableDecisions":["cancel","accept"]}"#,
                )),
                None,
            )
            .expect("a rejection is synthesized");
        book.pending
            .get_mut(&request.id)
            .expect("the request is pending")
            .request
            .expires_at = WallMs::EPOCH;

        let expired = book.due(WallMs::now()).expect("the deadline passed");
        assert_eq!(expired.result, DECLINE_RESULT);
        book.complete(expired.approval);
        assert!(book.get(request.id).is_none());
    }
}
