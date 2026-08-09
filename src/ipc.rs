//! Single-instance guard.
//!
//! Two supervisors fight each other over every managed process — one restarts
//! WiVRn while the other is still shutting it down, and both race to launch the
//! same companion apps. The first `lvr` binds a socket in `$XDG_RUNTIME_DIR`;
//! any later launch hands its request over and exits.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use tracing::warn;

use crate::service::AppState;

const SHOW: &[u8] = b"show\n";

/// Deliberately distinct from any other lvr build's socket, so this guard only
/// ever prevents *this* app from running twice.
const SOCKET_NAME: &str = "lvr-gemini.sock";

pub enum Acquired {
    /// We are the first instance; keep this listener alive for the whole run.
    Listener(UnixListener),
    /// Another instance answered and has been asked to show its window.
    AlreadyRunning,
    /// No socket could be used; run anyway, without the guard.
    Unavailable(String),
}

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|dir| dir.join(SOCKET_NAME))
}

pub fn acquire() -> Acquired {
    let Some(path) = socket_path() else {
        return Acquired::Unavailable("XDG_RUNTIME_DIR is not set".to_string());
    };

    if let Ok(mut stream) = UnixStream::connect(&path) {
        let _ = stream.write_all(SHOW);
        let _ = stream.flush();
        return Acquired::AlreadyRunning;
    }

    // Nobody answered, so anything at that path is a leftover from a crash.
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            return Acquired::Unavailable(format!("removing stale {:?}: {}", path, e));
        }
    }

    match UnixListener::bind(&path) {
        Ok(listener) => Acquired::Listener(listener),
        Err(e) => Acquired::Unavailable(format!("binding {:?}: {}", path, e)),
    }
}

/// Answer "show" requests from later launches, on a background thread.
pub fn serve(listener: UnixListener, state: AppState) {
    let spawned = std::thread::Builder::new()
        .name("lvr-ipc".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                if state.is_quitting() {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 32];
                let read = stream.read(&mut buffer).unwrap_or(0);
                if buffer[..read].starts_with(b"show") {
                    state.add_log("Another launch asked for the window.");
                    state.request_show_window();
                }
            }
        });
    if let Err(e) = spawned {
        warn!("could not start the IPC thread: {}", e);
    }
}

/// Best-effort cleanup so the next launch does not have to clear a stale file.
pub fn cleanup() {
    if let Some(path) = socket_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn socket_lives_in_the_runtime_dir_under_its_own_name() {
        if std::env::var_os("XDG_RUNTIME_DIR").is_some() {
            let path = socket_path().expect("runtime dir is set");
            assert!(path.ends_with(SOCKET_NAME));
        }
    }

    #[test]
    fn a_later_launch_reaches_the_first_instance() {
        let dir = std::env::temp_dir().join(format!("lvr-gemini-ipc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(SOCKET_NAME);
        let listener = UnixListener::bind(&path).expect("bind");

        let accepted = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 32];
            let read = stream.read(&mut buffer).unwrap_or(0);
            buffer[..read].starts_with(b"show")
        });

        std::thread::sleep(Duration::from_millis(50));
        let mut stream = UnixStream::connect(&path).expect("connect");
        stream.write_all(SHOW).expect("write");
        drop(stream);

        assert!(accepted.join().expect("join"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_socket_file_does_not_block_binding() {
        let dir = std::env::temp_dir().join(format!("lvr-gemini-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(SOCKET_NAME);
        std::fs::write(&path, b"not a socket").expect("write stale file");

        assert!(UnixStream::connect(&path).is_err());
        std::fs::remove_file(&path).expect("remove stale");
        assert!(UnixListener::bind(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
