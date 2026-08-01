//! Who holds what, and the two things that never need holding.
//!
//! The ledger is default-deny in the only way that counts: [`GrantLedger::holds`] answers `false` for
//! a device it has never seen. Runtime authority can only be added by [`GrantLedger::grant`], which requires
//! exact [`PcPresence`]. Startup may reconstruct the same approved rows through [`GrantLedger::from_persisted`];
//! that constructor is called before any remote listener exists and there is no method that can apply a persisted
//! row to a running ledger.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::SecurityError;
use crate::id::DeviceId;
use crate::presence::{GrantRequest, PcPresence};
use crate::scope::{DeviceScope, LocalScope};

/// Permission to do one local thing, once, now.
///
/// Returned by [`GrantLedger::authorize_local`] and never stored anywhere. A local scope is answered
/// per action at the machine, so the authorization is a value that is spent and dropped rather than a
/// row in a table.
///
/// Not `Clone`: two copies would be two authorizations from one typed phrase.
#[derive(Debug)]
pub struct LocalAuthorization {
    /// What was authorized.
    scope: LocalScope,
}

impl LocalAuthorization {
    /// What this authorizes.
    #[must_use]
    pub const fn scope(&self) -> LocalScope {
        self.scope
    }
}

/// What each paired device is allowed to do.
#[derive(Debug, Default)]
pub struct GrantLedger {
    /// Devices that hold at least one scope. A device with no scopes is not stored, so absence and
    /// emptiness are the same state and there is only one way to be unauthorized.
    granted: BTreeMap<DeviceId, BTreeSet<DeviceScope>>,
}

impl GrantLedger {
    /// An empty ledger. Every device is denied everything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconstruct grants previously written after exact PC presence.
    ///
    /// This is a constructor, not a mutating restore method. Assembly calls it before opening a remote listener,
    /// so a remote request cannot use persistence as a second grant path. Duplicate scopes collapse to the same
    /// set representation used by live grants.
    #[must_use]
    pub fn from_persisted(grants: impl IntoIterator<Item = (DeviceId, Vec<DeviceScope>)>) -> Self {
        let granted = grants
            .into_iter()
            .filter_map(|(device, scopes)| {
                let scopes: BTreeSet<DeviceScope> = scopes.into_iter().collect();
                if scopes.is_empty() {
                    None
                } else {
                    Some((device, scopes))
                }
            })
            .collect();
        Self { granted }
    }

    /// Give a device the scopes the operator approved.
    ///
    /// Both parameter types carry the design. `DeviceScope` means a [`LocalScope`] cannot be passed
    /// here at all, in any code, with no runtime check involved. `&PcPresence` means the scopes were
    /// read and approved by somebody at the machine, in this decision, moments ago.
    ///
    /// Grants exactly the set the witness names, and nothing else. Passing a witness earned for one
    /// set and asking for a different one is refused rather than intersected, because a partial grant
    /// is not what the operator agreed to and silently narrowing it would hide the mismatch.
    ///
    /// # Errors
    ///
    /// [`SecurityError::WitnessExpired`] for a stale witness, [`SecurityError::WitnessMismatch`] when
    /// the witness approved something else.
    ///
    /// A local-only capability cannot be put in the device grant at all:
    ///
    /// ```compile_fail
    /// use runtrol_security::{DeviceId, GrantLedger, LocalScope, PcPresence};
    ///
    /// fn attempt(ledger: &mut GrantLedger, device: DeviceId, witness: &PcPresence) {
    ///     ledger.grant(device, &[LocalScope::ConfigWrite], witness);
    /// }
    /// ```
    pub fn grant(
        &mut self,
        device: DeviceId,
        scopes: &[DeviceScope],
        witness: &PcPresence,
    ) -> Result<(), SecurityError> {
        let attempted = GrantRequest::DeviceScopes {
            device,
            scopes: scopes.to_vec(),
        };
        witness.check(&attempted)?;

        let held = self.granted.entry(device).or_default();
        for scope in scopes {
            held.insert(*scope);
        }
        Ok(())
    }

    /// Take scopes away from a device.
    ///
    /// No witness. A device may always shrink its own authority, and requiring presence to reduce
    /// permissions would mean a phone that is worried about itself has to wait for someone to walk to
    /// a keyboard.
    ///
    /// Returns how many scopes were actually removed, so an audit line can record a revoke that found
    /// nothing rather than implying it did something.
    pub fn revoke(&mut self, device: DeviceId, scopes: &[DeviceScope]) -> usize {
        let Some(held) = self.granted.get_mut(&device) else {
            return 0;
        };
        let removed = scopes.iter().filter(|scope| held.remove(scope)).count();
        if held.is_empty() {
            self.granted.remove(&device);
        }
        removed
    }

    /// Take everything away from a device.
    ///
    /// The unconditional self-shrink from the security posture. No witness, no scope required, and it
    /// works for a device the ledger has never heard of, because "I want to hold nothing" must never
    /// fail.
    pub fn revoke_all(&mut self, device: DeviceId) -> usize {
        self.granted.remove(&device).map_or(0, |held| held.len())
    }

