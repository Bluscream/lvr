//! Single-instance guard.
//!
//! Two supervisors would fight each other over every managed process, so the
//! first `lvr` binds a socket in `$XDG_RUNTIME_DIR` and any later launch just
//! asks it to raise its window.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::state::Shared;

const SHOW: &str = "show";

pub enum Acquired {
    /// We are the first instance; keep this listener alive.
    Listener(UnixListener),
    /// Another instance answered and has been told to show itself.
    AlreadyRunning,
    /// No socket could be used; run anyway, without the guard.
    Unavailable(anyhow::Error),
}

pub fn socket_path() -> Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is not set")?;
    Ok(dir.join("lvr.sock"))
}

/// Message a later launch sends to the running instance.
pub fn show_request(tab: Option<&str>) -> String {
    match tab {
        Some(tab) => format!("{SHOW} {tab}\n"),
        None => format!("{SHOW}\n"),
    }
}

/// Parse such a message back into "show, optionally on this tab".
pub fn parse_request(message: &str) -> Option<Option<String>> {
    let line = message.lines().next()?.trim();
    let rest = line.strip_prefix(SHOW)?.trim();
    Some((!rest.is_empty()).then(|| rest.to_string()))
}

/// Try to become the single instance.
pub fn acquire(request: &str) -> Acquired {
    let path = match socket_path() {
        Ok(path) => path,
        Err(err) => return Acquired::Unavailable(err),
    };

    if let Ok(mut stream) = UnixStream::connect(&path) {
        // Someone is listening: hand the request over and step aside.
        let _ = stream.write_all(request.as_bytes());
        let _ = stream.flush();
        return Acquired::AlreadyRunning;
    }

    // Nobody answered: any file there is a leftover from a crash.
    if path.exists()
        && let Err(err) = std::fs::remove_file(&path)
    {
        return Acquired::Unavailable(
            anyhow::Error::new(err).context(format!("removing stale {}", path.display())),
        );
    }

    match UnixListener::bind(&path) {
        Ok(listener) => Acquired::Listener(listener),
        Err(err) => Acquired::Unavailable(
            anyhow::Error::new(err).context(format!("binding {}", path.display())),
        ),
    }
}

/// Answer "show" requests from later launches, on a background thread.
pub fn serve(listener: UnixListener, shared: Shared) {
    std::thread::Builder::new()
        .name("lvr-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if shared.is_quitting() {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 128];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let message = String::from_utf8_lossy(&buffer[..read]).into_owned();
                if let Some(tab) = parse_request(&message) {
                    shared.info("Another launch asked for the window");
                    shared.request_show_tab(tab);
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|err| tracing::warn!("could not start the IPC thread: {err}"));
}

/// Best-effort cleanup so the next launch does not have to remove a stale file.
pub fn cleanup() {
    if let Ok(path) = socket_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn requests_round_trip_through_the_wire_format() {
        assert_eq!(parse_request(&show_request(None)), Some(None));
        assert_eq!(
            parse_request(&show_request(Some("logs"))),
            Some(Some("logs".to_string()))
        );
        assert_eq!(parse_request("quit\n"), None);
        assert_eq!(parse_request(""), None);
    }

    #[test]
    fn socket_path_lives_in_the_runtime_dir() {
        let previous = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: single-threaded test that restores the variable afterwards.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/test") };
        assert_eq!(
            socket_path().unwrap(),
            PathBuf::from("/run/user/test/lvr.sock")
        );
        match previous {
            Some(value) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", value) },
            None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
        }
    }

    #[test]
    fn a_second_connect_reaches_the_first_listener() {
        let dir = std::env::temp_dir().join(format!("lvr-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("lvr.sock");
        let listener = UnixListener::bind(&path).expect("bind");

        let accepted = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 32];
            let read = stream.read(&mut buffer).unwrap_or(0);
            buffer[..read].starts_with(b"show")
        });

        std::thread::sleep(Duration::from_millis(50));
        let mut stream = UnixStream::connect(&path).expect("connect");
        stream
            .write_all(show_request(None).as_bytes())
            .expect("write");
        drop(stream);

        assert!(accepted.join().expect("join"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_socket_file_does_not_block_binding() {
        let dir = std::env::temp_dir().join(format!("lvr-stale-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("lvr.sock");
        std::fs::write(&path, b"not a socket").expect("write stale file");

        assert!(UnixStream::connect(&path).is_err());
        std::fs::remove_file(&path).expect("remove stale");
        assert!(UnixListener::bind(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
