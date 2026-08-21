//! Project-scoped Runtime credentials owned by Agent Tools.

use std::cmp::Reverse;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};

use runtrol_core::{AgentToolSlot, RuntrolHome};
use runtrol_provider::AbsPath;
use runtrol_runtime_client::{IntegrationCredentials, IntegrationIdentity};
use runtrol_runtime_protocol::{AppScope, IntegrationGrant};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::AgentToolsError;

const SLOT_DOMAIN: &[u8] = b"runtrol/agent-tools/root-slot/1";
const RECORD_SCHEMA: u8 = 1;
const MAX_GRANT_BYTES: u64 = 64 * 1024;
const MAX_SLOTS: usize = 64;

/// Exact public Runtime authority Agent Tools may request and persist.
pub(crate) const SCOPES: [AppScope; 7] = [
    AppScope::ProviderRead,
    AppScope::ModelRead,
    AppScope::SessionList,
    AppScope::SessionOutputRead,
    AppScope::SessionStart,
    AppScope::SessionInputWrite,
    AppScope::SessionStop,
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrantRecord {
    schema: u8,
    root: String,
    grant: IntegrationGrant,
}

/// A prepared project identity, before or after Runtime approval.
pub(crate) struct PreparedCredential {
    pub(crate) root: AbsPath,
    pub(crate) identity: IntegrationIdentity,
    slot: AgentToolSlot,
}

/// One approved credential selected for an MCP process.
#[derive(Clone)]
pub(crate) struct ApprovedCredential {
    pub(crate) root: AbsPath,
    pub(crate) credentials: IntegrationCredentials,
}

/// One exact project slot already present on disk, including a partial slot left by an interrupted command.
pub(crate) struct StoredCredential {
    pub(crate) root: AbsPath,
    pub(crate) grant: Option<IntegrationGrant>,
    slot: AgentToolSlot,
}

/// The bounded Agent Tools area inside one Runtrol home.
pub(crate) struct CredentialStore {
    home: RuntrolHome,
}

impl CredentialStore {
    pub(crate) fn open() -> Result<Self, AgentToolsError> {
        Ok(Self {
            home: RuntrolHome::open()?,
        })
    }

    pub(crate) fn prepare(&self, root: &str) -> Result<PreparedCredential, AgentToolsError> {
        let root = AbsPath::canonicalize(root)?;
        if !root.as_std_path().is_dir() {
            return Err(AgentToolsError::Authority(format!(
                "Agent Tools root {} is not a directory",
                root.as_str()
            )));
        }
        let slot = self.slot(&root)?;
        match std::fs::create_dir(slot.directory().as_std_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if !slot.directory().as_std_path().is_dir() {
                    return Err(AgentToolsError::io(
                        "opening the project credential directory",
                        slot.directory().as_std_path(),
                        &error,
                    ));
                }
            }
            Err(error) => {
                return Err(AgentToolsError::io(
                    "creating the project credential directory",
                    slot.directory().as_std_path(),
                    &error,
                ));
            }
        }
        let secret = if slot.grant().as_std_path().exists() {
            runtrol_vault::ProtectedSecret::load(slot.identity())?
        } else {
            runtrol_vault::ProtectedSecret::load_or_create(slot.identity())?
        };
        let identity = IntegrationIdentity::from_secret_bytes(*secret.as_bytes());
        Ok(PreparedCredential {
            root,
            identity,
            slot,
        })
    }

    pub(crate) fn existing(&self, root: &str) -> Result<Option<StoredCredential>, AgentToolsError> {
        let root = AbsPath::canonicalize(root)?;
        let slot = self.slot(&root)?;
        if !slot.directory().as_std_path().exists() {
            return Ok(None);
        }
        if !slot.directory().as_std_path().is_dir() {
            return Err(AgentToolsError::Credential {
                path: slot.directory().as_str().to_owned(),
                why: "the project credential slot is not a directory".to_owned(),
            });
        }
        let grant = if slot.grant().as_std_path().exists() {
            let record = read_record(slot.grant())?;
            validate_record(&record, &root, slot.grant())?;
            Some(record.grant)
        } else {
            None
        };
        Ok(Some(StoredCredential { root, grant, slot }))
    }

    pub(crate) fn has_approved_other_than(&self, root: &AbsPath) -> Result<bool, AgentToolsError> {
        Ok(self
            .records()?
            .into_iter()
            .any(|(approved_root, _, _)| approved_root != *root))
    }

    pub(crate) fn approved_roots(&self) -> Result<Vec<AbsPath>, AgentToolsError> {
        let mut roots = self
            .records()?
            .into_iter()
            .map(|(root, _, _)| root)
            .collect::<Vec<_>>();
        roots.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(roots)
    }

    pub(crate) fn remove(stored: &StoredCredential) -> Result<(), AgentToolsError> {
        match std::fs::remove_file(stored.slot.grant().as_std_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AgentToolsError::io(
                    "removing an approved project grant",
                    stored.slot.grant().as_std_path(),
                    &error,
                ));
            }
        }
        runtrol_vault::ProtectedSecret::delete(stored.slot.identity())?;
        match std::fs::remove_dir(stored.slot.directory().as_std_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AgentToolsError::io(
                "removing the empty project credential directory",
                stored.slot.directory().as_std_path(),
                &error,
            )),
        }
    }

    pub(crate) fn approved(
        prepared: &PreparedCredential,
    ) -> Result<Option<ApprovedCredential>, AgentToolsError> {
        if !prepared.slot.grant().as_std_path().exists() {
            return Ok(None);
        }
        let record = read_record(prepared.slot.grant())?;
        validate_record(&record, &prepared.root, prepared.slot.grant())?;
        Ok(Some(ApprovedCredential {
            root: prepared.root.clone(),
            credentials: IntegrationCredentials::new(prepared.identity.clone(), record.grant),
        }))
    }

    pub(crate) fn persist(
        prepared: &PreparedCredential,
        grant: IntegrationGrant,
    ) -> Result<ApprovedCredential, AgentToolsError> {
        let record = GrantRecord {
            schema: RECORD_SCHEMA,
            root: prepared.root.as_str().to_owned(),
            grant,
        };
        validate_record(&record, &prepared.root, prepared.slot.grant())?;
        persist_new(prepared.slot.grant(), &record)?;
        Ok(ApprovedCredential {
            root: prepared.root.clone(),
            credentials: IntegrationCredentials::new(prepared.identity.clone(), record.grant),
        })
    }

    pub(crate) fn select_for_current_directory(
        &self,
    ) -> Result<ApprovedCredential, AgentToolsError> {
        let current = std::env::current_dir().map_err(|error| {
            AgentToolsError::io(
                "reading the MCP process working directory",
                self.home.paths().agent_tools().as_std_path(),
                &error,
            )
        })?;
        let current = AbsPath::canonicalize(current.to_str().ok_or_else(|| {
            AgentToolsError::Authority("the MCP working directory is not UTF-8".to_owned())
        })?)?;
        let mut records = self.records()?;
        records.sort_by_key(|record| Reverse(record.0.as_str().len()));
        let Some((root, record, slot)) = records
            .into_iter()
            .find(|(root, _, _)| current.is_under(root))
        else {
            return Err(AgentToolsError::Authority(format!(
                "Agent Tools is not enabled for the current directory {}. run `runtrol tools enable {}` locally",
                current.as_str(),
                current.as_str()
            )));
        };
        let secret = runtrol_vault::ProtectedSecret::load(slot.identity())?;
        let identity = IntegrationIdentity::from_secret_bytes(*secret.as_bytes());
        Ok(ApprovedCredential {
            root,
            credentials: IntegrationCredentials::new(identity, record.grant),
        })
    }

    fn records(&self) -> Result<Vec<(AbsPath, GrantRecord, AgentToolSlot)>, AgentToolsError> {
        let directory = self.home.paths().agent_tools();
        let entries = std::fs::read_dir(directory.as_std_path()).map_err(|error| {
            AgentToolsError::io(
                "listing project credential slots",
                directory.as_std_path(),
                &error,
            )
        })?;
        let mut found = Vec::new();
        for entry in entries.take(MAX_SLOTS + 1) {
            if found.len() == MAX_SLOTS {
                return Err(AgentToolsError::Credential {
                    path: directory.as_str().to_owned(),
                    why: format!("more than {MAX_SLOTS} project credential slots exist"),
                });
            }
            let entry = entry.map_err(|error| {
                AgentToolsError::io(
                    "reading a project credential slot",
                    directory.as_std_path(),
                    &error,
                )
            })?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !valid_slot_name(&name) || !entry.path().is_dir() {
                continue;
            }
            let slot = self.home.paths().agent_tool_slot(&name)?;
            if !slot.grant().as_std_path().exists() {
                continue;
            }
            let record = read_record(slot.grant())?;
            let root = AbsPath::new(&record.root)?;
            validate_record(&record, &root, slot.grant())?;
            if slot_hash(&root) != name {
                return Err(AgentToolsError::Credential {
                    path: slot.grant().as_str().to_owned(),
                    why: "the project root does not match its credential slot".to_owned(),
                });
            }
            found.push((root, record, slot));
        }
        Ok(found)
    }

    fn slot(&self, root: &AbsPath) -> Result<AgentToolSlot, AgentToolsError> {
        Ok(self.home.paths().agent_tool_slot(&slot_hash(root))?)
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

fn validate_record(
    record: &GrantRecord,
    root: &AbsPath,
    path: &AbsPath,
) -> Result<(), AgentToolsError> {
    if record.schema != RECORD_SCHEMA {
        return Err(AgentToolsError::Credential {
            path: path.as_str().to_owned(),
            why: format!("unsupported schema {}", record.schema),
        });
    }
    if record.root != root.as_str() || record.grant.roots != [record.root.clone()] {
        return Err(AgentToolsError::Credential {
            path: path.as_str().to_owned(),
            why: "the grant is not bound to exactly this project root".to_owned(),
        });
    }
    if record.grant.scopes != SCOPES {
        return Err(AgentToolsError::Credential {
            path: path.as_str().to_owned(),
            why: "the grant does not contain exactly the fixed Agent Tools scopes".to_owned(),
        });
    }
    Ok(())
}

fn read_record(path: &AbsPath) -> Result<GrantRecord, AgentToolsError> {
    let mut file = File::open(path.as_std_path()).map_err(|error| {
        AgentToolsError::io(
            "opening an approved project grant",
            path.as_std_path(),
            &error,
        )
    })?;
    let length = file
        .metadata()
        .map_err(|error| {
            AgentToolsError::io(
                "measuring an approved project grant",
                path.as_std_path(),
                &error,
            )
        })?
        .len();
    if length > MAX_GRANT_BYTES {
        return Err(AgentToolsError::Credential {
            path: path.as_str().to_owned(),
            why: format!("the grant exceeds {MAX_GRANT_BYTES} bytes"),
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    file.read_to_end(&mut bytes).map_err(|error| {
        AgentToolsError::io(
            "reading an approved project grant",
            path.as_std_path(),
            &error,
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| AgentToolsError::Credential {
        path: path.as_str().to_owned(),
        why: format!("it is not the closed grant record: {error}"),
    })
}

fn persist_new(path: &AbsPath, record: &GrantRecord) -> Result<(), AgentToolsError> {
    let bytes = serde_json::to_vec(record).map_err(|error| AgentToolsError::Credential {
        path: path.as_str().to_owned(),
        why: format!("the approved grant cannot be encoded: {error}"),
    })?;
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(|error| AgentToolsError::Credential {
        path: path.as_str().to_owned(),
        why: format!("a unique temporary name cannot be generated: {error}"),
    })?;
    let temporary = path
        .as_std_path()
        .with_file_name(format!("grant.json.new-{}", hex(&suffix)));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            AgentToolsError::io("creating a new approved grant", &temporary, &error)
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| AgentToolsError::io("writing a new approved grant", &temporary, &error))?;
    drop(file);
    std::fs::rename(&temporary, path.as_std_path())
        .map_err(|error| AgentToolsError::io("installing a new approved grant", &temporary, &error))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        if let Some(high) = char::from_digit(u32::from(byte >> 4), 16) {
            output.push(high);
        }
        if let Some(low) = char::from_digit(u32::from(byte & 0x0f), 16) {
            output.push(low);
        }
    }
    output
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
}
