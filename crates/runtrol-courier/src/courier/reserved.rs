//! Initial mail shares the ordinary mailbox and byte budget, behind a closed delivery boundary.

use super::{CallEnvelope, CallKind, Courier, UnixMillis};
use crate::{Receipt, Refusal};

impl Courier {
    /// Hold one initial tell for a new recipient until `session_started` opens delivery.
    ///
    /// The sender must be live. The recipient must have no mailbox, including an earlier reservation.
    /// Ordinary sends and receives remain refused while it is reserved. Expiry and session cleanup use
    /// the same mailbox as every other message, so there is no second body owner or byte allowance.
    ///
    /// # Errors
    /// Refuses a non-tell, an existing recipient, or any ordinary envelope and capacity violation.
    pub fn reserve_initial(
        &mut self,
        envelope: CallEnvelope,
        now: UnixMillis,
    ) -> Result<Receipt, Refusal> {
        if envelope.kind != CallKind::Tell {
            return Err(Refusal::InitialMailKind);
        }
        let target = envelope.target;
        if self.mailboxes.contains_key(&target) {
            return Err(Refusal::RecipientAlreadyReserved(target));
        }
        self.session_started(target);
        match self.send(envelope, now) {
            Ok(receipt) => {
                if let Some(mailbox) = self.mailboxes.get_mut(&target) {
                    mailbox.ready = false;
                }
                Ok(receipt)
            }
            Err(error) => {
                self.session_ended(target);
                Err(error)
            }
        }
    }
}
