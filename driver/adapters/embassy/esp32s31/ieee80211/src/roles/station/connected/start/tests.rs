use embassy_futures::block_on;
use open_esp_radio_esp32s31_hal::Radio;

use crate::roles::station::epoch::{Esp32s31DisconnectedStaEpoch, Esp32s31StoppedStaRx};

use super::*;

struct TestRxResources {
    value: u8,
    fail: bool,
}

struct TestRadioPeripheral;

impl<'arena> Esp32s31ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, u8>
    for TestRxResources
{
    type Connected = (u8, u8);
    type Error = u8;

    async fn materialize(
        self,
        receive: u8,
        _hardware: &mut CooperativeRadioHardware<'arena>,
    ) -> Result<Self::Connected, (u8, Self, Self::Error)> {
        if self.fail {
            Err((receive, self, 99))
        } else {
            Ok((receive, self.value))
        }
    }
}

struct TestRxFrontier;
struct TestDelay;

impl Esp32s31RxFrontierDelay for TestDelay {
    async fn after_micros(_micros: u32) {}
}

impl Esp32s31StoppedStaRx for TestRxFrontier {
    type Preconnected<D>
        = u8
    where
        D: Esp32s31RxFrontierDelay;
    type Persistent = TestRxResources;

    fn split_for_reconnect<D>(self) -> (Self::Preconnected<D>, Self::Persistent)
    where
        D: Esp32s31RxFrontierDelay,
    {
        (
            17,
            TestRxResources {
                value: 18,
                fail: false,
            },
        )
    }
}

#[test]
fn connected_start_unifies_initial_and_reconnected_owner_frontiers() {
    let radio = Radio::claim(TestRadioPeripheral)
        .unwrap_or_else(|_| panic!("radio singleton must be free for connected-start test"));
    let radio = radio.assume_powered_for_validation().into_running();
    let (_platform, registers, _interrupt_setup) = radio.into_runtime_parts();
    let arena = Esp32s31RadioOwnerArena::new();

    let started = block_on(start_esp32s31_initial_connected_epoch(
        registers,
        7,
        Esp32s31InitialConnectedEpochResources::new(
            &arena,
            TestRxResources {
                value: 8,
                fail: false,
            },
            9_u16,
            10_u32,
        ),
    ))
    .unwrap_or_else(|_| panic!("initial owner transition must succeed"));
    assert_eq!(started.rx, (7, 8));
    assert_eq!(started.aggregate_tx, 9);
    assert_eq!(started.control, 10);
    let reclaimed = started
        .hardware
        .try_into_reclaimed_registers()
        .unwrap_or_else(|_| {
            panic!("initial transition must return the PAC owner and arena binding")
        });
    let published = reclaimed
        .try_republish()
        .unwrap_or_else(|_| panic!("reconnected test must use the exact returned arena binding"));
    let disconnected = Esp32s31DisconnectedStaEpoch::new(
        (),
        CooperativeRadioHardware::new(published),
        TestRxFrontier,
        19_u16,
        20_u32,
    );
    let (_, reconnected) = disconnected.prepare_reconnect::<TestDelay>();
    let started = block_on(start_esp32s31_reconnected_connected_epoch(reconnected))
        .unwrap_or_else(|_| panic!("reconnected owner transition must succeed"));
    assert_eq!(started.rx, (17, 18));
    assert_eq!(started.aggregate_tx, 19);
    assert_eq!(started.control, 20);
    let registers = started
        .hardware
        .try_into_reclaimed_registers()
        .unwrap_or_else(|_| panic!("reconnected transition must retain its arena binding"))
        .into_owner();

    let failure = block_on(start_esp32s31_initial_connected_epoch(
        registers,
        21,
        Esp32s31InitialConnectedEpochResources::new(
            &arena,
            TestRxResources {
                value: 22,
                fail: true,
            },
            23_u16,
            24_u32,
        ),
    ))
    .err()
    .unwrap_or_else(|| panic!("RX promotion failure must return every owner"));
    match failure {
        Esp32s31ConnectedEpochStartFailure::Receive {
            phase,
            error,
            hardware,
            receive,
            rx_resources,
            aggregate_tx,
            control,
        } => {
            assert_eq!(phase, Esp32s31ConnectedEpochStartPhase::Initial);
            assert_eq!(error, 99);
            assert_eq!(receive, 21);
            assert_eq!(rx_resources.value, 22);
            assert_eq!(aggregate_tx, 23);
            assert_eq!(control, 24);
            let _registers = hardware.try_into_registers().unwrap_or_else(|_| {
                panic!("failed RX promotion must retain the published PAC owner")
            });
        }
        Esp32s31ConnectedEpochStartFailure::RegisterPublication { .. } => {
            panic!("empty arena must accept the returned PAC owner")
        }
    }
}
