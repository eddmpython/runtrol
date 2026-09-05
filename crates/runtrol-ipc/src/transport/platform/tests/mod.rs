//! Windows pipe authority against real local, restricted, distinct-logon, and loopback UNC clients.

use super::*;
use tokio::io::AsyncWriteExt as _;
use windows_sys::Win32::Security::{
    CreateRestrictedToken, ImpersonateLoggedOnUser, LOGON32_LOGON_NEW_CREDENTIALS,
    LOGON32_PROVIDER_WINNT50, LogonUserW, SID_AND_ATTRIBUTES, TOKEN_DUPLICATE,
};
use windows_sys::Win32::Storage::FileSystem::SECURITY_ANONYMOUS;

#[expect(
    unsafe_code,
    reason = "the test derives a token that removes only this logon's ability to grant object access"
)]
fn token_without_logon_access() -> OwnedHandle {
    let logon = process_logon().expect("the process exposes its logon SID");
    let mut original = core::ptr::null_mut();
    assert_ne!(
        // SAFETY: the current process is live, the output pointer is writable, and the checked handle is owned.
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &raw mut original,
            )
        },
        0
    );
    let original = OwnedHandle(original);
    let disabled = SID_AND_ATTRIBUTES {
        Sid: logon.sid(),
        Attributes: 0,
    };
    let mut restricted = core::ptr::null_mut();
    assert_ne!(
        // SAFETY: the SID belongs to the live logon buffer; zero counts pair with null pointers. The
        // resulting token keeps the user SID, marks the logon SID deny-only, and is closed by its owner.
        unsafe {
            CreateRestrictedToken(
                original.0,
                0,
                1,
                &raw const disabled,
                0,
                core::ptr::null(),
                0,
                core::ptr::null(),
                &raw mut restricted,
            )
        },
        0
    );
    OwnedHandle(restricted)
}

#[expect(
    unsafe_code,
    reason = "the test impersonates an owned test token only during two synchronous client opens"
)]
fn connect_with_token(
    token: &OwnedHandle,
    owner: &str,
    logon: &str,
) -> (
    std::io::Result<NamedPipeClient>,
    std::io::Result<NamedPipeClient>,
) {
    // SAFETY: the checked token is owned by this scope; the guard reverts this thread before return.
    assert_ne!(unsafe { ImpersonateLoggedOnUser(token.0) }, 0);
    let _revert = RevertGuard;
    (
        ClientOptions::new().open(owner),
        ClientOptions::new().open(logon),
    )
}

#[tokio::test]
async fn logon_dacl_refuses_a_same_user_token_that_cannot_grant_logon_access() {
    let owner_address = format!(r"\\.\pipe\runtrol-owner-restricted-{}", std::process::id());
    let logon_address = format!(r"\\.\pipe\runtrol-logon-restricted-{}", std::process::id());
    let _owner = super::super::Listener::bind_owner_only(&owner_address)
        .await
        .expect("owner binds");
    let _logon = super::super::Listener::bind_logon_only(&logon_address)
        .await
        .expect("logon binds");
    let token = token_without_logon_access();
    let (owner, logon) = connect_with_token(&token, &owner_address, &logon_address);
    assert!(
        owner.is_ok(),
        "the unchanged user SID still opens the owner-only pipe: {owner:?}"
    );
    let error = logon.expect_err("the logon SID must grant access independently of the user SID");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    drop(owner);
    // The restricted impersonation must have ended before any async work resumes.
    drop(
        ClientOptions::new()
            .open(&logon_address)
            .expect("the original logon opens the same pipe"),
    );
}

#[expect(
    unsafe_code,
    reason = "the test creates an isolated outbound-credential logon with synthetic strings and never uses it on a network"
)]
fn new_credentials_logon() -> OwnedHandle {
    let user: Vec<u16> = "runtrol-courier-probe\0".encode_utf16().collect();
    let domain: Vec<u16> = ".\0".encode_utf16().collect();
    let password: Vec<u16> = "unused-probe-password\0".encode_utf16().collect();
    let mut token = core::ptr::null_mut();
    // This logon type clones the current local token; the synthetic outbound credentials are never used.
    // https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-logonuserw
    // SAFETY: all input strings are NUL terminated and live until the call ends; the output handle is owned.
    let created = unsafe {
        LogonUserW(
            user.as_ptr(),
            domain.as_ptr(),
            password.as_ptr(),
            LOGON32_LOGON_NEW_CREDENTIALS,
            LOGON32_PROVIDER_WINNT50,
            &raw mut token,
        )
    };
    assert_ne!(
        created,
        0,
        "NEW_CREDENTIALS logon failed: {}",
        std::io::Error::last_os_error()
    );
    assert!(!token.is_null());
    OwnedHandle(token)
}

