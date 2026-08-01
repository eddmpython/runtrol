//! The phone-facing HTTP boundary, exercised as bytes over a real loopback socket.
//!
//! These are not router unit tests. Each case starts the production connection server, sends an HTTP/1.1
//! request over TCP, and observes the response bytes. That is the layer where duplicate headers, browser
//! metadata, upgrade requests, and CORS policy can otherwise disagree with a pure policy helper.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use runtrol_security::{Caller, DeviceId};
use runtrol_transport::{AccessToken, DeviceCredential, PhoneHttp, StatusCode, response};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

const ORIGIN: &str = "https://phone.runtrol.test";
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Seen {
    response: String,
    calls: usize,
    device: DeviceId,
}

async fn send(request: impl FnOnce(u16) -> String) -> Seen {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback listener binds");
    let port = listener.local_addr().expect("it has an address").port();
    let device = DeviceId::now();
    let token = AccessToken::parse(TOKEN).expect("the fixture token is strong and canonical");
    let server = PhoneHttp::loopback(port, [ORIGIN], [DeviceCredential::new(device, &token)])
        .expect("the policy is valid");
    let calls = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&calls);

    let serving = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("the request connects");
        server
            .serve_connection(stream, move |request| {
                let called = Arc::clone(&called);
                async move {
                    called.fetch_add(1, Ordering::SeqCst);
                    let admitted = request
                        .extensions()
                        .get::<Caller>()
                        .cloned()
                        .expect("the handler only receives an authenticated caller");
                    assert_eq!(admitted, Caller::Device { device });
                    response(StatusCode::OK, "inside")
                }
            })
            .await
            .expect("the HTTP connection is served");
    });

    let mut client = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the gate reaches the listener");
    client
        .write_all(request(port).as_bytes())
        .await
        .expect("the request is written");
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = client.read(&mut chunk).await.expect("the response is read");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if response_is_complete(&bytes) {
            break;
        }
    }
    drop(client);
    serving.await.expect("the server task finishes");

    Seen {
        response: String::from_utf8(bytes).expect("HTTP headers and this fixture body are UTF-8"),
        calls: calls.load(Ordering::SeqCst),
        device,
    }
}

fn response_is_complete(bytes: &[u8]) -> bool {
    let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers_end = headers_end + 4;
    let Ok(headers) = std::str::from_utf8(&bytes[..headers_end]) else {
        return false;
    };
    let length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.eq_ignore_ascii_case("content-length") {
            return None;
        }
        Some(
            value
                .trim()
                .parse::<usize>()
                .expect("the production server writes a numeric content length"),
        )
    });
    length.is_some_and(|length| bytes.len() >= headers_end + length)
}

fn request(port: u16, extra: &str) -> String {
    format!(
        "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {ORIGIN}\r\nSec-Fetch-Site: same-origin\r\nX-Runtrol-Proto: 1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 0\r\nConnection: close\r\n{extra}\r\n"
    )
}

fn status(response: &str) -> u16 {
    let value = response
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .expect("the response has an HTTP status field");
    value.parse().expect("the HTTP status is a number")
}

fn lower_headers(response: &str) -> String {
    response
        .split("\r\n\r\n")
        .next()
        .expect("the response has headers")
        .to_ascii_lowercase()
}

#[tokio::test]
async fn a_valid_request_reaches_the_handler_as_the_paired_device() {
    let seen = send(|port| request(port, "")).await;
    assert_eq!(status(&seen.response), 200, "{}", seen.response);
    assert_eq!(seen.calls, 1);
    assert!(seen.response.ends_with("inside"), "{}", seen.response);
    assert_ne!(seen.device.as_bytes(), &[0; 16]);

    let headers = lower_headers(&seen.response);
    assert!(
        headers.contains(&format!("access-control-allow-origin: {ORIGIN}")),
        "the configured origin is echoed exactly: {}",
        seen.response
    );
    assert!(headers.contains("vary: origin"), "{}", seen.response);
    assert!(!headers.contains("access-control-allow-origin: *"));
    assert!(!headers.contains("access-control-allow-credentials"));
    assert!(!headers.contains("set-cookie"));
}

