//! A deterministic ACP v1 process used by the manifest-only integration gate.
//!
//! This is not a shipped provider. It is a child executable with the same process and standard-stream boundary as
//! one, so the gate can prove the generic driver without a credential, network call, token, or provider name.

use std::io::{BufRead as _, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

const NATIVE_SESSION: &str = "fixture-session";
const UNIQUE_SESSIONS_ENV: &str = "RUNTROL_ACP_FIXTURE_UNIQUE_SESSIONS";

/// Provider-owned session metadata used only when a gate asks for persistence.
///
/// This is deliberately not a transcript. It records only the provider's native session identifier and how many
/// turns reached provider-declared completion, which is enough to prove that deleting runtrol's home did not delete
/// the provider's session state.
#[derive(serde::Deserialize, serde::Serialize)]
struct SessionMarker {
    native: String,
    completed_turns: u64,
}

enum Mode {
    Version,
    Serve {
        state: Option<PathBuf>,
        reply_bytes: Option<usize>,
    },
    Resume {
        state: PathBuf,
        native: String,
    },
    /// A terminal interface for a hosted PTY: a banner, then every line typed comes back. What the
    /// generation handover gate hosts, so a terminal can outlive the Runtime generation that opened it.
    Terminal,
}

fn main() -> ExitCode {
    let Ok(mode) = mode() else {
        return ExitCode::FAILURE;
    };
    let result = match mode {
        Mode::Version => writeln!(std::io::stdout().lock(), "acp-fixture 1.0.0").map_err(|_| ()),
        Mode::Serve { state, reply_bytes } => serve(state.as_deref(), reply_bytes),
        Mode::Resume { state, native } => resume(&state, &native),
        Mode::Terminal => terminal(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn mode() -> Result<Mode, ()> {
    let words = std::env::args().skip(1).collect::<Vec<_>>();
    match words.as_slice() {
        [flag] if flag == "--version" => Ok(Mode::Version),
        [flag] if flag == "--tui" => Ok(Mode::Terminal),
        [] => Ok(Mode::Serve {
            state: None,
            reply_bytes: None,
        }),
        [state_flag, state] if state_flag == "--state" => Ok(Mode::Serve {
            state: Some(PathBuf::from(state)),
            reply_bytes: None,
        }),
        [reply_flag, reply_bytes] if reply_flag == "--reply-bytes" => {
            let reply_bytes = reply_bytes.parse::<usize>().map_err(|_| ())?;
            Ok(Mode::Serve {
                state: None,
                reply_bytes: Some(reply_bytes),
            })
        }
        [state_flag, state, resume_flag, native]
            if state_flag == "--state" && resume_flag == "--resume" =>
        {
            Ok(Mode::Resume {
                state: PathBuf::from(state),
                native: native.clone(),
            })
        }
        _ => Err(()),
    }
}

/// The ACP standard mode state this fixture announces on both open paths.
///
/// One definition, so the driver's announced-modes gate and the mode chip always meet the same live
/// producer whether a session is new or resumed.
fn announced_mode_state() -> Value {
    json!({
        "currentModeId": "default",
        "availableModes": [
            {"id": "default", "name": "Default"},
            {"id": "focus", "name": "Focus"}
        ]
    })
}

fn serve(state: Option<&Path>, reply_bytes: Option<usize>) -> Result<(), ()> {
    let input = std::io::stdin();
    let mut output = std::io::stdout().lock();
    let mut lines = input.lock().lines();
    while let Some(line) = lines.next() {
        let line = line.map_err(|_| ())?;
        let frame: Value = serde_json::from_str(&line).map_err(|_| ())?;
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            if frame.get("id") == Some(&json!("fixture-client-question")) {
                // The one refusal was consumed below. A second answer means the deferred question was answered
                // twice when it later crossed the event stream.
                return Err(());
            }
            continue;
        };
        let id = frame.get("id").cloned();
        match method {
            "initialize" => answer(
                &mut output,
                id.as_ref().ok_or(())?,
                &json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "loadSession": true,
                        "promptCapabilities": {"image": true},
                        "sessionCapabilities": {
                            "list": {},
                            "additionalDirectories": {}
                        }
                    },
                    "agentInfo": {"name": "ACP fixture", "version": "1.0.0"}
                }),
            )?,
            "session/new" => {
                let native = native_session(&frame)?;
                client_question(&mut output)?;
                let refusal = lines.next().ok_or(())?.map_err(|_| ())?;
                require_refusal(&refusal)?;
                if let Some(path) = state {
                    write_marker(
                        path,
                        &SessionMarker {
                            native: native.clone(),
                            completed_turns: 0,
                        },
                    )?;
                }
                answer(
                    &mut output,
                    id.as_ref().ok_or(())?,
                    &json!({"sessionId": native, "modes": announced_mode_state()}),
                )?;
            }
            "session/load" => {
                let requested = frame
                    .pointer("/params/sessionId")
                    .and_then(Value::as_str)
                    .ok_or(())?;
                if let Some(path) = state {
                    let marker = read_marker(path)?;
                    if marker.native != requested || marker.completed_turns == 0 {
                        return Err(());
                    }
                }
                answer(
                    &mut output,
                    id.as_ref().ok_or(())?,
                    &json!({"modes": announced_mode_state()}),
                )?;
            }
            "session/list" => list_sessions(&mut output, &frame, id.as_ref().ok_or(())?)?,
            "session/prompt" => {
                let session = frame
                    .pointer("/params/sessionId")
                    .and_then(Value::as_str)
                    .ok_or(())?;
                let completed_turns = match state {
                    Some(path) => {
                        let mut marker = read_marker(path)?;
                        if marker.native != session {
                            return Err(());
                        }
                        marker.completed_turns = marker.completed_turns.checked_add(1).ok_or(())?;
                        write_marker(path, &marker)?;
                        marker.completed_turns
                    }
                    None => 1,
                };
                notify_reply(&mut output, session, completed_turns, reply_bytes)?;
                answer(
                    &mut output,
                    id.as_ref().ok_or(())?,
                    &json!({"stopReason": "end_turn"}),
                )?;
            }
            "session/cancel" => {}
            _ => refuse(&mut output, id.as_ref().ok_or(())?)?,
        }
    }
    Ok(())
}

