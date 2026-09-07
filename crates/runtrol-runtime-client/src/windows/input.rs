//! One bounded, serial owner input connection. Text is returned only by an explicit claim.

use std::collections::VecDeque;

use runtrol_runtime_protocol::{
    JsonRpcId, JsonRpcRequest, JsonRpcResponse, MAX_TERMINAL_VIEW_QUEUE_CHUNKS, RuntimeMethod,
    WatchWindowInputResult, WindowClaimInputParams, WindowInputClaim, WindowInputEndedNotification,
    WindowInputOfferedNotification, WindowInputReceiptParams,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::terminal::{
    EmptyResult, decode_notification, decode_params, decode_response, parse_method,
    require_subscription,
};
use crate::{ClientError, RuntimeClient};

/// A body-free offer or the end of this exact owner receiver.
#[derive(Debug, PartialEq, Eq)]
pub enum WindowInputNotification {
    /// Claim this sequence once before invoking the owner's input API.
    Offered(WindowInputOfferedNotification),
    /// This owner registration can no longer receive input.
    Ended(WindowInputEndedNotification),
}

/// One receiver. Mutable access serializes next, claim and receipt; dropping it closes its connection.
pub struct WindowInputSubscription<'runtime> {
    runtime: &'runtime mut RuntimeClient,
    started: WatchWindowInputResult,
    pending: VecDeque<WindowInputNotification>,
    closed: bool,
}

impl<'runtime> WindowInputSubscription<'runtime> {
    pub(super) fn new(
        runtime: &'runtime mut RuntimeClient,
        started: WatchWindowInputResult,
    ) -> Result<Self, ClientError> {
        if !(1..=usize::from(MAX_TERMINAL_VIEW_QUEUE_CHUNKS)).contains(&started.max_pending_offers)
        {
            runtime.connection.close();
            return Err(ClientError::Protocol(
                "owner input queue negotiation is invalid".to_owned(),
            ));
        }
        Ok(Self {
            runtime,
            started,
            pending: VecDeque::new(),
            closed: false,
        })
    }

    /// Subscription identity and the Runtime's existing pending-offer bound.
    #[must_use]
    pub const fn started(&self) -> &WatchWindowInputResult {
        &self.started
    }

    /// Close the exact receiver immediately, including any cancelled partial read.
    pub fn close(&mut self) {
        self.closed = true;
        self.pending.clear();
        self.runtime.connection.close();
    }

    /// Receive the next offer or typed end without automatically claiming text.
    ///
    /// # Errors
    /// Transport or protocol failure. Cancelling this future makes the connection unusable.
    pub async fn next(&mut self) -> Result<WindowInputNotification, ClientError> {
        let operation = self.begin()?;
        let result = match operation.receiver.pending.pop_front() {
            Some(offer) => Ok(offer),
            None => match operation.receiver.runtime.connection.receive().await {
                Ok(payload) => operation.receiver.notification(&payload),
                Err(error) => Err(error),
            },
        };
        operation.finish(result)
    }

    /// Claim one exact offered sequence. A typed refusal is final for that claim and requires no receipt.
    ///
    /// # Errors
    /// Authority, execution, lease, transport or protocol failure. The SDK never retries a claim.
    pub async fn claim_input(&mut self, sequence: u64) -> Result<WindowInputClaim, ClientError> {
        let params = WindowClaimInputParams {
            subscription_id: self.started.subscription_id.clone(),
            sequence,
        };
        self.command(RuntimeMethod::WindowsClaimInput, &params)
            .await
    }

    /// Report the owner API's structural outcome for a claimed or locally refused offer.
    ///
    /// # Errors
    /// Missing delivery, invalid receipt, transport or protocol failure. Lost receipts are never replayed.
    pub async fn input_receipt(
        &mut self,
        params: &WindowInputReceiptParams,
    ) -> Result<(), ClientError> {
        require_subscription(&self.started.subscription_id, &params.subscription_id)?;
        let _: EmptyResult = self
            .command(RuntimeMethod::WindowsInputReceipt, params)
            .await?;
        Ok(())
    }

    fn begin(&mut self) -> Result<Operation<'_, 'runtime>, ClientError> {
        if self.closed {
            return Err(ClientError::Protocol(
                "owner input connection ended or an operation was cancelled".to_owned(),
            ));
        }
        Ok(Operation {
            receiver: self,
            completed: false,
        })
    }

    async fn command<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: RuntimeMethod,
        params: &P,
    ) -> Result<R, ClientError> {
        let operation = self.begin()?;
        let result = operation.receiver.exchange(method, params).await;
        operation.finish(result)
    }

    async fn exchange<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: RuntimeMethod,
        params: &P,
    ) -> Result<R, ClientError> {
        let id = JsonRpcId::Number(self.runtime.next_id);
        self.runtime.next_id = self.runtime.next_id.checked_add(1).ok_or_else(|| {
            ClientError::Protocol("owner input request identifiers exhausted".to_owned())
        })?;
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: id.clone(),
            method: method.to_string(),
            params: serde_json::to_value(params)
                .map_err(|error| ClientError::Protocol(error.to_string()))?,
        };
        let payload = serde_json::to_vec(&request)
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        self.runtime.connection.send(&payload).await?;
        loop {
            let payload = self.runtime.connection.receive().await?;
            if let Ok(response) = serde_json::from_slice::<JsonRpcResponse>(&payload) {
                return decode_response(response, &id);
            }
            let notification = self.notification(&payload)?;
            if matches!(notification, WindowInputNotification::Ended(_)) {
                return Err(ClientError::Protocol(
                    "owner input registration ended during its operation".to_owned(),
                ));
            }
            if self.pending.len() >= self.started.max_pending_offers {
                return Err(ClientError::Protocol(
                    "owner input offer queue exceeded its negotiated bound".to_owned(),
                ));
            }
            self.pending.push_back(notification);
        }
    }

    fn notification(&mut self, payload: &[u8]) -> Result<WindowInputNotification, ClientError> {
        let notification = decode_notification(payload, "owner input")?;
        match parse_method(&notification, "owner input")? {
            RuntimeMethod::WindowsInputOffered => {
                let offer: WindowInputOfferedNotification =
                    decode_params(notification.params, "owner input offer")?;
                require_subscription(&self.started.subscription_id, &offer.subscription_id)?;
                Ok(WindowInputNotification::Offered(offer))
            }
            RuntimeMethod::WindowsInputEnded => {
                let ended: WindowInputEndedNotification =
                    decode_params(notification.params, "owner input end")?;
                require_subscription(&self.started.subscription_id, &ended.subscription_id)?;
                self.close();
                Ok(WindowInputNotification::Ended(ended))
            }
            _ => Err(ClientError::Protocol(
                "owner input received a different method".to_owned(),
            )),
        }
    }
}

impl Drop for WindowInputSubscription<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

struct Operation<'borrow, 'runtime> {
    receiver: &'borrow mut WindowInputSubscription<'runtime>,
    completed: bool,
}

impl Operation<'_, '_> {
    fn finish<T>(mut self, result: Result<T, ClientError>) -> Result<T, ClientError> {
        self.completed = result.is_ok() || matches!(&result, Err(ClientError::Runtime(_)));
        result
    }
}

impl Drop for Operation<'_, '_> {
    fn drop(&mut self) {
        // Cancellation may interrupt either part of a frame. Close now, even if the caller retains the receiver.
        if !self.completed {
            self.receiver.close();
        }
    }
}
