//! Closed update transaction with an exact rollback inverse.

use semver::Version;

/// One external operation required by an update transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    /// Ask the confirmed channel to install this exact release.
    Install(Version),
    /// Independently verify ownership and local provider health for this release.
    Verify(Version),
}

/// Terminal result of a provider update transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateFinish {
    /// The target release installed and verified.
    Updated,
    /// The target failed and the exact starting release was restored and verified.
    RolledBack {
        /// Why the target could not be accepted.
        why: String,
    },
    /// Neither the target nor a verified exact rollback could be established.
    Failed {
        /// Closed failure reason assembled from bounded runtrol-owned messages.
        why: String,
    },
}

/// An invalid update transaction request.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransactionError {
    /// Updates move only to a strictly greater plain semantic release.
    #[error("provider update target must be a greater plain semantic release")]
    InvalidOrder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    InstallTarget,
    VerifyTarget,
    InstallRollback { target_failure: String },
    VerifyRollback { target_failure: String },
    Finished,
}

/// A deterministic install, verify, and exact rollback state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateTransaction {
    from: Version,
    target: Version,
    state: State,
}

impl UpdateTransaction {
    /// Begin an update whose only rollback is the exact installed release.
    ///
    /// # Errors
    ///
    /// [`TransactionError::InvalidOrder`] when either release is not plain semantic versioning or the target is not
    /// strictly greater than the installed release.
    pub fn begin(from: Version, target: Version) -> Result<Self, TransactionError> {
        if !from.pre.is_empty()
            || !from.build.is_empty()
            || !target.pre.is_empty()
            || !target.build.is_empty()
            || target <= from
        {
            return Err(TransactionError::InvalidOrder);
        }
        Ok(Self {
            from,
            target,
            state: State::InstallTarget,
        })
    }

    /// External operation required next, or `None` after a terminal result.
    #[must_use]
    pub fn action(&self) -> Option<UpdateAction> {
        match self.state {
            State::InstallTarget => Some(UpdateAction::Install(self.target.clone())),
            State::VerifyTarget => Some(UpdateAction::Verify(self.target.clone())),
            State::InstallRollback { .. } => Some(UpdateAction::Install(self.from.clone())),
            State::VerifyRollback { .. } => Some(UpdateAction::Verify(self.from.clone())),
            State::Finished => None,
        }
    }

    /// Record the bounded result of the current action and return a terminal result when the transaction ends.
    #[must_use]
    pub fn advance(&mut self, result: Result<(), String>) -> Option<UpdateFinish> {
        let state = core::mem::replace(&mut self.state, State::Finished);
        match (state, result) {
            (State::InstallTarget, Ok(())) => {
                self.state = State::VerifyTarget;
                None
            }
            (State::InstallTarget | State::VerifyTarget, Err(why)) => {
                self.state = State::InstallRollback {
                    target_failure: why,
                };
                None
            }
            (State::VerifyTarget, Ok(())) => Some(UpdateFinish::Updated),
            (State::InstallRollback { target_failure }, Ok(())) => {
                self.state = State::VerifyRollback { target_failure };
                None
            }
            (State::InstallRollback { target_failure }, Err(rollback_failure)) => {
                Some(UpdateFinish::Failed {
                    why: format!(
                        "{target_failure}; rollback installation failed: {rollback_failure}"
                    ),
                })
            }
            (State::VerifyRollback { target_failure }, Ok(())) => Some(UpdateFinish::RolledBack {
                why: target_failure,
            }),
            (State::VerifyRollback { target_failure }, Err(rollback_failure)) => {
                Some(UpdateFinish::Failed {
                    why: format!(
                        "{target_failure}; rollback verification failed: {rollback_failure}"
                    ),
                })
            }
            (State::Finished, _) => Some(UpdateFinish::Failed {
                why: "the provider update transaction was already finished".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(value: &str) -> Version {
        Version::parse(value).expect("fixture version is semantic")
    }

    #[test]
    fn target_success_needs_install_then_verification() {
        let mut transaction =
            UpdateTransaction::begin(version("1.0.0"), version("2.0.0")).expect("valid update");
        assert_eq!(
            transaction.action(),
            Some(UpdateAction::Install(version("2.0.0")))
        );
        assert_eq!(transaction.advance(Ok(())), None);
        assert_eq!(
            transaction.action(),
            Some(UpdateAction::Verify(version("2.0.0")))
        );
        assert_eq!(transaction.advance(Ok(())), Some(UpdateFinish::Updated));
        assert_eq!(transaction.action(), None);
    }

    #[test]
    fn target_failure_restores_and_verifies_the_exact_starting_release() {
        let mut transaction =
            UpdateTransaction::begin(version("1.5.0"), version("2.0.0")).expect("valid update");
        assert_eq!(transaction.advance(Ok(())), None);
        assert_eq!(transaction.advance(Err("broken target".to_owned())), None);
        assert_eq!(
            transaction.action(),
            Some(UpdateAction::Install(version("1.5.0")))
        );
        assert_eq!(transaction.advance(Ok(())), None);
        assert_eq!(
            transaction.action(),
            Some(UpdateAction::Verify(version("1.5.0")))
        );
        assert_eq!(
            transaction.advance(Ok(())),
            Some(UpdateFinish::RolledBack {
                why: "broken target".to_owned(),
            })
        );
    }

    #[test]
    fn downgrade_and_prerelease_transactions_are_refused() {
        assert_eq!(
            UpdateTransaction::begin(version("2.0.0"), version("1.0.0")),
            Err(TransactionError::InvalidOrder)
        );
        assert_eq!(
            UpdateTransaction::begin(version("1.0.0"), version("2.0.0-beta.1")),
            Err(TransactionError::InvalidOrder)
        );
    }
}
