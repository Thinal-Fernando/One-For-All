//! Sends events to the OFA app's local API.
//!
//! A plain HTTP/1.1 request over a TCP socket is all this needs, and short
//! timeouts keep `ofa hook` from ever holding up an agent: if the app isn't
//! running, it gives up within a fraction of a second.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use ofa_protocol::{Event, EVENTS_PATH};

// Loopback connects in well under a millisecond when the app is listening.
// When it isn't, Windows retries the refused connection for about two
// seconds instead of failing at once, so cap the wait.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const IO_TIMEOUT: Duration = Duration::from_secs(1);

pub fn send(event: &Event) -> Result<(), String> {
    let path = ofa_protocol::token_path().ok_or("APPDATA is not set")?;
    let token = std::fs::read_to_string(&path)
        .map_err(|err| format!("no token at {} (is OFA running?): {err}", path.display()))?;
    let body = serde_json::to_vec(event).map_err(|err| err.to_string())?;
    let status = post(EVENTS_PATH, token.trim(), &body)?;
    match status {
        200..=299 => Ok(()),
        401 => Err("the app refused the token".into()),
        other => Err(format!("the app answered {other}")),
    }
}

/// Posts JSON and returns the response's status code.
fn post(path: &str, token: &str, body: &[u8]) -> Result<u16, String> {
    let addr = ofa_protocol::api_addr();
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|err| format!("OFA isn't running ({err})"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|err| err.to_string())?;

    let head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|err| err.to_string())?;

    // Only the status line matters: "HTTP/1.1 204 No Content".
    let mut response = [0u8; 64];
    let read = stream.read(&mut response).map_err(|err| err.to_string())?;
    parse_status(&response[..read]).ok_or_else(|| "the app sent a bad response".into())
}

fn parse_status(response: &[u8]) -> Option<u16> {
    let line = std::str::from_utf8(response).ok()?.lines().next()?;
    let mut parts = line.split(' ');
    parts.next().filter(|v| v.starts_with("HTTP/"))?;
    parts.next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_status_code() {
        assert_eq!(parse_status(b"HTTP/1.1 204 No Content\r\n\r\n"), Some(204));
        assert_eq!(parse_status(b"HTTP/1.1 401 Unauthorized\r\n"), Some(401));
        assert_eq!(parse_status(b"garbage"), None);
        assert_eq!(parse_status(b""), None);
    }
}
