//! Structural identity of one operating-system process incarnation.
//!
//! Process identifiers are recycled. Any authority tied only to a PID can therefore move to an unrelated process
//! after the original exits. This value keeps the PID beside the kernel start stamp observed for that exact
//! incarnation. The stamp is opaque and platform-native because only equality matters across one machine boot.

/// One exact operating-system process incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessIdentity {
    pid: u32,
    started: u64,
}

impl ProcessIdentity {
    /// Build an identity only from usable nonzero kernel facts.
    #[must_use]
    pub const fn new(pid: u32, started: u64) -> Option<Self> {
        if pid == 0 || started == 0 {
            return None;
        }
        Some(Self { pid, started })
    }

    /// Operating-system process identifier.
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }

    /// Opaque kernel start stamp in the platform's native unit.
    #[must_use]
    pub const fn started(self) -> u64 {
        self.started
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessIdentity;

    #[test]
    fn zero_kernel_facts_are_not_process_identities() {
        assert_eq!(ProcessIdentity::new(0, 1), None);
        assert_eq!(ProcessIdentity::new(1, 0), None);
        assert_eq!(
            ProcessIdentity::new(7, 11).map(|identity| (identity.pid(), identity.started())),
            Some((7, 11))
        );
    }
}
