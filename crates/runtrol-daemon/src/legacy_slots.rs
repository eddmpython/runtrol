//! What earlier Agent Tools builds left inside this home: digest-named credential slots and the Runtime grants
//! they were enrolled under. Read here, removed here, never created here.
//!
//! An earlier build kept one slot per enabled project under `agent-tools/`: a protected identity file and a closed
//! grant record naming one Runtime integration. That surface is gone. What remains is the obligation to take its
//! residue away exactly: an exact slot is removed and its grant revoked, and anything in that directory that is not
//! provably such a slot is reported and preserved.

use std::fs::File;
use std::io::Read as _;

use runtrol_core::AgentToolSlot;
use runtrol_ipc::wire::{LegacyLocalLine, LegacyLocalState};
use runtrol_provider::AbsPath;
use runtrol_runtime_protocol::{AppScope, IntegrationGrant};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::compose::Composed;
use crate::integration_admin::IntegrationAdmin;

const SLOT_DOMAIN: &[u8] = b"runtrol/agent-tools/root-slot/1";
const RECORD_SCHEMA: u8 = 1;
const MAX_GRANT_BYTES: u64 = 64 * 1024;
const MAX_SLOTS: usize = 64;

/// The client name every Agent Tools enrollment connected under; the Runtime kept it as the grant label.
const CLIENT_NAME: &str = "runtrol-agent-tools";
/// The instance name every Agent Tools enrollment was requested under, followed by its public key.
const ENROLLMENT_PREFIX: &str = "runtrol-agent-tools:";

/// The exact public Runtime authority earlier builds requested for every slot; a record with any other set is not
/// one of theirs.
const SCOPES: [AppScope; 7] = [
    AppScope::ProviderRead,
    AppScope::ModelRead,
    AppScope::SessionList,
    AppScope::SessionOutputRead,
    AppScope::SessionStart,
    AppScope::SessionInputWrite,
    AppScope::SessionStop,
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrantRecord {
    schema: u8,
    root: String,
    grant: IntegrationGrant,
}

/// Inspect every entry of the slot directory without creating, changing, or removing any file.
pub(crate) fn inventory(composed: &Composed) -> Vec<LegacyLocalLine> {
    let directory = composed.home.paths().agent_tools();
    let entries = match std::fs::read_dir(directory.as_std_path()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            return vec![line(
                "*",
                LegacyLocalState::Unreadable,
                None,
                None,
                Some(format!("cannot list {}: {error}", directory.as_str())),
            )];
        }
    };
    let mut lines = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index == MAX_SLOTS {
            lines.push(line(
                "*",
                LegacyLocalState::Overflow,
                None,
                None,
                Some(format!("more than {MAX_SLOTS} local entries exist")),
            ));
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                lines.push(line(
                    "*",
                    LegacyLocalState::Unreadable,
                    None,
                    None,
                    Some(format!(
                        "cannot read an entry of {}: {error}",
                        directory.as_str()
                    )),
                ));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !valid_slot_name(&name) || !is_directory {
            lines.push(line(
                &name,
                LegacyLocalState::Unrecognized,
                None,
                None,
                Some("not an exact Runtrol credential slot; preserve it".to_owned()),
            ));
            continue;
        }
        lines.push(inventory_slot(composed, &name));
    }
    lines.sort_by(|left, right| left.slot.cmp(&right.slot));
    lines
}

fn inventory_slot(composed: &Composed, name: &str) -> LegacyLocalLine {
    let slot = match composed.home.paths().agent_tool_slot(name) {
        Ok(slot) => slot,
        Err(error) => {
            return line(
                name,
                LegacyLocalState::Unreadable,
                None,
                None,
                Some(error.to_string()),
            );
        }
    };
    let identity_present = slot.identity().as_std_path().is_file();
    if !slot.grant().as_std_path().exists() {
        let detail = if identity_present {
            "protected identity present, approved grant absent"
        } else {
            "protected identity and approved grant absent"
        };
        return line(
            name,
            LegacyLocalState::Partial,
            None,
            None,
            Some(detail.to_owned()),
        );
    }
    let record = match read_record(slot.grant()) {
        Ok(record) => record,
        Err(why) => return line(name, LegacyLocalState::Invalid, None, None, Some(why)),
    };
    let root = match AbsPath::new(&record.root) {
        Ok(root) => root,
        Err(error) => {
            return line(
                name,
                LegacyLocalState::Invalid,
                None,
                None,
                Some(error.to_string()),
            );
        }
    };
    if let Err(why) = validate_record(&record, &root) {
        return line(
            name,
            LegacyLocalState::Invalid,
            Some(root.as_str()),
            None,
            Some(why),
        );
    }
    if slot_hash(&root) != name {
        return line(
            name,
            LegacyLocalState::Invalid,
            Some(root.as_str()),
            None,
            Some("the project root does not match its credential slot".to_owned()),
        );
    }
    line(
        name,
        if identity_present {
            LegacyLocalState::Approved
        } else {
            LegacyLocalState::OrphanGrant
        },
        Some(root.as_str()),
        Some(&record.grant.integration_id.to_string()),
        (!identity_present).then(|| "protected identity absent".to_owned()),
    )
}

