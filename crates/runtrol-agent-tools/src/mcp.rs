//! Dual-era MCP stdio framing and the fixed Agent Tools catalogue.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::AgentToolsError;

const MAX_LINE_BYTES: usize = 1024 * 1024;
const MODERN_REVISION: &str = "2026-07-28";
const LEGACY_REVISION: &str = "2025-11-25";
const SERVER_NAME: &str = "runtrol-agent-tools";

/// Serve newline-delimited MCP JSON-RPC on standard input and standard output.
///
/// Diagnostics never use stdout. Each request is bounded before JSON parsing, and one response is flushed before
/// another request is read.
///
/// # Errors
///
/// Standard input or output fails, or a response cannot be encoded.
pub async fn serve() -> Result<(), AgentToolsError> {
    let input = std::io::stdin();
    let output = std::io::stdout();
    serve_io(input.lock(), output.lock()).await
}

async fn serve_io<R: std::io::BufRead, W: std::io::Write>(
    mut input: R,
    mut output: W,
) -> Result<(), AgentToolsError> {
    loop {
        let line = match read_bounded_line(&mut input) {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(ReadLineError::TooLarge) => {
                write_response(
                    &mut output,
                    &rpc_error(
                        Value::Null,
                        -32_600,
                        "request exceeds the 1 MiB stdio limit",
                    ),
                )?;
                continue;
            }
            Err(ReadLineError::Io(error)) => {
                return Err(AgentToolsError::Mcp(format!(
                    "standard input failed: {error}"
                )));
            }
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let request = match serde_json::from_slice::<RpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut output,
                    &rpc_error(Value::Null, -32_700, &format!("invalid JSON: {error}")),
                )?;
                continue;
            }
        };
        let id = request.id.clone();
        let response = dispatch(request).await;
        if id.is_some() {
            write_response(&mut output, &response)?;
        }
    }
}

#[derive(Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn dispatch(request: RpcRequest) -> Value {
    let id = request.id.unwrap_or(Value::Null);
    if request.jsonrpc != "2.0" {
        return rpc_error(id, -32_600, "jsonrpc must be 2.0");
    }
    match request.method.as_str() {
        "server/discover" => success(id, discover_result()),
        "initialize" => success(id, initialize_result(&request.params)),
        "ping" => success(id, with_server_meta(json!({}))),
        "tools/list" => success(id, tools_result()),
        "tools/call" => match call_params(request.params) {
            Ok((name, arguments)) => match crate::runtime::call(&name, arguments).await {
                Ok(value) => success(id, tool_result(value, false)),
                Err(error) => success(id, tool_result(json!({ "error": error.to_string() }), true)),
            },
            Err(error) => rpc_error(id, -32_602, &error),
        },
        "notifications/initialized" | "notifications/cancelled" => success(id, json!({})),
        _ => rpc_error(id, -32_601, "method not found"),
    }
}

fn discover_result() -> Value {
    with_server_meta(json!({
        "resultType": "complete",
        "supportedVersions": [MODERN_REVISION],
        "capabilities": { "tools": {} },
        "instructions": "Use Runtrol to delegate bounded work to installed coding CLIs. Start is exclusive. Input and events pass unchanged. Provider approvals always remain with a person in Runtrol."
    }))
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(LEGACY_REVISION);
    let selected = match requested {
        "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" => requested,
        _ => LEGACY_REVISION,
    };
    json!({
        "protocolVersion": selected,
        "capabilities": { "tools": {} },
        "serverInfo": server_info(),
        "instructions": "Use Runtrol to delegate bounded work. Provider approvals require a person in Runtrol."
    })
}

