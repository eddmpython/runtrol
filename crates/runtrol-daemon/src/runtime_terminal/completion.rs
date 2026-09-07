//! Project Core completion facts into the public terminal contract.

pub(crate) const fn terminal_failure(
    failure: runtrol_core::terminal::TerminalFailure,
) -> runtrol_runtime_protocol::TerminalFailure {
    use runtrol_core::terminal::TerminalFailure as Core;
    use runtrol_runtime_protocol::TerminalFailure as Public;
    match failure {
        Core::HostInitializationFailed => Public::HostInitializationFailed,
        Core::OutputReadFailed => Public::OutputReadFailed,
        Core::ControlStateLost => Public::ControlStateLost,
        Core::InputDeliveryUnknown => Public::InputDeliveryUnknown,
    }
}
