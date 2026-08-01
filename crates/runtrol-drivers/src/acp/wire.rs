//! The small ACP v1 wire surface runtrol originates or decides on.
//!
//! The official schema crate currently enables `serde_json/preserve_order` unconditionally. Cargo features unify
//! across a workspace, so depending on it would add an ordered map and its memory cost to every JSON user in the
//! process. These narrow structures mirror the stable v1 method vocabulary without importing that global feature.
//! They are deliberately not a second content model: native content is serialized through `Opaque` verbatim.

use runtrol_provider::Opaque;
use serde::{Deserialize, Serialize};

pub(super) const INITIALIZE: &str = "initialize";
pub(super) const SESSION_NEW: &str = "session/new";
pub(super) const SESSION_LOAD: &str = "session/load";
pub(super) const SESSION_PROMPT: &str = "session/prompt";
pub(super) const SESSION_CANCEL: &str = "session/cancel";
pub(super) const SESSION_UPDATE: &str = "session/update";

/// ACP v1 initialization parameters.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Initialize<'a> {
    pub(super) protocol_version: u16,
    pub(super) client_capabilities: Empty,
    pub(super) client_info: Implementation<'a>,
}

/// An intentionally empty capability object.
#[derive(Clone, Copy, Serialize)]
pub(super) struct Empty {}

/// Product information sent during protocol initialization.
#[derive(Serialize)]
pub(super) struct Implementation<'a> {
    pub(super) name: &'a str,
    pub(super) version: &'a str,
}

/// The initialization fields runtrol decides on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Initialized {
    pub(super) protocol_version: u16,
    #[serde(default)]
    pub(super) agent_capabilities: serde_json::Map<String, serde_json::Value>,
}

/// Parameters shared by new and load session requests.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NewSession<'a> {
    pub(super) cwd: &'a str,
    pub(super) mcp_servers: [(); 0],
}

/// Parameters for loading a provider-owned session.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LoadSession<'a> {
    pub(super) session_id: &'a str,
    pub(super) cwd: &'a str,
    pub(super) mcp_servers: [(); 0],
}

/// The identity returned by a new session request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NewSessionResult<'a> {
    pub(super) session_id: &'a str,
}

/// One prompt request, preserving native blocks through their raw serializer.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Prompt<'a> {
    pub(super) session_id: &'a str,
    pub(super) prompt: Vec<PromptBlock<'a>>,
}

/// A standard text block or an extension block supplied whole by the caller.
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum PromptBlock<'a> {
    Text(TextBlock<'a>),
    Native(&'a Opaque),
}

/// ACP text content.
#[derive(Serialize)]
pub(super) struct TextBlock<'a> {
    #[serde(rename = "type")]
    pub(super) type_: &'static str,
    pub(super) text: &'a str,
}

/// Parameters for cancellation.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Cancel<'a> {
    pub(super) session_id: &'a str,
}

/// The only prompt result field the supervisor decides on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PromptResult<'a> {
    pub(super) stop_reason: &'a str,
}

#[cfg(test)]
mod tests {
    use runtrol_provider::Opaque;

    use super::*;
    use crate::framing::{RequestId, jsonrpc};

    #[test]
    fn a_native_prompt_block_is_not_reencoded() {
        let original = r#"{"z":1, "a":[2,3]}"#;
        let native = Opaque::owned(original.to_owned());
        let frame = jsonrpc::write_question(
            &RequestId::Number(7),
            SESSION_PROMPT,
            &Prompt {
                session_id: "session-1",
                prompt: vec![PromptBlock::Native(&native)],
            },
        )
        .expect("the prompt serializes");
        assert!(frame.contains(original), "native JSON was altered: {frame}");
    }

    #[test]
    fn stable_v1_methods_have_the_standard_spellings() {
        assert_eq!(INITIALIZE, "initialize");
        assert_eq!(SESSION_NEW, "session/new");
        assert_eq!(SESSION_LOAD, "session/load");
        assert_eq!(SESSION_PROMPT, "session/prompt");
        assert_eq!(SESSION_CANCEL, "session/cancel");
        assert_eq!(SESSION_UPDATE, "session/update");
    }
}