/// Remove every exact slot and revoke every Runtime grant an earlier Agent Tools build held, preserving the rest.
///
/// Grants go before the slots that hold their credentials, so an interrupted run never leaves authority standing
/// with no local record of it. A grant whose slot is already gone is still authority in the Runtime; it is ours by
/// the two names only that product ever enrolled under, and revoking it can take nothing from anybody else. A
/// second run finds nothing exact and reports the same preserved lines.
pub(crate) fn cleanup(composed: &Composed) -> Vec<LegacyLocalLine> {
    let mut live_grants = live_agent_tools_grants(composed);
    let mut lines = Vec::new();
    for found in inventory(composed) {
        if !matches!(
            found.state,
            LegacyLocalState::Approved | LegacyLocalState::OrphanGrant | LegacyLocalState::Partial
        ) {
            lines.push(found);
            continue;
        }
        if let Some(integration_id) = found.integration_id.as_deref()
            && let Some(at) = live_grants
                .iter()
                .position(|grant| grant.as_ref() == integration_id)
            && let Err(why) = IntegrationAdmin::revoke(composed, integration_id)
        {
            // Authority that could not be taken back must stay visible beside its credential, not be orphaned
            // by deleting the record of it.
            lines.push(LegacyLocalLine {
                state: LegacyLocalState::Unreadable,
                detail: Some(
                    format!("the Runtime grant could not be revoked, slot preserved: {why}").into(),
                ),
                ..found
            });
            live_grants.remove(at);
            continue;
        } else if let Some(integration_id) = found.integration_id.as_deref()
            && let Some(at) = live_grants
                .iter()
                .position(|grant| grant.as_ref() == integration_id)
        {
            live_grants.remove(at);
        }
        match remove_slot(composed, &found.slot) {
            Ok(()) => lines.push(LegacyLocalLine {
                state: LegacyLocalState::Removed,
                detail: None,
                ..found
            }),
            Err(why) => lines.push(LegacyLocalLine {
                state: LegacyLocalState::Unreadable,
                detail: Some(format!("removal failed, slot preserved: {why}").into()),
                ..found
            }),
        }
    }
    for integration_id in live_grants {
        lines.push(match IntegrationAdmin::revoke(composed, &integration_id) {
            Ok(()) => line(
                "-",
                LegacyLocalState::Revoked,
                None,
                Some(&integration_id),
                Some("Runtime grant without a local slot".to_owned()),
            ),
            Err(why) => line(
                "-",
                LegacyLocalState::Unreadable,
                None,
                Some(&integration_id),
                Some(format!("the Runtime grant could not be revoked: {why}")),
            ),
        });
    }
    lines
}

/// Every unrevoked Runtime grant that carries both names only Agent Tools ever enrolled under.
///
/// Both, so a foreign client sharing one of them is not revoked.
fn live_agent_tools_grants(composed: &Composed) -> Vec<Box<str>> {
    IntegrationAdmin::integrations(composed)
        .into_iter()
        .filter(|row| {
            !row.revoked
                && row.label.as_ref() == CLIENT_NAME
                && row.client_instance_id.starts_with(ENROLLMENT_PREFIX)
        })
        .map(|row| row.integration_id)
        .collect()
}

fn remove_slot(composed: &Composed, name: &str) -> Result<(), String> {
    let slot: AgentToolSlot = composed
        .home
        .paths()
        .agent_tool_slot(name)
        .map_err(|error| error.to_string())?;
    match std::fs::remove_file(slot.grant().as_std_path()) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "removing the grant {}: {error}",
                slot.grant().as_str()
            ));
        }
    }
    runtrol_vault::ProtectedSecret::delete(slot.identity()).map_err(|error| error.to_string())?;
    match std::fs::remove_dir(slot.directory().as_std_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "removing the slot directory {}: {error}",
            slot.directory().as_str()
        )),
    }
}

