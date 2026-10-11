//! The lifecycle authority of a lower-MAC port.
//!
//! A port has one owner, which decides its lifecycle and its channel for
//! every interface. [`EventRouter::owner`] hands out its one [`PortOwner`];
//! a [`PortClient`](crate::client::PortClient) has no lifecycle method. With
//! one issuer and one reader of the router's lifecycle queue, commands run
//! one at a time and each meets its own terminal. A client that needs
//! another channel (a scan, an announced switch) asks the owner, which
//! carries it out.

use oer_ieee80211_lower_mac::{
    Channel, EventsLost, Ieee80211LowerMacPort, LifecycleCommand, LifecycleEvent, LowerMacSetting,
    SettingError,
};

use crate::{client::PortClientError, router::EventRouter};

/// The one issuer of a port's lifecycle commands and channel moves; it is
/// neither `Clone` nor `Copy`.
pub struct PortOwner<'r, 'p, P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize> {
    router: &'r EventRouter<'p, P, WAITERS, RX>,
    /// Commands the port accepted whose terminal is not read yet: a
    /// command future dropped while it waited leaves its terminal queued.
    unread: u32,
    /// Lifecycle terminals the router's queue dropped.
    terminals_lost: u32,
}

impl<'r, 'p, P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize>
    PortOwner<'r, 'p, P, WAITERS, RX>
{
    pub(crate) const fn new(router: &'r EventRouter<'p, P, WAITERS, RX>) -> Self {
        Self {
            router,
            unread: 0,
            terminals_lost: 0,
        }
    }

    pub const fn router(&self) -> &'r EventRouter<'p, P, WAITERS, RX> {
        self.router
    }

    /// Lifecycle terminals the router's queue dropped while a command ran.
    pub const fn terminals_lost(&self) -> u32 {
        self.terminals_lost
    }

    /// Move the enabled port to `channel` permanently: `Disable`, then
    /// [`LowerMacSetting::Channel`], then `Enable`, each to its terminal.
    /// The port goes through a full lifecycle, so `Disable` ends every
    /// admitted operation first. A move that keeps the port enabled goes
    /// through [`LowerMacLiveRetune`](oer_ieee80211_lower_mac::LowerMacLiveRetune)
    /// instead.
    ///
    /// A channel outside the port's capabilities is refused before the
    /// port is touched (`Setting(UnsupportedChannel)`). The port must be
    /// enabled: a disabled one refuses `Disable` (`Lifecycle(AlreadyInState)`)
    /// and stays as it is. An `Enable` that fails (`LifecycleFailed`,
    /// `LifecycleLost`) leaves the port disabled, and a service on it ends
    /// on that error (`is_port_lost`).
    pub async fn move_channel(
        &mut self,
        channel: Channel,
    ) -> Result<(), PortClientError<P::Fault>> {
        let port = self.router.port();
        if !port.capabilities().supports_channel(channel) {
            return Err(PortClientError::Setting(SettingError::UnsupportedChannel));
        }
        self.lifecycle(LifecycleCommand::Disable).await?;
        port.apply(LowerMacSetting::Channel(channel))?
            .map_err(PortClientError::Setting)?;
        self.lifecycle(LifecycleCommand::Enable).await
    }

    /// Run one lifecycle command to its terminal event. It is cancel-safe:
    /// the terminal of a command whose future was dropped is read and
    /// discarded before the next command, which meets its own terminal.
    pub async fn lifecycle(
        &mut self,
        command: LifecycleCommand,
    ) -> Result<(), PortClientError<P::Fault>> {
        while self.unread > 0 {
            if self.router.lifecycle().await.is_none() {
                return Err(self.poisoned());
            }
            self.unread -= 1;
        }
        self.router
            .port()
            .lifecycle(command)
            .await?
            .map_err(PortClientError::Lifecycle)?;
        self.unread = 1;
        let terminal = self.router.lifecycle().await;
        if terminal.is_some() {
            self.unread = 0;
        }
        match terminal {
            Some(Ok(LifecycleEvent::Failed { .. })) => Err(PortClientError::LifecycleFailed),
            Some(Ok(_)) => Ok(()),
            // The router's own queue overflowed: the terminal may be among
            // the dropped entries.
            Some(Err(EventsLost)) => {
                self.terminals_lost = self.terminals_lost.saturating_add(1);
                Err(PortClientError::LifecycleLost)
            }
            None => Err(self.poisoned()),
        }
    }

    fn poisoned(&self) -> PortClientError<P::Fault> {
        PortClientError::Poisoned(
            self.router
                .poisoned()
                .expect("the router ended its queue on poisoning"),
        )
    }
}
