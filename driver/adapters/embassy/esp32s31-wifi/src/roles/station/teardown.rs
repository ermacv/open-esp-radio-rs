//! Ordered connected-to-disconnected ESP32-S31 station transition.
//!
//! Interrupt publication and the staged protocol consumer must already be
//! quiesced by the platform executor. This transaction then owns the reusable
//! driver order: revoke association control state, stop RX DMA, prove TX idle,
//! recover descriptor/sequence resources and clear both association keys.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_embassy_net::PinnedTxFrame;
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{CcmpKeyHardware, StaCcmpClearReport, StaGroupCcmpSlot, clear_sta_ccmp_slots},
    rx::{RxDma, RxRingError},
};
use open_esp_radio_ieee80211::station::StaTxSequenceCounters;

use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::ConnectedTxSecurity;
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::WifiTxResources;

use crate::{
    datapath::rx::dma::{Esp32s31StagedRxProducer, Esp32s31StoppedRx},
    datapath::services::SingleRoleServices,
    datapath::tx::resources::AggregateTxResources,
    roles::station::control::{
        ConnectedControlError, ConnectedControlHardware, ConnectedControlShutdown,
        ConnectedControlTx, Esp32s31ConnectedControl,
    },
    roles::station::tx::{Esp32s31ConnectedTx, Esp32s31ConnectedTxTeardownParts},
};

/// RX owner already stopped by a wider physical composition.
///
/// STA+AP has one common DMA producer, so the paired boundary must stop it
/// before either logical role is dismantled. Wrapping that exact stopped
/// owner lets the ordinary STA teardown retain its control/TX/key ordering
/// without attempting to stop the hardware a second time.
pub struct Esp32s31AlreadyStoppedRx<R>(R);

impl<R> Esp32s31AlreadyStoppedRx<R> {
    pub const fn new(stopped: R) -> Self {
        Self(stopped)
    }

    pub fn into_inner(self) -> R {
        self.0
    }
}

impl<H, R> Esp32s31ConnectedStaRxTeardown<H> for Esp32s31AlreadyStoppedRx<R> {
    type Stopped = R;
    type Error = core::convert::Infallible;

