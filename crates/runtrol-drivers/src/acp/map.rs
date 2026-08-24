//! ACP session updates into the standard content plane.
//!
//! ACP already supplies runtrol's content vocabulary. Mapping therefore lifts only routing and decision fields,
//! while every renderable object remains an exact slice of the provider line. An extension discriminator is not
//! an error and becomes `Unmapped` with the complete frame.

use bytes::Bytes;
use runtrol_provider::{
    Chunk, Cost, EventBody, MessageId, Opaque, ToolCallFrame, ToolCallId, ToolCallStatus, ToolKind,
    Unmapped, Usage,
};
use serde::Deserialize;
use serde_json::value::RawValue;

const MAX_TAG_BYTES: usize = 128;

/// A currency is a code or a short symbol. Bounded because this string is lifted from a provider frame and then
/// held in the gauge for as long as the daemon runs, and an unbounded held string is not a memory contract.
const MAX_CURRENCY_BYTES: usize = 32;
const MAX_ROUTING_TEXT_BYTES: usize = 256;

/// An ACP update could not be routed safely.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum MapError {
    /// A required routing field was absent or had the wrong shape.
    #[error("the ACP update has no usable {field}")]
    Missing { field: &'static str },
    /// The update belongs to another session.
    #[error("the ACP update names a different session")]
    WrongSession,
    /// A provider-owned identifier exceeded the provider seam's bound.
    #[error("the ACP update has an unusable {field}: {detail}")]
    BadId { field: &'static str, detail: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionUpdate<'line> {
    session_id: &'line str,
    #[serde(borrow)]
    update: &'line RawValue,
}

#[derive(Deserialize)]
struct Tagged<'line> {
    #[serde(rename = "sessionUpdate")]
    session_update: &'line str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Content<'line> {
    #[serde(borrow)]
    content: &'line RawValue,
    #[serde(default)]
    message_id: Option<&'line str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tool<'line> {
    #[serde(rename = "toolCallId")]
    id: &'line str,
    #[serde(default)]
    kind: Option<&'line str>,
    #[serde(default)]
    status: Option<&'line str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentMode<'line> {
    current_mode_id: &'line str,
}

#[derive(Deserialize)]
struct UsageFields<'line> {
    used: u64,
    size: u64,
    #[serde(default, borrow)]
    cost: Option<&'line RawValue>,
}

#[derive(Deserialize)]
struct CostFields<'line> {
    amount: f64,
    currency: &'line str,
}

/// Map one `session/update` notification.
pub(super) fn update(line: &Bytes, params: &Bytes, native: &str) -> Result<EventBody, MapError> {
    let notification: SessionUpdate<'_> =
        serde_json::from_slice(params).map_err(|_| MapError::Missing {
            field: "sessionId or update object",
        })?;
    if notification.session_id != native {
        return Err(MapError::WrongSession);
    }

    let tagged: Tagged<'_> =
        serde_json::from_str(notification.update.get()).map_err(|_| MapError::Missing {
            field: "sessionUpdate discriminator",
        })?;
    check_text(
        tagged.session_update,
        MAX_TAG_BYTES,
        "sessionUpdate discriminator",
    )?;

    match tagged.session_update {
        "user_message_chunk" => chunk(notification.update, line, EventBody::UserMessageChunk),
        "agent_message_chunk" => chunk(notification.update, line, EventBody::AgentMessageChunk),
        "agent_thought_chunk" => chunk(notification.update, line, EventBody::AgentThoughtChunk),
        "tool_call" => tool(notification.update, line, false),
        "tool_call_update" => tool(notification.update, line, true),
        "plan" => Ok(EventBody::Plan {
            payload: opaque(line, notification.update)?,
        }),
        "available_commands_update" => Ok(EventBody::AvailableCommandsUpdate {
            payload: opaque(line, notification.update)?,
        }),
        "current_mode_update" => current_mode(notification.update, line),
        "config_option_update" => Ok(EventBody::ConfigOptionUpdate {
            payload: opaque(line, notification.update)?,
        }),
        "session_info_update" => Ok(EventBody::SessionInfoUpdate {
            payload: opaque(line, notification.update)?,
        }),
        "usage_update" => usage(notification.update, line),
        tag => Ok(EventBody::Unmapped(Unmapped {
            tag: tag.into(),
            turn: None,
            payload: whole(line)?,
            unknown_to_binding: true,
        })),
    }
}

/// Preserve a non-update frame whole.
pub(super) fn unmapped(line: &Bytes, tag: &str) -> Result<EventBody, MapError> {
    check_text(tag, MAX_TAG_BYTES, "method")?;
    Ok(EventBody::Unmapped(Unmapped {
        tag: tag.into(),
        turn: None,
        payload: whole(line)?,
        unknown_to_binding: true,
    }))
}

fn chunk(
    raw: &RawValue,
    parent: &Bytes,
    wrap: fn(Chunk) -> EventBody,
) -> Result<EventBody, MapError> {
    let fields: Content<'_> = serde_json::from_str(raw.get()).map_err(|_| MapError::Missing {
        field: "content block",
    })?;
    let message_id = fields
        .message_id
        .map(MessageId::new)
        .transpose()
        .map_err(|error| MapError::BadId {
            field: "messageId",
            detail: error.to_string(),
        })?;
    Ok(wrap(Chunk {
        message_id,
        // ACP reports streaming chunks. A subscriber appends them in arrival order.
        delta: true,
        parent: None,
        content: opaque(parent, fields.content)?,
    }))
}

