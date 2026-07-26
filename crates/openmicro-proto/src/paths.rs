//! Where the daemon's sockets live.
//!
//! Three separate binaries have to agree on these paths, and when they disagree
//! nothing reports an error: the hook is deliberately silent when it cannot
//! connect, so a mismatch just means the macropad quietly never lights up. The
//! rule lives here so there is only one of it.

use std::path::PathBuf;

/// Directory holding the sockets.
///
/// `$XDG_RUNTIME_DIR` when set, and the system temp directory otherwise —
/// which honours `$TMPDIR`. Every caller must agree on the fallback too: a
/// daemon that fell back to `$TMPDIR` while the hook hard-coded `/tmp` left the
/// hook writing to a socket nobody was listening on.
pub fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// Socket the adapters push agent-state events to.
pub fn hook_socket() -> PathBuf {
    runtime_dir().join("openmicro.sock")
}

/// Socket the TUI uses for commands and snapshots.
pub fn control_socket() -> PathBuf {
    runtime_dir().join("openmicro-ctl.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_sockets_are_different_files_in_one_directory() {
        assert_ne!(hook_socket(), control_socket());
        assert_eq!(hook_socket().parent(), control_socket().parent());
        assert_eq!(hook_socket().parent(), Some(runtime_dir().as_path()));
    }

    #[test]
    fn the_fallback_is_the_temp_dir_not_a_hard_coded_slash_tmp() {
        // `std::env::temp_dir` honours `$TMPDIR`. Hard-coding `/tmp` in one
        // binary and not another is what made the hook talk to a socket nobody
        // was listening on.
        if std::env::var("XDG_RUNTIME_DIR").is_err() {
            assert_eq!(runtime_dir(), std::env::temp_dir());
        }
    }
}
