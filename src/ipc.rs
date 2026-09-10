//! Single-instance guard.
//!
//! Two supervisors would fight each other over every managed process, so the
//! first `lvr` binds a socket in `$XDG_RUNTIME_DIR` and any later launch just
//! asks it to raise its window.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::state::Shared;

const SHOW: &str = "show";

pub enum Acquired {
    /// We are the first instance; keep this listener alive.
    Listener(UnixListener, File),
    /// Another instance answered and has been told to show itself.
    AlreadyRunning,
    /// The guard could not be established; starting another supervisor is unsafe.
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
    let mut words = line.splitn(2, char::is_whitespace);
    if words.next()? != SHOW {
        return None;
    }
    let rest = words.next().unwrap_or("").trim();
    Some((!rest.is_empty()).then(|| rest.to_string()))
}

/// Try to become the single instance.
pub fn acquire(request: &str) -> Acquired {
    let path = match socket_path() {
        Ok(path) => path,
        Err(err) => return Acquired::Unavailable(err),
    };

    let lock_path = path.with_extension("lock");
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(err) => return Acquired::Unavailable(err.into()),
    };
    // The lock covers stale-socket cleanup and is held for the entire GUI lifetime.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::WouldBlock {
            return Acquired::Unavailable(err.into());
        }
        if let Ok(mut stream) = UnixStream::connect(&path) {
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            if let Err(err) = stream.write_all(request.as_bytes()) {
                tracing::warn!("Could not raise existing LinuxVR window: {err}");
            }
        } else {
            tracing::warn!(
                "LinuxVR owns the instance lock but is not yet accepting window requests"
            );
        }
        return Acquired::AlreadyRunning;
    }
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            if let Err(err) = std::fs::remove_file(&path) {
                return Acquired::Unavailable(err.into());
            }
        }
        Ok(_) => {
            return Acquired::Unavailable(anyhow::anyhow!(
                "{} exists and is not a socket",
                path.display()
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Acquired::Unavailable(err.into()),
    }
    match UnixListener::bind(&path) {
        Ok(listener) => Acquired::Listener(listener, lock),
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
                if let Err(err) = stream.set_read_timeout(Some(Duration::from_secs(1))) {
                    shared.warn(format!("IPC timeout setup failed: {err}"));
                    continue;
                }
                let mut message = String::new();
                if BufReader::new((&mut stream).take(128))
                    .read_line(&mut message)
                    .is_err()
                {
                    continue;
                }
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
        assert_eq!(parse_request("showcase\n"), None);
        assert_eq!(parse_request(""), None);
    }

    #[test]
    fn socket_path_lives_in_the_runtime_dir() {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            assert_eq!(socket_path().unwrap(), PathBuf::from(dir).join("lvr.sock"));
        } else {
            assert!(socket_path().is_err());
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
