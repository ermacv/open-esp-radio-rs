//! Ordered connected-to-disconnected ESP32-S31 station transition.
//!
//! Interrupt publication and the staged protocol consumer must already be
//! quiesced by the platform executor. This transaction then owns the reusable
//! driver order: revoke association control state, stop RX DMA, prove TX idle,
//! recover descriptor/sequence resources and clear both association keys.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{
        CcmpKeyHardware, StaCcmpClearReport, StaGroupCcmpKeyMaterial, StaGroupCcmpSlot,
        clear_sta_ccmp_slots,
    },
    rx::{RxDma, RxRingError},
};
use open_esp_radio_ieee80211::station::StaTxSequenceCounters;

use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::StaCcmpRxReplayControlEndpoint;
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::ConnectedTxSecurity;
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::WifiTxResources;

use crate::{
    datapath::PinnedTxFrame,
    datapath::rx::dma::Esp32s31StagedRxProducer,
    datapath::services::SingleRoleServices,
    datapath::tx::resources::AggregateTxResources,
    roles::station::control::{
        ConnectedControlError, ConnectedControlHardware, ConnectedControlShutdown,
        ConnectedControlTx, Esp32s31ConnectedControl,
    },
    roles::station::tx::{Esp32s31ConnectedTx, Esp32s31ConnectedTxTeardownParts},
};

/// RX owner already parked by a wider physical composition.
///
/// STA+AP has one common DMA producer, so the paired boundary must stop it
/// before either logical role is dismantled. Wrapping that exact stopped
/// owner lets the ordinary STA teardown retain its control/TX/key ordering
/// without attempting to stop the hardware a second time.
pub struct Esp32s31AlreadyParkedRx<R>(R);

impl<R> Esp32s31AlreadyParkedRx<R> {
    pub const fn new(parked: R) -> Self {
        Self(parked)
    }

    pub fn into_inner(self) -> R {
        self.0
    }
}

impl<H, R> Esp32s31ConnectedStaRxPark<H> for Esp32s31AlreadyParkedRx<R> {
    type Parked = R;
    type Error = core::convert::Infallible;

    fn try_park(self, _hardware: &mut H) -> Result<Self::Parked, (Self, Self::Error)> {
        Ok(self.0)
    }
}

/// Control-plane shutdown capability used by the connected teardown port.
pub trait Esp32s31ConnectedStaControlTeardown<H, X> {
    type Report;
    type Error;

    fn shutdown(&mut self, hardware: &mut H, tx: &mut X) -> Result<Self::Report, Self::Error>;
}

impl<'resources, M, H, X, const CAPACITY: usize> Esp32s31ConnectedStaControlTeardown<H, X>
    for Esp32s31ConnectedControl<'resources, M, CAPACITY>
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx,
{
    type Report = ConnectedControlShutdown;
    type Error = ConnectedControlError;

    fn shutdown(&mut self, hardware: &mut H, tx: &mut X) -> Result<Self::Report, Self::Error> {
        Esp32s31ConnectedControl::shutdown(self, hardware, tx)
    }
}

impl<
    'resources,
    M,
    H,
    X,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> Esp32s31ConnectedStaControlTeardown<H, X>
    for crate::roles::station::esp_now_tx::Esp32s31EspNowConnectedControl<
        'resources,
        M,
        CONTROL_CAPACITY,
        TX_CAPACITY,
        PEERS,
    >
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx,
{
    type Report = crate::roles::station::esp_now_tx::Esp32s31EspNowConnectedControlShutdown<PEERS>;
    type Error = crate::roles::station::esp_now_tx::Esp32s31EspNowConnectedControlError;

    fn shutdown(&mut self, hardware: &mut H, tx: &mut X) -> Result<Self::Report, Self::Error> {
        crate::roles::station::esp_now_tx::Esp32s31EspNowConnectedControl::shutdown(
            self, hardware, tx,
        )
    }
}

/// Logical RX parking capability used by the connected teardown port.
///
/// Parking ends peer/protocol ownership but deliberately leaves the physical
/// DMA walker live for the next Wi-Fi role.
pub trait Esp32s31ConnectedStaRxPark<H>: Sized {
    type Parked;
    type Error;

