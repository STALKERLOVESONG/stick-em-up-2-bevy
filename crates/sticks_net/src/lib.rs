//! LAN play for Stick 'Em Up 2, without the engine: what the original did with Unity's legacy
//! networking (`Network`, `NetworkView` RPCs and state streams) and its own `ServerDiscovery`.
//!
//! * [`discovery`]: the UDP announcements hosts broadcast and clients list (the original's
//!   packet format);
//! * [`transport`]: the host and client connections (framed TCP or links of another
//!   transport, polled once per frame);
//! * [`protocol`]: the messages, one per RPC or state stream of the original;
//! * [`room`]: lobby rooms (name, password, size, members, chat; not in the original);
//! * [`wire`]: the binary encoding.
//!
//! The game crate decides what is sent when; this crate only moves bytes.

pub mod discovery;
pub mod protocol;
pub mod room;
pub mod slots;
pub mod transport;
pub mod wire;

pub use discovery::{GameAddress, NETWORK_VERSION, SERVER_PORT, ServerListing};
pub use protocol::{AvatarState, HelicopterState, LoadKind, LobbySettings, Message, NetId, WireAim, WireColor, WireControls, WirePlayerId, WireScore};
pub use room::{Room, RoomConfig, RoomMember, RoomSummary};
pub use slots::{Assignments, Machine, SlotTable};
pub use transport::{Client, ClientEvent, FrameSink, Host, HostEvent, Link, PeerId};
