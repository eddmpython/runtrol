//! Integration authentication and live authority revalidation.

use std::sync::Arc;
use std::time::Duration;

use runtrol_runtime_protocol::{AppScope, RuntimeErrorKind, RuntimeMethod};

use crate::Composed;
use crate::runtime_auth::{
    AuthorizationFailure, AuthorizedIntegration, ClientContext, authenticate,
    authenticate_against_row, integration_key, refresh_against_row,
};
use crate::runtime_terminal::has_scopes;

use super::connection_state::{PublicAuthority, PublicState};

pub(super) fn authorized<'a>(
    state: &'a mut PublicState,
    composed: &Composed,
    needed: Option<AppScope>,
) -> Result<&'a AuthorizedIntegration, AuthorizationFailure> {
    match needed {
        Some(scope) => authorized_scopes(state, composed, std::slice::from_ref(&scope)),
        None => authorized_scopes(state, composed, &[]),
    }
}

pub(super) fn authorized_scopes<'a>(
    state: &'a mut PublicState,
    composed: &Composed,
    needed: &[AppScope],
) -> Result<&'a AuthorizedIntegration, AuthorizationFailure> {
    let PublicState::Ready { authority, .. } = state else {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::NotInitialized,
            message: "Runtime initialization is not complete",
        });
    };
    let PublicAuthority::Authorized(current) = authority else {
        return Err(AuthorizationFailure {
            kind: match authority {
                PublicAuthority::Pending(_) => RuntimeErrorKind::EnrollmentPending,
                PublicAuthority::Anonymous => RuntimeErrorKind::Unauthenticated,
                PublicAuthority::Authorized(_) => RuntimeErrorKind::Internal,
            },
            message: "local integration approval and authenticated reconnect are required",
        });
    };
    refresh_current_in_place(composed, current)?;
    if !has_scopes(&current.grant, needed) {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::ScopeDenied,
            message: "the integration grant lacks the required app scope",
        });
    }
    Ok(current)
}

/// How long a draining generation waits for its successor's authority relay before refusing a connection.
///
/// A new window's first request lands on a draining generation the moment the locator shows it, and the
/// successor's relay arrives on its own one-second cadence. On a busy machine the first relay can land after
/// the first request. Waiting a few relay rounds turns that race into a slightly slower first answer instead
/// of a refusal after every update.
const RELAY_PATIENCE_STEP: Duration = Duration::from_millis(400);
const RELAY_PATIENCE_TRIES: u32 = 6;

/// Authenticate, and while this generation is draining give the successor's relay a moment to arrive.
pub(super) async fn authenticate_with_relay_patience(
    composed: &Composed,
    context: &ClientContext,
    authentication: &runtrol_runtime_protocol::IntegrationAuthentication,
) -> Result<AuthorizedIntegration, AuthorizationFailure> {
    let mut attempt = 0;
    loop {
        match authenticate_current(composed, context, authentication) {
            Err(failure)
                if failure.kind == RuntimeErrorKind::RuntimeUnavailable
                    && composed.draining.load(std::sync::atomic::Ordering::Acquire)
                    && attempt < RELAY_PATIENCE_TRIES =>
            {
                attempt += 1;
                tokio::time::sleep(RELAY_PATIENCE_STEP).await;
            }
            answered => return answered,
        }
    }
}

fn authenticate_current(
    composed: &Composed,
    context: &ClientContext,
    authentication: &runtrol_runtime_protocol::IntegrationAuthentication,
) -> Result<AuthorizedIntegration, AuthorizationFailure> {
    if !composed.draining.load(std::sync::atomic::Ordering::Acquire) {
        return authenticate(&composed.integration_authority, context, authentication);
    }
    let key = integration_key(&authentication.integration_id)?;
    let row = composed
        .generation_authority
        .row(key)
        .map_err(relay_authorization_failure)?;
    authenticate_against_row(context, authentication, key, &row)
}

pub(crate) fn refresh_current(
    composed: &Composed,
    current: &AuthorizedIntegration,
) -> Result<AuthorizedIntegration, AuthorizationFailure> {
    let mut refreshed = current.clone();
    refresh_current_in_place(composed, &mut refreshed)?;
    Ok(refreshed)
}

/// Revalidate a live connection without cloning its scopes and roots when the generation is unchanged.
pub(super) fn refresh_current_in_place(
    composed: &Composed,
    current: &mut AuthorizedIntegration,
) -> Result<(), AuthorizationFailure> {
    let row = current_authority_row(composed, current)?;
    refresh_against_row(current, &row)
}

pub(super) fn current_authority_row(
    composed: &Composed,
    current: &AuthorizedIntegration,
) -> Result<Arc<runtrol_store::IntegrationRow>, AuthorizationFailure> {
    if !composed.draining.load(std::sync::atomic::Ordering::Acquire) {
        return match composed.integration_authority.row(current.key) {
            Some(row) => Ok(row),
            None if composed.integration_authority.was_revoked(current.key) => {
                Err(AuthorizationFailure {
                    kind: RuntimeErrorKind::IntegrationRevoked,
                    message: "the integration grant was revoked",
                })
            }
            None => Err(AuthorizationFailure {
                kind: RuntimeErrorKind::Unauthenticated,
                message: "the integration grant no longer exists",
            }),
        };
    }
    let row = composed
        .generation_authority
        .row(current.key)
        .map_err(relay_authorization_failure)?;
    Ok(Arc::new(row))
}