    /// Whether a device holds a scope.
    ///
    /// `false` for a device that is not in the table, which is what makes this default-deny rather
    /// than default-deny by convention.
    #[must_use]
    pub fn holds(&self, device: DeviceId, scope: DeviceScope) -> bool {
        self.granted
            .get(&device)
            .is_some_and(|held| held.contains(&scope))
    }

    /// Confirm a device holds a scope, as an error rather than a boolean.
    ///
    /// The form a request handler wants: `?` at the top of the handler, and the refusal already
    /// carries which device asked for what.
    ///
    /// # Errors
    ///
    /// [`SecurityError::ScopeMissing`] when the device does not hold it.
    pub fn require(&self, device: DeviceId, scope: DeviceScope) -> Result<(), SecurityError> {
        if self.holds(device, scope) {
            return Ok(());
        }
        Err(SecurityError::ScopeMissing {
            device,
            scope: scope.name(),
        })
    }

    /// Everything a device holds, in a stable order.
    ///
    /// The phone shows this so the operator can see what they approved from either end. Empty for an
    /// unknown device.
    #[must_use]
    pub fn scopes_of(&self, device: DeviceId) -> Vec<DeviceScope> {
        self.granted
            .get(&device)
            .map(|held| held.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Every device with at least one scope.
    #[must_use]
    pub fn devices(&self) -> Vec<DeviceId> {
        self.granted.keys().copied().collect()
    }

    /// Authorize one local action, once, now.
    ///
    /// There is no `grant_local`, and this returns a value rather than writing a row. A local scope
    /// that could be stored against a device would be a device scope, and the wall between the two is
    /// the point.
    ///
    /// # Errors
    ///
    /// [`SecurityError::WitnessExpired`] for a stale witness, [`SecurityError::WitnessMismatch`] when
    /// the operator approved something else.
    pub fn authorize_local(
        &self,
        scope: LocalScope,
        witness: &PcPresence,
    ) -> Result<LocalAuthorization, SecurityError> {
        witness.check(&GrantRequest::Local(scope))?;
        Ok(LocalAuthorization { scope })
    }

    /// Kill every session, from anywhere, with no permission at all.
    ///
    /// Deliberately a method on the ledger that consults nothing. The security posture requires panic
    /// to work unconditionally, and there is no `DeviceScope::Panic` precisely because a scope that is
    /// always granted is not a scope: modelling it would suggest it could be withheld, and somebody
    /// would eventually write the check.
    ///
    /// Returns unit rather than a permission object, because there is nothing to prove.
    pub const fn panic_is_always_permitted(&self, _device: DeviceId) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh witness for `request`.
    ///
    /// Built directly rather than through a challenge. These tests are about the ledger, and routing
    /// them through the console would serialise every one of them on a process-wide singleton for no
    /// added coverage: the challenge path has its own tests next door.
    fn witness_for(request: GrantRequest) -> PcPresence {
        PcPresence::for_tests(request)
    }

    fn device_witness(device: DeviceId, scopes: &[DeviceScope]) -> PcPresence {
        witness_for(GrantRequest::DeviceScopes {
            device,
            scopes: scopes.to_vec(),
        })
    }

    #[test]
    fn an_unknown_device_holds_nothing() {
        let ledger = GrantLedger::new();
        let stranger = DeviceId::now();
        assert!(!ledger.holds(stranger, DeviceScope::SessionList));
        assert!(ledger.scopes_of(stranger).is_empty());
        assert!(matches!(
            ledger.require(stranger, DeviceScope::SessionList),
            Err(SecurityError::ScopeMissing { .. })
        ));
    }

    #[test]
    fn a_witness_grants_exactly_what_the_operator_read() {
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let approved = [DeviceScope::SessionList, DeviceScope::SessionOutputRead];
        let witness = device_witness(device, &approved);

        ledger.grant(device, &approved, &witness).expect("granted");
        assert!(ledger.holds(device, DeviceScope::SessionList));
        assert!(ledger.holds(device, DeviceScope::SessionOutputRead));
        assert!(
            !ledger.holds(device, DeviceScope::SessionInputWrite),
            "nothing outside the approved set"
        );
    }

    #[test]
    fn a_witness_cannot_be_spent_on_a_wider_set() {
        // The operator read two scopes. Handing the same witness back with a third is refused
        // outright rather than narrowed, because a silent narrowing hides the attempt.
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let approved = [DeviceScope::SessionList];
        let witness = device_witness(device, &approved);

        let wider = [DeviceScope::SessionList, DeviceScope::SessionDelete];
        assert!(matches!(
            ledger.grant(device, &wider, &witness),
            Err(SecurityError::WitnessMismatch { .. })
        ));
        assert!(
            !ledger.holds(device, DeviceScope::SessionList),
            "nothing was granted"
        );
    }

    #[test]
    fn a_witness_for_one_device_cannot_grant_to_another() {
        let mut ledger = GrantLedger::new();
        let approved_device = DeviceId::now();
        let other_device = DeviceId::now();
        let scopes = [DeviceScope::SessionList];
        let witness = device_witness(approved_device, &scopes);

        assert!(matches!(
            ledger.grant(other_device, &scopes, &witness),
            Err(SecurityError::WitnessMismatch { .. })
        ));
    }

    #[test]
    fn a_local_witness_cannot_grant_a_device_scope() {
        // The type wall stops `LocalScope` reaching `grant` at compile time. This covers the other
        // route: a witness earned for a local action being spent on a device grant.
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let witness = witness_for(GrantRequest::Local(LocalScope::ConfigWrite));
        assert!(matches!(
            ledger.grant(device, &[DeviceScope::SessionList], &witness),
            Err(SecurityError::WitnessMismatch { .. })
        ));
    }

    #[test]
    fn revoking_needs_no_witness() {
        // A device may always shrink itself, without waiting for anyone to reach a keyboard.
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let scopes = [DeviceScope::SessionList, DeviceScope::SessionOutputRead];
        let witness = device_witness(device, &scopes);
        ledger.grant(device, &scopes, &witness).expect("granted");

        assert_eq!(ledger.revoke(device, &[DeviceScope::SessionList]), 1);
        assert!(!ledger.holds(device, DeviceScope::SessionList));
        assert!(ledger.holds(device, DeviceScope::SessionOutputRead));
    }

    #[test]
    fn revoking_reports_what_it_actually_removed() {
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        assert_eq!(
            ledger.revoke(device, &[DeviceScope::SessionList]),
            0,
            "a revoke that found nothing must not claim it did something"
        );
        assert_eq!(ledger.revoke_all(device), 0);
    }

    #[test]
    fn a_device_with_nothing_left_is_forgotten() {
        // Absence and emptiness must be one state, so there is only one way to be unauthorized.
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let scopes = [DeviceScope::SessionList];
        let witness = device_witness(device, &scopes);
        ledger.grant(device, &scopes, &witness).expect("granted");
        assert_eq!(ledger.devices(), vec![device]);

        ledger.revoke(device, &scopes);
        assert!(ledger.devices().is_empty());
    }

    #[test]
    fn revoke_all_takes_everything() {
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let scopes = [
            DeviceScope::SessionList,
            DeviceScope::SessionOutputRead,
            DeviceScope::SessionInputWrite,
        ];
        let witness = device_witness(device, &scopes);
        ledger.grant(device, &scopes, &witness).expect("granted");
        assert_eq!(ledger.revoke_all(device), 3);
        assert!(ledger.scopes_of(device).is_empty());
    }

    #[test]
    fn a_local_action_is_authorized_and_never_stored() {
        let ledger = GrantLedger::new();
        let witness = witness_for(GrantRequest::Local(LocalScope::ModeDangerous));
        let authorization = ledger
            .authorize_local(LocalScope::ModeDangerous, &witness)
            .expect("the operator approved exactly this");
        assert_eq!(authorization.scope(), LocalScope::ModeDangerous);
        assert!(
            ledger.devices().is_empty(),
            "a local authorization must leave no trace in the device table"
        );
    }

    #[test]
    fn a_local_witness_is_bound_to_its_own_scope() {
        let ledger = GrantLedger::new();
        let witness = witness_for(GrantRequest::Local(LocalScope::ConfigWrite));
        assert!(matches!(
            ledger.authorize_local(LocalScope::ModeDangerous, &witness),
            Err(SecurityError::WitnessMismatch { .. })
        ));
    }

    #[test]
    fn panic_consults_nothing() {
        // An empty ledger, a device nobody has ever paired, and it still works. If this ever needs a
        // grant, the security posture has been broken.
        let ledger = GrantLedger::new();
        ledger.panic_is_always_permitted(DeviceId::now());
    }

    #[test]
    fn scopes_are_reported_in_a_stable_order() {
        // The phone renders this list. An order that shifted between calls would look like the
        // permissions were changing.
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let scopes = [
            DeviceScope::SessionOutputRead,
            DeviceScope::SessionList,
            DeviceScope::AuditRead,
        ];
        let witness = device_witness(device, &scopes);
        ledger.grant(device, &scopes, &witness).expect("granted");
        assert_eq!(ledger.scopes_of(device), ledger.scopes_of(device));
        assert_eq!(ledger.scopes_of(device).len(), 3);
    }

    #[test]
    fn persisted_grants_are_reconstructed_without_creating_an_empty_device() {
        let granted = DeviceId::now();
        let empty = DeviceId::now();
        let ledger = GrantLedger::from_persisted([
            (
                granted,
                vec![DeviceScope::SessionList, DeviceScope::SessionList],
            ),
            (empty, Vec::new()),
        ]);

        assert!(ledger.holds(granted, DeviceScope::SessionList));
        assert_eq!(ledger.scopes_of(granted).len(), 1);
        assert!(!ledger.devices().contains(&empty));
    }
}
