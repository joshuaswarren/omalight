//! The Omalight engine side of the channel.
//!
//! [`server`] owns a LightCraft [`lightcraft_engine::Session`]. [`client`] is what the Omarchy
//! window, the headless check, and the MCP server speak. Rendering stays in LightCraft; this
//! crate only moves commands and JPEGs.

pub mod ai;
pub mod client;
pub mod config;
pub mod doctor;
pub mod engine;
pub mod fixture;
pub mod identity;
pub mod jpeg;
pub mod mdns;
pub mod notify;
pub mod pair;
pub mod proxy;
pub mod server;
pub mod setup;

pub use omalight_protocol as protocol;
