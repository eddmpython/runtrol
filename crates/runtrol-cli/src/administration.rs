//! Interactive local administration for the independently usable Runtime.
//!
//! This surface never opens storage and never grants authority itself. It reads the private owner-only daemon wire,
//! shows the exact bounded subject, and sends the operator's narrowed decision back to the daemon implementation also
//! used by Studio.

use std::io::{BufRead, Write};
use std::path::Path;

use runtrol_ipc::wire::{
    IntegrationEnrollmentLine, IntegrationLine, Request, Response, RuntimeForgetLine,
    RuntimeKeyRotationLine, RuntimeSharedOpenLine,
};

use crate::ask::{Failed, Outcome, request};

/// A local administration command could not be understood or completed safely.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AdministrationFailure {
    /// The requested administration command has an invalid exact shape.
    #[error("{0}")]
    Usage(&'static str),
    /// Authority-changing administration was attempted without an attached owner terminal.
    #[error(
        "this administration command requires interactive terminal input; piped input is refused"
    )]
    NonInteractive,
    /// The interactive input ended before the decision was complete.
    #[error("interactive input ended before the administration decision was complete")]
    InputClosed,
    /// An interactive selection did not name a valid subset.
    #[error("{0}")]
    InvalidSelection(String),
    /// The daemon returned a response that does not belong to the exact request.
    #[error("the daemon answered {operation} with {answer}")]
    Unexpected {
        /// Operation being carried out.
        operation: &'static str,
        /// Response variant received.
        answer: String,
    },
    /// The private local daemon exchange failed.
    #[error(transparent)]
    Request(#[from] Failed),
    /// The terminal could not be read or written.
    #[error("the interactive terminal could not be used: {0}")]
    Io(#[from] std::io::Error),
    /// Machine-readable output could not be encoded.
    #[error("the administration inventory could not be encoded: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdministrationCommand<'words> {
    IntegrationList { json: bool },
    IntegrationReview { pending_id: &'words str },
    IntegrationRevoke { integration_id: &'words str },
    RequestReview { confirmation_id: &'words str },
    ProviderHelp { provider_id: &'words str },
}

impl AdministrationCommand<'_> {
    const fn changes_authority(self) -> bool {
        matches!(
            self,
            Self::IntegrationReview { .. }
                | Self::IntegrationRevoke { .. }
                | Self::RequestReview { .. }
        )
    }
}

/// Whether the argument vector belongs to the independent Runtime administration surface.
#[must_use]
pub fn is_administration(words: &[String]) -> bool {
    matches!(
        words.first().map(String::as_str),
        Some("integrations" | "requests" | "providers")
    )
}

/// Run one local administration command against the executable generation's private endpoint.
///
/// `interactive` must describe a real terminal attached to standard input and the prompt stream. Passing `true`
/// from an environment variable or command flag would violate this boundary; the executable derives it from the OS.
///
/// # Errors
///
/// [`AdministrationFailure`] when the command is malformed, an authority mutation has no interactive owner terminal,
/// the operator's exact confirmation differs, or the daemon exchange cannot complete.
pub async fn administer<Read, Output>(
    address: &str,
    executable: &Path,
    words: &[String],
    interactive: bool,
    input: &mut Read,
    output: &mut Output,
) -> Result<Outcome, AdministrationFailure>
where
    Read: BufRead,
    Output: Write,
{
    let command = parse(words)?;
    if command.changes_authority() && !interactive {
        return Err(AdministrationFailure::NonInteractive);
    }
    match command {
        AdministrationCommand::IntegrationList { json } => {
            list_integrations(address, executable, json, output).await
        }
        AdministrationCommand::IntegrationReview { pending_id } => {
            review_integration(address, executable, pending_id, input, output).await
        }
        AdministrationCommand::IntegrationRevoke { integration_id } => {
            revoke_integration(address, executable, integration_id, input, output).await
        }
        AdministrationCommand::RequestReview { confirmation_id } => {
            review_request(address, executable, confirmation_id, input, output).await
        }
        AdministrationCommand::ProviderHelp { provider_id } => {
            show_provider_help(address, executable, provider_id, output).await
        }
    }
}

fn parse(words: &[String]) -> Result<AdministrationCommand<'_>, AdministrationFailure> {
    let shape = words.iter().map(String::as_str).collect::<Vec<_>>();
    match shape.as_slice() {
        ["integrations", "list"] => Ok(AdministrationCommand::IntegrationList { json: false }),
        ["integrations", "list", "--json"] => {
            Ok(AdministrationCommand::IntegrationList { json: true })
        }
        ["integrations", "review", pending_id] => {
            Ok(AdministrationCommand::IntegrationReview { pending_id })
        }
        ["integrations", "revoke", integration_id] => {
            Ok(AdministrationCommand::IntegrationRevoke { integration_id })
        }
        ["requests", "review", confirmation_id] => {
            Ok(AdministrationCommand::RequestReview { confirmation_id })
        }
        ["providers", "help", provider_id] => {
            Ok(AdministrationCommand::ProviderHelp { provider_id })
        }
        ["integrations", ..] => Err(AdministrationFailure::Usage(
            "usage: runtrol integrations list [--json] | review <pending-id> | revoke <integration-id>",
        )),
        ["requests", ..] => Err(AdministrationFailure::Usage(
            "usage: runtrol requests review <pending-id>",
        )),
        ["providers", ..] => Err(AdministrationFailure::Usage(
            "usage: runtrol providers help <provider-id>",
        )),
        _ => Err(AdministrationFailure::Usage(
            "not a Runtime administration command",
        )),
    }
}

async fn list_integrations<Output: Write>(
    address: &str,
    executable: &Path,
    json: bool,
    output: &mut Output,
) -> Result<Outcome, AdministrationFailure> {
    let response = request(address, executable, Request::Integrations).await?;
    let Some(response) = accepted(response, output)? else {
        return Ok(Outcome::Refused);
    };
    let Response::Integrations(rows) = response else {
        return Err(unexpected("integration listing", &response));
    };
    if json {
        writeln!(output, "{}", serde_json::to_string_pretty(&rows)?)?;
        return Ok(Outcome::Carried);
    }
    if rows.is_empty() {
        writeln!(output, "No Runtime integrations.")?;
        return Ok(Outcome::Carried);
    }
    for row in rows {
        writeln!(output, "Integration: {}", row.integration_id)?;
        writeln!(output, "  Label: {}", row.label)?;
        writeln!(output, "  Instance: {}", row.client_instance_id)?;
        writeln!(
            output,
            "  State: {}",
            if row.revoked { "revoked" } else { "active" }
        )?;
        writeln!(output, "  Key generation: {}", row.key_generation)?;
        writeln!(output, "  Grant generation: {}", row.grant_generation)?;
        writeln!(output, "  Scopes: {}", joined(&row.scopes))?;
        writeln!(output, "  Roots: {}", joined(&row.roots))?;
    }
    Ok(Outcome::Carried)
}

async fn review_integration<Read: BufRead, Output: Write>(
    address: &str,
    executable: &Path,
    pending_id: &str,
    input: &mut Read,
    output: &mut Output,
) -> Result<Outcome, AdministrationFailure> {
    let response = request(address, executable, Request::IntegrationEnrollments).await?;
    let Some(response) = accepted(response, output)? else {
        return Ok(Outcome::Refused);
    };
    let Response::IntegrationEnrollments(rows) = response else {
        return Err(unexpected("integration enrollment listing", &response));
    };
    let Some(enrollment) = rows
        .into_iter()
        .find(|row| row.pending_id.as_ref() == pending_id)
    else {
        writeln!(output, "No pending integration has ID {pending_id}.")?;
        return Ok(Outcome::Refused);
    };
    show_enrollment(&enrollment, output)?;
    let action = prompt(input, output, "Decision [approve/deny/cancel]: ")?;
    match action.trim() {
        "deny" => {
            let response = request(
                address,
                executable,
                Request::IntegrationEnrollmentDeny {
                    pending_id: pending_id.into(),
                },
            )
            .await?;
            done(response, "integration denial", output)
        }
        "approve" => {
            let scopes = choose_subset(input, output, "scope", &enrollment.scopes, false)?;
            let roots = choose_subset(input, output, "root", &enrollment.roots, true)?;
            let typed = prompt(input, output, "Retype the full pending ID to approve: ")?;
            if typed != pending_id {
                return Err(AdministrationFailure::InvalidSelection(
                    "the full pending ID did not match; nothing was approved".to_owned(),
                ));
            }
            let begun = request(
                address,
                executable,
                Request::IntegrationApprovalBegin {
                    pending_id: pending_id.into(),
                    scopes,
                    roots,
                },
            )
            .await?;
            let Some(begun) = accepted(begun, output)? else {
                return Ok(Outcome::Refused);
            };
            let Response::IntegrationApprovalChallenge {
                challenge_id,
                prompt: challenge,
            } = begun
            else {
                return Err(unexpected("integration approval", &begun));
            };
            writeln!(output, "{challenge}")?;
            let answer = prompt(input, output, "Type the exact local challenge: ")?;
            let finished = request(
                address,
                executable,
                Request::IntegrationApprovalFinish {
                    challenge_id,
                    answer: answer.into(),
                },
            )
            .await?;
            let Some(finished) = accepted(finished, output)? else {
                return Ok(Outcome::Refused);
            };
            let Response::IntegrationApproved { integration_id } = finished else {
                return Err(unexpected("integration approval completion", &finished));
            };
            writeln!(output, "Approved integration {integration_id}.")?;
            Ok(Outcome::Carried)
        }
        "cancel" => Ok(Outcome::Refused),
        _ => Err(AdministrationFailure::InvalidSelection(
            "decision must be approve, deny, or cancel".to_owned(),
        )),
    }
}

async fn revoke_integration<Read: BufRead, Output: Write>(
    address: &str,
    executable: &Path,
    integration_id: &str,
    input: &mut Read,
    output: &mut Output,
) -> Result<Outcome, AdministrationFailure> {
    let response = request(address, executable, Request::Integrations).await?;
    let Some(response) = accepted(response, output)? else {
        return Ok(Outcome::Refused);
    };
    let Response::Integrations(rows) = response else {
        return Err(unexpected("integration listing", &response));
    };
    let Some(row) = rows
        .into_iter()
        .find(|row| row.integration_id.as_ref() == integration_id && !row.revoked)
    else {
        writeln!(output, "No active integration has ID {integration_id}.")?;
        return Ok(Outcome::Refused);
    };
    show_integration(&row, output)?;
    let typed = prompt(input, output, "Retype the full integration ID to revoke: ")?;
    if typed != integration_id {
        return Err(AdministrationFailure::InvalidSelection(
            "the full integration ID did not match; nothing was revoked".to_owned(),
        ));
    }
    let response = request(
        address,
        executable,
        Request::IntegrationRevoke {
            integration_id: integration_id.into(),
        },
    )
    .await?;
    done(response, "integration revocation", output)
}

enum PresenceRequest {
    Forget(RuntimeForgetLine),
    KeyRotation(RuntimeKeyRotationLine),
    SharedOpen(RuntimeSharedOpenLine),
}

impl PresenceRequest {
    fn confirmation_id(&self) -> &str {
        match self {
            Self::Forget(row) => &row.confirmation_id,
            Self::KeyRotation(row) => &row.confirmation_id,
            Self::SharedOpen(row) => &row.confirmation_id,
        }
    }

    fn confirmation_request(&self) -> Request {
        match self {
            Self::Forget(row) => Request::RuntimeForgetConfirm {
                confirmation_id: row.confirmation_id.clone(),
            },
            Self::KeyRotation(row) => Request::RuntimeKeyRotationConfirm {
                confirmation_id: row.confirmation_id.clone(),
            },
            Self::SharedOpen(row) => Request::RuntimeSharedOpenConfirm {
                confirmation_id: row.confirmation_id.clone(),
            },
        }
    }
}

async fn review_request<Read: BufRead, Output: Write>(
    address: &str,
    executable: &Path,
    confirmation_id: &str,
    input: &mut Read,
    output: &mut Output,
) -> Result<Outcome, AdministrationFailure> {
    let mut pending = Vec::new();
    let responses = [
        Request::RuntimeForgetRequests,
        Request::RuntimeKeyRotationRequests,
        Request::RuntimeSharedOpenRequests,
    ];
    for request_kind in responses {
        let response = request(address, executable, request_kind).await?;
        let Some(response) = accepted(response, output)? else {
            return Ok(Outcome::Refused);
        };
        match response {
            Response::RuntimeForgetRequests(rows) => {
                pending.extend(rows.into_iter().map(PresenceRequest::Forget));
            }
            Response::RuntimeKeyRotationRequests(rows) => {
                pending.extend(rows.into_iter().map(PresenceRequest::KeyRotation));
            }
            Response::RuntimeSharedOpenRequests(rows) => {
                pending.extend(rows.into_iter().map(PresenceRequest::SharedOpen));
            }
            other => return Err(unexpected("Runtime request listing", &other)),
        }
    }
    let Some(selected) = pending
        .into_iter()
        .find(|candidate| candidate.confirmation_id() == confirmation_id)
    else {
        writeln!(
            output,
            "No pending Runtime request has ID {confirmation_id}."
        )?;
        return Ok(Outcome::Refused);
    };
    show_presence_request(&selected, output)?;
    let typed = prompt(input, output, "Retype the full request ID to confirm: ")?;
    if typed != confirmation_id {
        return Err(AdministrationFailure::InvalidSelection(
            "the full request ID did not match; nothing was confirmed".to_owned(),
        ));
    }
    let response = request(address, executable, selected.confirmation_request()).await?;
    done(response, "Runtime request confirmation", output)
}

async fn show_provider_help<Output: Write>(
    address: &str,
    executable: &Path,
    provider_id: &str,
    output: &mut Output,
) -> Result<Outcome, AdministrationFailure> {
    let response = request(
        address,
        executable,
        Request::ProviderHelp {
            provider_id: provider_id.into(),
        },
    )
    .await?;
    let Some(response) = accepted(response, output)? else {
        return Ok(Outcome::Refused);
    };
    let Response::ProviderHelp(provider) = response else {
        return Err(unexpected("provider help", &response));
    };
    writeln!(
        output,
        "Provider: {} ({})",
        provider.display_name, provider.provider_id
    )?;
    writeln!(output, "State: {}", provider.installation_state)?;
    if let Some(version) = provider.version {
        writeln!(output, "Version: {version}")?;
    }
    if let Some(why) = provider.why {
        writeln!(output, "Reason: {why}")?;
    }
    writeln!(
        output,
        "Commands below are for you to review and run; Runtrol did not execute them."
    )?;
    optional_line(output, "Install", provider.install.as_deref())?;
    optional_line(output, "Sign in", provider.sign_in.as_deref())?;
    optional_line(output, "Diagnose", provider.diagnose.as_deref())?;
    Ok(Outcome::Carried)
}

fn show_enrollment<Output: Write>(
    enrollment: &IntegrationEnrollmentLine,
    output: &mut Output,
) -> Result<(), std::io::Error> {
    writeln!(output, "Pending integration: {}", enrollment.pending_id)?;
    writeln!(
        output,
        "  Client: {} {}",
        enrollment.client_name, enrollment.client_version
    )?;
    writeln!(output, "  Instance: {}", enrollment.client_instance_id)?;
    writeln!(output, "  Key fingerprint: {}", enrollment.key_fingerprint)?;
    writeln!(output, "  Manifest digest: {}", enrollment.manifest_digest)?;
    writeln!(output, "  Requested scopes: {}", joined(&enrollment.scopes))?;
    writeln!(output, "  Requested roots: {}", joined(&enrollment.roots))?;
    writeln!(output, "  Expires at Unix ms: {}", enrollment.expires_at_ms)
}

fn show_integration<Output: Write>(
    integration: &IntegrationLine,
    output: &mut Output,
) -> Result<(), std::io::Error> {
    writeln!(output, "Integration: {}", integration.integration_id)?;
    writeln!(output, "  Label: {}", integration.label)?;
    writeln!(output, "  Instance: {}", integration.client_instance_id)?;
    writeln!(output, "  Scopes: {}", joined(&integration.scopes))?;
    writeln!(output, "  Roots: {}", joined(&integration.roots))?;
    writeln!(
        output,
        "  Grant generation: {}",
        integration.grant_generation
    )
}

fn show_presence_request<Output: Write>(
    request: &PresenceRequest,
    output: &mut Output,
) -> Result<(), std::io::Error> {
    match request {
        PresenceRequest::Forget(row) => {
            writeln!(
                output,
                "Runtime session forget request: {}",
                row.confirmation_id
            )?;
            writeln!(
                output,
                "  Integration: {} ({})",
                row.integration_label, row.integration_id
            )?;
            writeln!(output, "  Session pointer removed: {}", row.session_id)?;
            writeln!(
                output,
                "  Provider-owned conversation state is not deleted."
            )?;
            writeln!(output, "  Expires at Unix ms: {}", row.expires_at_ms)
        }
        PresenceRequest::KeyRotation(row) => {
            writeln!(
                output,
                "Runtime key rotation request: {}",
                row.confirmation_id
            )?;
            writeln!(
                output,
                "  Integration: {} ({})",
                row.integration_label, row.integration_id
            )?;
            writeln!(
                output,
                "  Current key generation: {}",
                row.current_key_generation
            )?;
            writeln!(output, "  New key fingerprint: {}", row.new_key_fingerprint)?;
            writeln!(
                output,
                "  Existing key credentials stop authenticating after rotation."
            )?;
            writeln!(output, "  Expires at Unix ms: {}", row.expires_at_ms)
        }
        PresenceRequest::SharedOpen(row) => {
            writeln!(
                output,
                "Runtime shared-writer request: {}",
                row.confirmation_id
            )?;
            writeln!(
                output,
                "  Integration: {} ({})",
                row.integration_label, row.integration_id
            )?;
            writeln!(output, "  Operation: {}", row.operation)?;
            writeln!(output, "  Provider: {}", row.provider_id)?;
            writeln!(
                output,
                "  Workspace with an existing writer: {}",
                row.workspace
            )?;
            writeln!(output, "  Expires at Unix ms: {}", row.expires_at_ms)
        }
    }
}

fn choose_subset<Read: BufRead, Output: Write>(
    input: &mut Read,
    output: &mut Output,
    label: &str,
    values: &[Box<str>],
    allow_empty: bool,
) -> Result<Vec<Box<str>>, AdministrationFailure> {
    if values.is_empty() {
        return Ok(Vec::new());
    }
    writeln!(output, "Requested {label}s:")?;
    for (index, value) in values.iter().enumerate() {
        writeln!(output, "  {}. {value}", index + 1)?;
    }
    let empty_note = if allow_empty {
        "; type none to keep none"
    } else {
        ""
    };
    let selected = prompt(
        input,
        output,
        &format!("Keep {label} numbers separated by commas (Enter keeps all{empty_note}): "),
    )?;
    if selected.is_empty() {
        return Ok(values.to_vec());
    }
    if allow_empty && selected == "none" {
        return Ok(Vec::new());
    }
    let mut indexes = selected
        .split(',')
        .map(str::trim)
        .map(|number| {
            number.parse::<usize>().map_err(|_| {
                AdministrationFailure::InvalidSelection(format!(
                    "{number:?} is not a numbered {label}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    indexes.sort_unstable();
    indexes.dedup();
    if indexes.is_empty() && !allow_empty {
        return Err(AdministrationFailure::InvalidSelection(format!(
            "at least one {label} must remain"
        )));
    }
    indexes
        .into_iter()
        .map(|index| {
            index
                .checked_sub(1)
                .and_then(|offset| values.get(offset))
                .cloned()
                .ok_or_else(|| {
                    AdministrationFailure::InvalidSelection(format!(
                        "{index} is outside the displayed {label} list"
                    ))
                })
        })
        .collect()
}

fn prompt<Read: BufRead, Output: Write>(
    input: &mut Read,
    output: &mut Output,
    message: &str,
) -> Result<String, AdministrationFailure> {
    write!(output, "{message}")?;
    output.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Err(AdministrationFailure::InputClosed);
    }
    while line.ends_with(['\r', '\n']) {
        line.pop();
    }
    Ok(line)
}

fn accepted<Output: Write>(
    response: Response,
    output: &mut Output,
) -> Result<Option<Response>, std::io::Error> {
    if let Response::Failed(failure) = &response {
        writeln!(output, "{}", failure.message)?;
        if failure.needs_the_operator {
            writeln!(output, "This needs the owner at the Runtime machine.")?;
        }
        return Ok(None);
    }
    Ok(Some(response))
}

fn done<Output: Write>(
    response: Response,
    operation: &'static str,
    output: &mut Output,
) -> Result<Outcome, AdministrationFailure> {
    let Some(response) = accepted(response, output)? else {
        return Ok(Outcome::Refused);
    };
    if !matches!(response, Response::Done) {
        return Err(unexpected(operation, &response));
    }
    writeln!(output, "Done.")?;
    Ok(Outcome::Carried)
}

fn unexpected(operation: &'static str, response: &Response) -> AdministrationFailure {
    AdministrationFailure::Unexpected {
        operation,
        answer: format!("{response:?}"),
    }
}

fn joined(values: &[Box<str>]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join(", ")
    }
}

fn optional_line<Output: Write>(
    output: &mut Output,
    label: &str,
    value: Option<&str>,
) -> Result<(), std::io::Error> {
    writeln!(output, "{label}: {}", value.unwrap_or("not declared"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn authority_commands_have_no_yes_or_environment_bypass_shape() {
        assert!(parse(&typed("integrations review pending --yes")).is_err());
        assert!(parse(&typed("integrations revoke identity --yes")).is_err());
        assert!(parse(&typed("requests review request --yes")).is_err());
        assert!(
            parse(&typed("integrations review pending"))
                .expect("the exact review shape parses")
                .changes_authority()
        );
    }

    #[tokio::test]
    async fn non_interactive_mutation_is_refused_before_any_daemon_connection() {
        let words = typed("integrations revoke integration-id");
        let result = administer(
            "an endpoint that must never be reached",
            Path::new("a program that must never be started"),
            &words,
            false,
            &mut std::io::Cursor::new(Vec::<u8>::new()),
            &mut Vec::new(),
        )
        .await;
        assert!(matches!(result, Err(AdministrationFailure::NonInteractive)));
    }

    #[test]
    fn read_only_inventory_has_a_machine_readable_shape() {
        assert_eq!(
            parse(&typed("integrations list --json")).expect("the JSON shape parses"),
            AdministrationCommand::IntegrationList { json: true }
        );
        assert!(
            !parse(&typed("providers help measured"))
                .expect("provider help parses")
                .changes_authority()
        );
    }

    #[test]
    fn narrowing_accepts_only_displayed_numbered_values() {
        let values = vec![
            Box::<str>::from("provider.read"),
            Box::<str>::from("session.open"),
        ];
        let mut input = std::io::Cursor::new(b"2\n".to_vec());
        let mut output = Vec::new();
        let selected = choose_subset(&mut input, &mut output, "scope", &values, false)
            .expect("one displayed scope is retained");
        assert_eq!(selected, vec![Box::<str>::from("session.open")]);

        let mut invalid = std::io::Cursor::new(b"3\n".to_vec());
        assert!(choose_subset(&mut invalid, &mut Vec::new(), "scope", &values, false).is_err());
    }

    #[test]
    fn exact_identity_input_keeps_spaces_instead_of_normalizing_them() {
        let mut input = std::io::Cursor::new(b" pending-id \r\n".to_vec());
        let typed = prompt(&mut input, &mut Vec::new(), "challenge")
            .expect("interactive input is readable");
        assert_eq!(typed, " pending-id ");
        assert_ne!(typed, "pending-id");
    }
}