const fn relay_authorization_failure(
    failure: crate::generation_authority::RelayFailure,
) -> AuthorizationFailure {
    match failure {
        crate::generation_authority::RelayFailure::Missing => AuthorizationFailure {
            kind: RuntimeErrorKind::IntegrationRevoked,
            message: "the integration is not in the draining generation's frozen authority",
        },
        crate::generation_authority::RelayFailure::State
        | crate::generation_authority::RelayFailure::Unavailable => AuthorizationFailure {
            kind: RuntimeErrorKind::RuntimeUnavailable,
            message: "the successor authority relay is unavailable",
        },
    }
}
pub(super) fn required_scope(method: RuntimeMethod) -> Option<AppScope> {
    match method {
        RuntimeMethod::ProvidersList
        | RuntimeMethod::ProvidersWatch
        | RuntimeMethod::ProvidersUsage
        | RuntimeMethod::ProvidersGetCapabilities => Some(AppScope::ProviderRead),
        RuntimeMethod::ProvidersListModels => Some(AppScope::ModelRead),
        RuntimeMethod::ProvidersListNativeSessions
        | RuntimeMethod::ProvidersNativeActivity
        | RuntimeMethod::ProvidersFocusNative => Some(AppScope::SessionNativeDiscover),
        RuntimeMethod::SessionsList
        | RuntimeMethod::SessionsWatchIndex
        | RuntimeMethod::SessionsGet
        | RuntimeMethod::TerminalsList
        | RuntimeMethod::TerminalsWatchIndex
        | RuntimeMethod::WindowsRegister
        | RuntimeMethod::WindowsUpdate
        | RuntimeMethod::WindowsList
        | RuntimeMethod::WindowsWatchIndex
        | RuntimeMethod::WindowsMirrorOpen
        | RuntimeMethod::WindowsMirrorOutput
        | RuntimeMethod::WindowsMirrorEnd
        | RuntimeMethod::WindowsReveal
        | RuntimeMethod::WindowsWatchReveals => Some(AppScope::SessionList),
        RuntimeMethod::SessionsStart => Some(AppScope::SessionStart),
        RuntimeMethod::SessionsAdoptNative | RuntimeMethod::SessionsResume => {
            Some(AppScope::SessionResume)
        }
        RuntimeMethod::SessionsAcquireControl
        | RuntimeMethod::SessionsSubmitInput
        | RuntimeMethod::SessionsSubmitBlocks
        | RuntimeMethod::SessionsSetModel
        | RuntimeMethod::SessionsSetMode
        | RuntimeMethod::TerminalsAcquireControl
        | RuntimeMethod::TerminalsRenewControl
        | RuntimeMethod::TerminalsReleaseControl
        | RuntimeMethod::TerminalsWrite
        | RuntimeMethod::TerminalsResize
        | RuntimeMethod::TerminalsSetDialogue
        | RuntimeMethod::TerminalsStop => Some(AppScope::SessionInputWrite),
        RuntimeMethod::SessionsWatchEvents
        | RuntimeMethod::ApprovalsListPending
        | RuntimeMethod::TerminalsOpen
        | RuntimeMethod::TerminalsAttach
        | RuntimeMethod::TerminalsDetach => Some(AppScope::SessionOutputRead),
        RuntimeMethod::SessionsInterrupt | RuntimeMethod::SessionsCool => {
            Some(AppScope::SessionStop)
        }
        RuntimeMethod::SessionsForget
        | RuntimeMethod::SessionsDeleteNative
        | RuntimeMethod::SessionsArchiveNative => Some(AppScope::SessionDelete),
        RuntimeMethod::ApprovalsRespond
        | RuntimeMethod::SessionsRenewControl
        | RuntimeMethod::SessionsReleaseControl
        | RuntimeMethod::Initialize
        | RuntimeMethod::Initialized
        | RuntimeMethod::Challenge
        | RuntimeMethod::IntegrationsRequestEnrollment
        | RuntimeMethod::IntegrationsWatchEnrollment
        | RuntimeMethod::IntegrationsGetGrant
        | RuntimeMethod::IntegrationsRotateKey
        | RuntimeMethod::ProvidersChanged
        | RuntimeMethod::ProvidersWatchEnded
        | RuntimeMethod::ProvidersUsageChanged
        | RuntimeMethod::SessionsEvent
        | RuntimeMethod::SessionsLagged
        | RuntimeMethod::SessionsIndexChanged
        | RuntimeMethod::SessionsIndexEnded
        | RuntimeMethod::TerminalsIndexChanged
        | RuntimeMethod::TerminalsIndexEnded
        | RuntimeMethod::TerminalsOutput
        | RuntimeMethod::TerminalsLagged
        | RuntimeMethod::TerminalsExited
        | RuntimeMethod::WindowsIndexChanged
        | RuntimeMethod::WindowsIndexEnded
        | RuntimeMethod::WindowsRevealRequested
        | RuntimeMethod::WindowsRevealsEnded
        | RuntimeMethod::PanicStop => None,
    }
}
