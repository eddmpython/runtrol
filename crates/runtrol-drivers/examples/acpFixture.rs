//! A deterministic ACP v1 process used by the manifest-only integration gate.
//!
//! This is not a shipped provider. It is a child executable with the same process and standard-stream boundary as
//! one, so the gate can prove the generic driver without a credential, network call, token, or provider name.

use std::io::{BufRead as _, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

fn main() -> ExitCode {
    if std::env::args().skip(1).any(|word| word == "--version") {
        let mut output = std::io::stdout().lock();
        return match writeln!(output, "acp-fixture 1.0.0") {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }

    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn serve() -> Result<(), ()> {
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
                        "promptCapabilities": {"image": true}
                    },
                    "agentInfo": {"name": "ACP fixture", "version": "1.0.0"}
                }),
            )?,
            "session/new" => {
                client_question(&mut output)?;
                let refusal = lines.next().ok_or(())?.map_err(|_| ())?;
                require_refusal(&refusal)?;
                answer(
                    &mut output,
                    id.as_ref().ok_or(())?,
                    &json!({"sessionId": "fixture-session"}),
                )?;
            }
            "session/load" => answer(&mut output, id.as_ref().ok_or(())?, &json!({}))?,
            "session/prompt" => {
                let session = frame
                    .pointer("/params/sessionId")
                    .and_then(Value::as_str)
                    .ok_or(())?;
                notify(
                    &mut output,
                    &json!({
                        "sessionId": session,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": "fixture reply"},
                            "messageId": "fixture-message"
                        }
                    }),
                )?;
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
