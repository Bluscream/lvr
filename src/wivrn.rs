//! Talking to the WiVRn server over D-Bus.
//!
//! The server (flatpak `io.github.wivrn.wivrn`) owns `io.github.wivrn.Server`
//! on the session bus and exposes `HeadsetConnected`, `SessionRunning`, a
//! `Disconnect()` and a `Quit()` method — which is everything `lvr` needs to
//! supervise it without guessing from the process table.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::watch;
use futures_util::StreamExt;
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
}

/// Session-bus client. Reconnects lazily so `lvr` can start before WiVRn does.
///
/// Liveness comes from `NameOwnerChanged` rather than a `NameHasOwner` call per
/// tick, and every property on this interface is declared `emits-change`, so the
/// proxy caches them from `PropertiesChanged` instead of round-tripping. A full
/// [`Self::poll`] therefore costs no bus traffic at all while nothing moves.
pub struct WivrnClient {
    connection: Option<Connection>,
    /// Proxy, tagged with the ownership generation it was built under.
    proxy: Option<(u64, WivrnServerProxy<'static>)>,
    /// Latest known ownership of [`BUS_NAME`], maintained by `watcher`.
    running: Option<watch::Receiver<bool>>,
    /// Bumped on every ownership change, to invalidate caches belonging to a
    /// previous owner.
    generation: Arc<AtomicU64>,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

impl WivrnClient {
    pub fn new() -> Self {
        Self {
            connection: None,
            proxy: None,
            running: None,
            generation: Arc::new(AtomicU64::new(0)),
            watcher: None,
        }
    }

    async fn connection(&mut self) -> Result<&Connection> {
        if self.connection.is_none() {
            let connection = tokio::time::timeout(
                Duration::from_secs(5),
                zbus::connection::Builder::session()?
                    .method_timeout(Duration::from_secs(5))
                    .build(),
            )
            .await
            .context("session D-Bus connection timed out after 5s")?
            .context("connecting to the session D-Bus")?;
            self.connection = Some(connection);
        }
        Ok(self.connection.as_ref().expect("just connected"))
    }

    /// Drop a connection that errored so the next call redials.
    fn reset(&mut self) {
        self.connection = None;
        self.proxy = None;
        self.running = None;
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
    }

    /// Connect if needed and make sure the ownership watcher is running.
    ///
    /// Returns a receiver that always holds the current ownership of
    /// [`BUS_NAME`]: seeded once with `NameHasOwner`, then kept current by
    /// `NameOwnerChanged` signals.
    async fn ensure_watch(&mut self) -> Result<watch::Receiver<bool>> {
        if let Some(running) = &self.running {
            return Ok(running.clone());
        }
        let connection = self.connection().await?.clone();
        let dbus = DBusProxy::new(&connection)
            .await
            .context("opening the D-Bus daemon proxy")?;

        // Subscribe before the initial read, so an ownership change racing with
        // startup is queued rather than missed.
        let mut changes = dbus
            .receive_name_owner_changed()
            .await
            .context("subscribing to NameOwnerChanged")?;
        let initial = dbus
            .name_has_owner(BUS_NAME.try_into().context("WiVRn bus name is invalid")?)
            .await
            .context("asking whether WiVRn is on the bus")?;

        let (tx, rx) = watch::channel(initial);
        let generation = self.generation.clone();
        self.watcher = Some(tokio::spawn(async move {
            while let Some(signal) = changes.next().await {
                let Ok(args) = signal.args() else { continue };
                if args.name().as_str() != BUS_NAME {
                    continue;
                }
                generation.fetch_add(1, Ordering::SeqCst);
                // A send failure means the client went away; so should we.
                if tx.send(args.new_owner().is_some()).is_err() {
                    return;
                }
            }
        }));
        self.running = Some(rx.clone());
        Ok(rx)
    }

    async fn proxy(&mut self) -> Result<&WivrnServerProxy<'static>> {
        let generation = self.generation.load(Ordering::SeqCst);
        // Cached properties belong to one owner; a new owner needs a new proxy.
        if self.proxy.as_ref().is_none_or(|(built, _)| *built != generation) {
            let connection = self.connection().await?.clone();
            let proxy = WivrnServerProxy::builder(&connection)
                .cache_properties(CacheProperties::Yes)
                .build()
                .await
                .context("building the WiVRn D-Bus proxy")?;
            self.proxy = Some((generation, proxy));
        }
        Ok(&self.proxy.as_ref().expect("just built").1)
    }

    /// Is anyone currently owning the WiVRn bus name?
    pub async fn is_running(&mut self) -> bool {
        match self.ensure_watch().await {
            Ok(running) => *running.borrow(),
            Err(_) => {
                self.reset();
                false
            }
        }
    }

    /// Read everything we care about in one go.
    pub async fn poll(&mut self) -> WivrnState {
        if !self.is_running().await {
            // Never report a stale headset from an owner that is gone.
            self.proxy = None;
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
        self.wait_until(false, timeout).await
    }

    /// Wait until the bus name appears, up to `timeout`.
    pub async fn wait_until_up(&mut self, timeout: Duration) -> bool {
        self.wait_until(true, timeout).await
    }

    /// Wait for ownership to reach `wanted`, up to `timeout`.
    ///
    /// Driven by `NameOwnerChanged`, so this settles as soon as the server
    /// actually appears or exits rather than on the next poll boundary.
    async fn wait_until(&mut self, wanted: bool, timeout: Duration) -> bool {
        let Ok(mut running) = self.ensure_watch().await else {
            self.reset();
            // Absent a bus, "gone" is true and "up" is not.
            return !wanted;
        };
        if *running.borrow() == wanted {
            return true;
        }
        tokio::time::timeout(timeout, async {
            while running.changed().await.is_ok() {
                if *running.borrow() == wanted {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }
}

impl Default for WivrnClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WivrnClient {
    fn drop(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
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
        assert!(state.system_name.is_empty());
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