#[tokio::test]
async fn a_distinct_logon_is_refused_before_its_frame_even_with_the_same_user() {
    let token = new_credentials_logon();
    let original_user = process_user()
        .expect("current user")
        .sid_string()
        .expect("user SID");
    let new_user = OwnedTokenSid::read(token.0, TokenSid::User)
        .expect("new user")
        .sid_string()
        .expect("new user SID");
    assert!(
        original_user == new_user,
        "NEW_CREDENTIALS must preserve the local user SID"
    );
    let original_logon = process_logon()
        .expect("current logon")
        .sid_string()
        .expect("logon SID");
    let new_logon = OwnedTokenSid::read(token.0, TokenSid::Logon)
        .expect("new logon")
        .sid_string()
        .expect("new logon SID");
    assert!(
        original_logon != new_logon,
        "NEW_CREDENTIALS did not create a distinct local logon SID"
    );
    let owner_address = format!(r"\\.\pipe\runtrol-owner-new-logon-{}", std::process::id());
    let logon_address = format!(r"\\.\pipe\runtrol-logon-new-logon-{}", std::process::id());
    let mut owner_listener = super::super::Listener::bind_owner_only(&owner_address)
        .await
        .expect("owner binds");
    let mut logon_listener = super::super::Listener::bind_logon_only(&logon_address)
        .await
        .expect("logon binds");
    let (owner, logon) = connect_with_token(&token, &owner_address, &logon_address);
    let mut owner = owner.expect("the same user's other logon opens the owner-only pipe");
    owner
        .write_all(&[0, 0, 0, 0])
        .await
        .expect("owner frame writes");
    let mut accepted_owner = owner_listener.accept().await.expect("owner accepted");
    assert!(
        accepted_owner
            .recv_bounded(16)
            .await
            .expect("owner frame reads")
            .expect("owner frame")
            .is_empty()
    );
    match logon {
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
        Ok(mut logon) => {
            // NEW_CREDENTIALS may preserve an enabled original-logon group even though TokenLogonSid
            // names a different logon. The pipe's impersonated SID check must still refuse its first frame.
            logon
                .write_all(&[0, 0, 0, 0])
                .await
                .expect("logon frame writes");
            let mut accepted_logon = logon_listener
                .accept()
                .await
                .expect("DACL admitted a cloned group");
            assert!(
                accepted_logon.recv_bounded(16).await.is_err(),
                "a distinct impersonated logon must be refused"
            );
            assert!(
                accepted_logon
                    .recv_bounded(16)
                    .await
                    .expect("refused connection ends")
                    .is_none()
            );
        }
    }
}

#[expect(
    unsafe_code,
    reason = "the test passes its owned security descriptor to Tokio's raw pipe constructor for a remote-accepting control"
)]
fn remote_accepting_control(address: &str) -> NamedPipeServer {
    let mut security = SecurityDescriptor::current_owner().expect("owner descriptor");
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(core::mem::size_of::<SECURITY_ATTRIBUTES>())
            .expect("attribute size"),
        lpSecurityDescriptor: security.as_mut_ptr(),
        bInheritHandle: 0,
    };
    // SAFETY: attributes and its descriptor stay live through creation. The control uses the production
    // owner DACL and differs only by permitting remote clients; Tokio owns the resulting server handle.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(false)
            .create_with_security_attributes_raw(address, (&raw mut attributes).cast())
    }
    .expect("remote-accepting control binds")
}

#[tokio::test]
async fn loopback_unc_reaches_a_control_but_not_the_restricted_pipe() {
    let control_name = format!("runtrol-remote-control-{}", std::process::id());
    let restricted_name = format!("runtrol-remote-restricted-{}", std::process::id());
    let _control = remote_accepting_control(&format!(r"\\.\pipe\{control_name}"));
    let _restricted =
        super::super::Listener::bind_owner_only(&format!(r"\\.\pipe\{restricted_name}"))
            .await
            .expect("restricted pipe binds");
    let control = ClientOptions::new()
        .open(format!(r"\\localhost\pipe\{control_name}"))
        .expect("loopback SMB must reach the same-owner remote-accepting control");
    let refused = ClientOptions::new()
        .open(format!(r"\\localhost\pipe\{restricted_name}"))
        .expect_err("the shared restricted pipe constructor must reject the UNC client");
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
    drop(control);
}

#[tokio::test]
async fn an_anonymous_pipe_context_cannot_borrow_its_processes_logon() {
    let address = format!(r"\\.\pipe\runtrol-logon-anonymous-{}", std::process::id());
    let mut listener = super::super::Listener::bind_logon_only(&address)
        .await
        .expect("logon binds");
    let serving = tokio::spawn(async move {
        let mut connection = listener
            .accept()
            .await
            .expect("DACL accepts the process logon");
        assert!(
            connection.recv_bounded(16).await.is_err(),
            "the impersonated pipe context must be identifiable"
        );
        assert!(
            connection
                .recv_bounded(16)
                .await
                .expect("the refused connection is closed")
                .is_none()
        );
    });
    let mut client = ClientOptions::new()
        .security_qos_flags(SECURITY_ANONYMOUS)
        .open(&address)
        .expect("DACL allows this logon to connect");
    client
        .write_all(&[0, 0, 0, 0])
        .await
        .expect("an empty frame writes");
    serving.await.expect("the anonymous context is refused");
}
