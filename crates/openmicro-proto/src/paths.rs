use std::path::PathBuf;

pub fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

pub fn hook_socket() -> PathBuf {
    runtime_dir().join("openmicro.sock")
}

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
        if std::env::var("XDG_RUNTIME_DIR").is_err() {
            assert_eq!(runtime_dir(), std::env::temp_dir());
        }
    }
}
