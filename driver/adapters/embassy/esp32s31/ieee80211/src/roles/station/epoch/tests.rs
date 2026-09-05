use super::*;

struct TestDelay;

impl Esp32s31RxFrontierDelay for TestDelay {
    async fn after_micros(_micros: u32) {}
}

struct StoppedRx(u8);

impl Esp32s31StoppedStaRx for StoppedRx {
    type Preconnected<D>
        = (u8, PhantomData<D>)
    where
        D: Esp32s31RxFrontierDelay;
    type Persistent = u16;

    fn split_for_reconnect<D>(self) -> (Self::Preconnected<D>, Self::Persistent)
    where
        D: Esp32s31RxFrontierDelay,
    {
        ((self.0, PhantomData), u16::from(self.0) + 100)
    }
}

#[test]
fn running_scan_round_trip_and_reconnect_preserve_every_owner() {
    let disconnected = Esp32s31DisconnectedStaEpoch::new("network", "hardware", StoppedRx(7), 8, 9);
    let scan = disconnected.into_running_scan_parts();
    assert_eq!(scan.hardware, "hardware");
    assert_eq!(scan.rx.0, 7);

    let disconnected = scan.retained.restore(scan.hardware, scan.rx);
    assert_eq!(disconnected.hardware(), &"hardware");
    assert_eq!(disconnected.rx().0, 7);

    let (network, reconnected) = disconnected.prepare_reconnect::<TestDelay>();
    assert_eq!(network, "network");
    let parts = reconnected.into_parts();
    assert_eq!(parts.hardware, "hardware");
    assert_eq!(parts.rx.0, 7);
    assert_eq!(parts.rx_resources, 107);
    assert_eq!(parts.aggregate_tx, 8);
    assert_eq!(parts.control, 9);
}
