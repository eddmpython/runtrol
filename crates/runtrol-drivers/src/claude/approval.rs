//! Claude's provider-native approval channel.
//!
//! The public event carries opaque option identifiers. This module privately retains the request handle, input,
//! and permission suggestion needed to construct the provider's native response. Provider payloads remain shared
//! slices of the input line, so a large tool input is not copied once per option.

use core::time::Duration;
use std::collections::BTreeMap;

use bytes::Bytes;
use runtrol_provider::{
    ApprovalId, ApprovalKind, ApprovalOption, ApprovalRequest, Opaque, OptionId,
    PermissionOptionKind, RiskClass, ToolCallId, ToolKind, TurnId, WallMs,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest as _, Sha256};

/// How many provider questions one session may leave pending.
const MAX_PENDING_APPROVALS: usize = 32;
/// How many provider permission suggestions one question may retain.
const MAX_SUGGESTIONS: usize = 14;
/// The maximum size of a provider request handle.
const MAX_REQUEST_ID_BYTES: usize = 256;
/// A remote round trip has this long before the fail-closed answer is due.
const APPROVAL_WINDOW_MS: u64 = 90_000;

#[derive(Debug, thiserror::Error)]
pub(super) enum ApprovalBuildError {
    #[error("the approval carried no request handle")]
    MissingRequestId,
    #[error("the approval request handle is outside the transport bound")]
    BadRequestId,
    #[error("the approval carried no request body")]
    MissingRequest,
    #[error("the approval request body was not readable")]
    UnreadableRequest,
    #[error("the approval offered more permission suggestions than runtrol will retain")]
    TooManySuggestions,
    #[error("the approval offered a permission suggestion with no stable provider label")]
    UnlabelledSuggestion,
    #[error("the session already holds the maximum number of pending approvals")]
    TooManyPending,
    #[error("the provider reused a request handle that is still pending")]
    DuplicateRequest,
    #[error("the approval is no longer pending")]
    NotPending,
    #[error("the approval subject changed")]
    SubjectChanged,
    #[error("the approval option is not backed by a provider response")]
    OptionMissing,
}

/// A parsed provider question that has not yet been assigned a runtrol approval id.
#[derive(Clone, Debug)]
pub struct IncomingApproval {
    native_request: Box<str>,
    subject: Opaque,
    input: Option<Opaque>,
    suggestions: Vec<Suggestion>,
    tool_call: Option<ToolCallId>,
    kind: ApprovalKind,
    tool: Option<ToolKind>,
}

#[derive(Clone, Debug)]
struct Suggestion {
    label: Box<str>,
    payload: Opaque,
}

/// What has to go back to the provider for one chosen option.
#[derive(Clone, Debug)]
pub(super) struct NativeAnswer {
    pub(super) approval: ApprovalId,
    request: Box<str>,
    choice: NativeChoice,
}

#[derive(Clone, Debug)]
enum NativeChoice {
    AllowOnce { input: Opaque },
    AllowAlways { input: Opaque, permission: Opaque },
    Deny,
}

struct PendingApproval {
    request: ApprovalRequest,
    native_request: Box<str>,
    answers: BTreeMap<OptionId, NativeChoice>,
    expiry_option: OptionId,
}

/// Pending provider questions for one Claude process.
#[derive(Default)]
pub(super) struct ApprovalBook {
    pending: BTreeMap<ApprovalId, PendingApproval>,
}

impl IncomingApproval {
    /// Bind a measured `control_request/can_use_tool` frame without copying its provider payloads.
    pub(super) fn read(
        line: &Bytes,
        request_id: Option<&str>,
        request: Option<&RawValue>,
    ) -> Result<Self, ApprovalBuildError> {
        let native_request = bounded_request_id(request_id)?;
        let request = request.ok_or(ApprovalBuildError::MissingRequest)?;
        let parsed = serde_json::from_str::<ToolRequest<'_>>(request.get())
            .map_err(|_| ApprovalBuildError::UnreadableRequest)?;
        if parsed.permission_suggestions.len() > MAX_SUGGESTIONS {
            return Err(ApprovalBuildError::TooManySuggestions);
        }

        let subject = borrowed(line, request.get())?;
        let input = parsed
            .input
            .filter(|input| input.get() != "null")
            .map(|input| borrowed(line, input.get()))
            .transpose()?;
        let suggestions = parsed
            .permission_suggestions
            .into_iter()
            .map(|suggestion| {
                let shape = serde_json::from_str::<SuggestionShape<'_>>(suggestion.get())
                    .map_err(|_| ApprovalBuildError::UnreadableRequest)?;
                let label = shape
                    .kind
                    .filter(|label| !label.is_empty())
                    .ok_or(ApprovalBuildError::UnlabelledSuggestion)?;
                Ok(Suggestion {
                    label: label.into(),
                    payload: borrowed(line, suggestion.get())?,
                })
            })
            .collect::<Result<Vec<_>, ApprovalBuildError>>()?;
        let (kind, tool) = classify(parsed.input);
        #[expect(
            clippy::manual_ok_err,
            reason = "an unusable optional provider tool id removes correlation but must not discard the approval"
        )]
        let tool_call = parsed.tool_use_id.and_then(|id| match ToolCallId::new(id) {
            Ok(tool_call) => Some(tool_call),
            Err(_) => None,
        });

        Ok(Self {
            native_request,
            subject,
            input,
            suggestions,
            tool_call,
            kind,
            tool,
        })
    }

    pub(super) fn native_request(&self) -> &str {
        &self.native_request
    }
}