fn tools_result() -> Value {
    with_server_meta(json!({
        "resultType": "complete",
        "tools": tools(),
    }))
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "runtrol_providers",
            "List Runtime-discovered coding providers and their current structural availability. Does not start a provider.",
            json!({ "type": "object", "additionalProperties": false }),
            true,
        ),
        tool(
            "runtrol_models",
            "Ask one Runtime-discovered provider for its current official or provider-owned model catalogue. Provider identifiers stay opaque.",
            json!({
                "type": "object",
                "properties": {
                    "providerId": { "type": "string", "description": "Exact providerId returned by runtrol_providers." }
                },
                "required": ["providerId"],
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            "runtrol_sessions",
            "List structural metadata for Runtime-managed sessions visible under this MCP process's one approved project root. No conversation content is returned.",
            json!({ "type": "object", "additionalProperties": false }),
            true,
        ),
        tool(
            "runtrol_start",
            "Start a new coding-agent session with exclusive workspace access, submit the caller-owned input unchanged, and release control. Overlapping writers are refused. Approvals are never answered by this tool.",
            json!({
                "type": "object",
                "properties": {
                    "providerId": { "type": "string", "description": "Exact providerId returned by runtrol_providers." },
                    "workspace": { "type": "string", "description": "Existing workspace at or below the locally approved project root." },
                    "input": { "type": "string", "description": "Initial instruction transported unchanged." },
                    "model": { "type": "string", "description": "Optional exact provider model id returned by runtrol_models." },
                    "reasoningEffort": { "type": "string", "description": "Optional exact provider reasoning id returned by runtrol_models." },
                    "permission": { "type": "string", "description": "Optional exact safe switchable permission token returned by runtrol_providers." }
                },
                "required": ["providerId", "workspace", "input"],
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            "runtrol_send",
            "Acquire exact generation control of one visible hot session, submit caller-owned input unchanged, and release control. Does not answer provider approvals.",
            session_schema(Some((
                "input",
                "string",
                "Instruction transported unchanged.",
            ))),
            false,
        ),
        tool(
            "runtrol_next_event",
            "Wait up to 30 seconds for one normalized public Runtime event and return it unchanged with the next reconnect cursor. Holds no transcript copy.",
            json!({
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Exact sessionId returned by runtrol_sessions or runtrol_start." },
                    "workspace": { "type": "string", "description": "Exact session workspace under this approved root." },
                    "after": {
                        "type": "object",
                        "description": "Optional exact cursor from an earlier call.",
                        "properties": {
                            "stream": { "type": "string" },
                            "epoch": { "type": "integer", "minimum": 0 },
                            "seq": { "type": "integer", "minimum": 0 }
                        },
                        "required": ["stream", "epoch", "seq"],
                        "additionalProperties": false
                    }
                },
                "required": ["sessionId", "workspace"],
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            "runtrol_stop",
            "Interrupt one exact visible session under a short-lived control lease, then release control. Does not delete provider conversation state.",
            session_schema(None),
            false,
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value, read_only: bool) -> Value {
    let mut definition = json!({
        "name": name,
        "description": description,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": false,
            "idempotentHint": read_only,
            "openWorldHint": false
        }
    });
    if let Some(object) = definition.as_object_mut() {
        object.insert("inputSchema".to_owned(), input_schema);
    }
    definition
}

fn session_schema(extra: Option<(&str, &str, &str)>) -> Value {
    let mut properties = serde_json::Map::from_iter([
        (
            "sessionId".to_owned(),
            json!({ "type": "string", "description": "Exact sessionId returned by runtrol_sessions or runtrol_start." }),
        ),
        (
            "workspace".to_owned(),
            json!({ "type": "string", "description": "Exact session workspace under this approved root." }),
        ),
    ]);
    let mut required = vec![json!("sessionId"), json!("workspace")];
    if let Some((name, kind, description)) = extra {
        properties.insert(
            name.to_owned(),
            json!({ "type": kind, "description": description }),
        );
        required.push(json!(name));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn call_params(params: Value) -> Result<(String, Value), String> {
    let Value::Object(mut object) = params else {
        return Err("tools/call params must be an object".to_owned());
    };
    let name = object
        .remove("name")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| "tools/call needs a string name".to_owned())?;
    let arguments = object.remove("arguments").unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err("tools/call arguments must be an object".to_owned());
    }
    Ok((name, arguments))
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| {
        "{\"error\":\"Agent Tools could not encode its structured result\"}".to_owned()
    });
    let mut result = json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    });
    if let Some(object) = result.as_object_mut() {
        object.insert("structuredContent".to_owned(), value);
    }
    with_server_meta(result)
}

fn with_server_meta(mut result: Value) -> Value {
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "_meta".to_owned(),
            json!({ "io.modelcontextprotocol/serverInfo": server_info() }),
        );
    }
    result
}

