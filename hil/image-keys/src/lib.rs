#![no_std]
//! The keys a HIL image reports, as a function of the Cargo
//! features it was built with.
//!
//! The runtime pages this set out for `base/image-keys/get`, and the host
//! derives the set each image class reports from the same function applied
//! to the class's features and the runtime manifest's feature graph. The two
//! cannot drift apart: the host identifies a flashed image by this set.
//!
//! The set holds the key of every endpoint the image serves, of every message
//! it sends (an endpoint's response changes its key without changing the
//! request's) and of every property it has.

use oer_hil_protocol::{Key, Message, base, bluetooth, ieee802154, network, phy, system};
use oer_hil_protocol::{telemetry, wifi};

/// The most keys one image reports.
pub const IMAGE_KEYS: usize = 80;

/// An image's keys, in the ascending order capability pages require.
pub type ImageKeys = heapless::Vec<Key, IMAGE_KEYS>;

/// What every image serves: discovery, rejection, boot and link health.
const BASE: [Key; 11] = [
    base::Hello::KEY,
    base::GetHello::KEY,
    base::GetImageKeys::KEY,
    base::ImageKeyPage::KEY,
    base::Rejected::KEY,
    base::GetBootStatus::KEY,
    base::BootEvidence::KEY,
    base::GetPostMortemCheckpoints::KEY,
    base::PostMortemCheckpoints::KEY,
    base::GetLinkHealth::KEY,
    base::LinkHealth::KEY,
];

/// The keys of a runtime image whose Cargo features are those for which
/// `enabled` is true.
pub fn image_keys(enabled: &dyn Fn(&str) -> bool) -> ImageKeys {
    let mut keys = ImageKeys::new();
    let mut add = |key: Key, present: bool| {
        if present {
            keys.push(key)
                .expect("an image reports at most IMAGE_KEYS keys");
        }
    };
    for key in BASE {
        add(key, true);
    }
    if enabled("system-watchdog") {
        add(system::WatchdogTest::KEY, true);
        add(system::WatchdogArmed::KEY, true);
        return sorted(keys);
    }
    if enabled("system-panic-reset") {
        add(system::InjectPanic::KEY, true);
        add(system::PanicInjected::KEY, true);
        return sorted(keys);
    }
    if enabled("bluetooth-radio") && !enabled("open-radio-hil") {
        if enabled("bluetooth-secure-gatt") {
            add(bluetooth::SecureGatt::KEY, true);
        } else if enabled("bluetooth-gatt") {
            add(bluetooth::Gatt::KEY, true);
        } else {
            add(bluetooth::Dtm::KEY, true);
            add(bluetooth::Hci::KEY, true);
            add(phy::RxHotSram::KEY, enabled("phy-rx-hot-sram"));
        }
        return sorted(keys);
    }
    let traffic = !enabled("memory-benchmark");
    add(bluetooth::Gatt::KEY, enabled("wifi-ble-coex"));
    add(phy::FaultInjection::KEY, enabled("phy-fault-injection"));
    add(
        phy::RegisterImage::KEY,
        enabled("open-radio-hil") && traffic,
    );
    add(phy::StartupArtifact::KEY, traffic);
    add(phy::RxHotSram::KEY, enabled("phy-rx-hot-sram"));
    for key in [
        network::Udp::KEY,
        network::Tcp::KEY,
        network::Rx::KEY,
        network::Tx::KEY,
        network::Bidirectional::KEY,
        network::RuntimeConfiguration::KEY,
        network::UdpMultiFlow::KEY,
        network::DataPlanePlacement::KEY,
        wifi::RuntimeInitialization::KEY,
        wifi::StationEpochControl::KEY,
        wifi::RoleControl::KEY,
        wifi::AccessPoint::KEY,
        wifi::StationAccessPoint::KEY,
        wifi::MonitorCapture::KEY,
        wifi::StationLifecycleEvents::KEY,
    ] {
        add(key, traffic);
    }
    add(wifi::DriverObservation::KEY, enabled("driver-observation"));
    add(
        network::RxDeliveryReport::KEY,
        enabled("rx-delivery-telemetry"),
    );
    add(
        telemetry::RxOwnership::KEY,
        enabled("rx-ownership-telemetry"),
    );
    add(telemetry::PcProfile::KEY, enabled("pc-profile"));
    add(telemetry::RxClock::KEY, enabled("rx-clock-probe"));
    add(
        telemetry::StationExit::KEY,
        enabled("station-exit-evidence") && !enabled("driver-observation"),
    );
    add(
        telemetry::TaskPoll::KEY,
        enabled("connected-datapath-poll-telemetry"),
    );
    add(
        wifi::TxArchitectureProbe::KEY,
        enabled("tx-architecture-probes"),
    );
    add(
        telemetry::Core0RxCycles::KEY,
        enabled("core0-rx-cycle-telemetry") || enabled("core0-rx-coarse-telemetry"),
    );
    add(telemetry::MacIrq::KEY, enabled("mac-irq-telemetry"));
    add(system::TimebaseProbe::KEY, true);
    add(system::IpcCall::KEY, enabled("open-radio-hil"));
    add(
        system::SourceGate::KEY,
        enabled("open-radio-hil") && traffic,
    );
    add(system::MemoryBenchmark::KEY, !traffic);
    add(
        ieee802154::EventStatusProbe::KEY,
        enabled("ieee802154-event-status-probe"),
    );
    add(
        ieee802154::EdEventProbe::KEY,
        enabled("ieee802154-ed-event-probe"),
    );
    add(ieee802154::AirCheck::KEY, enabled("ieee802154-radio"));
    add(ieee802154::Session::KEY, enabled("ieee802154-radio"));
    add(ieee802154::Thread::KEY, enabled("ieee802154-thread"));
    add(
        ieee802154::RouteProbe::KEY,
        enabled("ieee802154-route-probe"),
    );
    add(ieee802154::MacTrace::KEY, enabled("ieee802154-trace"));
    sorted(keys)
}

