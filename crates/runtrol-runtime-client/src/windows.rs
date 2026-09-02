//! The window registry: a VS Code window registers itself and the terminals it observes; every reader lists or
//! watches the index.

use runtrol_runtime_protocol::{
    ListWindowsParams, RuntimeMethod, WatchWindowIndexParams, WatchWindowIndexResult,
    WindowIndexChangedNotification, WindowIndexEndedNotification, WindowIndexSnapshot,
    WindowMirrorEndParams, WindowMirrorOpenParams, WindowMirrorOpened, WindowMirrorOutputParams,
    WindowRegisterParams, WindowRegistration, WindowUpdateParams,
};

use crate::ClientError;
use crate::client::RuntimeClient;
use crate::terminal::{
    EmptyResult, decode_notification, decode_params, parse_method, require_subscription,
};

/// Window registry operations on one authenticated connection.
pub struct WindowClient<'runtime> {
    runtime: &'runtime mut RuntimeClient,
}

impl<'runtime> WindowClient<'runtime> {
    pub(crate) fn new(runtime: &'runtime mut RuntimeClient) -> Self {
        Self { runtime }
    }

    /// Register this connection's window. The registration lives as long as the connection.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure.
    pub async fn register(
        &mut self,
        params: &WindowRegisterParams,
    ) -> Result<WindowRegistration, ClientError> {
        self.runtime
            .call(RuntimeMethod::WindowsRegister, params)
            .await
    }

    /// Publish the terminals this connection's window observes now.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure, including a connection that registered no window.
    pub async fn update(&mut self, params: &WindowUpdateParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call(RuntimeMethod::WindowsUpdate, params)
            .await?;
        Ok(())
    }

    /// Open a mirror of a terminal this connection's window observes.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure, including a shell the transparent shim already brokers.
    pub async fn mirror_open(
        &mut self,
        params: &WindowMirrorOpenParams,
    ) -> Result<WindowMirrorOpened, ClientError> {
        self.runtime
            .call(RuntimeMethod::WindowsMirrorOpen, params)
            .await
    }

    /// Feed one chunk of the observed execution's raw output into its mirror.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure, including a mirror this connection does not feed.
    pub async fn mirror_output(
        &mut self,
        params: &WindowMirrorOutputParams,
    ) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call(RuntimeMethod::WindowsMirrorOutput, params)
            .await?;
        Ok(())
    }

    /// The observed execution ended, or this window stops mirroring it.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure, including a mirror this connection does not feed.
    pub async fn mirror_end(&mut self, params: &WindowMirrorEndParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call(RuntimeMethod::WindowsMirrorEnd, params)
            .await?;
        Ok(())
    }

    /// Every registered window.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure.
    pub async fn list(&mut self) -> Result<WindowIndexSnapshot, ClientError> {
        self.runtime
            .call(RuntimeMethod::WindowsList, &ListWindowsParams::default())
            .await
    }

    /// Convert this connection into one window-index subscription.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure.
    pub async fn watch_index(&mut self) -> Result<WindowIndexSubscription<'_>, ClientError> {
        let started: WatchWindowIndexResult = self
            .runtime
            .call(
                RuntimeMethod::WindowsWatchIndex,
                &WatchWindowIndexParams::default(),
            )
            .await?;
        Ok(WindowIndexSubscription {
            runtime: self.runtime,
            subscription_id: started.subscription_id.clone(),
            started,
        })
    }
}

/// A change to the window index, or its end.
#[derive(Debug, PartialEq, Eq)]
pub enum WindowIndexNotification {
    /// The whole index after a change.
    Changed(WindowIndexChangedNotification),
    /// The subscription ended.
    Ended(WindowIndexEndedNotification),
}

/// One dedicated window-index stream on this connection.
pub struct WindowIndexSubscription<'runtime> {
    runtime: &'runtime mut RuntimeClient,
    subscription_id: String,
    started: WatchWindowIndexResult,
}

impl WindowIndexSubscription<'_> {
    /// The initial snapshot and subscription identity.
    #[must_use]
    pub const fn started(&self) -> &WatchWindowIndexResult {
        &self.started
    }

    /// The next change or end.
    ///
    /// # Errors
    ///
    /// Transport failure or a notification outside the selected protocol revision.
    pub async fn next(&mut self) -> Result<WindowIndexNotification, ClientError> {
        let payload = self.runtime.connection.receive().await?;
        let notification = decode_notification(&payload, "window index")?;
        match parse_method(&notification, "window index")? {
            RuntimeMethod::WindowsIndexChanged => {
                let changed: WindowIndexChangedNotification =
                    decode_params(notification.params, "window index change")?;
                require_subscription(&self.subscription_id, &changed.subscription_id)?;
                Ok(WindowIndexNotification::Changed(changed))
            }
            RuntimeMethod::WindowsIndexEnded => {
                let ended: WindowIndexEndedNotification =
                    decode_params(notification.params, "window index end")?;
                require_subscription(&self.subscription_id, &ended.subscription_id)?;
                Ok(WindowIndexNotification::Ended(ended))
            }
            _ => Err(ClientError::Protocol(
                "the dedicated window index stream received a different method".to_owned(),
            )),
        }
    }
}