fn line(
    slot: &str,
    state: LegacyLocalState,
    root: Option<&str>,
    integration_id: Option<&str>,
    detail: Option<String>,
) -> LegacyLocalLine {
    LegacyLocalLine {
        slot: slot.into(),
        root: root.map(Into::into),
        integration_id: integration_id.map(Into::into),
        state,
        detail: detail.map(Into::into),
    }
}

fn slot_hash(root: &AbsPath) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SLOT_DOMAIN);
    hasher.update(root.as_str().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn valid_slot_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_record(record: &GrantRecord, root: &AbsPath) -> Result<(), String> {
    if record.schema != RECORD_SCHEMA {
        return Err(format!("unsupported schema {}", record.schema));
    }
    if record.root != root.as_str() || record.grant.roots != [record.root.clone()] {
        return Err("the grant is not bound to exactly this project root".to_owned());
    }
    if record.grant.scopes != SCOPES {
        return Err("the grant does not contain exactly the fixed Agent Tools scopes".to_owned());
    }
    Ok(())
}

fn read_record(path: &AbsPath) -> Result<GrantRecord, String> {
    let mut file = File::open(path.as_std_path())
        .map_err(|error| format!("opening the grant {}: {error}", path.as_str()))?;
    let length = file
        .metadata()
        .map_err(|error| format!("measuring the grant {}: {error}", path.as_str()))?
        .len();
    if length > MAX_GRANT_BYTES {
        return Err(format!("the grant exceeds {MAX_GRANT_BYTES} bytes"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("reading the grant {}: {error}", path.as_str()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("it is not the closed grant record: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_names_are_fixed_lowercase_digests() {
        let root = if cfg!(windows) {
            AbsPath::new(r"C:\work").expect("absolute Windows path")
        } else {
            AbsPath::new("/work").expect("absolute Unix path")
        };
        let slot = slot_hash(&root);
        assert!(valid_slot_name(&slot));
        assert!(!slot.contains(root.as_str()));
    }

    #[test]
    fn a_record_is_ours_only_with_the_schema_the_root_binding_and_the_fixed_scopes() {
        let root = if cfg!(windows) {
            AbsPath::new(r"C:\work").expect("absolute Windows path")
        } else {
            AbsPath::new("/work").expect("absolute Unix path")
        };
        let grant = |scopes: Vec<AppScope>, roots: Vec<String>| GrantRecord {
            schema: RECORD_SCHEMA,
            root: root.as_str().to_owned(),
            grant: IntegrationGrant {
                integration_id: runtrol_runtime_protocol::IntegrationId::new("int_1"),
                scopes,
                roots,
                key_generation: 1,
                grant_generation: 1,
            },
        };
        assert!(
            validate_record(
                &grant(SCOPES.to_vec(), vec![root.as_str().to_owned()]),
                &root
            )
            .is_ok()
        );
        assert!(
            validate_record(
                &grant(vec![AppScope::ProviderRead], vec![root.as_str().to_owned()]),
                &root
            )
            .is_err()
        );
        assert!(
            validate_record(&grant(SCOPES.to_vec(), vec!["elsewhere".to_owned()]), &root).is_err()
        );
    }

    /// A home with one grant the way an earlier Agent Tools build enrolled it, one slot naming that grant, and one
    /// stray file beside the slot.
    fn earlier_build_residue() -> (
        std::path::PathBuf,
        crate::compose::Composed,
        String,
        runtrol_runtime_protocol::IntegrationId,
    ) {
        use base64ct::{Base64UrlUnpadded, Encoding as _};
        use ed25519_dalek::{Signer as _, SigningKey};
        use runtrol_runtime_protocol::self_approval_signing_payload;

        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "runtrol-legacy-slots-{}-{sequence}",
            std::process::id()
        ));
        let home = scratch.join("home");
        let project = scratch.join("project");
        std::fs::create_dir_all(&home).expect("create runtrol home");
        std::fs::create_dir_all(&project).expect("create project root");
        let composed = crate::compose::Composed::for_tests(
            home.to_str().expect("UTF-8 home"),
            runtrol_drivers::builtin(),
        )
        .expect("compose");

        let signing = SigningKey::from_bytes(&[5; 32]);
        let Ok(bytes) = crate::integration_admin::random_key() else {
            panic!("the operating system supplies randomness for a test enrollment key");
        };
        let enrollment = runtrol_store::EnrollmentKey::from_bytes(bytes);
        let now = runtrol_provider::WallMs::now();
        let row = runtrol_store::EnrollmentRow {
            public_key: signing.verifying_key().to_bytes(),
            client_instance_id: "runtrol-agent-tools:key".into(),
            client_name: CLIENT_NAME.into(),
            client_version: "0.1.44".into(),
            manifest_digest: [7; 32],
            scopes: SCOPES
                .iter()
                .map(|scope| scope.to_string().into())
                .collect(),
            roots: vec![project.to_str().expect("UTF-8 root").into()],
            created_at: now,
            expires_at: runtrol_provider::WallMs::from_millis(now.as_millis() + 600_000),
            state: runtrol_store::EnrollmentState::Pending,
        };
        assert!(
            composed
                .store
                .create_enrollment(enrollment, &row)
                .expect("create enrollment")
        );
        let pending = crate::runtime_auth::pending_id(enrollment);
        let payload = self_approval_signing_payload(&pending).expect("canonical payload");
        let signature = Base64UrlUnpadded::encode_string(&signing.sign(&payload).to_bytes());
        let integration =
            match IntegrationAdmin::self_approve(&composed, pending.as_str(), &signature) {
                Ok(integration) => integration,
                Err(error) => panic!("an earlier build approved its own enrollment: {error}"),
            };

        let root = AbsPath::canonicalize(project.to_str().expect("UTF-8 root")).expect("root");
        let slot_name = slot_hash(&root);
        let slot = composed
            .home
            .paths()
            .agent_tool_slot(&slot_name)
            .expect("slot paths");
        std::fs::create_dir_all(slot.directory().as_std_path()).expect("create slot");
        let record = serde_json::json!({
            "schema": RECORD_SCHEMA,
            "root": root.as_str(),
            "grant": {
                "integrationId": integration.as_str(),
                "scopes": SCOPES.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "roots": [root.as_str()],
                "keyGeneration": 1,
                "grantGeneration": 1,
            },
        });
        std::fs::write(
            slot.grant().as_std_path(),
            serde_json::to_vec(&record).expect("record"),
        )
        .expect("write grant");
        std::fs::write(
            composed
                .home
                .paths()
                .agent_tools()
                .join("stray.txt")
                .expect("path")
                .as_std_path(),
            b"not a slot",
        )
        .expect("write stray");
        (scratch, composed, slot_name, integration)
    }

    #[test]
    fn cleanup_revokes_the_grant_removes_the_exact_slot_and_preserves_the_stray_entry() {
        let (scratch, composed, slot_name, integration) = earlier_build_residue();

        let before = inventory(&composed);
        assert!(
            before.iter().any(|line| line.slot.as_ref() == slot_name
                && line.state == LegacyLocalState::OrphanGrant),
            "{before:?}"
        );
        let listed = |composed: &crate::compose::Composed| {
            IntegrationAdmin::integrations(composed)
                .into_iter()
                .any(|row| row.integration_id.as_ref() == integration.as_str())
        };
        assert!(
            listed(&composed),
            "the earlier build's grant is live before cleanup"
        );

        let first = cleanup(&composed);
        let removed = first
            .iter()
            .find(|line| line.slot.as_ref() == slot_name)
            .expect("the slot line");
        assert_eq!(removed.state, LegacyLocalState::Removed, "{first:?}");
        assert!(first.iter().any(|line| {
            line.slot.as_ref() == "stray.txt" && line.state == LegacyLocalState::Unrecognized
        }));
        assert!(
            !listed(&composed),
            "the earlier build's grant is revoked and no longer listed as live"
        );
        assert!(
            !composed
                .home
                .paths()
                .agent_tool_slot(&slot_name)
                .expect("slot paths")
                .directory()
                .as_std_path()
                .exists()
        );

        let second = cleanup(&composed);
        assert!(
            second
                .iter()
                .all(|line| matches!(line.state, LegacyLocalState::Unrecognized)),
            "a second run finds nothing exact: {second:?}"
        );

        drop(composed);
        let _ignored = std::fs::remove_dir_all(&scratch);
    }
}
