//! Integration with [apple/container](https://github.com/apple/container).
//!
//! Unlike Docker, Apple Container offers no daemon socket and no event
//! stream, so this module shells out to the `container` CLI and the caller
//! polls. Every container gets its own IP, so routing targets the container
//! address directly instead of a published host port.

// Wired up incrementally; see BACKLOG.md blocks B–D.
#[allow(dead_code)]
pub mod cli;
#[allow(dead_code)]
pub mod discovery;
pub mod provider;
pub mod watcher;

pub use provider::AppleContainerProvider;

/// The uid the `container` CLI must run as, when it isn't ours.
///
/// Apple Container's API server is a per-user launchd agent that rejects
/// callers with a foreign euid. A root daemon must therefore impersonate
/// the console user (the one logged into the GUI session, owner of
/// `/dev/console`). Returns `None` when no impersonation is needed or
/// possible: not root, no console user, or console user is root.
pub fn run_as_uid_for_root() -> Option<u32> {
    // SAFETY: geteuid takes no arguments and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        return None;
    }

    use std::os::unix::fs::MetadataExt;
    let console_uid = std::fs::metadata("/dev/console").ok()?.uid();
    (console_uid != 0).then_some(console_uid)
}
