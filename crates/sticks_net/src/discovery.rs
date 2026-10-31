//! LAN game discovery, as `ServerDiscovery` / `ServerBroadcaster` do it: a host sends a UDP
//! broadcast every second, `"SEU2\0<version>\0<port>\0<info>"` (UTF-8, `info` is the machine
//! name), to port 25791; clients listen on that port and list every host heard from within the
//! last 2.25 s.
//!
//! The packet format is the original's, so the two games see each other's hosts: the version
//! number tells them apart (see [`NETWORK_VERSION`]), and joining across them shows the
//! original's version-mismatch error instead of connecting.
//!
//! TODO(UNCERTAIN): the original binds one sender per IPv4 interface address (source port
//! 25751) so every LAN hears it; std cannot enumerate interfaces, so one socket sends to the
//! limited broadcast address and the OS picks the interface (its default route's).

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// `ServerBroadcaster.Start`: `ServerDiscovery.GameNetworkIdentifier`.
pub const IDENTIFIER: &str = "SEU2";
/// `ServerDiscovery.BroadcastPort` as `ServerBroadcaster` sets it.
pub const BROADCAST_PORT: u16 = 25791;
/// `NetworkSetup.SERVER_PORT`: the game connection.
pub const SERVER_PORT: u16 = 25790;
/// `ServerDiscovery.GameNetworkVersion`. The original is version 5 (Unity's RakNet networking);
/// this game speaks a different protocol on the same port, so it announces a higher version and
/// the original's rule applies across the two: "The host of that game is running a newer
/// version" one way, "an older version" the other.
pub const NETWORK_VERSION: u32 = 1005;
/// `Server.broadcastInterval`.
pub const BROADCAST_INTERVAL: Duration = Duration::from_secs(1);
/// `Client.GetServerList`: advertisements older than 2250 ms are dropped.
pub const LISTING_LIFETIME: Duration = Duration::from_millis(2250);

/// Where a game is reached.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GameAddress {
    /// A host on the local network: TCP to this end point.
    Lan(SocketAddr),
    /// A host anywhere, reached peer to peer by its public key (`sticks_p2p`; not in the
    /// original).
    Online(String),
}

impl std::fmt::Display for GameAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GameAddress::Lan(a) => a.fmt(f),
            GameAddress::Online(id) => write!(f, "{}…", id.get(..10).unwrap_or(id)),
        }
    }
}

/// A host heard on the LAN (`ServerDiscovery.Client.ServerListing`), or one an invitation
/// names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerListing {
    pub address: GameAddress,
    pub version: u32,
    pub info: Option<String>,
}

impl ServerListing {
    /// `ServerList.GetServerName`: the info up to its first NUL.
    pub fn name(&self) -> String {
        match &self.info {
            Some(info) => info.split('\0').next().unwrap_or_default().to_owned(),
            None => self.address.to_string(),
        }
    }

    pub fn is_online(&self) -> bool {
        matches!(self.address, GameAddress::Online(_))
    }
}

/// The announcement bytes (`Server.Broadcast`).
pub fn announcement(version: u32, port: u16, info: Option<&str>) -> Vec<u8> {
    let mut text = format!("{IDENTIFIER}\0{version}\0{port}");
    if let Some(info) = info {
        text.push('\0');
        text.push_str(info);
    }
    text.into_bytes()
}

/// `Client.InterpretData`: `\A(SEU2)\0(\d+)\0(\d+)(?:\0(.*))?\z`.
pub fn parse_announcement(bytes: &[u8], sender: IpAddr) -> Option<ServerListing> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut parts = text.splitn(4, '\0');
    if parts.next()? != IDENTIFIER {
        return None;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let version = parts.next().filter(|s| digits(s))?.parse::<u32>().ok()?;
    let port = parts.next().filter(|s| digits(s))?.parse::<u16>().ok()?;
    // `.*` without `Singleline` stops at a newline, and `\z` then fails.
    let info = match parts.next() {
        Some(info) if info.contains('\n') => return None,
        Some(info) => Some(info.to_owned()),
        None => None,
    };
    Some(ServerListing { address: GameAddress::Lan(SocketAddr::new(sender, port)), version, info })
}

/// `ServerDiscovery.Server`: the host's announcements.
#[derive(Default)]
pub struct Broadcaster {
    socket: Option<UdpSocket>,
    next: Option<Instant>,
}

