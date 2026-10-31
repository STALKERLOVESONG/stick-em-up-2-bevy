//! The game connection: TCP on [`crate::discovery::SERVER_PORT`] with length-prefixed frames,
//! polled without blocking once per rendered frame — or a [`Link`]: frames carried by another
//! transport (the peer-to-peer connections of `sticks_p2p`) through channels.
//!
//! The original used Unity's legacy networking (RakNet over UDP): reliable ordered RPCs plus
//! state streams. On a LAN a TCP stream with `TCP_NODELAY` gives the same ordering and
//! reliability; the state streams' "latest value wins" is kept by the game layer, which sends
//! the latest state every frame.
//!
//! The host is the hub (`Network.InitializeServer`, at most four connections); clients talk to
//! the host only, which relays (`RPCMode.Others` / `All` from a client goes through the server
//! in RakNet too). A host accepts LAN and peer-to-peer clients alike.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// `NetworkSetup.MAX_NETWORK_CONNECTIONS`.
pub const MAX_CONNECTIONS: usize = 4;
/// Largest frame accepted; anything bigger means a broken peer.
pub const MAX_FRAME: usize = 8 << 20;
/// How long a connection attempt may take before it fails.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The sending half of a [`Link`], implemented by the transport that carries it.
pub trait FrameSink: Send + Sync {
    /// Queue one frame (never blocks).
    fn send(&self, frame: Vec<u8>);
    /// Send what is queued, then close.
    fn close(&self);
}

/// A connection made by another transport: frames go out through `sink`, received frames
/// arrive on `incoming`; the connection is over once that channel's sender is gone.
pub struct Link {
    pub sink: Box<dyn FrameSink>,
    /// In a `Mutex` only so the link can live in shared (`Sync`) game state.
    pub incoming: Mutex<Receiver<Vec<u8>>>,
    /// Who is at the other end, for the log.
    pub label: String,
}

/// An in-memory [`FrameSink`]: frames go straight to the other link of [`Link::pair`].
struct ChannelSink(Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>);

impl FrameSink for ChannelSink {
    fn send(&self, frame: Vec<u8>) {
        if let Ok(tx) = self.0.lock() {
            if let Some(tx) = tx.as_ref() {
                let _ = tx.send(frame);
            }
        }
    }

    fn close(&self) {
        if let Ok(mut tx) = self.0.lock() {
            *tx = None;
        }
    }
}

impl Link {
    /// Two links joined in memory (for tests and local play).
    pub fn pair() -> (Link, Link) {
        let (a_tx, a_rx) = channel();
        let (b_tx, b_rx) = channel();
        let a = Link { sink: Box::new(ChannelSink(Mutex::new(Some(b_tx)))), incoming: Mutex::new(a_rx), label: "memory A".to_owned() };
        let b = Link { sink: Box::new(ChannelSink(Mutex::new(Some(a_tx)))), incoming: Mutex::new(b_rx), label: "memory B".to_owned() };
        (a, b)
    }
}

enum Stream {
    Tcp { stream: TcpStream, incoming: Vec<u8>, outgoing: Vec<u8> },
    Link(Link),
}

/// One framed connection, polled without blocking.
pub struct Connection {
    stream: Stream,
    closed: bool,
}

