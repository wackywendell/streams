//! Durable records, pure reconciliation, and file publication.

// Share independent fixture adapters with the integration-test harness.
#[cfg(test)]
extern crate self as streams;

#[cfg(test)]
#[path = "../tests/support/herdr.rs"]
mod fake_herdr;

pub mod application;
pub mod artifacts;
pub mod cancellation;
pub mod codec;
pub mod config;
mod file_io;
pub mod file_store;
pub mod frontend;
pub mod model;
pub mod operations;
pub mod process;
pub mod runtime;
pub mod storage;
pub mod sync;
pub mod tui;
pub mod web_url;