fn server_info() -> Value {
    json!({
        "name": SERVER_NAME,
        "version": env!("CARGO_PKG_VERSION")
    })
}

fn success(id: Value, result: Value) -> Value {
    let mut response = serde_json::Map::new();
    response.insert("jsonrpc".to_owned(), Value::String("2.0".to_owned()));
    response.insert("id".to_owned(), id);
    response.insert("result".to_owned(), result);
    Value::Object(response)
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    let mut response = json!({
        "jsonrpc": "2.0",
        "error": { "code": code, "message": message }
    });
    if let Some(object) = response.as_object_mut() {
        object.insert("id".to_owned(), id);
    }
    response
}

fn write_response(
    output: &mut impl std::io::Write,
    response: &Value,
) -> Result<(), AgentToolsError> {
    serde_json::to_writer(&mut *output, response).map_err(|error| {
        AgentToolsError::Mcp(format!("cannot encode a JSON-RPC response: {error}"))
    })?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| AgentToolsError::Mcp(format!("standard output failed: {error}")))
}

#[derive(Debug)]
enum ReadLineError {
    TooLarge,
    Io(std::io::Error),
}

fn read_bounded_line(input: &mut impl std::io::BufRead) -> Result<Option<Vec<u8>>, ReadLineError> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let available = input.fill_buf().map_err(ReadLineError::Io)?;
        if available.is_empty() {
            return if oversized {
                Err(ReadLineError::TooLarge)
            } else if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |at| at + 1);
        if !oversized {
            if line.len().saturating_add(take) > MAX_LINE_BYTES {
                oversized = true;
                line.clear();
            } else {
                let Some(chunk) = available.get(..take) else {
                    return Err(ReadLineError::Io(std::io::Error::other(
                        "the buffered line boundary was inconsistent",
                    )));
                };
                line.extend_from_slice(chunk);
            }
        }
        let ended = available.get(take.saturating_sub(1)) == Some(&b'\n');
        input.consume(take);
        if ended {
            if oversized {
                return Err(ReadLineError::TooLarge);
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.truncate(line.len().saturating_sub(1));
            }
            return Ok(Some(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn catalogue_is_stable_complete_and_has_no_approval_tool() {
        let names = tools()
            .into_iter()
            .filter_map(|tool| tool.get("name")?.as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "runtrol_providers",
                "runtrol_models",
                "runtrol_sessions",
                "runtrol_start",
                "runtrol_send",
                "runtrol_next_event",
                "runtrol_stop",
            ]
        );
        assert!(!names.iter().any(|name| name.contains("approval")));
    }

    #[test]
    fn line_reader_bounds_before_json_parsing_and_resynchronizes() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 1];
        input.extend_from_slice(b"\n{}\n");
        let mut cursor = Cursor::new(input);
        assert!(matches!(
            read_bounded_line(&mut cursor),
            Err(ReadLineError::TooLarge)
        ));
        assert_eq!(
            read_bounded_line(&mut cursor).expect("second line readable"),
            Some(b"{}".to_vec())
        );
    }

    #[test]
    fn both_protocol_eras_receive_their_expected_discovery_shape() {
        let legacy = initialize_result(&json!({ "protocolVersion": "2024-11-05" }));
        assert_eq!(
            legacy.get("protocolVersion").and_then(Value::as_str),
            Some("2024-11-05")
        );
        let modern = discover_result();
        assert_eq!(
            modern.get("supportedVersions"),
            Some(&json!([MODERN_REVISION]))
        );
        assert_eq!(
            modern
                .get("_meta")
                .and_then(|meta| meta.get("io.modelcontextprotocol/serverInfo"))
                .and_then(|info| info.get("name"))
                .and_then(Value::as_str),
            Some(SERVER_NAME)
        );
    }
}