    fn try_park(self, hardware: &mut H) -> Result<Self::Parked, (Self, Self::Error)>;
}

impl<
    'storage,
    'pool,
    'queue,
    H,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
> Esp32s31ConnectedStaRxPark<H>
    for Esp32s31StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
    >
where
    H: RxDma,
{
    type Parked = Self;
    type Error = RxRingError;

    fn try_park(self, _hardware: &mut H) -> Result<Self::Parked, (Self, Self::Error)> {
        if self.can_park_for_role_handoff() {
            Ok(self)
        } else {
            Err((self, RxRingError::Busy))
        }
    }
}

/// Idle connected-TX decomposition used by the connected teardown port.
pub trait Esp32s31ConnectedStaTxTeardown: Sized {
    type Resources;
    type Aggregate;

    fn try_return(
        self,
    ) -> Result<Esp32s31ConnectedTxTeardownParts<Self::Resources, Self::Aggregate>, Self>;
}

impl<
    'slot,
    'ampdu,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> Esp32s31ConnectedStaTxTeardown
    for Esp32s31ConnectedTx<
        'slot,
        'ampdu,
        crate::datapath::PinnedTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
        P,
        E,
        T,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    type Resources = WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>;
    type Aggregate = AggregateTxResources<
        'ampdu,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        SLOTS,
        AMPDU_BUFFER_SIZE,
    >;

    fn try_return(
        self,
    ) -> Result<Esp32s31ConnectedTxTeardownParts<Self::Resources, Self::Aggregate>, Self> {
        self.try_into_station_parts()
    }
}

/// Complete successful driver frontier after one connected epoch.
pub struct Esp32s31ConnectedStaTeardownSuccess<H, R, T, A, C> {
    pub hardware: H,
    pub parked_rx: R,
    pub tx_resources: T,
    pub sequences: StaTxSequenceCounters,
    pub aggregate: A,
    pub control: C,
    pub security: Esp32s31ConnectedStaSecurityStopReport,
}

/// Group-key ownership retained outside the connected ordinary TX owner.
pub enum Esp32s31ConnectedStaGroupSecurity {
    Open,
    Wpa2Personal(StaGroupCcmpSlot),
    /// Pre-control owner retaining the secret rollback key and replay-control
    /// endpoint beside the hardware slot. Once installed into connected
    /// control, shutdown returns the ordinary slot-only variant above.
    Wpa2PersonalRekey {
        group: StaGroupCcmpSlot,
        material: StaGroupCcmpKeyMaterial,
        replay: StaCcmpRxReplayControlEndpoint,
    },
}

/// Observable result of the security teardown edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ConnectedStaSecurityStopReport {
    OpenNoKeys,
    Wpa2Personal(StaCcmpClearReport),
    /// A composition bug supplied unlike pairwise/group modes. Every token
    /// that did exist was still cleared before this report was returned.
    ModeMismatchCleared {
        pairwise_hardware_index: Option<u8>,
        group_hardware_index: Option<u8>,
    },
}

/// Owner-preserving failure at the exact teardown stage that could not
/// complete. A board-level fault policy can retain these values without
/// guessing which hardware frontier remains live.
pub enum Esp32s31ConnectedStaTeardownFailure<H, R, S, X, C, CE, RE> {
    Control {
        error: CE,
        services: SingleRoleServices<H, R, X, C>,
        group_security: Esp32s31ConnectedStaGroupSecurity,
    },
    Rx {
        error: RE,
        hardware: H,
        rx: R,
        tx: X,
        control: C,
        group_security: Esp32s31ConnectedStaGroupSecurity,
    },
    TxActive {
        hardware: H,
        parked_rx: S,
        tx: X,
        control: C,
        group_security: Esp32s31ConnectedStaGroupSecurity,
    },
}

pub struct Esp32s31ConnectedStaTeardownPort;

