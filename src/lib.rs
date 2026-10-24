//! Durable catalog, pure reconciliation, and Temper-backed persistence protocols.

// Share independent fixture adapters with the integration-test harness.
#[cfg(test)]
extern crate self as streams_temper;

#[cfg(test)]
#[path = "../tests/support/memory.rs"]
mod memory;

pub mod agent;
pub mod application;
pub mod cancellation;
pub mod child_protocol;
pub mod codec;
pub mod config;
mod file_io;
pub mod frontend;
pub mod model;
pub mod operations;
pub mod process;
pub mod runtime;
pub mod storage;
pub mod sync;
pub mod tui;
pub mod yaml_store;
