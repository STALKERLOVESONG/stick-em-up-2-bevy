//! Internet play for Stick 'Em Up 2, peer to peer (not in the original, whose network games
//! were LAN only).
//!
//! Every running game has an iroh endpoint: a QUIC end point named by a public key. A host's key
//! is its game code; whoever has the code connects straight to the host's machine. iroh finds
//! the machine through n0's public address lookup, punches through both sides' NATs, and when no
//! direct path can be made carries the (end-to-end encrypted) packets through one of n0's public
//! relays. There is no game server: the host stays the hub of its game, as on a LAN.
//!
//! The game sees the connections as [`sticks_net::Link`]s, so [`sticks_net::Host`] and
//! [`sticks_net::Client`] treat them like their LAN connections. Over a connection one QUIC
//! stream carries the game's frames (a little-endian `u32` length, then the bytes); an empty
//! frame every second keeps the path (and the NAT mappings) alive.
//!
//! A second protocol answers a probe: a machine with an invitation asks the host whether its game
//! is still open, and the host answers with its LAN announcement (`SEU2\0version\0port\0name`),
//! so an invitation is only listed while its game exists.
//!
//! The async work runs on a small tokio runtime owned by [`P2p`]; the game thread only polls
//! channels.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, PublicKey};
use sticks_net::transport::MAX_FRAME;
use sticks_net::{FrameSink, Link};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, error::TryRecvError, unbounded_channel};

/// The game connection.
pub const GAME_ALPN: &[u8] = b"seu2/game";
/// "Is your game open?"
pub const PROBE_ALPN: &[u8] = b"seu2/probe";
/// The first bytes a joining machine sends on its stream (QUIC only tells the host about a new
/// stream once something is written on it).
const STREAM_MAGIC: &[u8; 4] = b"SEU2";
/// Finding the host and making a path can take a while on the internet.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(1);
/// How long a closing side waits for the other to take what it sent last.
const CLOSE_GRACE: Duration = Duration::from_secs(1);
const LARGEST_ANNOUNCEMENT: usize = 4096;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `text` is a game code (an endpoint's public key).
pub fn is_game_code(text: &str) -> bool {
    text.parse::<PublicKey>().is_ok()
}

struct Hosting {
    links: Sender<Link>,
    announcement: Vec<u8>,
}

#[derive(Default)]
struct Shared {
    endpoint: tokio::sync::OnceCell<Arc<Endpoint>>,
    /// This endpoint's code, once bound.
    code: Mutex<Option<String>>,
    /// The endpoint has reached a relay (others can find it).
    online: AtomicBool,
    /// Why the endpoint could not be made, if it could not.
    failure: Mutex<Option<String>>,
    hosting: Mutex<Option<Hosting>>,
}

/// This machine's peer-to-peer endpoint.
pub struct P2p {
    runtime: tokio::runtime::Runtime,
    shared: Arc<Shared>,
}

impl P2p {
    /// Start the runtime and begin binding the endpoint (in the background).
    pub fn start() -> io::Result<P2p> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("p2p").enable_all().build()?;
        let p2p = P2p { runtime, shared: Arc::new(Shared::default()) };
        p2p.warm_up();
        Ok(p2p)
    }

    /// Bind the endpoint if that has not happened (or failed before).
    pub fn warm_up(&self) {
        let shared = self.shared.clone();
        self.runtime.spawn(async move {
            let _ = endpoint(&shared).await;
        });
    }

    /// This machine's game code, once the endpoint is bound.
    pub fn code(&self) -> Option<String> {
        lock(&self.shared.code).clone()
    }

    /// Whether others can find this machine yet.
    pub fn is_online(&self) -> bool {
        self.shared.online.load(Ordering::Relaxed)
    }

    /// Why the endpoint could not be made (the last attempt).
    pub fn failure(&self) -> Option<String> {
        lock(&self.shared.failure).clone()
    }

    /// Accept joining machines from now on: each arrives on the returned channel. Probes are
    /// answered with `announcement`.
    pub fn host(&self, announcement: Vec<u8>) -> Receiver<Link> {
        let (tx, rx) = channel();
        *lock(&self.shared.hosting) = Some(Hosting { links: tx, announcement });
        self.warm_up();
        rx
    }

    /// Turn joining machines (and probes) away again.
    pub fn stop_hosting(&self) {
        *lock(&self.shared.hosting) = None;
    }

    /// Connect to the game whose code is `code`.
    pub fn connect(&self, code: &str) -> Receiver<io::Result<Link>> {
        let (tx, rx) = channel();
        let shared = self.shared.clone();
        let code = code.trim().to_owned();
        self.runtime.spawn(async move {
            let result = match tokio::time::timeout(CONNECT_TIMEOUT, dial(&shared, &code)).await {
                Ok(r) => r,
                Err(_) => Err("the host could not be reached".to_owned()),
            };
            let _ = tx.send(result.map_err(io::Error::other));
        });
        rx
    }

    /// Ask the machine with `code` whether it hosts a game: its announcement bytes, or `None`.
    pub fn probe(&self, code: &str) -> Receiver<Option<Vec<u8>>> {
        let (tx, rx) = channel();
        let shared = self.shared.clone();
        let code = code.trim().to_owned();
        self.runtime.spawn(async move {
            let answer = tokio::time::timeout(PROBE_TIMEOUT, probe(&shared, &code)).await.ok().and_then(Result::ok);
            let _ = tx.send(answer);
        });
        rx
    }
}

