//! Library target — re-exports all daemon modules for integration tests.
//!
//! The binary in `main.rs` wires everything together; the library exposes
//! the public API so `tests/transport_integration.rs` can import types.
//!
//! Phase 1 modules are compiled but not actively used in Phase 2 — they will
//! be reactivated in Phase 3 (pairing flow). The `dead_code` allow prevents
//! spurious warnings until then.
#![allow(dead_code)]

pub mod config;
pub mod tls;
pub mod transport;

// Phase 1 modules — compiled for continuity; wired back in Phase 3.
pub(crate) mod dbus;
pub(crate) mod identity;
pub(crate) mod keepalive;
pub(crate) mod mdns;
pub(crate) mod network;
pub(crate) mod pairing;
pub(crate) mod server;
pub(crate) mod store;