impl Broadcaster {
    /// `Server.Broadcast`: at most once a second.
    pub fn broadcast(&mut self, now: Instant, port: u16, info: &str) {
        if self.next.is_some_and(|t| now < t) {
            return;
        }
        self.next = Some(now + BROADCAST_INTERVAL);
        if self.socket.is_none() {
            self.socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok().filter(|s| s.set_broadcast(true).is_ok());
        }
        if let Some(socket) = &self.socket {
            let packet = announcement(NETWORK_VERSION, port, Some(info));
            // A failed send (no network) is retried next second, like the original.
            let _ = socket.send_to(&packet, (Ipv4Addr::BROADCAST, BROADCAST_PORT));
        }
    }

    /// `Server.Stop`.
    pub fn stop(&mut self) {
        self.socket = None;
        self.next = None;
    }
}

/// `ServerDiscovery.Client`: listens while a screen that lists games is up.
#[derive(Default)]
pub struct Listener {
    socket: Option<UdpSocket>,
    heard: Vec<(ServerListing, Instant)>,
}

impl Listener {
    /// `Client.GetServerList`: read every waiting announcement, drop stale ones and return the
    /// rest in the order they were first heard.
    pub fn servers(&mut self, now: Instant) -> Vec<ServerListing> {
        if self.socket.is_none() {
            // Another program (or a second copy of the game) may hold the port; listing then
            // stays empty, as the original's would after its exception.
            self.socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, BROADCAST_PORT)).ok().filter(|s| s.set_nonblocking(true).is_ok());
        }
        if let Some(socket) = &self.socket {
            let mut buffer = [0u8; 2048];
            loop {
                match socket.recv_from(&mut buffer) {
                    Ok((n, from)) => {
                        if let Some(listing) = parse_announcement(&buffer[..n], from.ip()) {
                            match self.heard.iter_mut().find(|(l, _)| l.address == listing.address) {
                                Some(entry) => *entry = (listing, now),
                                None => self.heard.push((listing, now)),
                            }
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
        self.heard.retain(|(_, at)| now.duration_since(*at) < LISTING_LIFETIME);
        self.heard.iter().map(|(l, _)| l.clone()).collect()
    }

    /// `Client.Stop`.
    pub fn stop(&mut self) {
        self.socket = None;
    }
}

/// The original's text when the listing's version differs (`ServerList.ConnectToServerOrShowVersionError`).
pub fn version_mismatch_message(ours: u32, theirs: u32) -> Option<&'static str> {
    use std::cmp::Ordering::*;
    match ours.cmp(&theirs) {
        Equal => None,
        Less => Some("The host of that game is running a newer version of the game.\n\nPlease update yours to join."),
        Greater => Some("The host of that game is running an older version of the game.\n\nPlease ask them to update so you can join."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));

    #[test]
    fn announcements_parse_like_the_original_regex() {
        let packet = announcement(5, 25790, Some("DESKTOP-1"));
        assert_eq!(packet, b"SEU2\x005\x0025790\x00DESKTOP-1");
        let listing = parse_announcement(&packet, HOST).unwrap();
        assert_eq!(listing.address, GameAddress::Lan(SocketAddr::new(HOST, 25790)));
        assert_eq!(listing.version, 5);
        assert_eq!(listing.name(), "DESKTOP-1");
        // No info: the name is the end point.
        let bare = parse_announcement(b"SEU2\x001005\x0025790", HOST).unwrap();
        assert_eq!(bare.info, None);
        assert_eq!(bare.name(), "192.168.1.20:25790");
        // Info past a NUL is not part of the name.
        assert_eq!(parse_announcement(b"SEU2\x005\x001\x00A\x00B", HOST).unwrap().name(), "A");
        for bad in [&b"SEU3\x005\x0025790"[..], b"SEU2\x00x\x0025790", b"SEU2\x005\x0099999", b"SEU2\x005\x00", b"SEU2\x005\x001\x00a\nb"] {
            assert_eq!(parse_announcement(bad, HOST), None, "{bad:?}");
        }
    }

    #[test]
    fn versions_compare_with_the_original_messages() {
        assert_eq!(version_mismatch_message(5, 5), None);
        assert!(version_mismatch_message(NETWORK_VERSION, 5).unwrap().contains("older"));
        assert!(version_mismatch_message(5, NETWORK_VERSION).unwrap().contains("newer"));
    }
}
