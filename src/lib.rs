//! Durable catalog, pure reconciliation, and Temper-backed persistence protocols.

pub mod agent;
pub mod application;
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