/// The endpoint, bound on first use; its accept loop starts with it.
async fn endpoint(shared: &Arc<Shared>) -> Result<Arc<Endpoint>, String> {
    let result = shared
        .endpoint
        .get_or_try_init(|| async {
            let endpoint = Endpoint::builder(presets::N0).alpns(vec![GAME_ALPN.to_vec(), PROBE_ALPN.to_vec()]).bind().await.map_err(|e| e.to_string())?;
            let endpoint = Arc::new(endpoint);
            *lock(&shared.code) = Some(endpoint.id().to_string());
            *lock(&shared.failure) = None;
            tokio::spawn(accept_loop(shared.clone(), endpoint.clone()));
            let (s, e) = (shared.clone(), endpoint.clone());
            tokio::spawn(async move {
                e.online().await;
                s.online.store(true, Ordering::Relaxed);
            });
            Ok::<_, String>(endpoint)
        })
        .await
        .cloned();
    if let Err(e) = &result {
        *lock(&shared.failure) = Some(e.clone());
    }
    result
}

async fn accept_loop(shared: Arc<Shared>, endpoint: Arc<Endpoint>) {
    while let Some(incoming) = endpoint.accept().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else { return };
            let alpn = connection.alpn().to_vec();
            if alpn == PROBE_ALPN {
                answer_probe(&shared, connection).await;
            } else if alpn == GAME_ALPN {
                accept_game(&shared, connection).await;
            } else {
                connection.close(1u32.into(), b"unknown protocol");
            }
        });
    }
}

async fn answer_probe(shared: &Shared, connection: Connection) {
    let announcement = lock(&shared.hosting).as_ref().map(|h| h.announcement.clone());
    let Some(announcement) = announcement else {
        connection.close(1u32.into(), b"no game");
        return;
    };
    let Ok(Ok((mut send, mut recv))) = tokio::time::timeout(PROBE_TIMEOUT, connection.accept_bi()).await else {
        connection.close(1u32.into(), b"no question");
        return;
    };
    let _ = recv.read_to_end(64).await;
    if send.write_all(&announcement).await.is_ok() {
        let _ = send.finish();
    }
    // The asking side closes once it has the answer.
    let _ = tokio::time::timeout(PROBE_TIMEOUT, connection.closed()).await;
}

async fn accept_game(shared: &Shared, connection: Connection) {
    let links = lock(&shared.hosting).as_ref().map(|h| h.links.clone());
    let Some(links) = links else {
        connection.close(1u32.into(), b"no game");
        return;
    };
    let Ok(Ok((send, mut recv))) = tokio::time::timeout(CONNECT_TIMEOUT, connection.accept_bi()).await else {
        connection.close(1u32.into(), b"no stream");
        return;
    };
    let mut magic = [0u8; 4];
    if recv.read_exact(&mut magic).await.is_err() || &magic != STREAM_MAGIC {
        connection.close(1u32.into(), b"not a game stream");
        return;
    }
    let label = format!("peer {}", connection.remote_id().fmt_short());
    let link = spawn_link(connection, send, recv, label);
    // A host that stopped meanwhile drops the link, which closes it.
    let _ = links.send(link);
}

