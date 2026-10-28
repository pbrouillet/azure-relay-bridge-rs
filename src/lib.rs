//! Azure Relay Bridge library — re-exports internal modules for integration testing.
#![allow(dead_code)]

mod banner;
pub mod broker;
pub mod config;
pub mod config_loader;
pub mod host;
pub mod hosts;
pub mod http_forward;
pub mod http_remote_forwarder;
pub mod preamble;
pub mod remote_bridge;
#[cfg(unix)]
pub mod socket;
pub mod stream_pump;
pub mod tcp;
pub mod udp;

// These modules are binary-only (CLI parsing, logging setup, service)
pub mod cli;
mod logging;
mod service;
