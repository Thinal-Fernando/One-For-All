//! Types shared by the OFA app and the `ofa` helper, so the two can't drift apart.
//!
//! Milestone 1 only fixes where the local API listens; the event types arrive
//! in milestone 3.

use std::net::{Ipv4Addr, SocketAddr};

/// Default port for the local API. Picked from the dynamic range to stay clear
/// of common dev servers.
pub const API_PORT: u16 = 47_821;

/// Version of the wire protocol between `ofa.exe` and the app.
pub const PROTOCOL_VERSION: u32 = 1;

/// Address of the local API. It only ever binds to loopback, so nothing
/// outside this machine can reach it.
pub fn api_addr() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, API_PORT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_is_loopback_only() {
        let addr = api_addr();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.to_string(), "127.0.0.1:47821");
    }
}