#[tokio::test]
async fn a_rebound_host_is_refused_before_routing() {
    let seen = send(|port| {
        request(port, "").replace(&format!("Host: 127.0.0.1:{port}"), "Host: attacker.test")
    })
    .await;
    assert_eq!(status(&seen.response), 421, "{}", seen.response);
    assert_eq!(seen.calls, 0, "a rejected Host reached the route");
}

#[tokio::test]
async fn an_unknown_or_missing_origin_is_refused_before_routing() {
    for altered in [
        |port| request(port, "").replace(ORIGIN, "https://attacker.test"),
        |port| request(port, "").replace(&format!("Origin: {ORIGIN}\r\n"), ""),
    ] {
        let seen = send(altered).await;
        assert_eq!(status(&seen.response), 403, "{}", seen.response);
        assert_eq!(seen.calls, 0, "an untrusted origin reached the route");
    }
}

#[tokio::test]
async fn csrf_metadata_and_the_non_simple_protocol_header_are_mandatory() {
    for altered in [
        |port| {
            request(port, "").replace("Sec-Fetch-Site: same-origin", "Sec-Fetch-Site: cross-site")
        },
        |port| request(port, "").replace("X-Runtrol-Proto: 1\r\n", ""),
    ] {
        let seen = send(altered).await;
        assert_eq!(status(&seen.response), 403, "{}", seen.response);
        assert_eq!(seen.calls, 0, "a CSRF-shaped request reached the route");
    }
}

#[tokio::test]
async fn a_cookie_is_never_an_authenticator() {
    let seen = send(|port| {
        request(port, "Cookie: authorization=Bearer%20anything\r\n")
            .replace(&format!("Authorization: Bearer {TOKEN}\r\n"), "")
    })
    .await;
    assert_eq!(status(&seen.response), 401, "{}", seen.response);
    assert_eq!(
        seen.calls, 0,
        "ambient browser state authenticated a request"
    );

    let headers = lower_headers(&seen.response);
    assert!(!headers.contains("set-cookie"));
    assert!(!headers.contains("access-control-allow-credentials"));
}

#[tokio::test]
async fn cors_preflight_echoes_only_the_configured_origin() {
    let accepted = send(|port| {
        format!(
            "OPTIONS /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {ORIGIN}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization,x-runtrol-proto\r\nConnection: close\r\n\r\n"
        )
    })
    .await;
    assert_eq!(status(&accepted.response), 204, "{}", accepted.response);
    assert_eq!(accepted.calls, 0, "preflight reached the RPC route");
    let headers = lower_headers(&accepted.response);
    assert!(headers.contains(&format!("access-control-allow-origin: {ORIGIN}")));
    assert!(!headers.contains("access-control-allow-origin: *"));
    assert!(!headers.contains("access-control-allow-credentials"));

    let refused = send(|port| {
        format!(
            "OPTIONS /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://attacker.test\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization,x-runtrol-proto\r\nConnection: close\r\n\r\n"
        )
    })
    .await;
    assert_eq!(status(&refused.response), 403, "{}", refused.response);
    assert_eq!(refused.calls, 0);
}

#[tokio::test]
async fn a_websocket_upgrade_is_authenticated_before_the_handler_can_answer_101() {
    let seen = send(|port| {
        format!(
            "GET /stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {ORIGIN}\r\nSec-Fetch-Site: same-origin\r\nX-Runtrol-Proto: 1\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        )
    })
    .await;
    assert_eq!(status(&seen.response), 401, "{}", seen.response);
    assert_eq!(
        seen.calls, 0,
        "an unauthenticated upgrade reached the route"
    );
}