impl Esp32s31ConnectedStaTeardownPort {
    /// Consume stopped connected services and return the peer-independent
    /// station owners in the only hardware-safe order.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_teardown<H, R, X, C>(
        services: SingleRoleServices<H, R, X, C>,
        group_security: Esp32s31ConnectedStaGroupSecurity,
    ) -> Result<
        Esp32s31ConnectedStaTeardownSuccess<H, R::Parked, X::Resources, X::Aggregate, C::Report>,
        Esp32s31ConnectedStaTeardownFailure<H, R, R::Parked, X, C, C::Error, R::Error>,
    >
    where
        H: CcmpKeyHardware,
        C: Esp32s31ConnectedStaControlTeardown<H, X>,
        R: Esp32s31ConnectedStaRxPark<H>,
        X: Esp32s31ConnectedStaTxTeardown,
    {
        let (mut hardware, rx, mut tx, mut control) = services.into_parts();
        let control_observation = match control.shutdown(&mut hardware, &mut tx) {
            Ok(report) => report,
            Err(error) => {
                return Err(Esp32s31ConnectedStaTeardownFailure::Control {
                    error,
                    services: SingleRoleServices::with_control(hardware, rx, tx, control),
                    group_security,
                });
            }
        };
        let parked_rx = match rx.try_park(&mut hardware) {
            Ok(parked) => parked,
            Err((rx, error)) => {
                return Err(Esp32s31ConnectedStaTeardownFailure::Rx {
                    error,
                    hardware,
                    rx,
                    tx,
                    control,
                    group_security,
                });
            }
        };
        let returned_tx = match tx.try_return() {
            Ok(returned) => returned,
            Err(tx) => {
                return Err(Esp32s31ConnectedStaTeardownFailure::TxActive {
                    hardware,
                    parked_rx,
                    tx,
                    control,
                    group_security,
                });
            }
        };
        let security = match (returned_tx.security, group_security) {
            (ConnectedTxSecurity::Open, Esp32s31ConnectedStaGroupSecurity::Open) => {
                Esp32s31ConnectedStaSecurityStopReport::OpenNoKeys
            }
            (
                ConnectedTxSecurity::Wpa2Personal(pairwise),
                Esp32s31ConnectedStaGroupSecurity::Wpa2Personal(group),
            ) => Esp32s31ConnectedStaSecurityStopReport::Wpa2Personal(clear_sta_ccmp_slots(
                &mut hardware,
                pairwise,
                group,
            )),
            (
                ConnectedTxSecurity::Wpa2Personal(pairwise),
                Esp32s31ConnectedStaGroupSecurity::Wpa2PersonalRekey { group, .. },
            ) => Esp32s31ConnectedStaSecurityStopReport::Wpa2Personal(clear_sta_ccmp_slots(
                &mut hardware,
                pairwise,
                group,
            )),
            (
                ConnectedTxSecurity::Wpa2Personal(pairwise),
                Esp32s31ConnectedStaGroupSecurity::Open,
            ) => {
                let pairwise_hardware_index = pairwise.hardware_index();
                pairwise.clear(&mut hardware);
                Esp32s31ConnectedStaSecurityStopReport::ModeMismatchCleared {
                    pairwise_hardware_index: Some(pairwise_hardware_index),
                    group_hardware_index: None,
                }
            }
            (ConnectedTxSecurity::Open, Esp32s31ConnectedStaGroupSecurity::Wpa2Personal(group)) => {
                let group_hardware_index = group.hardware_index();
                group.clear(&mut hardware);
                Esp32s31ConnectedStaSecurityStopReport::ModeMismatchCleared {
                    pairwise_hardware_index: None,
                    group_hardware_index: Some(group_hardware_index),
                }
            }
            (
                ConnectedTxSecurity::Open,
                Esp32s31ConnectedStaGroupSecurity::Wpa2PersonalRekey { group, .. },
            ) => {
                let group_hardware_index = group.hardware_index();
                group.clear(&mut hardware);
                Esp32s31ConnectedStaSecurityStopReport::ModeMismatchCleared {
                    pairwise_hardware_index: None,
                    group_hardware_index: Some(group_hardware_index),
                }
            }
        };
        drop(control);
        Ok(Esp32s31ConnectedStaTeardownSuccess {
            hardware,
            parked_rx,
            tx_resources: returned_tx.resources,
            sequences: returned_tx.sequences,
            aggregate: returned_tx.aggregate,
            control: control_observation,
            security,
        })
    }
}

#[cfg(test)]
mod tests;
