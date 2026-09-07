//! A provider's structural turn boundary, bound to the process incarnation that could have emitted it.

use runtrol_provider::{ProcessIdentity, WallMs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeTurn {
    pub(crate) answering: bool,
    pub(crate) at: Option<WallMs>,
}

impl NativeTurn {
    pub(crate) fn state_for(self, owner: Option<ProcessIdentity>) -> Option<bool> {
        let born = owner.and_then(owner_started_at)?;
        (self.at? >= born).then_some(self.answering)
    }

    pub(crate) fn answers_for(self, owner: ProcessIdentity) -> bool {
        self.state_for(Some(owner)) == Some(true)
    }
}

/// Only Windows currently publishes an absolute birth stamp through the process identity contract.
pub(crate) fn owner_started_at(owner: ProcessIdentity) -> Option<WallMs> {
    #[cfg(windows)]
    {
        const FILETIME_TICKS_PER_MILLISECOND: u64 = 10_000;
        const WINDOWS_TO_UNIX_EPOCH_MS: u64 = 11_644_473_600_000;
        let millis = (owner.started() / FILETIME_TICKS_PER_MILLISECOND)
            .checked_sub(WINDOWS_TO_UNIX_EPOCH_MS)?;
        Some(WallMs::from_millis(millis))
    }
    #[cfg(not(windows))]
    {
        let _unsupported = owner;
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn missing_or_previous_incarnation_boundary_is_unknown_without_an_age_clock() {
        let owner = ProcessIdentity::new(7, (11_644_473_600_000 + 1000) * 10_000).unwrap();
        assert_eq!(
            NativeTurn {
                answering: true,
                at: None
            }
            .state_for(Some(owner)),
            None
        );
        assert_eq!(
            NativeTurn {
                answering: true,
                at: Some(WallMs::from_millis(999))
            }
            .state_for(Some(owner)),
            None
        );
        let current = NativeTurn {
            answering: true,
            at: Some(WallMs::from_millis(1000)),
        };
        assert_eq!(current.state_for(Some(owner)), Some(true));
        assert_eq!(current.state_for(None), None);
        assert_eq!(
            NativeTurn {
                answering: false,
                ..current
            }
            .state_for(Some(owner)),
            Some(false)
        );
    }
}
