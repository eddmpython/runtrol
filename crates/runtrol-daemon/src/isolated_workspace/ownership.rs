//! Typed terminal ownership. These are private Core facts, never caller-supplied wire authority.

use runtrol_provider::{ProcessIdentity, TerminalId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ProcessStamp {
    pid: u32,
    started: u64,
}

impl From<ProcessIdentity> for ProcessStamp {
    fn from(value: ProcessIdentity) -> Self {
        Self {
            pid: value.pid(),
            started: value.started(),
        }
    }
}

impl ProcessStamp {
    pub(super) fn validate(self) -> Result<(), String> {
        ProcessIdentity::new(self.pid, self.started)
            .map(|_| ())
            .ok_or_else(|| "invalid process ownership".to_owned())
    }

    pub(crate) fn is_live(self) -> bool {
        runtrol_childproc::matches_process_start(self.pid, self.started)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct TerminalOwner {
    pub(crate) runtime: ProcessStamp,
    pub(crate) terminal: TerminalId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SpawnTicket {
    reservation: Uuid,
    pub(crate) lead: TerminalOwner,
    pub(crate) worker: TerminalOwner,
    pub(crate) activation: u64,
}

impl SpawnTicket {
    pub(crate) fn new(
        runtime: ProcessIdentity,
        lead: TerminalId,
        worker: TerminalId,
        activation: u64,
    ) -> Result<Self, String> {
        let runtime = ProcessStamp::from(runtime);
        let ticket = Self {
            reservation: Uuid::now_v7(),
            lead: TerminalOwner {
                runtime,
                terminal: lead,
            },
            worker: TerminalOwner {
                runtime,
                terminal: worker,
            },
            activation,
        };
        ticket.validate()?;
        Ok(ticket)
    }

    pub(crate) fn reservation_id(self) -> String {
        self.reservation.to_string()
    }

    pub(super) fn validate(self) -> Result<(), String> {
        self.lead.runtime.validate()?;
        self.worker.runtime.validate()?;
        if self.lead.runtime != self.worker.runtime
            || self.lead.terminal == self.worker.terminal
            || self.activation == 0
            || self.reservation.get_version_num() != 7
        {
            return Err("invalid terminal worktree ownership".to_owned());
        }
        Ok(())
    }
}

/// Minted only after Gate has retired this exact reservation or observed this exact worker exit.
/// It is neither deserializable nor exposed by the legacy private IPC release command.
pub(crate) struct EndedSpawn {
    pub(super) ticket: SpawnTicket,
}

impl EndedSpawn {
    pub(crate) const fn after_gate_retired(ticket: SpawnTicket) -> Self {
        Self { ticket }
    }
}
