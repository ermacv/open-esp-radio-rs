//! Semantic and cross-field validation for one scenario.

use super::*;

impl Scenario {
    pub(super) fn validate(&self) -> Result<()> {
        if self.schema != SCENARIO_SCHEMA {
            return Err(format!(
                "{}: scenario schema {} is unsupported (expected {SCENARIO_SCHEMA})",
                self.source.display(),
                self.schema
            )
            .into());
        }
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(format!(
                "{}: invalid scenario id `{}`",
                self.source.display(),
                self.id
            )
            .into());
        }
        if self.description.trim().is_empty() {
            return Err(format!("{}: scenario description is empty", self.source.display()).into());
        }
        bounded(self.repetitions, 1, 20, self, "repetitions")?;
        if self.rx_checksum == WifiRxChecksumPolicy::AssumeValidDiagnostic
            && (self.image != ImageClass::DiagnosticTaskPoll
                || !matches!(
                    self.workload,
                    Workload::Udp {
                        direction: Direction::Rx,
                        ..
                    }
                ))
        {
            return Err(format!(
                "{}: assume-valid-diagnostic RX checksum policy is restricted to the UDP RX task-poll diagnostic",
                self.source.display()
            )
            .into());
        }
        if self.tx_udp_checksum == WifiTxUdpChecksumPolicy::OmitIpv4Diagnostic
            && (!matches!(
                self.image,
                ImageClass::DiagnosticTaskPoll | ImageClass::DiagnosticCore0RxCoarse
            ) || !matches!(
                self.workload,
                Workload::Udp {
                    direction: Direction::Tx,
                    ..
                } | Workload::AccessPoint {
                    traffic: AccessPointTraffic::UdpMultiClient {
                        direction: Direction::Tx,
                        ..
                    },
                    ..
                }
            ))
        {
            return Err(format!(
                "{}: omit-ipv4-diagnostic TX UDP checksum policy is restricted to a UDP TX task-poll or coarse-cycle diagnostic",
                self.source.display()
            )
            .into());
        }
        if matches!(
            self.tx_buffer,
            WifiTxBufferPolicy::OwnedSramPromotionBurstDiagnostic
                | WifiTxBufferPolicy::PsramDirectDmaDiagnostic
        ) && (!matches!(
            self.image,
            ImageClass::DiagnosticTaskResidence
                | ImageClass::DiagnosticTxArchitecture
                | ImageClass::DiagnosticTaskPoll
                | ImageClass::DiagnosticCore0RxCoarse
        ) || !matches!(
            self.workload,
            Workload::AccessPoint {
                traffic: AccessPointTraffic::Udp {
                    direction: Direction::Tx,
                    ..
                } | AccessPointTraffic::UdpMultiClient {
                    direction: Direction::Tx,
                    ..
                },
                ..
            }
        )) {
            return Err(format!(
                "{}: TX architecture policies are restricted to a compatible AP UDP TX diagnostic image",
                self.source.display()
            )
            .into());
        }
        if self.rx_admission == WifiRxAdmissionPolicy::DeferredReadyDiagnostic
            && self.image != ImageClass::DiagnosticCore0RxCycles
        {
            return Err(format!(
                "{}: deferred-ready-diagnostic RX admission is restricted to the Core0 RX cycle diagnostic",
                self.source.display()
            )
            .into());
        }
        if self.rx_dispatch != WifiRxDispatchPolicy::Asynchronous
            && (!matches!(
                self.image,
                ImageClass::DiagnosticCore0RxCoarse | ImageClass::DiagnosticCore0RxCycles
            ) || !is_rx_only_udp_workload(&self.workload))
        {
            return Err(format!(
                "{}: direct-immediate-diagnostic RX dispatch is restricted to a Core0 UDP RX diagnostic",
                self.source.display()
            )
            .into());
        }
        if self.rx_continuation != WifiRxContinuationPolicy::ImmediateSoftwareProbe
            && (!matches!(
                self.image,
                ImageClass::DiagnosticCore0RxCoarse | ImageClass::DiagnosticCore0RxCycles
            ) || !is_rx_only_udp_workload(&self.workload))
        {
            return Err(format!(
                "{}: selectable RX continuation is restricted to a Core0 UDP RX diagnostic",
                self.source.display()
            )
            .into());
        }
        let coarse_ap_tx_cache_diagnostic = self.image == ImageClass::DiagnosticCore0RxCoarse
            && matches!(
                self.workload,
                Workload::AccessPoint {
                    traffic: AccessPointTraffic::Udp {
                        direction: Direction::Tx,
                        ..
                    },
                    ..
                }
            );
        if self.l1_cache_counters
            && self.image != ImageClass::DiagnosticCore0RxCycles
            && !coarse_ap_tx_cache_diagnostic
        {
            return Err(format!(
                "{}: L1 cache counters require the Core0 RX cycle image or an AP UDP TX coarse-cycle diagnostic",
                self.source.display()
            )
            .into());
        }
        let boot_smoke_image = self.image == ImageClass::BootSmoke;
        if boot_smoke_image && !matches!(self.workload, Workload::BootSmoke) {
            return Err(format!(
                "{}: boot-smoke image accepts only boot-smoke workload",
                self.source.display()
            )
            .into());
        }
        if !boot_smoke_image && matches!(self.workload, Workload::BootSmoke) {
            return Err(format!(
                "{}: boot-smoke workload requires boot-smoke image",
                self.source.display()
            )
            .into());
        }
        let event_status_image = self.image == ImageClass::DiagnosticIeee802154EventStatus;
        let event_status_workload = matches!(self.workload, Workload::Ieee802154EventStatus { .. });
        if event_status_image != event_status_workload {
            return Err(format!(
                "{}: IEEE 802.15.4 EVENT_STATUS workload and diagnostic image must be selected together",
                self.source.display()
            )
            .into());
        }
        let ed_event_image = self.image == ImageClass::DiagnosticIeee802154EdEvent;
        let ed_event_workload = matches!(self.workload, Workload::Ieee802154EdEvent { .. });
        if ed_event_image != ed_event_workload {
            return Err(format!(
                "{}: IEEE 802.15.4 ED event workload and diagnostic image must be selected together",
                self.source.display()
            )
            .into());
        }
        if self.evidence.openwrt_tx_monitor_rx
            && (!matches!(
                self.image,
                ImageClass::DiagnosticTaskResidence
                    | ImageClass::DiagnosticTaskPoll
                    | ImageClass::DiagnosticRxDelivery
                    | ImageClass::DiagnosticCore0RxCoarse
                    | ImageClass::DiagnosticCore0RxCycles
            ) || !matches!(
                self.workload,
                Workload::Udp {
                    direction: Direction::Rx | Direction::Bidirectional,
                    ..
                }
            ))
        {
            return Err(format!(
                "{}: OpenWrt TX-monitor RX evidence requires an RX-bearing UDP diagnostic with task-poll, delivery, or Core0-cycle evidence",
                self.source.display()
            )
            .into());
        }
        let independent_ap_air = matches!(
            &self.workload,
            Workload::AccessPoint {
                client: AccessPointClient::OpenWrt,
                traffic: AccessPointTraffic::Udp { .. } | AccessPointTraffic::UdpMultiClient { .. },
                ..
            } | Workload::StationAccessPoint { .. }
        );
        if self.evidence.independent_laptop_air_monitor
            && !self.evidence.openwrt_tx_monitor_rx
            && !independent_ap_air
        {
            return Err(format!(
                "{}: independent laptop air evidence requires either correlated OpenWrt TX-monitor evidence or an OpenWrt-client AP workload",
                self.source.display()
            )
            .into());
        }
        if let Some(link) = self.link
            && !self.image.requires_driver_observation()
            && (link.minimum_mcs.is_some()
                || link.guard_interval != HtGuardIntervalExpectation::Any)
        {
            return Err(format!(
                "{}: per-frame MCS/GI requirements require a driver-observation image",
                self.source.display(),
            )
            .into());
        }
        if self.fixture_mutation.openwrt_fixed_guard_interval != HtGuardIntervalExpectation::Any {
            let Some(link) = self.link else {
                return Err(format!(
                    "{}: OpenWrt fixed-GI mutation requires an HT link expectation",
                    self.source.display(),
                )
                .into());
            };
            if !matches!(link.phy, PhyExpectation::Ht20 | PhyExpectation::Ht40)
                || link.guard_interval != self.fixture_mutation.openwrt_fixed_guard_interval
            {
                return Err(format!(
                    "{}: OpenWrt fixed-GI mutation must equal the strict HT guard-interval expectation",
                    self.source.display(),
                )
                .into());
            }
            if !self.evidence.openwrt_tx_monitor_rx || !self.evidence.independent_laptop_air_monitor
            {
                return Err(format!(
                    "{}: OpenWrt fixed-GI mutation requires AP and independent air evidence",
                    self.source.display(),
                )
                .into());
            }
        }
        if let Some(mcs) = self.fixture_mutation.openwrt_client_fixed_ht_mcs {
            if mcs > 7 {
                return Err(format!(
                    "{}: OpenWrt AP-client fixed HT MCS must be within 0..=7",
                    self.source.display(),
                )
                .into());
            }
            if !matches!(
                self.workload,
                Workload::AccessPoint {
                    client: AccessPointClient::OpenWrt,
                    ..
                }
            ) || self
                .link
                .is_none_or(|link| link.phy != PhyExpectation::Ht40)
            {
                return Err(format!(
                    "{}: OpenWrt AP-client fixed HT MCS requires an HT40 AP workload with the controlled OpenWrt client",
                    self.source.display(),
                )
                .into());
            }
            if !self.image.requires_driver_observation()
                || !self.evidence.independent_laptop_air_monitor
            {
                return Err(format!(
                    "{}: OpenWrt AP-client fixed HT MCS requires driver and independent-air evidence",
                    self.source.display(),
                )
                .into());
            }
        }
        if self.fixture_mutation.openwrt_client_fixed_guard_interval
            != HtGuardIntervalExpectation::Any
        {
            let Some(link) = self.link else {
                return Err(format!(
                    "{}: OpenWrt AP-client fixed GI requires an HT link expectation",
                    self.source.display(),
                )
                .into());
            };
            if !matches!(
                self.workload,
                Workload::AccessPoint {
                    client: AccessPointClient::OpenWrt,
                    ..
                }
            ) || link.phy != PhyExpectation::Ht40
                || link.guard_interval != self.fixture_mutation.openwrt_client_fixed_guard_interval
            {
                return Err(format!(
                    "{}: OpenWrt AP-client fixed GI must equal the strict GI expectation of an HT40 AP workload",
                    self.source.display(),
                )
                .into());
            }
            if !self.image.requires_driver_observation()
                || !self.evidence.independent_laptop_air_monitor
            {
                return Err(format!(
                    "{}: OpenWrt AP-client fixed GI requires driver and independent-air evidence",
                    self.source.display(),
                )
                .into());
            }
        }
        if self.isolation == Isolation::MatrixSession {
            return Err(format!(
                "{}: matrix-session requires a multi-cell workload, which schema {SCENARIO_SCHEMA} does not define",
                self.source.display()
            )
            .into());
        }
        let station_link_required = matches!(
            self.workload,
            Workload::Udp { .. }
                | Workload::Tcp { .. }
                | Workload::Icmp { .. }
                | Workload::StationReconnect { .. }
                | Workload::StationApLoss { .. }
                | Workload::StationApAbsence { .. }
                | Workload::WifiRole { .. }
                | Workload::MonitorCapture { .. }
                | Workload::StationAccessPoint { .. }
                | Workload::StationAccessPointReconnect { .. }
        );
        let link_allowed =
            station_link_required || matches!(self.workload, Workload::AccessPoint { .. });
        if (station_link_required && self.link.is_none()) || (!link_allowed && self.link.is_some())
        {
            return Err(format!(
                "{}: this workload has an invalid `[link]` expectation",
                self.source.display()
            )
            .into());
        }
        if let Some(link) = self.link
            && let Some(minimum_mcs) = link.minimum_mcs
        {
            if self.image == ImageClass::Performance {
                return Err(format!(
                    "{}: performance images do not collect the driver observation required by minimum_mcs",
                    self.source.display(),
                )
                .into());
            }
            let maximum_mcs = match link.phy {
                PhyExpectation::Ht20 | PhyExpectation::Ht40 => 7,
                PhyExpectation::He20 => 9,
            };
            if minimum_mcs > maximum_mcs {
                return Err(format!(
                    "{}: minimum_mcs={minimum_mcs} exceeds the {} capability MCS{maximum_mcs}",
                    self.source.display(),
                    link.phy.id(),
                )
                .into());
            }
        }
        if let Some(link) = self.link
            && link.guard_interval != HtGuardIntervalExpectation::Any
        {
            if !matches!(link.phy, PhyExpectation::Ht20 | PhyExpectation::Ht40) {
                return Err(format!(
                    "{}: guard_interval={} is valid only for an HT link",
                    self.source.display(),
                    link.guard_interval.id(),
                )
                .into());
            }
            if !self.image.requires_driver_observation() {
                return Err(format!(
                    "{}: strict guard_interval requires complete target-side driver observation",
                    self.source.display(),
                )
                .into());
            }
        }
        match &self.workload {
            Workload::BootSmoke => {}
            Workload::Timebase {
                boots,
                intervals,
                period_millis,
            } => {
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*intervals, 2, 100, self, "intervals")?;
                bounded(*period_millis, 1, 1_000, self, "period_millis")?;
            }
            Workload::Ieee802154EventStatus {
                boots,
                poll_limit,
                timer_threshold,
            } => {
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*poll_limit, 1, 1_000_000, self, "poll_limit")?;
                bounded(*timer_threshold, 1, 1_000, self, "timer_threshold")?;
                if self.criteria != Criteria::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 EVENT_STATUS diagnostic does not accept network criteria",
                    );
                }
                if self.evidence != EvidenceConfig::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 EVENT_STATUS diagnostic does not accept external Wi-Fi evidence",
                    );
                }
            }
            Workload::Ieee802154EdEvent {
                boots,
                poll_limit,
                timer_threshold,
            } => {
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*poll_limit, 1, 1_000_000, self, "poll_limit")?;
                bounded(*timer_threshold, 1, 1_000, self, "timer_threshold")?;
                if self.criteria != Criteria::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 ED event diagnostic does not accept network criteria",
                    );
                }
                if self.evidence != EvidenceConfig::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 ED event diagnostic does not accept external Wi-Fi evidence",
                    );
                }
            }
            Workload::Udp {
                direction,
                duration_seconds,
                rx_rate_bps,
                tx_rate_bps,
                payload_bytes,
                ..
            } => {
                bounded(*duration_seconds, 5, 300, self, "duration_seconds")?;
                if matches!(
                    self.image,
                    ImageClass::DiagnosticCore0RxCoarse | ImageClass::DiagnosticCore0RxCycles
                ) && *duration_seconds > CORE0_RX_CYCLE_MAX_DURATION_SECONDS
                {
                    return Err(format!(
                        "{}: Core0 cycle diagnostic duration {} exceeds the u32-safe {} second interval",
                        self.source.display(),
                        duration_seconds,
                        CORE0_RX_CYCLE_MAX_DURATION_SECONDS,
                    )
                    .into());
                }
                bounded(*payload_bytes, 64, 1472, self, "payload_bytes")?;
                validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, self)?;
            }
            Workload::Tcp {
                direction,
                duration_seconds,
                rx_rate_bps,
                tx_rate_bps,
                chunk_bytes,
            } => {
                bounded(*duration_seconds, 5, 300, self, "duration_seconds")?;
                bounded(*chunk_bytes, 64, 32768, self, "chunk_bytes")?;
                validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, self)?;
            }
            Workload::Icmp {
                count,
                interval_ms,
                timeout_ms,
                payload_bytes,
            } => {
                bounded(*count, 1, u16::MAX, self, "count")?;
                bounded(*interval_ms, 1, 10_000, self, "interval_ms")?;
                bounded(*timeout_ms, 1, 60_000, self, "timeout_ms")?;
                bounded(*payload_bytes, 0, 1400, self, "payload_bytes")?;
            }
            Workload::StationReconnect {
                cycles,
                boots,
                timeout_seconds,
            } => {
                bounded(*cycles, 1, 8, self, "cycles")?;
                bounded(*boots, 1, 100, self, "boots")?;
                bounded(*timeout_seconds, 10, 300, self, "timeout_seconds")?;
            }
            Workload::StationApLoss { timeout_seconds }
            | Workload::StationApAbsence { timeout_seconds } => {
                bounded(*timeout_seconds, 30, 300, self, "timeout_seconds")?;
            }
            Workload::WifiRole {
                timeout_seconds,
                channel,
                dwell_seconds,
                snapshot_length,
                ..
            } => {
                bounded(*timeout_seconds, 10, 180, self, "timeout_seconds")?;
                if let Some(channel) = channel {
                    bounded(*channel, 1, 13, self, "channel")?;
                }
                if let Some(seconds) = dwell_seconds {
                    bounded(*seconds, 1, 30, self, "dwell_seconds")?;
                }
                if let Some(length) = snapshot_length {
                    bounded(*length, 0, 2304, self, "snapshot_length")?;
                }
            }
            Workload::MonitorCapture {
                timeout_seconds,
                duration_seconds,
                channel,
                snapshot_length,
            } => {
                bounded(*timeout_seconds, 10, 180, self, "timeout_seconds")?;
                bounded(*duration_seconds, 1, 30, self, "duration_seconds")?;
                if let Some(channel) = channel {
                    bounded(*channel, 1, 13, self, "channel")?;
                }
                bounded(*snapshot_length, 0, 2304, self, "snapshot_length")?;
            }
            Workload::AccessPoint {
                cycles,
                boots,
                timeout_seconds,
                client,
                security,
                traffic,
            } => {
                bounded(*cycles, 2, 8, self, "cycles")?;
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*timeout_seconds, 20, 180, self, "timeout_seconds")?;
                validate_access_point_traffic(traffic, self)?;
                if *security == WifiAccessPointSecurity::Open
                    && *client != AccessPointClient::OpenWrt
                {
                    return self.criteria_error(
                        "open AP qualification requires the controlled OpenWrt client",
                    );
                }
                if *client == AccessPointClient::OpenWrt {
                    if self.criteria.minimum_concurrent_ap_clients.unwrap_or(1) != 1 {
                        return self.criteria_error(
                            "an OpenWrt primary AP client requires exactly one concurrent client",
                        );
                    }
                    if !matches!(
                        traffic,
                        AccessPointTraffic::Udp { .. } | AccessPointTraffic::Icmp { .. }
                    ) {
                        return self.criteria_error(
                            "the OpenWrt primary AP client supports UDP and ICMP workloads",
                        );
                    }
                }
                if matches!(traffic, AccessPointTraffic::UdpMultiClient { .. }) {
                    if *client != AccessPointClient::Laptop {
                        return self.criteria_error(
                            "multi-client AP UDP requires the laptop primary client",
                        );
                    }
                    if self.criteria.minimum_concurrent_ap_clients != Some(2) {
                        return self.criteria_error(
                            "multi-client AP UDP requires minimum_concurrent_ap_clients = 2",
                        );
                    }
                }
            }
            Workload::StationAccessPoint {
                timeout_seconds,
                duration_seconds,
                direction: _,
                rate_bps_per_flow,
                minimum_bps_per_flow,
                maximum_fairness_skew_percent,
                payload_bytes,
            } => {
                bounded(*timeout_seconds, 30, 180, self, "timeout_seconds")?;
                bounded(*duration_seconds, 5, 120, self, "duration_seconds")?;
                if matches!(
                    self.image,
                    ImageClass::DiagnosticCore0RxCoarse | ImageClass::DiagnosticCore0RxCycles
                ) && *duration_seconds > CORE0_RX_CYCLE_MAX_DURATION_SECONDS
                {
                    return Err(format!(
                        "{}: Core0 cycle diagnostic duration {} exceeds the u32-safe {} second interval",
                        self.source.display(),
                        duration_seconds,
                        CORE0_RX_CYCLE_MAX_DURATION_SECONDS,
                    )
                    .into());
                }
                bounded(*payload_bytes, 64, 1472, self, "payload_bytes")?;
                bounded(
                    *maximum_fairness_skew_percent,
                    1,
                    100,
                    self,
                    "maximum_fairness_skew_percent",
                )?;
                if !(100_000..=100_000_000).contains(rate_bps_per_flow) {
                    return self.criteria_error(
                        "rate_bps_per_flow must be within 100 Kbit/s..=100 Mbit/s",
                    );
                }
                if *minimum_bps_per_flow == 0 || minimum_bps_per_flow > rate_bps_per_flow {
                    return self.criteria_error(
                        "minimum_bps_per_flow must be nonzero and cannot exceed the offer",
                    );
                }
            }
            Workload::StationAccessPointReconnect { timeout_seconds } => {
                bounded(*timeout_seconds, 30, 180, self, "timeout_seconds")?;
            }
        }
        if !self.image.requires_driver_observation() && self.criteria.require_no_beacon_loss {
            return self
                .criteria_error("this image cannot use the driver-observed beacon-loss verdict");
        }
        if self.image == ImageClass::Performance {
            if self.criteria.exact_delivery {
                return self.criteria_error(
                    "performance images cannot claim exact delivery without driver observation",
                );
            }
            if !matches!(
                self.workload,
                Workload::Udp { .. }
                    | Workload::Tcp { .. }
                    | Workload::Icmp { .. }
                    | Workload::AccessPoint { .. }
                    | Workload::StationAccessPoint { .. }
            ) {
                return self.criteria_error(
                    "performance images admit only externally measured network workloads",
                );
            }
        }
        self.validate_criteria()?;
        Ok(())
    }

    fn validate_criteria(&self) -> Result<()> {
        let (
            rx_offer,
            tx_offer,
            udp,
            bidirectional_udp,
            icmp,
            station_data_plane,
            multi_client_udp,
        ) = match &self.workload {
            Workload::Udp {
                direction,
                rx_rate_bps,
                tx_rate_bps,
                ..
            } => (
                *rx_rate_bps,
                *tx_rate_bps,
                true,
                *direction == Direction::Bidirectional,
                false,
                true,
                false,
            ),
            Workload::Tcp {
                rx_rate_bps,
                tx_rate_bps,
                ..
            } => (*rx_rate_bps, *tx_rate_bps, false, false, false, true, false),
            Workload::Icmp { .. } => (None, None, false, false, true, true, false),
            Workload::StationReconnect { .. } => (None, None, false, false, false, true, false),
            Workload::StationAccessPoint { .. } => (None, None, false, false, false, true, false),
            Workload::StationAccessPointReconnect { .. } => {
                (None, None, false, false, false, true, false)
            }
            Workload::AccessPoint { traffic, .. } => match traffic {
                AccessPointTraffic::Udp {
                    direction,
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                } => (
                    *rx_rate_bps,
                    *tx_rate_bps,
                    true,
                    *direction == Direction::Bidirectional,
                    false,
                    false,
                    false,
                ),
                AccessPointTraffic::UdpMultiClient {
                    direction,
                    rx_rate_bps_per_flow,
                    tx_rate_bps_per_flow,
                    secondary_rx_rate_bps,
                    secondary_tx_rate_bps,
                    ..
                } => (
                    summed_two_flow_offer(*rx_rate_bps_per_flow, *secondary_rx_rate_bps),
                    summed_two_flow_offer(*tx_rate_bps_per_flow, *secondary_tx_rate_bps),
                    true,
                    *direction == Direction::Bidirectional,
                    false,
                    false,
                    true,
                ),
                AccessPointTraffic::Tcp {
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                } => (
                    *rx_rate_bps,
                    *tx_rate_bps,
                    false,
                    false,
                    false,
                    false,
                    false,
                ),
                AccessPointTraffic::Icmp { .. } => (None, None, false, false, true, false, false),
                AccessPointTraffic::None => (None, None, false, false, false, false, false),
            },
            _ => (None, None, false, false, false, false, false),
        };
        if self.criteria.exact_delivery && !udp {
            return self.criteria_error("exact_delivery is valid only for UDP workloads");
        }
        if let Some(floor) = self.criteria.minimum_rx_bps {
            let offer = rx_offer.ok_or_else(|| {
                format!(
                    "{}: minimum_rx_bps requires an RX data plane",
                    self.source.display()
                )
            })?;
            if floor > offer {
                return self.criteria_error("minimum_rx_bps cannot exceed rx_rate_bps");
            }
        }
        if let Some(floor) = self.criteria.minimum_tx_bps {
            if tx_offer.is_none() {
                return self.criteria_error("minimum_tx_bps requires a TX data plane");
            }
            if tx_offer.is_some_and(|offer| floor > offer) {
                return self.criteria_error("minimum_tx_bps cannot exceed tx_rate_bps");
            }
        }
        if let Some(floor) = self.criteria.minimum_combined_bps {
            if !bidirectional_udp {
                return self
                    .criteria_error("minimum_combined_bps requires a bidirectional UDP workload");
            }
            let offered_sum = rx_offer
                .and_then(|rx| tx_offer.and_then(|tx| rx.checked_add(tx)))
                .ok_or_else(|| {
                    format!(
                        "{}: minimum_combined_bps requires bounded RX and TX offers",
                        self.source.display()
                    )
                })?;
            if floor > offered_sum {
                return self
                    .criteria_error("minimum_combined_bps cannot exceed the RX+TX offered rate");
            }
        }
        if let Some(floor) = self.criteria.minimum_bps_per_flow {
            if !multi_client_udp {
                return self.criteria_error(
                    "minimum_bps_per_flow requires a multi-client AP UDP workload",
                );
            }
            let per_flow_offer = match &self.workload {
                Workload::AccessPoint {
                    traffic:
                        AccessPointTraffic::UdpMultiClient {
                            direction: Direction::Rx,
                            rx_rate_bps_per_flow: Some(rate),
                            secondary_rx_rate_bps,
                            ..
                        },
                    ..
                } => secondary_rx_rate_bps.unwrap_or(*rate).min(*rate),
                Workload::AccessPoint {
                    traffic:
                        AccessPointTraffic::UdpMultiClient {
                            direction: Direction::Tx,
                            tx_rate_bps_per_flow: Some(rate),
                            secondary_tx_rate_bps,
                            ..
                        },
                    ..
                } => secondary_tx_rate_bps.unwrap_or(*rate).min(*rate),
                Workload::AccessPoint {
                    traffic:
                        AccessPointTraffic::UdpMultiClient {
                            direction: Direction::Bidirectional,
                            rx_rate_bps_per_flow: Some(rx),
                            tx_rate_bps_per_flow: Some(tx),
                            secondary_rx_rate_bps,
                            secondary_tx_rate_bps,
                            ..
                        },
                    ..
                } => (*rx)
                    .min(secondary_rx_rate_bps.unwrap_or(*rx))
                    .min(*tx)
                    .min(secondary_tx_rate_bps.unwrap_or(*tx)),
                _ => unreachable!("validated multi-client UDP direction has an offer"),
            };
            if floor == 0 || floor > per_flow_offer {
                return self.criteria_error(
                    "minimum_bps_per_flow must be nonzero and cannot exceed either per-flow offer",
                );
            }
        }
        if let Some(maximum) = self.criteria.maximum_flow_skew_percent {
            if !multi_client_udp {
                return self.criteria_error(
                    "maximum_flow_skew_percent requires a multi-client AP UDP workload",
                );
            }
            if !(1..=100).contains(&maximum) {
                return self.criteria_error("maximum_flow_skew_percent must be within 1..=100");
            }
            if matches!(
                &self.workload,
                Workload::AccessPoint {
                    traffic: AccessPointTraffic::UdpMultiClient {
                        rx_rate_bps_per_flow,
                        tx_rate_bps_per_flow,
                        secondary_rx_rate_bps,
                        secondary_tx_rate_bps,
                        ..
                    },
                    ..
                } if secondary_rx_rate_bps.is_some_and(|rate| Some(rate) != *rx_rate_bps_per_flow)
                    || secondary_tx_rate_bps.is_some_and(|rate| Some(rate) != *tx_rate_bps_per_flow)
            ) {
                return self.criteria_error(
                    "maximum_flow_skew_percent is invalid for unequal offered rates",
                );
            }
        }
        if let Some(maximum) = self.criteria.maximum_secondary_tx_interarrival_ms {
            if maximum == 0 {
                return self.criteria_error("maximum_secondary_tx_interarrival_ms must be nonzero");
            }
            if !matches!(
                &self.workload,
                Workload::AccessPoint {
                    traffic: AccessPointTraffic::UdpMultiClient {
                        direction: Direction::Tx | Direction::Bidirectional,
                        secondary_tx_rate_bps: Some(_),
                        secondary_tx_pacing_group_datagrams: Some(_),
                        ..
                    },
                    ..
                }
            ) {
                return self.criteria_error(
                    "maximum_secondary_tx_interarrival_ms requires an explicitly paced secondary AP TX flow",
                );
            }
        }
        if let Some(minimum) = self.criteria.minimum_secondary_tx_datagrams {
            if minimum < 2 {
                return self.criteria_error(
                    "minimum_secondary_tx_datagrams must cover at least one inter-arrival",
                );
            }
            if !matches!(
                &self.workload,
                Workload::AccessPoint {
                    traffic: AccessPointTraffic::UdpMultiClient {
                        direction: Direction::Tx | Direction::Bidirectional,
                        secondary_tx_rate_bps: Some(_),
                        ..
                    },
                    ..
                }
            ) {
                return self.criteria_error(
                    "minimum_secondary_tx_datagrams requires an explicit secondary AP TX flow",
                );
            }
        }
        if let Some(maximum) = self.criteria.maximum_idle_channel_utilization_255 {
            if maximum == 0 {
                return self.criteria_error("maximum_idle_channel_utilization_255 must be nonzero");
            }
            if !matches!(
                self.workload,
                Workload::Udp {
                    direction: Direction::Rx | Direction::Tx,
                    ..
                }
            ) {
                return self.criteria_error(
                    "maximum_idle_channel_utilization_255 requires a station UDP RX or TX workload",
                );
            }
        }
        if (self.criteria.maximum_lost.is_some() || self.criteria.maximum_p95_ms.is_some()) && !icmp
        {
            return self.criteria_error("loss and latency criteria are valid only for ICMP");
        }
        if self.criteria.require_no_beacon_loss
            && !station_data_plane
            && !matches!(self.workload, Workload::AccessPoint { .. })
            && !matches!(self.workload, Workload::StationAccessPoint { .. })
        {
            return self.criteria_error(
                "require_no_beacon_loss requires a station or access-point data-plane workload",
            );
        }
        if let Some(minimum) = self.criteria.minimum_concurrent_ap_clients {
            if !matches!(self.workload, Workload::AccessPoint { .. }) {
                return self.criteria_error(
                    "minimum_concurrent_ap_clients requires an access-point workload",
                );
            }
            if !(1..=2).contains(&minimum) {
                return self
                    .criteria_error("current physical HIL supports 1..=2 concurrent AP clients");
            }
        }
        Ok(())
    }

    fn criteria_error<T>(&self, message: &str) -> Result<T> {
        Err(format!("{}: {message}", self.source.display()).into())
    }
}

