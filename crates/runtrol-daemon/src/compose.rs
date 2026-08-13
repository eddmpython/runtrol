//! Assembling the pieces into something that can run.
//!
//! The one place that knows about all of them. Every other crate here is deliberately unable to see most of the
//! others: the kernel cannot see a driver, a driver cannot see storage, the command surface cannot see either. Those
//! missing edges are what make the architecture checkable, and the price of having them is that somebody has to do the
//! joining. This is that somebody.
//!
//! # The order is not arbitrary
//!
//! The home is opened first, then the store's exclusive lock. Only the process holding that lock may interpret durable
//! child identities as crash leftovers. Containment recovers those exact process groups next, before provider files
//! are loaded and before anything can start a new child.
//!
//! # What composing does not do
//!
//! It does not probe. Measured, a cold start of one of these CLIs costs 300 to 900 ms before it prints anything, so
//! probing every provider here would put a second of nothing in front of the operator's first list. The probe happens
//! when something needs the answer, and its answer is remembered against the binary's own identity.

use std::sync::Arc;

use runtrol_childproc::Containment;
use runtrol_core::registry::{KindEntry, KindTable, ProviderRegistry};
use runtrol_core::{HomeError, RuntrolHome};
use runtrol_drivers::{Builtin, DriverKind};
use runtrol_ledger::Ledger;
use runtrol_provider::WallMs;
use runtrol_security::{DeviceId, DeviceLabels, DeviceScope, GrantLedger};
use runtrol_store::{DeviceRow, Store};
use runtrol_transport::{CredentialFingerprint, PublicKey, StaticKeypair};
use tokio::sync::Mutex;