    fn try_stop(self, _hardware: &mut H) -> Result<Self::Stopped, (Self, Self::Error)> {
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

/// RX-DMA stop capability used by the connected teardown port.
pub trait Esp32s31ConnectedStaRxTeardown<H>: Sized {
    type Stopped;
    type Error;

    fn try_stop(self, hardware: &mut H) -> Result<Self::Stopped, (Self, Self::Error)>;
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
> Esp32s31ConnectedStaRxTeardown<H>
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
    type Stopped = Esp32s31StoppedRx<
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
    >;
    type Error = RxRingError;

    fn try_stop(self, hardware: &mut H) -> Result<Self::Stopped, (Self, Self::Error)> {
        Esp32s31StagedRxProducer::try_stop(self, hardware)
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
        'resources,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
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
    pub stopped_rx: R,
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
        stopped_rx: S,
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
        Esp32s31ConnectedStaTeardownSuccess<H, R::Stopped, X::Resources, X::Aggregate, C::Report>,
        Esp32s31ConnectedStaTeardownFailure<H, R, R::Stopped, X, C, C::Error, R::Error>,
    >
    where
        H: CcmpKeyHardware,
        C: Esp32s31ConnectedStaControlTeardown<H, X>,
        R: Esp32s31ConnectedStaRxTeardown<H>,
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
        let stopped_rx = match rx.try_stop(&mut hardware) {
            Ok(stopped) => stopped,
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
                    stopped_rx,
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
        };
        drop(control);
        Ok(Esp32s31ConnectedStaTeardownSuccess {
            hardware,
            stopped_rx,
            tx_resources: returned_tx.resources,
            sequences: returned_tx.sequences,
            aggregate: returned_tx.aggregate,
            control: control_observation,
            security,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_hal::types::MacKeyInstallOutcome;
    use open_esp_radio_esp32s31_wifi_mac::crypto::{
        install_sta_group_ccmp, install_sta_pairwise_ccmp,
    };

    #[derive(Default)]
    struct Hardware {
        cleared: std::vec::Vec<u8>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(
            &mut self,
            _index: u8,
            _words: &[u32; 6],
        ) -> MacKeyInstallOutcome {
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, index: u8) {
            self.cleared.push(index);
        }
    }

    struct Control(bool);

    impl Esp32s31ConnectedStaControlTeardown<Hardware, Tx> for Control {
        type Report = u8;
        type Error = u8;

        fn shutdown(
            &mut self,
            _hardware: &mut Hardware,
            _tx: &mut Tx,
        ) -> Result<Self::Report, Self::Error> {
            if self.0 { Err(1) } else { Ok(2) }
        }
    }

    struct Rx(bool);

    impl Esp32s31ConnectedStaRxTeardown<Hardware> for Rx {
        type Stopped = u8;
        type Error = u8;

        fn try_stop(self, _hardware: &mut Hardware) -> Result<Self::Stopped, (Self, Self::Error)> {
            if self.0 { Err((self, 3)) } else { Ok(4) }
        }
    }

    struct Tx {
        active: bool,
        key: Option<open_esp_radio_esp32s31_wifi_mac::crypto::StaPairwiseCcmpSlot>,
    }

    impl Esp32s31ConnectedStaTxTeardown for Tx {
        type Resources = u8;
        type Aggregate = u8;

        fn try_return(
            mut self,
        ) -> Result<Esp32s31ConnectedTxTeardownParts<Self::Resources, Self::Aggregate>, Self>
        {
            if self.active {
                return Err(self);
            }
            Ok(Esp32s31ConnectedTxTeardownParts {
                resources: 5,
                pairwise_key: self.key.take().expect("test TX owns pairwise key"),
                sequences: StaTxSequenceCounters::new(6),
                aggregate: 7,
            })
        }
    }

    fn services(
        hardware: &mut Hardware,
        control_failure: bool,
        rx_failure: bool,
        tx_active: bool,
    ) -> (
        SingleRoleServices<Hardware, Rx, Tx, Control>,
        StaGroupCcmpSlot,
    ) {
        let pairwise =
            install_sta_pairwise_ccmp(hardware, [1, 2, 3, 4, 5, 6], &[0x11; 16]).unwrap();
        let group = install_sta_group_ccmp(hardware, 1, &[0x22; 16]).unwrap();
        (
            SingleRoleServices::with_control(
                core::mem::take(hardware),
                Rx(rx_failure),
                Tx {
                    active: tx_active,
                    key: Some(pairwise),
                },
                Control(control_failure),
            ),
            group,
        )
    }

    #[test]
    fn teardown_orders_control_rx_tx_and_both_key_clears() {
        let mut hardware = Hardware::default();
        let (services, group) = services(&mut hardware, false, false, false);
        let stopped = Esp32s31ConnectedStaTeardownPort::try_teardown(services, group)
            .unwrap_or_else(|_| panic!("idle mock owners must stop"));
        assert_eq!(stopped.stopped_rx, 4);
        assert_eq!(stopped.tx_resources, 5);
        assert_eq!(stopped.sequences.peek_non_qos(), 6);
        assert_eq!(stopped.aggregate, 7);
        assert_eq!(stopped.control, 2);
        assert_eq!(stopped.hardware.cleared, [1, 4]);
    }

    #[test]
    fn already_stopped_rx_crosses_teardown_without_a_second_hardware_stop() {
        let stopped = Esp32s31AlreadyStoppedRx::new(9_u8);
        let returned =
            <Esp32s31AlreadyStoppedRx<u8> as Esp32s31ConnectedStaRxTeardown<Hardware>>::try_stop(
                stopped,
                &mut Hardware::default(),
            )
            .unwrap_or_else(|_| unreachable!("already-stopped RX is infallible"));

        assert_eq!(returned, 9);
    }

    #[test]
    fn each_failed_stage_returns_its_exact_frontier() {
        for (control, rx, tx, expected) in [
            (true, false, false, 1),
            (false, true, false, 2),
            (false, false, true, 3),
        ] {
            let mut hardware = Hardware::default();
            let (services, group) = services(&mut hardware, control, rx, tx);
            let failure = Esp32s31ConnectedStaTeardownPort::try_teardown(services, group)
                .err()
                .expect("selected stage must fail");
            let observed = match failure {
                Esp32s31ConnectedStaTeardownFailure::Control { error, .. } => {
                    assert_eq!(error, 1);
                    1
                }
                Esp32s31ConnectedStaTeardownFailure::Rx { error, .. } => {
                    assert_eq!(error, 3);
                    2
                }
                Esp32s31ConnectedStaTeardownFailure::TxActive { .. } => 3,
            };
            assert_eq!(observed, expected);
        }
    }
}
