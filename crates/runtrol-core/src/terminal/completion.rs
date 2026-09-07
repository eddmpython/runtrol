//! One bounded completion record for the exact terminal generation.

/// A mechanical host failure, without process output, input, paths, or operating-system error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalFailure {
    /// The host could not initialize all terminal lanes after process birth.
    HostInitializationFailed,
    /// The output reader failed before normal end of stream.
    OutputReadFailed,
    /// The authoritative terminal control state could no longer be maintained.
    ControlStateLost,
    /// A submitted input write could not be confirmed in full.
    InputDeliveryUnknown,
}

/// The first host failure and the eventual process completion share one observation channel.
///
/// A failure alone never proves process exit. The exit code becomes available only after the exact
/// process lifetime and both raw-output and terminal-authority draining have completed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCompletion {
    /// Actual process code, absent while the host still owns any completion work.
    pub exit_code: Option<i32>,
    /// First structural host failure, if one occurred before completion.
    pub failure: Option<TerminalFailure>,
}

impl TerminalCompletion {
    pub(super) fn fail(&mut self, failure: TerminalFailure) {
        if self.exit_code.is_none() && self.failure.is_none() {
            self.failure = Some(failure);
        }
    }

    pub(super) fn complete(&mut self, code: i32) {
        if self.exit_code.is_none() {
            self.exit_code = Some(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_preserves_the_first_failure_and_the_actual_code() {
        let mut completion = TerminalCompletion::default();
        completion.fail(TerminalFailure::OutputReadFailed);
        assert_eq!(completion.exit_code, None);
        completion.fail(TerminalFailure::InputDeliveryUnknown);
        completion.complete(0);
        completion.fail(TerminalFailure::ControlStateLost);
        completion.complete(-1);
        assert_eq!(completion.exit_code, Some(0));
        assert_eq!(completion.failure, Some(TerminalFailure::OutputReadFailed));
    }
}