fn sorted(mut keys: ImageKeys) -> ImageKeys {
    keys.sort_unstable();
    keys
}

/// Every Cargo feature [`image_keys`] reads, so the host can check that the
/// runtime manifest declares each of them.
pub const READ_FEATURES: &[&str] = &[
    "system-watchdog",
    "system-panic-reset",
    "bluetooth-radio",
    "open-radio-hil",
    "bluetooth-secure-gatt",
    "bluetooth-gatt",
    "phy-rx-hot-sram",
    "memory-benchmark",
    "wifi-ble-coex",
    "phy-fault-injection",
    "driver-observation",
    "rx-delivery-telemetry",
    "rx-ownership-telemetry",
    "pc-profile",
    "rx-clock-probe",
    "station-exit-evidence",
    "connected-datapath-poll-telemetry",
    "tx-architecture-probes",
    "core0-rx-cycle-telemetry",
    "core0-rx-coarse-telemetry",
    "mac-irq-telemetry",
    "ieee802154-event-status-probe",
    "ieee802154-ed-event-probe",
    "ieee802154-radio",
    "ieee802154-thread",
    "ieee802154-route-probe",
    "ieee802154-trace",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn with(features: &[&str]) -> ImageKeys {
        image_keys(&|feature| features.contains(&feature))
    }

    fn has<M: Message>(keys: &ImageKeys) -> bool {
        keys.contains(&M::KEY)
    }

    #[test]
    fn the_family_follows_the_image_s_features() {
        let watchdog = with(&["system-watchdog"]);
        assert!(has::<system::WatchdogTest>(&watchdog) && !has::<network::Udp>(&watchdog));
        let panic = with(&["system-panic-reset"]);
        assert!(has::<system::InjectPanic>(&panic) && !has::<system::WatchdogTest>(&panic));
        let dtm = with(&["bluetooth-radio"]);
        assert!(has::<bluetooth::Dtm>(&dtm) && has::<bluetooth::Hci>(&dtm));
        assert!(!has::<network::Udp>(&dtm));
        assert!(has::<bluetooth::Gatt>(&with(&[
            "bluetooth-radio",
            "bluetooth-gatt"
        ])));
        assert!(has::<bluetooth::SecureGatt>(&with(&[
            "bluetooth-radio",
            "bluetooth-secure-gatt"
        ])));
        // The coexistence image is a Wi-Fi image with the GATT application.
        let coex = with(&["open-radio-hil", "bluetooth-radio", "wifi-ble-coex"]);
        assert!(has::<bluetooth::Gatt>(&coex) && has::<network::Udp>(&coex));
        assert!(!has::<bluetooth::Dtm>(&coex));
    }

    #[test]
    fn every_image_serves_the_base_module() {
        for features in [
            &["system-watchdog"][..],
            &["system-panic-reset"],
            &["bluetooth-radio"],
            &["open-radio-hil"],
        ] {
            let keys = with(features);
            assert!(BASE.iter().all(|key| keys.contains(key)), "{features:?}");
        }
    }

    #[test]
    fn a_memory_benchmark_image_offers_no_traffic() {
        let benchmark = with(&["open-radio-hil", "memory-benchmark"]);
        assert!(has::<system::MemoryBenchmark>(&benchmark));
        assert!(!has::<network::Udp>(&benchmark) && !has::<phy::RegisterImage>(&benchmark));
        let performance = with(&["open-radio-hil"]);
        assert!(!has::<system::MemoryBenchmark>(&performance));
        assert!(has::<network::Udp>(&performance) && has::<phy::RegisterImage>(&performance));
    }

    #[test]
    fn station_exit_evidence_is_its_own_only_without_driver_observation() {
        let exit = |features: &[&str]| has::<telemetry::StationExit>(&with(features));
        assert!(exit(&["open-radio-hil", "station-exit-evidence"]));
        assert!(!exit(&[
            "open-radio-hil",
            "station-exit-evidence",
            "driver-observation"
        ]));
    }

    #[test]
    fn the_set_is_sorted_and_distinct_so_its_pages_are_stable() {
        let all = with(READ_FEATURES);
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        base::ImageKeySet::new(&all);
    }
}
