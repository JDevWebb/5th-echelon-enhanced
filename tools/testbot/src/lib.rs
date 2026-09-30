//! Headless test players for 5th Echelon Enhanced.
//!
//! A [`bot::Bot`] speaks the game's Quazal protocol (auth server, ticket,
//! secure server) and its DLL's gRPC API, so server behaviour (sessions,
//! invites, private matches, cleanup) can be tested without the game.
//! `src/main.rs` runs the scenarios against a server.

pub mod bot;
pub mod conn;
pub mod load;