fn tool(raw: &RawValue, parent: &Bytes, is_update: bool) -> Result<EventBody, MapError> {
    let fields: Tool<'_> = serde_json::from_str(raw.get()).map_err(|_| MapError::Missing {
        field: "toolCallId",
    })?;
    let tool_call_id = ToolCallId::new(fields.id).map_err(|error| MapError::BadId {
        field: "toolCallId",
        detail: error.to_string(),
    })?;
    let frame = ToolCallFrame {
        tool_call_id,
        kind: fields.kind.map(tool_kind),
        status: fields.status.and_then(tool_status),
        delta: is_update,
        payload: opaque(parent, raw)?,
    };
    Ok(if is_update {
        EventBody::ToolCallUpdate(frame)
    } else {
        EventBody::ToolCall(frame)
    })
}

fn current_mode(raw: &RawValue, parent: &Bytes) -> Result<EventBody, MapError> {
    let fields: CurrentMode<'_> =
        serde_json::from_str(raw.get()).map_err(|_| MapError::Missing {
            field: "currentModeId",
        })?;
    check_text(
        fields.current_mode_id,
        MAX_ROUTING_TEXT_BYTES,
        "currentModeId",
    )?;
    Ok(EventBody::CurrentModeUpdate {
        mode_id: fields.current_mode_id.into(),
        // The notification names only the mode now in force; the switchable set is announced at
        // session open, not here.
        available_ids: None,
        payload: opaque(parent, raw)?,
    })
}

fn usage(raw: &RawValue, parent: &Bytes) -> Result<EventBody, MapError> {
    let fields: UsageFields<'_> =
        serde_json::from_str(raw.get()).map_err(|_| MapError::Missing {
            field: "usage gauge",
        })?;
    let cost = fields
        .cost
        .map(|cost| {
            let cost = serde_json::from_str::<CostFields<'_>>(cost.get()).map_err(|_| {
                MapError::Missing {
                    field: "usage cost",
                }
            })?;
            check_text(cost.currency, MAX_CURRENCY_BYTES, "usage cost currency")?;
            Ok(Cost {
                amount: cost.amount,
                currency: cost.currency.into(),
            })
        })
        .transpose()?;
    Ok(EventBody::UsageUpdate(Box::new(Usage {
        used: Some(fields.used),
        size: Some(fields.size),
        cost,
        detail: opaque(parent, raw)?,
    })))
}

