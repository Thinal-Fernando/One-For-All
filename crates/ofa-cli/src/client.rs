//! Sends events to the OFA app's local API.
//!
//! A plain HTTP/1.1 request over a TCP socket is all this needs, and short
//! timeouts keep `ofa hook` from ever holding up an agent: if the app isn't
//! running, it gives up within a fraction of a second.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use ofa_protocol::{Decision, Event, PermissionAnswer, EVENTS_PATH, PERMISSION_PATH};

// Loopback connects in well under a millisecond when the app is listening.
// When it isn't, Windows retries the refused connection for about two
// seconds instead of failing at once, so cap the wait.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const IO_TIMEOUT: Duration = Duration::from_secs(1);

/// How long to wait for an answer from the island. A little longer than
/// the app's own limit, which then replies with no decision.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(56 * 60);

pub fn send(event: &Event) -> Result<(), String> {
    let (status, _) = post(EVENTS_PATH, event, IO_TIMEOUT)?;
    check(status)
}

/// Shows a permission prompt on the island and waits for your answer there.
/// `None` means it was answered somewhere else, so Claude Code should carry
/// on with its own prompt.
pub fn ask(event: &Event) -> Result<Option<Decision>, String> {
    let (status, body) = post(PERMISSION_PATH, event, ANSWER_TIMEOUT)?;
    check(status)?;
    let answer: PermissionAnswer =
        serde_json::from_slice(&body).map_err(|err| format!("the app sent a bad answer: {err}"))?;
    Ok(answer.decision)
}

fn check(status: u16) -> Result<(), String> {
    match status {
        200..=299 => Ok(()),
        401 => Err("the app refused the token".into()),
        other => Err(format!("the app answered {other}")),
    }
}

/// Posts an event as JSON and returns the response's status and body.
fn post(path: &str, event: &Event, read_timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    let token_path = ofa_protocol::token_path().ok_or("APPDATA is not set")?;
    let token = std::fs::read_to_string(&token_path).map_err(|err| {
        format!(
            "no token at {} (is OFA running?): {err}",
            token_path.display()
        )
    })?;
    let body = serde_json::to_vec(event).map_err(|err| err.to_string())?;

    let addr = ofa_protocol::api_addr();
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|err| format!("OFA isn't running ({err})"))?;
    stream
        .set_read_timeout(Some(read_timeout))
        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|err| err.to_string())?;

    let head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Authorization: Bearer {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        token.trim(),
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&body))
        .map_err(|err| err.to_string())?;

    // The app closes the connection after its reply, so read to the end.
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|err| err.to_string())?;
    parse_response(&response).ok_or_else(|| "the app sent a bad response".into())
}

/// Splits "HTTP/1.1 200 OK\r\n...headers...\r\n\r\nbody" into status and body.
fn parse_response(response: &[u8]) -> Option<(u16, Vec<u8>)> {
    let end_of_head = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&response[..end_of_head]).ok()?;
    let mut parts = head.lines().next()?.split(' ');
    parts.next().filter(|v| v.starts_with("HTTP/"))?;
    let status = parts.next()?.parse().ok()?;
    Some((status, response[end_of_head + 4..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_status_and_body() {
        let (status, body) = parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\n\r\n{\"decision\":\"allow\"}",
        )
        .unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"{\"decision\":\"allow\"}");
    }

    #[test]
    fn an_empty_reply_has_an_empty_body() {
        assert_eq!(
            parse_response(b"HTTP/1.1 204 No Content\r\n\r\n"),
            Some((204, Vec::new()))
        );
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(parse_response(b"garbage"), None);
        assert_eq!(parse_response(b""), None);
        assert_eq!(parse_response(b"HTTP/1.1 204"), None);
    }
}