/// The daemon could not be assembled.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ComposeError {
    /// Children could not be made to die with this process.
    ///
    /// Not worked around and not downgraded. Starting agents that cannot be contained is the outcome the containment
    /// design exists to prevent, so the daemon refuses to start rather than running with the guarantee quietly absent.
    #[error("cannot contain the agents this daemon would start: {0}")]
    Containment(#[from] runtrol_childproc::SpawnError),

    /// runtrol's own directory could not be established.
    #[error(transparent)]
    Home(#[from] HomeError),

    /// The session-pointer store could not be opened or trusted.
    #[error(transparent)]
    Store(#[from] runtrol_store::StoreError),

    /// The separate Mission evidence and recovery ledger could not be opened or trusted.
    #[error(transparent)]
    Ledger(#[from] runtrol_ledger::LedgerError),

    /// The local fixed Mission Gate registry could not be restored safely.
    #[error("cannot restore the Mission Gate registry: {0}")]
    MissionGates(String),

    /// The local exact-digest capability trust index could not be restored safely.
    #[error("cannot restore the capability trust index: {0}")]
    CapabilityTrust(String),

    /// The per-user operating-system vault could not protect or restore the machine identity.
    #[error(transparent)]
    Vault(#[from] runtrol_vault::VaultError),

    /// Stored private key material could not reconstruct the configured Noise identity.
    #[error(transparent)]
    Crypto(#[from] runtrol_transport::CryptoError),

    /// A stored key is not a locally minted device identifier.
    #[error("a stored device identifier is not a locally minted UUIDv7")]
    StoredDeviceId,

    /// Stored device authority could not be reconstructed exactly.
    ///
    /// The untrusted stored value is not echoed. Starting with a silently shortened or relabelled grant would make
    /// the enforced authority differ from what the operator approved.
    #[error("stored device {device} cannot be restored: {why}")]
    StoredDevice {
        /// The locally minted device identifier whose row was refused.
        device: DeviceId,
        /// The stable validation rule that failed.
        why: &'static str,
    },
}

/// One paired device restored before a remote listener can exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedDevice {
    /// The locally minted identity used by the scope ledger.
    pub id: DeviceId,
    /// The authenticated Noise peer pinned during pairing.
    pub remote_static_key: PublicKey,
    /// The non-secret image used to authenticate its bearer credential.
    pub credential_fingerprint: CredentialFingerprint,
    /// Validated labels safe for local display.
    pub labels: DeviceLabels,
    /// When exact PC presence approved the pairing.
    pub paired_at: WallMs,
}

/// Everything a running daemon holds.
pub struct Composed {
    /// runtrol's own directory, and every path inside it.
    pub home: RuntrolHome,
    /// The minimal session-pointer database. It has no type capable of holding conversation content.
    pub store: Store,
    /// Bounded metadata-only Mission evidence and recovery state.
    pub ledger: Ledger,
    /// Local Mission validation, scheduler, and `GateDefinition` registry state.
    pub(crate) missions: Mutex<crate::mission::MissionController>,
    /// Local project capability candidate and exact-digest trust state.
    pub(crate) growth: Mutex<crate::growth::GrowthController>,
    /// Local-only pending approval challenges for public Runtime integrations.
    pub(crate) integration_admin: crate::integration_admin::IntegrationAdmin,
    /// The guarantee that children die with this process.
    ///
    /// Shared, because every driver hands it to every child it starts, and held for the process lifetime because
    /// dropping it is the kill.
    pub containment: Arc<Containment>,
    /// Which providers exist, and what this build can do about each.
    pub registry: ProviderRegistry,
    /// Who has been granted what, reconstructed from durable rows before any remote listener exists.
    pub granted: GrantLedger,
    /// Paired Noise and HTTP identities, reconstructed beside the grant ledger from the same durable rows.
    pub paired_devices: Vec<PairedDevice>,
    /// The stable PC Noise identity, protected by the current user's operating-system vault.
    ///
    /// Windows has the production DPAPI implementation. Other platforms remain `None` until their native user
    /// protector is implemented, and no raw-key file is used as a fallback.
    pub pc_identity: Option<Arc<StaticKeypair>>,
    /// The table that turns a kind into a driver.
    ///
    /// Kept because building a driver is deferred: it needs a resolved program, which needs a probe, which happens when
    /// something asks rather than at boot.
    pub kinds: &'static [DriverKind],
}

impl Composed {
    /// Assemble a daemon.
    ///
    /// `home` is the operator's own choice when they made one, and the platform's directory otherwise.
    ///
    /// # Errors
    ///
    /// [`ComposeError::Containment`] when children cannot be made to die with this process, [`ComposeError::Home`] when
    /// runtrol's directory cannot be established. Both stop the start: a daemon that cannot contain its agents or
    /// cannot find its own files is worse than no daemon.
    pub fn assemble(home: Option<&str>, builtin: Builtin) -> Result<Self, ComposeError> {
        let home = match home {
            Some(chosen) => RuntrolHome::open_at(chosen)?,
            None => RuntrolHome::open()?,
        };

        let store = Store::open(home.paths().database())?;
        let ledger = Ledger::open(home.paths().mission_ledger())?;
        let containment = Arc::new(Containment::establish_tracked(
            home.paths().process_guards().as_std_path(),
        )?);
        let registry = load(&home, builtin);
        let mut missions =
            crate::mission::MissionController::open(home.paths().mission_gates().clone())
                .map_err(ComposeError::MissionGates)?;
        let runtime_ids: Vec<Box<str>> = registry
            .usable()
            .map(|provider| provider.id().as_str().into())
            .collect();
        let mut growth =
            crate::growth::GrowthController::open(home.paths().capability_trust().clone())
                .map_err(ComposeError::CapabilityTrust)?;
        missions
            .recover(&ledger, &runtime_ids, &mut growth)
            .map_err(ComposeError::MissionGates)?;
        let (granted, paired_devices) = restore_device_authority(&store)?;
        let pc_identity = load_machine_identity(&home)?;
        Ok(Self {
            home,
            store,
            ledger,
            missions: Mutex::new(missions),
            growth: Mutex::new(growth),
            integration_admin: crate::integration_admin::IntegrationAdmin::default(),
            containment,
            registry,
            granted,
            paired_devices,
            pc_identity,
            kinds: builtin.kinds,
        })
    }

    /// Assemble everything except the containment.
    ///
    /// The containment cannot be established in a test: on one platform it puts the calling process into the group it
    /// is about to kill, which terminates the runner. Measured, and the reason the guarantee is proven by an integration
    /// test with a process it is allowed to kill.
    ///
    /// So this exists, and what it hands back is honest about what it is: a containment that holds nothing, which
    /// reports the weaker promise and refuses to claim a kill it did not perform. Everything else composing does is the
    /// same code.
    ///
    /// # Errors
    ///
    /// [`ComposeError::Home`] when runtrol's directory cannot be established.
    #[cfg(test)]
    pub(crate) fn for_tests(home: &str, builtin: Builtin) -> Result<Self, ComposeError> {
        let home = RuntrolHome::open_at(home)?;
        let registry = load(&home, builtin);
        let store = Store::open(home.paths().database())?;
        let ledger = Ledger::open(home.paths().mission_ledger())?;
        let mut missions =
            crate::mission::MissionController::open(home.paths().mission_gates().clone())
                .map_err(ComposeError::MissionGates)?;
        let runtime_ids: Vec<Box<str>> = registry
            .usable()
            .map(|provider| provider.id().as_str().into())
            .collect();
        let mut growth =
            crate::growth::GrowthController::open(home.paths().capability_trust().clone())
                .map_err(ComposeError::CapabilityTrust)?;
        missions
            .recover(&ledger, &runtime_ids, &mut growth)
            .map_err(ComposeError::MissionGates)?;
        let (granted, paired_devices) = restore_device_authority(&store)?;
        let pc_identity = load_machine_identity(&home)?;
        Ok(Self {
            home,
            store,
            ledger,
            missions: Mutex::new(missions),
            growth: Mutex::new(growth),
            integration_admin: crate::integration_admin::IntegrationAdmin::default(),
            containment: Arc::new(Containment::without_any()),
            registry,
            granted,
            paired_devices,
            pc_identity,
            kinds: builtin.kinds,
        })
    }

    /// What this build can do about a kind, by the name a manifest spells.
    #[must_use]
    pub fn driver_for(&self, kind: &str) -> Option<&'static DriverKind> {
        self.kinds.iter().find(|entry| entry.kind == kind)
    }
}

#[cfg(windows)]
fn load_machine_identity(home: &RuntrolHome) -> Result<Option<Arc<StaticKeypair>>, ComposeError> {
    let secret = runtrol_vault::MachineSecret::load_or_create(home.paths().machine_identity())?;
    let identity = StaticKeypair::from_private(secret.as_bytes())?;
    Ok(Some(Arc::new(identity)))
}

#[cfg(not(windows))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the platform implementations keep one fallible assembly signature so adding a native protector cannot change callers"
)]
fn load_machine_identity(_: &RuntrolHome) -> Result<Option<Arc<StaticKeypair>>, ComposeError> {
    Ok(None)
}

fn restore_device_authority(
    store: &Store,
) -> Result<(GrantLedger, Vec<PairedDevice>), ComposeError> {
    let listed = store.list_devices()?;
    if let Some((_, error)) = listed.unreadable.into_iter().next() {
        return Err(ComposeError::Store(error));
    }

    let mut grants = Vec::with_capacity(listed.devices.len());
    let mut devices = Vec::with_capacity(listed.devices.len());
    for (key, row) in listed.devices {
        let Some(device) = DeviceId::from_bytes(key.to_bytes()) else {
            return Err(ComposeError::StoredDeviceId);
        };
        let DeviceRow {
            remote_static_key,
            credential_fingerprint,
            name,
            platform,
            scopes: stored_scopes,
            paired_at,
        } = row;
        if remote_static_key.iter().all(|byte| *byte == 0) {
            return Err(ComposeError::StoredDevice {
                device,
                why: "the Noise public key is zero",
            });
        }
        let labels =
            DeviceLabels::new(&name, &platform).map_err(|_| ComposeError::StoredDevice {
                device,
                why: "the display labels are invalid",
            })?;
        let mut scopes = Vec::with_capacity(stored_scopes.len());
        for stored in stored_scopes {
            scopes.push(DeviceScope::from_stored(&stored).map_err(|_| {
                ComposeError::StoredDevice {
                    device,
                    why: "a scope is unknown to this build",
                }
            })?);
        }

        grants.push((device, scopes));
        devices.push(PairedDevice {
            id: device,
            remote_static_key: PublicKey::from_bytes(remote_static_key),
            credential_fingerprint: CredentialFingerprint::from_bytes(credential_fingerprint),
            labels,
            paired_at,
        });
    }

    Ok((GrantLedger::from_persisted(grants), devices))
}

/// Read the providers this machine declares.
///
/// Files only. The order is fixed so that whatever the operator wrote wins, and it is the loader's order rather than
/// this function's: all that happens here is naming the operator's directory as the last source.
fn load(home: &RuntrolHome, builtin: Builtin) -> ProviderRegistry {
    let kinds = KindTable::new(
        builtin
            .kinds
            .iter()
            .map(|entry| KindEntry {
                kind: entry.kind,
                // The kernel's table is data and this one carries a constructor, so the conversion is the constructor
                // being dropped. That asymmetry is the seam: the kernel decides whether a kind is served and never how.
                unavailable: entry.unavailable,
            })
            .collect::<Vec<_>>(),
    );

    ProviderRegistry::build(
        builtin.manifests,
        // The directory beside the executable is how a packaged build ships an extra provider. Absent here until there
        // is a packaged build to ship one, and naming it before then would be a path with no writer.
        None,
        Some(home.paths().providers()),
        &kinds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_device_row(name: &str, scopes: Vec<Box<str>>) -> DeviceRow {
        let key = runtrol_transport::StaticKeypair::generate()
            .expect("device key")
            .public_key();
        let token = runtrol_transport::AccessToken::parse(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("canonical token");
        DeviceRow {
            remote_static_key: key.to_bytes(),
            credential_fingerprint: token.fingerprint().to_bytes(),
            name: name.into(),
            platform: "Android".into(),
            scopes,
            paired_at: WallMs::from_millis(1_767_225_600_000),
        }
    }

    /// A directory of this test's own, removed when the test ends.
    struct Scratch {
        root: String,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("runtrol-compose-{name}"));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous run");
            }
            Self {
                root: root
                    .to_str()
                    .expect("the temporary path is UTF-8")
                    .to_owned(),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.root) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    /// The registry as composing builds it, without establishing containment.
    ///
    /// Containment cannot be established in a test: on one platform it puts the calling process into the group it is
    /// about to kill, which terminates the runner. Measured, and the reason the guarantee is proven by an integration
    /// test with a process it is allowed to kill. Everything else composing does is exercised here.
    fn registry_of(scratch: &Scratch) -> (RuntrolHome, ProviderRegistry) {
        let home = RuntrolHome::open_at(&scratch.root).expect("a fresh home opens");
        let registry = load(&home, runtrol_drivers::builtin());
        (home, registry)
    }

    #[test]
    fn a_fresh_machine_has_providers_with_no_file_anywhere() {
        // The manifests are compiled in, so a first run is not an empty list with instructions attached.
        let scratch = Scratch::make("fresh");
        let (_home, registry) = registry_of(&scratch);

        assert!(!registry.is_empty(), "a fresh install has providers");
        assert!(
            registry.rejected().is_empty(),
            "and none of the built-ins is refused: {:?}",
            registry.rejected()
        );
        assert_eq!(
            registry.usable().count(),
            registry.len(),
            "every built-in names a kind this build serves"
        );
    }

    #[test]
    fn the_operators_own_file_replaces_a_built_in() {
        // The whole reason the discovery order is fixed. An operator whose CLI moved must be able to fix it without
        // editing runtrol, and the proof is that a file with a built-in's id wins.
        let scratch = Scratch::make("shadow");
        let (home, before) = registry_of(&scratch);
        let existing = before
            .all()
            .next()
            .expect("at least one built-in")
            .manifest
            .clone();

        let path = home
            .paths()
            .providers()
            .join(&format!("{}.toml", existing.id))
            .expect("a valid file name");
        let mine = format!(
            "schema = 1\nid = \"{}\"\ndisplay_name = \"Mine\"\nkind = \"{}\"\n[bin]\nnames = [\"mine\"]\n",
            existing.id,
            existing.kind.as_str()
        );
        std::fs::write(path.as_std_path(), mine).expect("write the operator's file");

        let after = load(&home, runtrol_drivers::builtin());
        assert_eq!(after.len(), before.len(), "one id is still one provider");
        assert_eq!(
            after
                .get(existing.id)
                .map(|one| &*one.manifest.display_name),
            Some("Mine")
        );
        assert_eq!(after.shadowed().len(), 1, "and the shadowing is reported");
    }

    #[test]
    fn a_broken_file_the_operator_wrote_does_not_take_away_the_built_ins() {
        // A mistyped key in one file must not cost the operator every provider they have.
        let scratch = Scratch::make("broken");
        let (home, before) = registry_of(&scratch);

        let path = home
            .paths()
            .providers()
            .join("broken.toml")
            .expect("a valid file name");
        std::fs::write(path.as_std_path(), "this is not toml at all").expect("write it");

        let after = load(&home, runtrol_drivers::builtin());
        assert_eq!(after.len(), before.len(), "the built-ins still work");
        assert_eq!(after.rejected().len(), 1, "and the bad file is reported");
    }

    #[test]
    fn the_kernels_table_carries_which_kinds_are_served_and_never_how() {
        // The seam. The kernel decides whether a kind is served; the crate that ships drivers decides how. The
        // conversion is the constructor being dropped, and that is the whole difference between the two tables.
        let served = runtrol_drivers::builtin();
        let kinds = KindTable::new(
            served
                .kinds
                .iter()
                .map(|entry| KindEntry {
                    kind: entry.kind,
                    unavailable: entry.unavailable,
                })
                .collect::<Vec<_>>(),
        );

        assert_eq!(
            kinds.len(),
            served.kinds.len(),
            "no kind is lost crossing over"
        );
        let printed = format!("{kinds:?}");
        assert!(
            !printed.contains("make") && !printed.contains("fn"),
            "the kernel's table must carry no way to build anything: {printed}"
        );
    }

    #[test]
    fn a_kind_this_build_cannot_serve_is_still_listed_with_its_reason() {
        // An operator with a perfectly good manifest for a kind this build has no driver for should see it marked, not
        // wonder where it went.
        let served = runtrol_drivers::builtin();
        let unserved = served
            .kinds
            .iter()
            .find(|entry| entry.make.is_none())
            .expect("this build knows kinds it cannot serve");

        let scratch = Scratch::make("unserved");
        let (home, _) = registry_of(&scratch);
        let path = home
            .paths()
            .providers()
            .join("theirs.toml")
            .expect("a valid file name");
        std::fs::write(
            path.as_std_path(),
            format!(
                "schema = 1\nid = \"theirs\"\ndisplay_name = \"Theirs\"\nkind = \"{}\"\n[bin]\nnames = [\"theirs\"]\n",
                unserved.kind
            ),
        )
        .expect("write it");

        let registry = load(&home, served);
        let theirs = registry
            .get(runtrol_provider::ProviderId::parse("theirs").expect("valid"))
            .expect("it is listed");
        assert!(!theirs.is_usable());
        match theirs.kind {
            runtrol_core::KindStatus::Unavailable { why } => assert!(!why.is_empty()),
            ref other => panic!("expected a named unavailability, got {other:?}"),
        }
    }

    #[test]
    fn composing_restores_device_identity_credential_and_grants() {
        let scratch = Scratch::make("restore-device");
        let first =
            Composed::for_tests(&scratch.root, runtrol_drivers::builtin()).expect("first assembly");
        let device = DeviceId::now();
        let row = stored_device_row(
            "Pixel 9",
            vec!["session.list".into(), "session.list".into()],
        );
        let expected_fingerprint = row.credential_fingerprint;
        first
            .store
            .put_device(
                runtrol_store::DeviceKey::from_bytes(*device.as_bytes()),
                &row,
            )
            .expect("device stored");
        drop(first);

        let restored = Composed::for_tests(&scratch.root, runtrol_drivers::builtin())
            .expect("restored assembly");
        assert!(restored.granted.holds(device, DeviceScope::SessionList));
        assert_eq!(restored.granted.scopes_of(device).len(), 1);
        assert_eq!(restored.paired_devices.len(), 1);
        let paired = restored.paired_devices.first().expect("restored device");
        assert_eq!(paired.id, device);
        assert_eq!(paired.labels.name(), "Pixel 9");
        assert_eq!(
            paired.credential_fingerprint.to_bytes(),
            expected_fingerprint
        );
    }

    #[test]
    fn composing_refuses_unknown_stored_authority() {
        let scratch = Scratch::make("unknown-device-scope");
        let first =
            Composed::for_tests(&scratch.root, runtrol_drivers::builtin()).expect("first assembly");
        let device = DeviceId::now();
        first
            .store
            .put_device(
                runtrol_store::DeviceKey::from_bytes(*device.as_bytes()),
                &stored_device_row("phone", vec!["session.future".into()]),
            )
            .expect("device stored");
        drop(first);

        match Composed::for_tests(&scratch.root, runtrol_drivers::builtin()) {
            Err(ComposeError::StoredDevice {
                device: refused,
                why: "a scope is unknown to this build",
            }) => assert_eq!(refused, device),
            Err(other) => panic!("expected stored-scope refusal, got {other}"),
            Ok(_) => panic!("unknown authority must not reach a running daemon"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn composing_restores_the_same_dpapi_protected_pc_identity() {
        let scratch = Scratch::make("restore-pc-identity");
        let first =
            Composed::for_tests(&scratch.root, runtrol_drivers::builtin()).expect("first assembly");
        let public = first
            .pc_identity
            .as_ref()
            .expect("Windows identity")
            .public_key();
        assert!(
            first
                .home
                .paths()
                .machine_identity()
                .as_std_path()
                .is_file()
        );
        drop(first);

        let restored = Composed::for_tests(&scratch.root, runtrol_drivers::builtin())
            .expect("restored assembly");
        assert_eq!(
            restored
                .pc_identity
                .as_ref()
                .expect("restored Windows identity")
                .public_key(),
            public
        );
    }

    #[test]
    fn composing_starts_no_process() {
        // Measured: a cold start of one of these CLIs costs 300 to 900 ms before it prints anything. Probing every
        // provider here would put a second of nothing in front of the operator's first list.
        let scratch = Scratch::make("noprocess");
        let began = std::time::Instant::now();
        let (_home, registry) = registry_of(&scratch);
        let took = began.elapsed();

        assert!(!registry.is_empty());
        assert!(
            took < std::time::Duration::from_millis(250),
            "composing took {took:?}, which is long enough to be a process start"
        );
    }
}