fn tool_kind(kind: &str) -> ToolKind {
    match kind {
        "read" => ToolKind::Read,
        "edit" => ToolKind::Edit,
        "delete" => ToolKind::Delete,
        "move" => ToolKind::Move,
        "search" => ToolKind::Search,
        "execute" => ToolKind::Execute,
        "think" => ToolKind::Think,
        "fetch" => ToolKind::Fetch,
        "switch_mode" => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

fn tool_status(status: &str) -> Option<ToolCallStatus> {
    match status {
        "pending" => Some(ToolCallStatus::Pending),
        "in_progress" => Some(ToolCallStatus::InProgress),
        "completed" => Some(ToolCallStatus::Completed),
        "failed" => Some(ToolCallStatus::Failed),
        _ => None,
    }
}

fn check_text(text: &str, max: usize, field: &'static str) -> Result<(), MapError> {
    if text.len() > max {
        return Err(MapError::BadId {
            field,
            detail: format!("{} bytes, over the {max} byte limit", text.len()),
        });
    }
    if text.chars().any(char::is_control) {
        return Err(MapError::BadId {
            field,
            detail: "contains a control character".to_owned(),
        });
    }
    Ok(())
}

fn opaque(parent: &Bytes, raw: &RawValue) -> Result<Opaque, MapError> {
    Opaque::borrowed_from(parent, raw.get()).ok_or(MapError::Missing {
        field: "provider payload slice",
    })
}

fn whole(line: &Bytes) -> Result<Opaque, MapError> {
    let text = core::str::from_utf8(line).map_err(|_| MapError::Missing {
        field: "UTF-8 provider frame",
    })?;
    Opaque::borrowed_from(line, text).ok_or(MapError::Missing {
        field: "whole provider frame",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standard_chunk_keeps_the_exact_content_slice() {
        let line = Bytes::from_static(
            br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content": {"type":"text","text":"hello"},"messageId":"m-1"}}}"#,
        );
        let params_text = core::str::from_utf8(&line)
            .expect("ASCII")
            .split_once(r#""params":"#)
            .map(|(_, rest)| &rest[..rest.len() - 1])
            .expect("params");
        let params = line.slice_ref(params_text.as_bytes());
        let body = update(&line, &params, "s-1").expect("standard update");
        let EventBody::AgentMessageChunk(chunk) = body else {
            panic!("expected an agent message chunk");
        };
        assert_eq!(
            chunk.message_id.as_ref().map(MessageId::as_str),
            Some("m-1")
        );
        assert_eq!(chunk.content.as_str(), r#"{"type":"text","text":"hello"}"#);
    }

    #[test]
    fn a_future_update_reaches_the_subscriber_as_the_whole_frame() {
        let line = Bytes::from_static(
            br#"{"method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"future_shape","z":1}}}"#,
        );
        let params_text = core::str::from_utf8(&line)
            .expect("ASCII")
            .split_once(r#""params":"#)
            .map(|(_, rest)| &rest[..rest.len() - 1])
            .expect("params");
        let params = line.slice_ref(params_text.as_bytes());
        let body = update(&line, &params, "s-1").expect("extension update");
        let EventBody::Unmapped(frame) = body else {
            panic!("expected an unmapped frame");
        };
        assert_eq!(&*frame.tag, "future_shape");
        assert_eq!(
            frame.payload.as_str(),
            core::str::from_utf8(&line).expect("ASCII")
        );
    }

    #[test]
    fn a_nested_standard_update_remains_serializable_from_the_whole_frame() {
        let line = Bytes::from_static(
            br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review changes","input":{"hint":"path"}}]}}}"#,
        );
        let params_text = core::str::from_utf8(&line)
            .expect("ASCII")
            .split_once(r#""params":"#)
            .map(|(_, rest)| &rest[..rest.len() - 1])
            .expect("params");
        let params = line.slice_ref(params_text.as_bytes());
        let body = update(&line, &params, "s-1").expect("standard update");
        let encoded = serde_json::to_string(&body).expect("serializable update");
        assert_eq!(
            encoded,
            r#"{"event":"availableCommandsUpdate","payload":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review changes","input":{"hint":"path"}}]}}"#,
            "the outer tag and exact nested provider object must cross without double encoding"
        );
    }
}