impl ApprovalBook {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn get(&self, approval: ApprovalId) -> Option<&ApprovalRequest> {
        self.pending.get(&approval).map(|pending| &pending.request)
    }

    pub(super) fn open(
        &mut self,
        incoming: IncomingApproval,
        turn: Option<TurnId>,
    ) -> Result<ApprovalRequest, ApprovalBuildError> {
        if self.pending.len() >= MAX_PENDING_APPROVALS {
            return Err(ApprovalBuildError::TooManyPending);
        }
        if self
            .pending
            .values()
            .any(|pending| pending.native_request == incoming.native_request)
        {
            return Err(ApprovalBuildError::DuplicateRequest);
        }

        let mut options = Vec::with_capacity(incoming.suggestions.len().saturating_add(2));
        let mut answers = BTreeMap::new();
        if let Some(input) = &incoming.input {
            let option = OptionId(0);
            options.push(ApprovalOption {
                id: option,
                label: "allow".into(),
                kind: PermissionOptionKind::AllowOnce,
            });
            answers.insert(
                option,
                NativeChoice::AllowOnce {
                    input: input.clone(),
                },
            );
            for (index, suggestion) in incoming.suggestions.iter().enumerate() {
                let raw = index.saturating_add(1);
                let option = OptionId(
                    u32::try_from(raw).map_err(|_| ApprovalBuildError::TooManySuggestions)?,
                );
                options.push(ApprovalOption {
                    id: option,
                    label: suggestion.label.clone(),
                    kind: PermissionOptionKind::AllowAlways,
                });
                answers.insert(
                    option,
                    NativeChoice::AllowAlways {
                        input: input.clone(),
                        permission: suggestion.payload.clone(),
                    },
                );
            }
        }
        let expiry_option = OptionId(
            u32::try_from(options.len()).map_err(|_| ApprovalBuildError::TooManySuggestions)?,
        );
        options.push(ApprovalOption {
            id: expiry_option,
            label: "deny".into(),
            kind: PermissionOptionKind::RejectOnce,
        });
        answers.insert(expiry_option, NativeChoice::Deny);

        let subject_digest: [u8; 32] = Sha256::digest(incoming.subject.as_str().as_bytes()).into();
        let approval = ApprovalId::now();
        let risk = if incoming.kind == ApprovalKind::Other {
            RiskClass::High
        } else {
            RiskClass::classify(incoming.kind, incoming.tool, &options)
        };
        let request = ApprovalRequest {
            id: approval,
            turn,
            tool_call: incoming.tool_call,
            kind: incoming.kind,
            risk,
            options,
            subject: incoming.subject,
            subject_incomplete: incoming.input.is_none(),
            subject_digest,
            expires_at: WallMs::now().plus_millis(APPROVAL_WINDOW_MS),
        };
        self.pending.insert(
            approval,
            PendingApproval {
                request: request.clone(),
                native_request: incoming.native_request,
                answers,
                expiry_option,
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
        let choice = pending
            .answers
            .get(&option)
            .cloned()
            .ok_or(ApprovalBuildError::OptionMissing)?;
        Ok(NativeAnswer {
            approval,
            request: pending.native_request.clone(),
            choice,
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

    pub(super) fn cancel(&mut self, native_request: &str) -> Option<ApprovalId> {
        let approval = self
            .pending
            .iter()
            .find(|(_, pending)| &*pending.native_request == native_request)
            .map(|(approval, _)| *approval)?;
        self.complete(approval);
        Some(approval)
    }

    pub(super) fn take_all(&mut self) -> Vec<ApprovalId> {
        let approvals = self.pending.keys().copied().collect();
        self.pending.clear();
        approvals
    }

    pub(super) fn rejections(&self) -> Vec<NativeAnswer> {
        self.pending.values().filter_map(expiry_answer).collect()
    }
}

impl NativeAnswer {
    pub(super) fn frame(&self) -> Result<String, serde_json::Error> {
        match &self.choice {
            NativeChoice::AllowOnce { input } => response_frame(
                &self.request,
                &AllowOnce {
                    behavior: "allow",
                    updated_input: input,
                },
            ),
            NativeChoice::AllowAlways { input, permission } => response_frame(
                &self.request,
                &AllowAlways {
                    behavior: "allow",
                    updated_input: input,
                    updated_permissions: core::slice::from_ref(permission),
                },
            ),
            NativeChoice::Deny => response_frame(&self.request, &Deny { behavior: "deny" }),
        }
    }
}

pub(super) fn error_frame(request: &str) -> Result<String, serde_json::Error> {
    serde_json::to_string(&ControlErrorFrame {
        kind: "control_response",
        response: ControlError {
            subtype: "error",
            request_id: request,
            error: "runtrol does not handle this control request",
        },
    })
}

pub(super) fn deny_frame(request: &str) -> Result<String, serde_json::Error> {
    response_frame(request, &Deny { behavior: "deny" })
}

pub(super) fn bounded_request_id(request: Option<&str>) -> Result<Box<str>, ApprovalBuildError> {
    let request = request.ok_or(ApprovalBuildError::MissingRequestId)?;
    if request.is_empty() || request.len() > MAX_REQUEST_ID_BYTES || request.contains(['\r', '\n'])
    {
        return Err(ApprovalBuildError::BadRequestId);
    }
    Ok(request.into())
}

fn expiry_answer(pending: &PendingApproval) -> Option<NativeAnswer> {
    pending
        .answers
        .get(&pending.expiry_option)
        .cloned()
        .map(|choice| NativeAnswer {
            approval: pending.request.id,
            request: pending.native_request.clone(),
            choice,
        })
}

fn borrowed(line: &Bytes, slice: &str) -> Result<Opaque, ApprovalBuildError> {
    Opaque::borrowed_from(line, slice).ok_or(ApprovalBuildError::UnreadableRequest)
}

fn classify(input: Option<&RawValue>) -> (ApprovalKind, Option<ToolKind>) {
    let Some(input) = input else {
        return (ApprovalKind::Other, None);
    };
    let Ok(shape) = serde_json::from_str::<InputShape<'_>>(input.get()) else {
        return (ApprovalKind::Other, None);
    };
    if shape.command.is_some() {
        return (ApprovalKind::Command, Some(ToolKind::Execute));
    }
    if shape.url.is_some() || shape.host.is_some() {
        return (ApprovalKind::Network, Some(ToolKind::Fetch));
    }
    if shape.file_path.is_some() || shape.path.is_some() || shape.patch.is_some() {
        return (ApprovalKind::FileChange, Some(ToolKind::Edit));
    }
    (ApprovalKind::Other, Some(ToolKind::Other))
}

fn response_frame<T: Serialize>(request: &str, answer: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string(&ControlFrame {
        kind: "control_response",
        response: ControlSuccess {
            subtype: "success",
            request_id: request,
            response: answer,
        },
    })
}

#[derive(Deserialize)]
struct ToolRequest<'line> {
    #[serde(default, borrow)]
    input: Option<&'line RawValue>,
    #[serde(default, borrow)]
    permission_suggestions: Vec<&'line RawValue>,
    #[serde(default)]
    tool_use_id: Option<&'line str>,
}

#[derive(Deserialize)]
struct SuggestionShape<'line> {
    #[serde(rename = "type")]
    kind: Option<&'line str>,
}

#[derive(Deserialize)]
struct InputShape<'line> {
    #[serde(default, borrow)]
    command: Option<&'line RawValue>,
    #[serde(default, borrow)]
    file_path: Option<&'line RawValue>,
    #[serde(default, borrow)]
    path: Option<&'line RawValue>,
    #[serde(default, borrow)]
    patch: Option<&'line RawValue>,
    #[serde(default, borrow)]
    url: Option<&'line RawValue>,
    #[serde(default, borrow)]
    host: Option<&'line RawValue>,
}

#[derive(Serialize)]
struct ControlFrame<'a, T> {
    #[serde(rename = "type")]
    kind: &'static str,
    response: ControlSuccess<'a, T>,
}