impl Connection {
    pub fn new(stream: TcpStream) -> io::Result<Connection> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(Connection { stream: Stream::Tcp { stream, incoming: Vec::new(), outgoing: Vec::new() }, closed: false })
    }

    pub fn from_link(link: Link) -> Connection {
        Connection { stream: Stream::Link(link), closed: false }
    }

    /// Who is at the other end, for the log.
    pub fn peer(&self) -> String {
        match &self.stream {
            Stream::Tcp { stream, .. } => stream.peer_addr().map_or_else(|_| "?".to_owned(), |a| a.to_string()),
            Stream::Link(link) => link.label.clone(),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Queue one message; [`Connection::flush`] writes it.
    pub fn send(&mut self, payload: &[u8]) {
        if self.closed {
            return;
        }
        match &mut self.stream {
            Stream::Tcp { outgoing, .. } => {
                outgoing.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                outgoing.extend_from_slice(payload);
            }
            Stream::Link(link) => link.sink.send(payload.to_vec()),
        }
    }

    /// Write as much as the socket takes now.
    pub fn flush(&mut self) {
        let Stream::Tcp { stream, outgoing, .. } = &mut self.stream else { return };
        while !self.closed && !outgoing.is_empty() {
            match stream.write(outgoing) {
                Ok(0) => self.closed = true,
                Ok(n) => {
                    outgoing.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => self.closed = true,
            }
        }
    }

    /// Every complete message received so far.
    pub fn receive(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        match &mut self.stream {
            Stream::Link(link) => {
                let Ok(rx) = link.incoming.lock() else {
                    self.closed = true;
                    return out;
                };
                loop {
                    match rx.try_recv() {
                        Ok(frame) => out.push(frame),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            self.closed = true;
                            break;
                        }
                    }
                }
            }
            Stream::Tcp { stream, incoming, .. } => {
                let mut buffer = [0u8; 16 * 1024];
                while !self.closed {
                    match stream.read(&mut buffer) {
                        Ok(0) => self.closed = true,
                        Ok(n) => incoming.extend_from_slice(&buffer[..n]),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => self.closed = true,
                    }
                }
                let mut at = 0;
                while incoming.len() - at >= 4 {
                    let n = u32::from_le_bytes(incoming[at..at + 4].try_into().unwrap_or_default()) as usize;
                    if n > MAX_FRAME {
                        self.closed = true;
                        break;
                    }
                    if incoming.len() - at - 4 < n {
                        break;
                    }
                    out.push(incoming[at + 4..at + 4 + n].to_vec());
                    at += 4 + n;
                }
                incoming.drain(..at);
            }
        }
        out
    }

    /// `Network.CloseConnection` / `Network.Disconnect`: send what is queued, then close.
    pub fn close(&mut self) {
        match &mut self.stream {
            Stream::Tcp { stream, outgoing, .. } => {
                // One blocking attempt to get the goodbye out.
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                let _ = stream.write_all(outgoing);
                outgoing.clear();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Stream::Link(link) => link.sink.close(),
        }
        self.closed = true;
    }
}

/// A client of the host (`NetworkPlayer`).
pub type PeerId = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// `OnPlayerConnected`.
    Connected(PeerId),
    Message(PeerId, Vec<u8>),
    /// `OnPlayerDisconnected`.
    Disconnected(PeerId),
}

/// `Network.InitializeServer(4, port)`.
pub struct Host {
    /// `None`: a host for links only (its LAN port was taken).
    listener: Option<TcpListener>,
    /// Connections other transports accepted (peer-to-peer clients).
    links: Option<Mutex<Receiver<Link>>>,
    peers: Vec<(PeerId, Connection)>,
    next_id: PeerId,
    /// `Network.maxConnections` (0 once a level loads: nobody joins a game in progress).
    pub max_connections: usize,
}

impl Host {
    pub fn start(port: u16) -> io::Result<Host> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        listener.set_nonblocking(true)?;
        Ok(Host { listener: Some(listener), links: None, peers: Vec::new(), next_id: 1, max_connections: MAX_CONNECTIONS })
    }

    /// A host that only takes the clients of [`Host::accept_links`].
    pub fn links_only() -> Host {
        Host { listener: None, links: None, peers: Vec::new(), next_id: 1, max_connections: MAX_CONNECTIONS }
    }

    /// Whether LAN clients can connect.
    pub fn on_lan(&self) -> bool {
        self.listener.is_some()
    }

    /// Also take the clients `links` delivers (they count against the same limit).
    pub fn accept_links(&mut self, links: Receiver<Link>) {
        self.links = Some(Mutex::new(links));
    }

    pub fn local_port(&self) -> Option<u16> {
        self.listener.as_ref()?.local_addr().ok().map(|a| a.port())
    }

    pub fn peers(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.peers.iter().map(|(id, _)| *id)
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// New connections, messages and losses since the last poll.
    pub fn poll(&mut self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        let mut arrived = Vec::new();
        while let Some(listener) = &self.listener {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Ok(connection) = Connection::new(stream) {
                        arrived.push(connection);
                    }
                }
                Err(_) => break,
            }
        }
        if let Some(links) = &self.links {
            if let Ok(rx) = links.lock() {
                arrived.extend(rx.try_iter().map(Connection::from_link));
            }
        }
        for mut connection in arrived {
            // Over the limit the connection is dropped at once (the client sees it fail, like
            // RakNet's "too many connections").
            if self.peers.len() >= self.max_connections {
                connection.close();
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            self.peers.push((id, connection));
            events.push(HostEvent::Connected(id));
        }
        for (id, connection) in &mut self.peers {
            for message in connection.receive() {
                events.push(HostEvent::Message(*id, message));
            }
        }
        self.peers.retain(|(id, connection)| {
            if connection.is_closed() {
                events.push(HostEvent::Disconnected(*id));
                false
            } else {
                true
            }
        });
        events
    }

    /// Who `peer` is, for the log.
    pub fn describe(&self, peer: PeerId) -> String {
        self.peers.iter().find(|(id, _)| *id == peer).map_or_else(String::new, |(_, c)| c.peer())
    }

    pub fn send(&mut self, to: PeerId, payload: &[u8]) {
        if let Some((_, c)) = self.peers.iter_mut().find(|(id, _)| *id == to) {
            c.send(payload);
        }
    }

    /// To every client but `except`.
    pub fn broadcast(&mut self, payload: &[u8], except: Option<PeerId>) {
        for (id, c) in &mut self.peers {
            if Some(*id) != except {
                c.send(payload);
            }
        }
    }

    pub fn flush(&mut self) {
        for (_, c) in &mut self.peers {
            c.flush();
        }
    }

    /// `Network.CloseConnection(player)`.
    pub fn kick(&mut self, peer: PeerId) {
        if let Some(i) = self.peers.iter().position(|(id, _)| *id == peer) {
            self.peers[i].1.close();
            self.peers.remove(i);
        }
    }

    /// `Network.Disconnect` on the server: every client is dropped.
    pub fn shut_down(mut self) {
        for (_, c) in &mut self.peers {
            c.close();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// `OnConnectedToServer`.
    Connected,
    /// `OnFailedToConnect`.
    Failed(String),
    Message(Vec<u8>),
    /// `OnDisconnectedFromServer`.
    Disconnected,
}

/// `Network.Connect`: the connection to a host, attempted on a helper thread so the game keeps
/// drawing (`NetworkPeerType.Connecting`).
pub struct Client {
    /// In a `Mutex` only so the client can live in shared (`Sync`) game state.
    pending: Option<Mutex<Receiver<io::Result<Connection>>>>,
    connection: Option<Connection>,
    reported_disconnect: bool,
}

impl Client {
    pub fn connect(address: SocketAddr) -> Client {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).and_then(Connection::new));
        });
        Client { pending: Some(Mutex::new(rx)), connection: None, reported_disconnect: false }
    }

    /// A connection another transport is making; `pending` delivers it (or why it failed).
    pub fn with_link(pending: Receiver<io::Result<Link>>) -> Client {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = pending.recv().unwrap_or_else(|_| Err(io::Error::other("connection attempt ended")));
            let _ = tx.send(result.map(Connection::from_link));
        });
        Client { pending: Some(Mutex::new(rx)), connection: None, reported_disconnect: false }
    }

    pub fn is_connecting(&self) -> bool {
        self.pending.is_some()
    }

    pub fn is_connected(&self) -> bool {
        self.connection.as_ref().is_some_and(|c| !c.is_closed())
    }

    pub fn poll(&mut self) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        if let Some(rx) = &self.pending {
            let received = match rx.lock() {
                Ok(rx) => rx.try_recv(),
                Err(_) => Err(TryRecvError::Disconnected),
            };
            match received {
                Ok(Ok(connection)) => {
                    self.pending = None;
                    self.connection = Some(connection);
                    events.push(ClientEvent::Connected);
                }
                Ok(Err(e)) => {
                    self.pending = None;
                    events.push(ClientEvent::Failed(e.to_string()));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    events.push(ClientEvent::Failed("connection attempt ended".to_owned()));
                }
            }
        }
        if let Some(c) = &mut self.connection {
            for message in c.receive() {
                events.push(ClientEvent::Message(message));
            }
            if c.is_closed() && !self.reported_disconnect {
                self.reported_disconnect = true;
                events.push(ClientEvent::Disconnected);
            }
        }
        events
    }

    pub fn send(&mut self, payload: &[u8]) {
        if let Some(c) = &mut self.connection {
            c.send(payload);
        }
    }

    pub fn flush(&mut self) {
        if let Some(c) = &mut self.connection {
            c.flush();
        }
    }

    pub fn disconnect(mut self) {
        if let Some(c) = &mut self.connection {
            c.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn host_and_client_exchange_frames_and_notice_disconnection() {
        let mut host = Host::start(0).unwrap();
        let port = host.local_port().unwrap();
        let mut client = Client::connect(SocketAddr::from(([127, 0, 0, 1], port)));
        wait_for(|| client.poll().contains(&ClientEvent::Connected).then_some(()));
        let peer = wait_for(|| host.poll().into_iter().find_map(|e| if let HostEvent::Connected(p) = e { Some(p) } else { None }));

        // Messages arrive whole and in order, also when large.
        let big = vec![7u8; 100_000];
        client.send(b"hello");
        client.send(&big);
        client.flush();
        let mut got = Vec::new();
        wait_for(|| {
            client.flush();
            for e in host.poll() {
                if let HostEvent::Message(p, m) = e {
                    assert_eq!(p, peer);
                    got.push(m);
                }
            }
            (got.len() == 2).then_some(())
        });
        assert_eq!(got[0], b"hello");
        assert_eq!(got[1], big);

        host.send(peer, b"welcome");
        host.flush();
        let reply = wait_for(|| client.poll().into_iter().find_map(|e| if let ClientEvent::Message(m) = e { Some(m) } else { None }));
        assert_eq!(reply, b"welcome");

        client.disconnect();
        wait_for(|| host.poll().contains(&HostEvent::Disconnected(peer)).then_some(()));
        assert_eq!(host.peer_count(), 0);
    }

    #[test]
    fn a_host_takes_clients_over_links_too() {
        let mut host = Host::start(0).unwrap();
        let (links_tx, links_rx) = channel();
        host.accept_links(links_rx);
        let (client_end, host_end) = Link::pair();
        let (pending_tx, pending_rx) = channel();
        let mut client = Client::with_link(pending_rx);
        pending_tx.send(Ok(client_end)).unwrap();
        links_tx.send(host_end).unwrap();
        wait_for(|| client.poll().contains(&ClientEvent::Connected).then_some(()));
        let peer = wait_for(|| host.poll().into_iter().find_map(|e| if let HostEvent::Connected(p) = e { Some(p) } else { None }));
        client.send(b"hi");
        let got = wait_for(|| host.poll().into_iter().find_map(|e| if let HostEvent::Message(p, m) = e { Some((p, m)) } else { None }));
        assert_eq!(got, (peer, b"hi".to_vec()));
        host.send(peer, b"yo");
        assert_eq!(wait_for(|| client.poll().into_iter().find_map(|e| if let ClientEvent::Message(m) = e { Some(m) } else { None })), b"yo");
        client.disconnect();
        wait_for(|| host.poll().contains(&HostEvent::Disconnected(peer)).then_some(()));

        // Over the limit a link is closed at once.
        host.max_connections = 0;
        let (client_end, host_end) = Link::pair();
        let (pending_tx, pending_rx) = channel();
        let mut client = Client::with_link(pending_rx);
        pending_tx.send(Ok(client_end)).unwrap();
        links_tx.send(host_end).unwrap();
        wait_for(|| {
            let _ = host.poll();
            client.poll().into_iter().find(|e| matches!(e, ClientEvent::Disconnected))
        });
        assert_eq!(host.peer_count(), 0);
    }

    #[test]
    fn a_full_host_refuses_more_clients() {
        let mut host = Host::start(0).unwrap();
        host.max_connections = 0;
        let port = host.local_port().unwrap();
        let mut client = Client::connect(SocketAddr::from(([127, 0, 0, 1], port)));
        // The TCP handshake completes, then the host drops it.
        wait_for(|| {
            let _ = host.poll();
            client.poll().into_iter().find(|e| matches!(e, ClientEvent::Disconnected | ClientEvent::Failed(_)))
        });
        assert_eq!(host.peer_count(), 0);
    }

    #[test]
    fn failing_to_connect_is_reported() {
        // Bind and drop to find a port nobody listens on.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut client = Client::connect(SocketAddr::from(([127, 0, 0, 1], port)));
        let failed = wait_for(|| client.poll().into_iter().find(|e| matches!(e, ClientEvent::Failed(_))));
        assert!(matches!(failed, ClientEvent::Failed(_)));
    }
}