fn list_sessions(output: &mut impl Write, frame: &Value, id: &Value) -> Result<(), ()> {
    let cwd = frame
        .pointer("/params/cwd")
        .and_then(Value::as_str)
        .ok_or(())?;
    let cursor = frame.pointer("/params/cursor").and_then(Value::as_str);
    let (suffix, next) = match cursor {
        None => ("one", Some("fixture-page-2")),
        Some("fixture-page-2") => ("two", None),
        Some(_) => return Err(()),
    };
    // The identifier carries the folder's hash, unconditionally: a real provider never reuses one session
    // identifier for two different conversations. When this fixture did (one literal id for every cwd), two
    // folders' listings collapsed into one row and a real window's first folder showed empty during the
    // 2026-08-19 eye pass. Uniqueness across folders is the realistic shape, not an option.
    answer(
        output,
        id,
        &json!({
            "sessions": [{
                "sessionId": format!("fixture-native-{:016x}-{suffix}", fnv_hash(cwd)),
                "cwd": cwd,
                "additionalDirectories": [],
                "title": format!("Fixture {suffix}"),
                "updatedAt": "2026-08-13T00:00:00Z",
                "_meta": {"preview": "must not cross Runtime"}
            }],
            "nextCursor": next
        }),
    )
}

fn native_session(frame: &Value) -> Result<String, ()> {
    if std::env::var_os(UNIQUE_SESSIONS_ENV).is_none_or(|value| value.is_empty()) {
        return Ok(NATIVE_SESSION.to_owned());
    }
    let workspace = frame
        .pointer("/params/cwd")
        .and_then(Value::as_str)
        .ok_or(())?;
    Ok(format!("fixture-session-{:016x}", fnv_hash(workspace)))
}

fn fnv_hash(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn notify_reply(
    output: &mut impl Write,
    session: &str,
    completed_turns: u64,
    reply_bytes: Option<usize>,
) -> Result<(), ()> {
    let reply = reply_bytes.map_or_else(
        || format!("fixture reply {completed_turns}"),
        |bytes| "x".repeat(bytes),
    );
    notify(
        output,
        &json!({
            "sessionId": session,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": reply},
                "messageId": "fixture-message"
            }
        }),
    )
}

fn read_marker(path: &Path) -> Result<SessionMarker, ()> {
    let encoded = std::fs::read(path).map_err(|_| ())?;
    serde_json::from_slice(&encoded).map_err(|_| ())
}

fn write_marker(path: &Path, marker: &SessionMarker) -> Result<(), ()> {
    let encoded = serde_json::to_vec(marker).map_err(|_| ())?;
    std::fs::write(path, encoded).map_err(|_| ())
}

/// The fixture as a terminal program: something on the screen at once, then an echo of every line until
/// the terminal closes. Nothing here is read for meaning, which is the point of a hosted terminal.
fn terminal() -> Result<(), ()> {
    let mut output = std::io::stdout().lock();
    writeln!(output, "acp-fixture terminal ready").map_err(|_| ())?;
    output.flush().map_err(|_| ())?;
    let input = std::io::stdin();
    for line in input.lock().lines() {
        let line = line.map_err(|_| ())?;
        writeln!(output, "echo: {line}").map_err(|_| ())?;
        output.flush().map_err(|_| ())?;
    }
    Ok(())
}

fn resume(path: &Path, native: &str) -> Result<(), ()> {
    let marker = read_marker(path)?;
    if marker.native != native || marker.completed_turns == 0 {
        return Err(());
    }
    let mut output = std::io::stdout().lock();
    write_frame(
        &mut output,
        &json!({"native": marker.native, "completedTurns": marker.completed_turns}),
    )
}

fn client_question(output: &mut impl Write) -> Result<(), ()> {
    write_frame(
        output,
        &json!({
            "jsonrpc": "2.0",
            "id": "fixture-client-question",
            "method": "fs/read_text_file",
            "params": {"path": "not-read-by-runtrol"}
        }),
    )
}

fn require_refusal(line: &str) -> Result<(), ()> {
    let frame: Value = serde_json::from_str(line).map_err(|_| ())?;
    if frame.get("id") != Some(&json!("fixture-client-question"))
        || frame.pointer("/error/code").and_then(Value::as_i64) != Some(-32601)
    {
        return Err(());
    }
    Ok(())
}

fn answer(output: &mut impl Write, id: &Value, result: &Value) -> Result<(), ()> {
    write_frame(
        output,
        &json!({"jsonrpc": "2.0", "id": id, "result": result}),
    )
}

fn notify(output: &mut impl Write, params: &Value) -> Result<(), ()> {
    write_frame(
        output,
        &json!({"jsonrpc": "2.0", "method": "session/update", "params": params}),
    )
}

fn refuse(output: &mut impl Write, id: &Value) -> Result<(), ()> {
    write_frame(
        output,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "method not found"}
        }),
    )
}

fn write_frame(output: &mut impl Write, frame: &Value) -> Result<(), ()> {
    serde_json::to_writer(&mut *output, frame).map_err(|_| ())?;
    output.write_all(b"\n").map_err(|_| ())?;
    output.flush().map_err(|_| ())
}