fn validate_access_point_traffic(traffic: &AccessPointTraffic, scenario: &Scenario) -> Result<()> {
    match traffic {
        AccessPointTraffic::None => Ok(()),
        AccessPointTraffic::Icmp {
            count,
            interval_ms,
            timeout_ms,
            payload_bytes,
        } => {
            bounded(*count, 1, u16::MAX, scenario, "traffic.count")?;
            bounded(*interval_ms, 1, 10_000, scenario, "traffic.interval_ms")?;
            bounded(*timeout_ms, 1, 60_000, scenario, "traffic.timeout_ms")?;
            bounded(*payload_bytes, 0, 1400, scenario, "traffic.payload_bytes")
        }
        AccessPointTraffic::Udp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            payload_bytes,
        } => {
            bounded(
                *duration_seconds,
                5,
                300,
                scenario,
                "traffic.duration_seconds",
            )?;
            bounded(*payload_bytes, 64, 1472, scenario, "traffic.payload_bytes")?;
            validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, scenario)
        }
        AccessPointTraffic::UdpMultiClient {
            direction,
            duration_seconds,
            rx_rate_bps_per_flow,
            tx_rate_bps_per_flow,
            secondary_rx_rate_bps,
            secondary_tx_rate_bps,
            secondary_tx_pacing_group_datagrams,
            payload_bytes,
        } => {
            bounded(
                *duration_seconds,
                5,
                300,
                scenario,
                "traffic.duration_seconds",
            )?;
            bounded(*payload_bytes, 64, 1472, scenario, "traffic.payload_bytes")?;
            validate_direction_rates(
                *direction,
                *rx_rate_bps_per_flow,
                *tx_rate_bps_per_flow,
                scenario,
            )?;
            let secondary_direction_valid = match direction {
                Direction::Rx => secondary_tx_rate_bps.is_none(),
                Direction::Tx => secondary_rx_rate_bps.is_none(),
                Direction::Bidirectional => true,
            };
            if !secondary_direction_valid {
                return scenario.criteria_error(
                    "secondary offered rates do not match the multi-client direction",
                );
            }
            for (field, rate) in [
                ("traffic.secondary_rx_rate_bps", *secondary_rx_rate_bps),
                ("traffic.secondary_tx_rate_bps", *secondary_tx_rate_bps),
            ] {
                if let Some(rate) = rate {
                    bounded(rate, 1_000, 1_000_000_000, scenario, field)?;
                }
            }
            if let Some(group) = secondary_tx_pacing_group_datagrams {
                bounded(
                    *group,
                    1,
                    64,
                    scenario,
                    "traffic.secondary_tx_pacing_group_datagrams",
                )?;
                if tx_rate_bps_per_flow.is_none() {
                    return scenario
                        .criteria_error("secondary TX pacing requires a target-TX data plane");
                }
            }
            Ok(())
        }
        AccessPointTraffic::Tcp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            chunk_bytes,
        } => {
            bounded(
                *duration_seconds,
                5,
                300,
                scenario,
                "traffic.duration_seconds",
            )?;
            bounded(*chunk_bytes, 64, 32_768, scenario, "traffic.chunk_bytes")?;
            validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, scenario)
        }
    }
}

fn bounded<T>(value: T, minimum: T, maximum: T, scenario: &Scenario, field: &str) -> Result<()>
where
    T: Ord + std::fmt::Display,
{
    if value < minimum || value > maximum {
        return Err(format!(
            "{}: {field}={value} is outside {minimum}..={maximum}",
            scenario.source.display()
        )
        .into());
    }
    Ok(())
}

pub(super) fn validate_direction_rates(
    direction: Direction,
    rx: Option<u64>,
    tx: Option<u64>,
    scenario: &Scenario,
) -> Result<()> {
    let valid = match direction {
        Direction::Rx => rx.is_some() && tx.is_none(),
        Direction::Tx => rx.is_none() && tx.is_some(),
        Direction::Bidirectional => rx.is_some() && tx.is_some(),
    };
    if !valid {
        return Err(format!(
            "{}: offered rates do not match {:?} workload",
            scenario.source.display(),
            direction
        )
        .into());
    }
    Ok(())
}

fn summed_two_flow_offer(primary: Option<u64>, secondary: Option<u64>) -> Option<u64> {
    primary.and_then(|primary| primary.checked_add(secondary.unwrap_or(primary)))
}
