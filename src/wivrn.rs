//! Talking to the WiVRn server over D-Bus.
//!
//! The server (flatpak `io.github.wivrn.wivrn`) owns `io.github.wivrn.Server`
//! on the session bus and exposes `HeadsetConnected`, `SessionRunning`, a
//! `Disconnect()` and a `Quit()` method — which is everything `lvr` needs to
//! supervise it without guessing from the process table.

use std::time::Duration;

use anyhow::{Context, Result};
use zbus::proxy::CacheProperties;
use zbus::{Connection, fdo::DBusProxy};

pub const BUS_NAME: &str = "io.github.wivrn.Server";

#[zbus::proxy(
    interface = "io.github.wivrn.Server",
    default_service = "io.github.wivrn.Server",
    default_path = "/io/github/wivrn/Server",
    gen_blocking = false
)]
pub trait WivrnServer {
    /// Disconnect the current headset but leave the server running.
    fn disconnect(&self) -> zbus::Result<()>;
    /// Shut the server down.
    fn quit(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn headset_connected(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn session_running(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn system_name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn bitrate(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn preferred_refresh_rate(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn pairing_enabled(&self) -> zbus::Result<bool>;
}

/// One reading of the server's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WivrnState {
    /// The D-Bus service is present, i.e. the server process is up.
    pub running: bool,
    pub headset_connected: bool,
    pub session_running: bool,
    pub system_name: String,
    pub bitrate: u32,
    pub refresh_rate: f64,
}

/// Session-bus client. Reconnects lazily so `lvr` can start before WiVRn does.
pub struct WivrnClient {
    connection: Option<Connection>,
}

impl WivrnClient {
    pub fn new() -> Self {
        Self { connection: None }
    }

    async fn connection(&mut self) -> Result<&Connection> {
        if self.connection.is_none() {
            let connection = Connection::session()
                .await
                .context("connecting to the session D-Bus")?;
            self.connection = Some(connection);
        }
        Ok(self.connection.as_ref().expect("just connected"))
    }

    /// Drop a connection that errored so the next call redials.
    fn reset(&mut self) {
        self.connection = None;
    }

    async fn proxy(&mut self) -> Result<WivrnServerProxy<'static>> {
        let connection = self.connection().await?.clone();
        // Uncached: the server comes and goes, and a stale cache would report a
        // headset as connected after the server died.
        WivrnServerProxy::builder(&connection)
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .context("building the WiVRn D-Bus proxy")
    }

    /// Is anyone currently owning the WiVRn bus name?
    pub async fn is_running(&mut self) -> bool {
        let Ok(connection) = self.connection().await.cloned() else {
            self.reset();
            return false;
        };
        let owned = async {
            let dbus = DBusProxy::new(&connection).await.ok()?;
            dbus.name_has_owner(BUS_NAME.try_into().ok()?).await.ok()
        }
        .await;
        match owned {
            Some(value) => value,
            None => {
                self.reset();
                false
            }
        }
    }

    /// Read everything we care about in one go.
    pub async fn poll(&mut self) -> WivrnState {
        if !self.is_running().await {
            return WivrnState::default();
        }
        let Ok(proxy) = self.proxy().await else {
            self.reset();
            return WivrnState::default();
        };
        // A property read failing here means the server vanished mid-poll.
        let Ok(headset_connected) = proxy.headset_connected().await else {
            return WivrnState {
                running: true,
                ..Default::default()
            };
        };
        WivrnState {
            running: true,
            headset_connected,
            session_running: proxy.session_running().await.unwrap_or(false),
            system_name: proxy.system_name().await.unwrap_or_default(),
            bitrate: proxy.bitrate().await.unwrap_or(0),
            refresh_rate: proxy.preferred_refresh_rate().await.unwrap_or(0.0),
        }
    }

    /// Ask the server to quit. `Ok(false)` means it was not running anyway.
    pub async fn quit(&mut self) -> Result<bool> {
        if !self.is_running().await {
            return Ok(false);
        }
        let proxy = self.proxy().await?;
        proxy.quit().await.context("calling WiVRn Quit()")?;
        Ok(true)
    }

    /// Disconnect the headset without stopping the server.
    pub async fn disconnect(&mut self) -> Result<bool> {
        if !self.is_running().await {
            return Ok(false);
        }
        let proxy = self.proxy().await?;
        proxy
            .disconnect()
            .await
            .context("calling WiVRn Disconnect()")?;
        Ok(true)
    }

    /// Wait until the bus name disappears, up to `timeout`.
    pub async fn wait_until_gone(&mut self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !self.is_running().await {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Wait until the bus name appears, up to `timeout`.
    pub async fn wait_until_up(&mut self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.is_running().await {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

impl Default for WivrnClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_all_off() {
        let state = WivrnState::default();
        assert!(!state.running);
        assert!(!state.headset_connected);
        assert!(!state.session_running);
        assert_eq!(state.bitrate, 0);
    }

    #[test]
    fn bus_name_matches_the_wivrn_interface() {
        assert_eq!(BUS_NAME, "io.github.wivrn.Server");
    }

    /// Exercises the real bus when one is available; skipped in headless CI.
    #[tokio::test]
    async fn polling_without_a_session_bus_reports_not_running() {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
            // A live session bus is present: polling must not panic and must
            // return a consistent state.
            let mut client = WivrnClient::new();
            let state = client.poll().await;
            assert_eq!(state.running, client.is_running().await);
            return;
        }
        let mut client = WivrnClient::new();
        assert_eq!(client.poll().await, WivrnState::default());
    }
}
