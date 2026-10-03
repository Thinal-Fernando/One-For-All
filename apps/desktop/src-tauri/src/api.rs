//! The local API that `ofa.exe` posts events to.
//!
//! It listens on 127.0.0.1 only. Every event must carry the secret token from
//! `%APPDATA%\OFA\token`, which the app creates on first run. The Host header
//! is checked too, so a web page can't reach the API by pointing its own
//! domain name at 127.0.0.1.
//!
//! Each request is handled on its own short-lived thread, because a
//! permission request stays open until you answer it.

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use ofa_protocol::{
    Event, Health, PermissionAnswer, EVENTS_PATH, HEALTH_PATH, MAX_BODY_BYTES, PERMISSION_PATH,
    PROTOCOL_VERSION,
};
use tauri::{AppHandle, Manager};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::sessions::Sessions;

/// Bytes of randomness in a token. Hex-encoded, that's 64 characters.
const TOKEN_BYTES: usize = 32;

/// Starts the API on its own thread. Fails if the port is taken, most likely
/// by another copy of OFA.
pub fn start(app: &AppHandle) -> io::Result<()> {
    let path = ofa_protocol::token_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is not set"))?;
    let token: Arc<str> = load_or_create_token(&path)?.into();

    let server = Server::http(ofa_protocol::api_addr()).map_err(io::Error::other)?;
    let app = app.clone();
    thread::Builder::new().name("api".into()).spawn(move || {
        for request in server.incoming_requests() {
            let (app, token) = (app.clone(), token.clone());
            let handled = thread::Builder::new()
                .name("api-request".into())
                .spawn(move || handle(&app, &token, request));
            if let Err(err) = handled {
                eprintln!("api: could not start a request thread: {err}");
            }
        }
    })?;
    Ok(())
}

fn handle(app: &AppHandle, token: &str, mut request: Request) {
    let (status, json) = route(app, token, &mut request);
    let response = match json {
        Some(json) => Response::from_string(json)
            .with_status_code(status)
            .with_header(
                Header::from_bytes("Content-Type", "application/json")
                    .expect("static header is valid"),
            )
            .boxed(),
        None => Response::empty(status).boxed(),
    };
    if let Err(err) = request.respond(response) {
        eprintln!("api: could not answer: {err}");
    }
}

/// Reads the token, or writes a fresh one if there is none yet or the file
/// is damaged. Keeping it across restarts means `ofa.exe` never has to
/// re-read it mid-session.
fn load_or_create_token(path: &Path) -> io::Result<String> {
    if let Ok(existing) = fs::read_to_string(path) {
        let existing = existing.trim();
        if is_valid_token(existing) {
            return Ok(existing.to_owned());
        }
    }
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|err| io::Error::other(err.to_string()))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, &token)?;
    Ok(token)
}

fn is_valid_token(token: &str) -> bool {
    token.len() == TOKEN_BYTES * 2 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Decides the answer to one request: a status code and an optional JSON body.
fn route(app: &AppHandle, token: &str, request: &mut Request) -> (u16, Option<String>) {
    if !host_is_loopback(header(request, "Host")) {
        return (403, None);
    }

    match (request.method(), request.url()) {
        (Method::Get, HEALTH_PATH) => {
            let health = Health {
                app_version: env!("CARGO_PKG_VERSION").into(),
                protocol: PROTOCOL_VERSION,
            };
            (200, serde_json::to_string(&health).ok())
        }
        (Method::Post, EVENTS_PATH) => match read_event(request, token) {
            Ok(event) => {
                app.state::<Sessions>().apply(app, event);
                (204, None)
            }
            Err(status) => (status, None),
        },
        (Method::Post, PERMISSION_PATH) => match read_event(request, token) {
            Ok(event) => {
                let decision = app.state::<Sessions>().wait_for_answer(app, event);
                (
                    200,
                    serde_json::to_string(&PermissionAnswer { decision }).ok(),
                )
            }
            Err(status) => (status, None),
        },
        (_, HEALTH_PATH | EVENTS_PATH | PERMISSION_PATH) => (405, None),
        _ => (404, None),
    }
}

/// Checks the token and reads one event from the body, or gives the error
/// status to answer with.
fn read_event(request: &mut Request, token: &str) -> Result<Event, u16> {
    if !authorized(header(request, "Authorization"), token) {
        return Err(401);
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| 400u16)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(413);
    }
    serde_json::from_slice(&body).map_err(|_| 400)
}

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// Only names that can't be pointed elsewhere by a web page.
fn host_is_loopback(host: Option<&str>) -> bool {
    let port = ofa_protocol::API_PORT;
    host.is_some_and(|host| {
        host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
    })
}

/// Checks `Bearer <token>` without stopping at the first wrong character, so
/// response timing gives nothing away.
fn authorized(header: Option<&str>, token: &str) -> bool {
    let Some(given) = header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return false;
    };
    let (given, token) = (given.as_bytes(), token.as_bytes());
    given.len() == token.len()
        && given
            .iter()
            .zip(token)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn only_the_exact_bearer_token_is_accepted() {
        assert!(authorized(Some(&format!("Bearer {TOKEN}")), TOKEN));
        assert!(!authorized(None, TOKEN));
        assert!(!authorized(Some(TOKEN), TOKEN));
        assert!(!authorized(Some(&format!("bearer {TOKEN}")), TOKEN));
        assert!(!authorized(Some(&format!("Bearer {TOKEN}0")), TOKEN));
        assert!(!authorized(Some(&format!("Bearer {}", &TOKEN[1..])), TOKEN));
        let mut wrong = TOKEN.to_owned();
        wrong.replace_range(63.., "0");
        assert!(!authorized(Some(&format!("Bearer {wrong}")), TOKEN));
    }

    #[test]
    fn host_must_be_loopback_with_our_port() {
        assert!(host_is_loopback(Some("127.0.0.1:47821")));
        assert!(host_is_loopback(Some("localhost:47821")));
        assert!(!host_is_loopback(None));
        assert!(!host_is_loopback(Some("evil.example:47821")));
        assert!(!host_is_loopback(Some("127.0.0.1")));
        assert!(!host_is_loopback(Some("127.0.0.1:80")));
    }

    #[test]
    fn token_is_created_once_then_reused() {
        let dir = std::env::temp_dir().join(format!("ofa-token-test-{}", std::process::id()));
        let path = dir.join("OFA").join("token");
        let _ = fs::remove_dir_all(&dir);

        let first = load_or_create_token(&path).unwrap();
        assert!(is_valid_token(&first));
        assert_eq!(load_or_create_token(&path).unwrap(), first);

        // A damaged file is replaced rather than trusted.
        fs::write(&path, "not a token").unwrap();
        let replaced = load_or_create_token(&path).unwrap();
        assert!(is_valid_token(&replaced));
        assert_ne!(replaced, first);

        fs::remove_dir_all(&dir).unwrap();
    }
}
