//! The lifecycle authority of a lower-MAC port.
//!
//! A port has one owner, which decides its lifecycle and its channel for
//! every interface. [`EventRouter::owner`] hands out its one [`PortOwner`];
//! a [`PortClient`](crate::client::PortClient) has no lifecycle method. With
//! one issuer and one reader of the router's lifecycle queue, commands run
//! one at a time and each meets its own terminal. A client that needs
//! another channel (a scan, an announced switch) asks the owner, which
//! carries it out.
//!
//! A client cannot reach the port's lifecycle, its channel or another
//! interface's state: it has no lifecycle method, no channel move, and no
//! raw port or router to call them on. Each refused call below has a twin
//! with the same setup that compiles, so a refusal proves the missing
//! authority and not a broken example.
//!
//! The setup compiles, and so do the calls a client and an owner may make:
//!
//! ```
//! # use oer_ieee80211_upper_mac_service::{PortRouter, client::{PortClient, PortClientEnv}};
//! # use oer_ieee80211_lower_mac::{Channel, Ieee80211LowerMacPort, LifecycleCommand, LowerMacSetting};
//! async fn client<X: PortClientEnv>(client: &mut PortClient<'_, X, 4, 4>, channel: Channel) {
//!     let _ = client.capabilities();
//!     let _ = client.set_he_bss_color(1);
//!     let _ = (channel, LifecycleCommand::Disable, core::marker::PhantomData::<LowerMacSetting>);
//! }
//! async fn owner<P: Ieee80211LowerMacPort>(router: &PortRouter<'_, P>, channel: Channel) {
//!     if let Some(mut owner) = router.owner() {
//!         let _ = owner.lifecycle(LifecycleCommand::Disable).await;
//!         let _ = owner.move_channel(channel).await;
//!     }
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_ieee80211_upper_mac_service::client::{PortClient, PortClientEnv};
//! # use oer_ieee80211_lower_mac::LifecycleCommand;
//! async fn f<X: PortClientEnv>(client: &mut PortClient<'_, X, 4, 4>) {
//!     let _ = client.lifecycle(LifecycleCommand::Disable).await;
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_ieee80211_upper_mac_service::client::{PortClient, PortClientEnv};
//! # use oer_ieee80211_lower_mac::Channel;
//! async fn f<X: PortClientEnv>(client: &mut PortClient<'_, X, 4, 4>, channel: Channel) {
//!     let _ = client.move_channel(channel).await;
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_ieee80211_upper_mac_service::client::{PortClient, PortClientEnv};
//! fn f<X: PortClientEnv>(client: &PortClient<'_, X, 4, 4>) {
//!     let _ = client.port();
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_ieee80211_upper_mac_service::client::{PortClient, PortClientEnv};
//! # use oer_ieee80211_lower_mac::LowerMacSetting;
//! fn f<X: PortClientEnv>(client: &PortClient<'_, X, 4, 4>, setting: LowerMacSetting) {
//!     let _ = client.apply(setting);
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_ieee80211_upper_mac_service::PortRouter;
//! # use oer_ieee80211_lower_mac::Ieee80211LowerMacPort;
//! fn f<P: Ieee80211LowerMacPort>(router: &PortRouter<'_, P>) {
//!     let _ = router.port();
//! }
//! ```

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

    /// The port, for this crate's own machinery (an absence window).
    pub(crate) fn port(&self) -> &'p P {
        self.router.port()
    }

    /// What the port supports.
    pub fn capabilities(&self) -> oer_ieee80211_lower_mac::LowerMacCapabilities {
        self.router.port().capabilities()
    }

    fn apply(&self, setting: LowerMacSetting) -> Result<(), PortClientError<P::Fault>> {
        self.router
            .port()
            .apply(setting)?
            .map_err(PortClientError::Setting)
    }

    /// The AIFSN and TXOP limits of the port's queues, which every
    /// interface shares (a client's planner takes the contention windows
    /// with `PortClient::contend_with`).
    pub fn install_edca(
        &self,
        parameters: oer_ieee80211_mac::extensions::wmm::WmmParameterSet,
    ) -> Result<(), PortClientError<P::Fault>> {
        self.apply(LowerMacSetting::Edca(parameters))
    }

    /// Open or close the transmit gate of every queue.
    pub fn set_tx_gate(&self, open: bool) -> Result<(), PortClientError<P::Fault>> {
        self.apply(LowerMacSetting::TxGate { open })
    }

    /// The coexistence priority the port receives beacons with.
    pub fn set_rx_beacon_priority(
        &self,
        priority: oer_ieee80211_lower_mac::RxBeaconPriority,
    ) -> Result<(), PortClientError<P::Fault>> {
        self.apply(LowerMacSetting::RxBeaconPriority(priority))
    }

    /// Delegate the beacon-receive coexistence priority to the client
    /// whose beacons it is, which changes it on time.
    pub fn grant_beacon_priority(&self) -> BeaconPriorityGrant<'p, P> {
        BeaconPriorityGrant {
            port: self.router.port(),
        }
    }

    /// Monitor reception of every frame the radio hears, for all
    /// interfaces.
    pub fn set_monitor(&self, enabled: bool) -> Result<(), PortClientError<P::Fault>>
    where
        P: oer_ieee80211_lower_mac::LowerMacMonitor,
    {
        self.router
            .port()
            .set_monitor(enabled)?
            .map_err(PortClientError::Setting)
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

/// The owner's delegation of one port-wide setting, the beacon-receive
/// coexistence priority, to the client whose beacons it is: the client
/// changes it at once, as its power manager's beacon window needs, and
/// nothing else of the port. Only [`PortOwner::grant_beacon_priority`]
/// makes one.
pub struct BeaconPriorityGrant<'p, P> {
    port: &'p P,
}

impl<P: Ieee80211LowerMacPort> BeaconPriorityGrant<'_, P> {
    /// The coexistence priority the port receives beacons with.
    pub fn set(
        &self,
        priority: oer_ieee80211_lower_mac::RxBeaconPriority,
    ) -> Result<(), PortClientError<P::Fault>> {
        self.port
            .apply(LowerMacSetting::RxBeaconPriority(priority))?
            .map_err(PortClientError::Setting)
    }
}
