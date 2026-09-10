//! Optional service-manager notifications; desktop launches need no systemd connection.
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::Duration;

pub fn notify(message: &str) {
    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    if let Ok(pid) = std::env::var("WATCHDOG_PID")
        && pid.parse::<u32>().ok() != Some(std::process::id())
    {
        return;
    }
    if let Err(err) = send(path.as_bytes(), message.as_bytes()) {
        tracing::debug!("Service notification failed: {err}");
    }
}

fn send(path: &[u8], message: &[u8]) -> std::io::Result<()> {
    let address = if let Some(name) = path.strip_prefix(b"@") {
        SocketAddr::from_abstract_name(name)?
    } else {
        SocketAddr::from_pathname(std::ffi::OsStr::from_bytes(path))?
    };
    let socket = UnixDatagram::unbound()?;
    socket.set_write_timeout(Some(Duration::from_millis(100)))?;
    socket.connect_addr(&address)?;
    socket.send(message)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sends_watchdog_message_to_test_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify.sock");
        let receiver = UnixDatagram::bind(&path).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        send(path.as_os_str().as_bytes(), b"WATCHDOG=1").unwrap();
        let mut bytes = [0; 64];
        let count = receiver.recv(&mut bytes).unwrap();
        assert_eq!(&bytes[..count], b"WATCHDOG=1");
    }
}
