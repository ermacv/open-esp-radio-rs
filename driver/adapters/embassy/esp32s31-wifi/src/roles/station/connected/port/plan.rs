use super::*;

/// Runtime-selected rate policy independent of HIL environment variables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaRateConfig {
    pub high_throughput_enabled: bool,
    pub fallback_legacy_rate: LegacyRate,
    pub fallback_ht_mcs: HtMcs,
    pub fallback_ht_guard_interval: HtGuardInterval,
    pub ht_mcs_override: Option<HtMcs>,
    pub ht_guard_interval_override: Option<HtGuardInterval>,
    pub he_mcs_override: Option<HeMcs>,
    pub he_guard_interval_and_ltf_override:
        Option<open_esp_radio_esp32s31_wifi_mac::rx::HeGuardIntervalAndLtf>,
    pub he_dcm_override: Option<HeDcmRate>,
}

/// Complete value policy for one connected driver epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaConfig {
    pub tx: Esp32s31ConnectedStaTxPolicy,
    pub block_ack: Esp32s31ConnectedStaBlockAckPolicy,
    pub receive: Esp32s31ConnectedStaRxPolicy,
    pub power: StationPowerMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaTxPolicy {
    pub rate: Esp32s31ConnectedStaRateConfig,
    pub unicast_attempt_limit: u8,
    pub completion_timeout_us: u64,
    pub aggregate_frame_limit: u8,
    pub aggregate_he_txop_limit: HeEdcaTxopLimit,
    /// Optional recovered queue/MPLEN/BSR preparation for AP Trigger frames.
    /// `None` leaves HE-SU behavior unchanged.
    pub he_trigger_based: Option<HeTriggerBasedTxConfig>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaBlockAckPolicy {
    pub tx_block_ack_window: u16,
    pub tx_block_ack_negotiation_timeout_us: u32,
    pub tx_block_ack_negotiation_attempt_limit: u8,
    pub tid0_amsdu: bool,
    pub rx_block_ack_maximum_window: u16,
    pub request_initial_tx_block_ack: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaRxPolicy {
    pub ingress: RxIngressConfig,
    pub beacon_miss_limit: u8,
}

/// Configuration failure detected before any connected owner moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ConnectedStaConfigError {
    MissingStationInterface,
    InterfaceRole(VifRole),
    InterfaceAddress {
        interface: [u8; 6],
        station: [u8; 6],
    },
    AggregateFrameLimit {
        limit: u8,
        capacity: usize,
    },
    RxBlockAckWindowExceedsStorage {
        window: u16,
        capacity: usize,
    },
    ZeroUnicastAttemptLimit,
    ZeroTxBlockAckNegotiationAttemptLimit,
    PeerDoesNotSupportQos,
    TxBlockAck(TxBlockAckError),
    RxBlockAck(RxBlockAckSessionsError),
    BeaconLoss(StaBeaconLossConfigError),
    PowerSave(StaPowerSavePolicyError),
}

/// Complete owner return when connected policy validation fails.
#[derive(Debug)]
pub struct Esp32s31ConnectedStaPrepareFailure {
    pub error: Esp32s31ConnectedStaConfigError,
    pub peer: Esp32s31ConnectedStaPeer,
}

/// Validated driver plan derived from the exact associated peer.
#[derive(Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaPlan {
    pub(super) interface: BoundVirtualInterface,
    pub(super) link: Esp32s31StaConnectedLink,
    pub(super) config: Esp32s31ConnectedStaConfig,
    pub(super) data_tx_rate: TxPhyRate,
    pub(super) aggregate_tx_rate: TxPhyRate,
    pub(super) aggregate_rate_policy: StaTxRatePolicy,
    pub(super) rate_control: Option<StaRateControlAssociation>,
    pub(super) beacon_loss: StaBeaconLossConfig,
    pub(super) power_save: Option<StaPowerSavePolicy>,
}

impl Esp32s31ConnectedStaPlan {
    pub const fn interface(&self) -> BoundVirtualInterface {
        self.interface
    }

    pub const fn link(&self) -> Esp32s31StaConnectedLink {
        self.link
    }

