//! Ordered connected-to-disconnected ESP32-S31 station transition.
//!
//! Interrupt publication and the staged protocol consumer must already be
//! quiesced by the platform executor. This transaction then owns the reusable
//! driver order: revoke association control state, stop RX DMA, prove TX idle,
//! recover descriptor/sequence resources and clear both association keys.

use core::pin::Pin;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{CcmpKeyHardware, StaCcmpClearReport, StaGroupCcmpSlot, clear_sta_ccmp_slots},
    rx::{RxDma, RxRingError},
    tx_ampdu::HtAmpduTxStorage,
};
use open_esp_radio_ieee80211::station::StaTxSequenceCounters;

use crate::{
    aggregate_tx::{Esp32s31ConnectedTx, Esp32s31ConnectedTxTeardownParts},
    backend::Esp32s31WifiBackend,
    connected_control::{
        ConnectedControlError, ConnectedControlHardware, ConnectedControlShutdown,
        ConnectedControlTx, Esp32s31ConnectedControl,
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    rx_backend::{Esp32s31ConnectedRx, Esp32s31StoppedRx},
    single_mpdu_tx::WifiTxResources,
};

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
> Esp32s31ConnectedStaRxTeardown<H>
    for Esp32s31ConnectedRx<
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
        Esp32s31ConnectedRx::try_stop(self, hardware)
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
    type Aggregate = Pin<&'ampdu mut HtAmpduTxStorage<SLOTS, AMPDU_BUFFER_SIZE>>;

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
    pub keys: StaCcmpClearReport,
}

/// Owner-preserving failure at the exact teardown stage that could not
/// complete. A reset policy can consume these values without guessing which
/// hardware frontier remains live.
pub enum Esp32s31ConnectedStaTeardownFailure<H, R, S, X, C, CE, RE> {
    Control {
        error: CE,
        backend: Esp32s31WifiBackend<H, R, X, C>,
        group_key: StaGroupCcmpSlot,
    },
    Rx {
        error: RE,
        hardware: H,
        rx: R,
        tx: X,
        control: C,
        group_key: StaGroupCcmpSlot,
    },
    TxActive {
        hardware: H,
        stopped_rx: S,
        tx: X,
        control: C,
        group_key: StaGroupCcmpSlot,
    },
}

pub struct Esp32s31ConnectedStaTeardownPort;

impl Esp32s31ConnectedStaTeardownPort {
    /// Consume a stopped runner backend and return the peer-independent
    /// station owners in the only hardware-safe order.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_teardown<H, R, X, C>(
        backend: Esp32s31WifiBackend<H, R, X, C>,
        group_key: StaGroupCcmpSlot,
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
        let (mut hardware, rx, mut tx, mut control) = backend.into_parts();
        let control_report = match control.shutdown(&mut hardware, &mut tx) {
            Ok(report) => report,
            Err(error) => {
                return Err(Esp32s31ConnectedStaTeardownFailure::Control {
                    error,
                    backend: Esp32s31WifiBackend::with_control(hardware, rx, tx, control),
                    group_key,
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
                    group_key,
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
                    group_key,
                });
            }
        };
        let keys = clear_sta_ccmp_slots(&mut hardware, returned_tx.pairwise_key, group_key);
        drop(control);
        Ok(Esp32s31ConnectedStaTeardownSuccess {
            hardware,
            stopped_rx,
            tx_resources: returned_tx.resources,
            sequences: returned_tx.sequences,
            aggregate: returned_tx.aggregate,
            control: control_report,
            keys,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_pac::MacKeyInstallOutcome;
    use open_esp_radio_esp32s31_wifi_mac::crypto::{
        install_sta_group_ccmp, install_sta_pairwise_ccmp,
    };

    #[derive(Default)]
    struct Hardware {
        cleared: std::vec::Vec<u8>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
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

    fn backend(
        hardware: &mut Hardware,
        control_failure: bool,
        rx_failure: bool,
        tx_active: bool,
    ) -> (
        Esp32s31WifiBackend<Hardware, Rx, Tx, Control>,
        StaGroupCcmpSlot,
    ) {
        let pairwise =
            install_sta_pairwise_ccmp(hardware, [1, 2, 3, 4, 5, 6], &[0x11; 16]).unwrap();
        let group = install_sta_group_ccmp(hardware, 1, &[0x22; 16]).unwrap();
        (
            Esp32s31WifiBackend::with_control(
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
        let (backend, group) = backend(&mut hardware, false, false, false);
        let stopped = Esp32s31ConnectedStaTeardownPort::try_teardown(backend, group)
            .unwrap_or_else(|_| panic!("idle mock owners must stop"));
        assert_eq!(stopped.stopped_rx, 4);
        assert_eq!(stopped.tx_resources, 5);
        assert_eq!(stopped.sequences.peek_non_qos(), 6);
        assert_eq!(stopped.aggregate, 7);
        assert_eq!(stopped.control, 2);
        assert_eq!(stopped.hardware.cleared, [1, 4]);
    }

    #[test]
    fn each_failed_stage_returns_its_exact_frontier() {
        for (control, rx, tx, expected) in [
            (true, false, false, 1),
            (false, true, false, 2),
            (false, false, true, 3),
        ] {
            let mut hardware = Hardware::default();
            let (backend, group) = backend(&mut hardware, control, rx, tx);
            let failure = Esp32s31ConnectedStaTeardownPort::try_teardown(backend, group)
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
