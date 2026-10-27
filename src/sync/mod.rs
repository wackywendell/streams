//! Publication transports bytes; local record application is independent.

pub mod git_io;
pub mod outcomes;
pub mod records;
pub mod state;

/// Deadline for one Git command or diagnostic native request.
pub const COMMAND_TIMEOUT_SECONDS: f64 = 30.0;
