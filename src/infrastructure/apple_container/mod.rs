//! Integration with [apple/container](https://github.com/apple/container).
//!
//! Unlike Docker, Apple Container offers no daemon socket and no event
//! stream, so this module shells out to the `container` CLI and the caller
//! polls. Every container gets its own IP, so routing targets the container
//! address directly instead of a published host port.

// Wired up incrementally; see BACKLOG.md blocks B–D.
#[allow(dead_code)]
pub mod cli;