    pub const fn data_tx_rate(&self) -> TxPhyRate {
        self.data_tx_rate
    }

    pub const fn aggregate_tx_rate(&self) -> TxPhyRate {
        self.aggregate_tx_rate
    }

    pub const fn beacon_loss(&self) -> StaBeaconLossConfig {
        self.beacon_loss
    }

    pub const fn power_save(&self) -> Option<StaPowerSavePolicy> {
        self.power_save
    }

    pub const fn rx_config(&self) -> ConnectedRxConfig {
        ConnectedRxConfig {
            station_address: self.link.station_address,
            bssid: self.link.bssid,
            association_id: self.link.association_id,
            ingress: self.config.receive.ingress,
        }
    }

    pub const fn single_mpdu_tx_config(&self) -> SingleMpduTxConfig {
        SingleMpduTxConfig {
            station_address: self.link.station_address,
            bssid: self.link.bssid,
            peer_qos: self.link.peer_qos,
            exchange: MacTxPlan {
                access_category: WmmAccessCategory::BestEffort,
                initial_rate: self.data_tx_rate,
                publication_limit: self.config.tx.unicast_attempt_limit,
                publication_timeout_micros: self.config.tx.completion_timeout_us,
            },
        }
    }

    pub const fn aggregate_tx_config(&self) -> AggregateTxConfig {
        AggregateTxConfig {
            rate: self.aggregate_tx_rate,
            frame_limit: self.config.tx.aggregate_frame_limit,
            attempt_limit: self.config.tx.unicast_attempt_limit,
            completion_timeout_us: self.config.tx.completion_timeout_us,
            he_txop_limit: self.config.tx.aggregate_he_txop_limit,
        }
    }
}
impl Esp32s31ConnectedStaPort {
    /// Return the portable service contract implemented by this production
    /// ESP32-S31 adapter. HMAC policy can inspect this value without importing
    /// PAC, DMA, interrupt or executor types.
    pub const fn capabilities() -> MacServiceCapabilities {
        ESP32S31_MAC_SERVICE_CAPABILITIES
    }

