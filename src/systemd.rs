//! Optional service-manager notifications; desktop launches need no systemd connection.
//!
//! Thin wrapper over the `sd-notify` crate, which handles the parts that are
//! easy to get subtly wrong: abstract (`@`-prefixed) socket names and the
//! `WATCHDOG_PID` check that keeps a child from answering its parent's watchdog.

/// Send a state notification, if we were started by a service manager.
pub fn notify(message: &str) {
    if std::env::var_os("NOTIFY_SOCKET").is_none() {
        return;
    }
    let state = match message.split('=').next().unwrap_or_default() {
        "READY" => sd_notify::NotifyState::Ready,
        "STOPPING" => sd_notify::NotifyState::Stopping,
        "WATCHDOG" => sd_notify::NotifyState::Watchdog,
        other => {
            tracing::debug!("Ignoring unsupported service notification {other:?}");
            return;
        }
    };
    // `unset_env = false`: the supervisor notifies repeatedly for the watchdog,
    // so NOTIFY_SOCKET has to survive the first call.
    if let Err(err) = sd_notify::notify(false, &[state]) {
        tracing::debug!("Service notification failed: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_we_send_is_recognised_and_harmless_without_systemd() {
        // Without NOTIFY_SOCKET these are no-ops; the point is that none of the
        // messages the supervisor actually sends panic on the way out.
        notify("READY=1");
        notify("WATCHDOG=1");
        notify("STOPPING=1");
        notify("NONSENSE");
    }
}
