//! Durable records, pure reconciliation, and file publication.

// Share independent fixture adapters with the integration-test harness.
#[cfg(test)]
extern crate self as streams;

#[cfg(test)]
#[path = "../tests/support/herdr.rs"]
mod fake_herdr;

pub mod agent;
pub mod application;
pub mod artifacts;
pub mod cancellation;
pub mod child_protocol;
pub mod codec;
pub mod config;
mod file_io;
pub mod frontend;
pub mod integration;
pub mod model;
pub mod operations;
pub mod process;
pub mod runtime;
pub mod storage;
pub mod sync;
pub mod tui;
pub mod web_url;
pub mod yaml_store;
