//! Azure Relay Bridge library — re-exports internal modules for integration testing.
#![allow(dead_code)]

pub mod config;
pub mod config_loader;
pub mod host;
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
mod cli;
mod logging;
mod service;