fn parse_code(code: &str) -> Result<PublicKey, String> {
    code.parse::<PublicKey>().map_err(|_| "that is not a game code".to_owned())
}

async fn dial(shared: &Arc<Shared>, code: &str) -> Result<Link, String> {
    let key = parse_code(code)?;
    let endpoint = endpoint(shared).await?;
    let connection = endpoint.connect(key, GAME_ALPN).await.map_err(|e| e.to_string())?;
    let (mut send, recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
    send.write_all(STREAM_MAGIC).await.map_err(|e| e.to_string())?;
    Ok(spawn_link(connection, send, recv, format!("host {}", key.fmt_short())))
}

async fn probe(shared: &Arc<Shared>, code: &str) -> Result<Vec<u8>, String> {
    let key = parse_code(code)?;
    let endpoint = endpoint(shared).await?;
    let connection = endpoint.connect(key, PROBE_ALPN).await.map_err(|e| e.to_string())?;
    let result = async {
        let (mut send, mut recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
        send.write_all(b"?").await.map_err(|e| e.to_string())?;
        let _ = send.finish();
        recv.read_to_end(LARGEST_ANNOUNCEMENT).await.map_err(|e| e.to_string())
    }
    .await;
    connection.close(0u32.into(), b"thanks");
    result
}

enum Command {
    Frame(Vec<u8>),
    Close,
}

struct Sink(UnboundedSender<Command>);

impl FrameSink for Sink {
    fn send(&self, frame: Vec<u8>) {
        let _ = self.0.send(Command::Frame(frame));
    }

    fn close(&self) {
        let _ = self.0.send(Command::Close);
    }
}

/// The game's side of a connection: a writer and a reader task.
fn spawn_link(connection: Connection, send: SendStream, recv: RecvStream, label: String) -> Link {
    let (out_tx, out_rx) = unbounded_channel();
    let (in_tx, in_rx) = channel();
    tokio::spawn(write_loop(connection.clone(), send, out_rx));
    tokio::spawn(read_loop(connection, recv, in_tx));
    Link { sink: Box::new(Sink(out_tx)), incoming: Mutex::new(in_rx), label }
}

fn push_frame(buffer: &mut Vec<u8>, frame: &[u8]) {
    buffer.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    buffer.extend_from_slice(frame);
}

async fn write_loop(connection: Connection, mut send: SendStream, mut commands: UnboundedReceiver<Command>) {
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut buffer = Vec::new();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(Command::Frame(frame)) = command else { break };
                buffer.clear();
                push_frame(&mut buffer, &frame);
                // Whatever else is queued goes out in the same write.
                let mut closing = false;
                loop {
                    match commands.try_recv() {
                        Ok(Command::Frame(f)) => push_frame(&mut buffer, &f),
                        Ok(Command::Close) | Err(TryRecvError::Disconnected) => {
                            closing = true;
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                    }
                }
                if send.write_all(&buffer).await.is_err() {
                    connection.close(0u32.into(), b"write failed");
                    return;
                }
                heartbeat.reset();
                if closing {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                if send.write_all(&0u32.to_le_bytes()).await.is_err() {
                    connection.close(0u32.into(), b"write failed");
                    return;
                }
            }
        }
    }
    // Closing (or the game dropped the link): let the other side take the rest, then go.
    let _ = send.finish();
    let _ = tokio::time::timeout(CLOSE_GRACE, send.stopped()).await;
    connection.close(0u32.into(), b"bye");
}

async fn read_loop(connection: Connection, mut recv: RecvStream, frames: Sender<Vec<u8>>) {
    loop {
        let mut header = [0u8; 4];
        if recv.read_exact(&mut header).await.is_err() {
            break;
        }
        let n = u32::from_le_bytes(header) as usize;
        if n > MAX_FRAME {
            break;
        }
        if n == 0 {
            continue;
        }
        let mut frame = vec![0u8; n];
        if recv.read_exact(&mut frame).await.is_err() || frames.send(frame).is_err() {
            break;
        }
    }
    // Dropping `frames` tells the game the connection is over.
    connection.close(0u32.into(), b"done");
}