    /// Materialize the station interface selected by the application radio
    /// plan into one concrete connected ESP32-S31 owner graph.
    ///
    /// The plan has already been checked against complete radio/MAC
    /// capabilities. This boundary still fails closed if a caller passes a
    /// plan without a station and returns the associated peer unchanged.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_wifi_plan_with_storage<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: Esp32s31ConnectedStaPeer,
        config: Esp32s31ConnectedStaConfig,
        wifi: WifiPlan,
    ) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
        let Some(interface) = wifi.station() else {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::MissingStationInterface,
                peer,
            });
        };
        Self::prepare_for_interface_with_storage::<AGGREGATE_SLOTS, RX_REORDER_SLOTS>(
            peer, config, interface,
        )
    }

    /// Prepare one explicitly identified STA VIF on a hardware channel
    /// context.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_interface_with_storage<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: Esp32s31ConnectedStaPeer,
        config: Esp32s31ConnectedStaConfig,
        interface: BoundVirtualInterface,
    ) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
        if interface.interface.role != VifRole::Station {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::InterfaceRole(interface.interface.role),
                peer,
            });
        }
        if interface.interface.address != peer.link.station_address {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::InterfaceAddress {
                    interface: interface.interface.address,
                    station: peer.link.station_address,
                },
                peer,
            });
        }
        if config.tx.aggregate_frame_limit == 0
            || usize::from(config.tx.aggregate_frame_limit) > AGGREGATE_SLOTS
        {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::AggregateFrameLimit {
                    limit: config.tx.aggregate_frame_limit,
                    capacity: AGGREGATE_SLOTS,
                },
                peer,
            });
        }
        if usize::from(config.block_ack.rx_block_ack_maximum_window) > RX_REORDER_SLOTS {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::RxBlockAckWindowExceedsStorage {
                    window: config.block_ack.rx_block_ack_maximum_window,
                    capacity: RX_REORDER_SLOTS,
                },
                peer,
            });
        }
        if config.tx.unicast_attempt_limit == 0 {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::ZeroUnicastAttemptLimit,
                peer,
            });
        }
        if config.block_ack.tx_block_ack_negotiation_attempt_limit == 0 {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::ZeroTxBlockAckNegotiationAttemptLimit,
                peer,
            });
        }
        if !peer.link.peer_qos {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::PeerDoesNotSupportQos,
                peer,
            });
        }
        if let Err(error) = StaTxBlockAckSessions::new(
            config.block_ack.tx_block_ack_window,
            config.block_ack.tx_block_ack_negotiation_timeout_us,
            config.block_ack.tid0_amsdu,
        ) {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::TxBlockAck(error),
                peer,
            });
        }
        if let Err(error) = RxBlockAckSessions::<1>::with_maximum_window(
            config.block_ack.rx_block_ack_maximum_window,
        ) {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::RxBlockAck(error),
                peer,
            });
        }
        let beacon_loss = match StaBeaconLossConfig::new(
            peer.link.beacon_interval_tu,
            config.receive.beacon_miss_limit,
        ) {
            Ok(beacon_loss) => beacon_loss,
            Err(error) => {
                return Err(Esp32s31ConnectedStaPrepareFailure {
                    error: Esp32s31ConnectedStaConfigError::BeaconLoss(error),
                    peer,
                });
            }
        };
        let power_save = match config.power.power_save_policy() {
            None => None,
            Some(policy) => match StaPowerSavePolicy::for_association(
                peer.link.beacon_interval_tu,
                policy.listen_interval(),
                policy.wake_guard_micros(),
                config.receive.beacon_miss_limit,
            ) {
                Ok(policy) => Some(policy),
                Err(error) => {
                    return Err(Esp32s31ConnectedStaPrepareFailure {
                        error: Esp32s31ConnectedStaConfigError::PowerSave(error),
                        peer,
                    });
                }
            },
        };

        let Esp32s31ConnectedStaPeer { link, rate_control } = peer;
        let data_policy = sta_tx_rate_policy(link, config.tx.rate, false);
        let aggregate_policy = sta_tx_rate_policy(link, config.tx.rate, true);
        let aggregate_tx_rate = rate_control.ampdu_tx_rate(aggregate_policy);
        Ok(Esp32s31ConnectedStaPlan {
            interface,
            link,
            config,
            data_tx_rate: data_policy.fallback_rate(),
            aggregate_tx_rate,
            aggregate_rate_policy: aggregate_policy,
            rate_control: Some(rate_control),
            beacon_loss,
            power_save,
        })
    }
}

const fn sta_tx_rate_policy(
    link: Esp32s31StaConnectedLink,
    config: Esp32s31ConnectedStaRateConfig,
    use_peer_capabilities: bool,
) -> StaTxRatePolicy {
    StaTxRatePolicy {
        association_phy: link.association_phy,
        high_throughput_enabled: config.high_throughput_enabled && link.peer_qos,
        fallback_legacy_rate: config.fallback_legacy_rate,
        fallback_ht_mcs: config.fallback_ht_mcs,
        fallback_ht_guard_interval: config.fallback_ht_guard_interval,
        ht_mcs_override: config.ht_mcs_override,
        ht_guard_interval_override: config.ht_guard_interval_override,
        he_mcs_override: config.he_mcs_override,
        he_guard_interval_and_ltf_override: config.he_guard_interval_and_ltf_override,
        he_dcm_override: config.he_dcm_override,
        he_800ns_gi_ltf: if use_peer_capabilities && link.peer_supports_one_ltf_800ns_gi {
            open_esp_radio_esp32s31_wifi_mac::rx::HeGuardIntervalAndLtf::OneLtf800Ns
        } else {
            open_esp_radio_esp32s31_wifi_mac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns
        },
        peer_supports_ht_short_guard_interval: link.peer_supports_ht_short_guard_interval,
        peer_supports_ldpc: use_peer_capabilities && link.peer_supports_ldpc,
        peer_dcm_receive: if use_peer_capabilities {
            link.peer_dcm_receive
        } else {
            HeDcmConstellation::NotSupported
        },
    }
}
