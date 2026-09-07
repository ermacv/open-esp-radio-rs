//! Startup diagnostics borrow failures so their hardware owners remain retained.

use oer_esp32s31_bluetooth_integration::BluetoothColdStartError;

type Error = BluetoothColdStartError<
    { super::MODEM_TIMER_CAPACITY },
    { super::SCHEDULER_CAPACITY },
    { super::HOST_TO_CONTROLLER_DEPTH },
    { super::CONTROLLER_TO_HOST_DEPTH },
    { super::PACKET_CAPACITY },
>;

pub(super) fn fail(error: &Error) -> ! {
    macro_rules! failure {
        ($stage:literal, $cause:expr) => {
            panic!(concat!("Bluetooth cold start: ", $stage, ": {:?}"), $cause)
        };
    }
    match error {
        Error::Timebase { error, .. } => failure!("timebase", error),
        Error::PlatformBusy { error, .. } => failure!("platform", error),
        Error::HciConfig { error, .. } => failure!("HCI config", error),
        Error::HciResources { error, .. } => failure!("HCI resources", error),
        Error::CalibrationSnapshotSchema { observed, .. } => failure!("snapshot schema", observed),
        Error::StorageInUse { error, .. } => failure!("storage reservation", error),
        Error::BlePhyMemory(f) => failure!("BLE PHY memory", f.failure().error),
        Error::DirectionFindingMemory(f) => failure!("direction finding memory", f.failure().error),
        Error::DtmMemory(f) => failure!("DTM memory", f.failure().error),
        Error::LegacyAdvertisingMemory(f) => failure!("advertising memory", f.failure().error),
        Error::PassiveScanMemory(f) => failure!("scan memory", f.failure().error),
        Error::PeripheralConnectionMemory(f) => failure!("connection memory", f.failure().error),
        Error::LegacyConnectableAdvertisingMemory(f) => {
            failure!("connectable advertising memory", f.failure().error)
        }
        Error::Clock(f) => failure!("clock", f.failure().failure.error()),
        Error::LowPower(f) => failure!("low power", f.failure().failure.error()),
        Error::PhyInitialization(f) => failure!("PHY registration", f.failure().failure.error()),
        Error::PhyClientAcquire(f) => failure!("PHY client", f.failure().failure.error()),
        Error::PhyTracking(f) => failure!("PHY tracking", f.failure().failure.error()),
        Error::RecheckStart(f) => failure!("time recheck", f.failure().error()),
        Error::HciBind(f) => failure!("HCI binding", f.failure().error()),
        Error::InterruptPublication(f) => failure!("interrupt publication", f.failure().error()),
        Error::SystemBuild(_) => panic!("Bluetooth cold start: final system composition failed"),
    }
}