#[derive(Serialize)]
struct ControlSuccess<'a, T> {
    subtype: &'static str,
    request_id: &'a str,
    response: T,
}

#[derive(Serialize)]
struct ControlErrorFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    response: ControlError<'a>,
}

#[derive(Serialize)]
struct ControlError<'a> {
    subtype: &'static str,
    request_id: &'a str,
    error: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AllowOnce<'a> {
    behavior: &'static str,
    updated_input: &'a Opaque,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AllowAlways<'a> {
    behavior: &'static str,
    updated_input: &'a Opaque,
    updated_permissions: &'a [Opaque],
}

#[derive(Serialize)]
struct Deny {
    behavior: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Bytes {
        Bytes::copy_from_slice(text.as_bytes())
    }

    fn incoming(text: &str) -> IncomingApproval {
        let bytes = line(text);
        let envelope = serde_json::from_slice::<TestEnvelope<'_>>(&bytes).expect("readable");
        IncomingApproval::read(&bytes, envelope.request_id, envelope.request)
            .expect("the recorded provider shape is bound")
    }

    #[derive(Deserialize)]
    struct TestEnvelope<'line> {
        request_id: Option<&'line str>,
        #[serde(borrow)]
        request: Option<&'line RawValue>,
    }

    #[test]
    fn permission_options_retain_the_native_input_and_suggestion() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                incoming(
                    r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Shell","input":{"command":"cargo test"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Shell"}]}],"tool_use_id":"tool-1"}}"#,
                ),
                Some(TurnId::first(0)),
            )
            .expect("offerable");

        assert_eq!(request.kind, ApprovalKind::Command);
        assert_eq!(request.risk, RiskClass::High);
        assert_eq!(request.options.len(), 3);
        assert!(!request.subject_incomplete);
        let pending = book.pending.get(&request.id).expect("pending");
        let once = pending.answers.get(&OptionId(0)).expect("allow once");
        let standing = pending.answers.get(&OptionId(1)).expect("allow always");
        match (once, standing) {
            (
                NativeChoice::AllowOnce { input: first },
                NativeChoice::AllowAlways { input: second, .. },
            ) => assert_eq!(
                first.bytes().as_ptr(),
                second.bytes().as_ptr(),
                "options must share the provider input instead of copying it"
            ),
            other => panic!("unexpected native choices: {other:?}"),
        }
        let native = book
            .answer(request.id, OptionId(1), request.subject_digest)
            .expect("the standing option is retained");
        let frame = native.frame().expect("serializable");
        let parsed = serde_json::from_str::<serde_json::Value>(&frame).expect("readable");
        assert_eq!(
            parsed
                .pointer("/response/request_id")
                .and_then(|v| v.as_str()),
            Some("req-1")
        );
        assert_eq!(
            parsed
                .pointer("/response/response/updatedInput/command")
                .and_then(|v| v.as_str()),
            Some("cargo test")
        );
        assert_eq!(
            parsed
                .pointer("/response/response/updatedPermissions/0/type")
                .and_then(|v| v.as_str()),
            Some("addRules")
        );
    }

    #[test]
    fn missing_input_is_rejection_only() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                incoming(
                    r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"Unknown"}}"#,
                ),
                None,
            )
            .expect("the request can still be refused");

        assert!(request.subject_incomplete);
        assert_eq!(request.options.len(), 1);
        assert!(
            request
                .options
                .first()
                .is_some_and(|option| option.kind.is_rejection())
        );
    }

    #[test]
    fn expiry_uses_the_native_deny_shape() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                incoming(
                    r#"{"type":"control_request","request_id":"req-3","request":{"subtype":"can_use_tool","input":{"path":"a.txt"}}}"#,
                ),
                None,
            )
            .expect("offerable");
        book.pending
            .get_mut(&request.id)
            .expect("pending")
            .request
            .expires_at = WallMs::EPOCH;

        let native = book.due(WallMs::now()).expect("due");
        let parsed = serde_json::from_str::<serde_json::Value>(&native.frame().expect("frame"))
            .expect("readable");
        assert_eq!(
            parsed
                .pointer("/response/response/behavior")
                .and_then(|v| v.as_str()),
            Some("deny")
        );
        book.complete(native.approval);
        assert!(book.get(request.id).is_none());
    }

    #[test]
    fn cancelling_by_provider_handle_removes_only_that_question() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                incoming(
                    r#"{"type":"control_request","request_id":"req-4","request":{"subtype":"can_use_tool","input":{"url":"https://example.invalid"}}}"#,
                ),
                None,
            )
            .expect("offerable");
        assert_eq!(book.cancel("req-4"), Some(request.id));
        assert!(book.get(request.id).is_none());
    }

    #[test]
    fn a_wrong_subject_digest_does_not_consume_the_question() {
        let mut book = ApprovalBook::new();
        let request = book
            .open(
                incoming(
                    r#"{"type":"control_request","request_id":"req-5","request":{"subtype":"can_use_tool","input":{"command":"cargo test"}}}"#,
                ),
                None,
            )
            .expect("offerable");

        assert!(matches!(
            book.answer(request.id, OptionId(0), [0; 32]),
            Err(ApprovalBuildError::SubjectChanged)
        ));
        assert!(book.get(request.id).is_some());
    }
}
