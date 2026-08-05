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
    pub rate: Esp32s31ConnectedStaRateConfig,
    pub rx_ingress: RxIngressConfig,
    pub unicast_attempt_limit: u8,
    pub completion_timeout_us: u64,
    pub aggregate_frame_limit: u8,
    pub aggregate_he_txop_limit: HeEdcaTxopLimit,
    pub tx_block_ack_window: u16,
    pub tx_block_ack_negotiation_timeout_us: u32,
    pub tid0_amsdu: bool,
    pub rx_block_ack_maximum_window: u16,
    pub beacon_miss_limit: u8,
    pub request_initial_tx_block_ack: bool,
}

/// Configuration failure detected before any connected owner moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ConnectedStaConfigError {
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
    PeerDoesNotSupportQos,
    TxBlockAck(TxBlockAckError),
    RxBlockAck(StaRxBlockAckSessionsError),
    BeaconLoss(StaBeaconLossConfigError),
}

/// Complete owner return when connected policy validation fails.
#[derive(Debug)]
pub struct Esp32s31ConnectedStaPrepareFailure {
    pub error: Esp32s31ConnectedStaConfigError,
    pub peer: Esp32s31ConnectedStaPeer,
}

/// Validated driver plan derived from the exact associated peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaPlan {
    pub(super) interface: BoundVirtualInterface,
    pub(super) link: Esp32s31StaConnectedLink,
    pub(super) config: Esp32s31ConnectedStaConfig,
    pub(super) data_tx_rate: TxPhyRate,
    pub(super) aggregate_tx_rate: TxPhyRate,
    pub(super) beacon_loss: StaBeaconLossConfig,
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

    pub const fn rx_config(&self) -> ConnectedRxConfig {
        ConnectedRxConfig {
            station_address: self.link.station_address,
            bssid: self.link.bssid,
            association_id: self.link.association_id,
            ingress: self.config.rx_ingress,
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
                publication_limit: self.config.unicast_attempt_limit,
                publication_timeout_micros: self.config.completion_timeout_us,
            },
        }
    }

    pub const fn aggregate_tx_config(&self) -> AggregateTxConfig {
        AggregateTxConfig {
            rate: self.aggregate_tx_rate,
            frame_limit: self.config.aggregate_frame_limit,
            attempt_limit: self.config.unicast_attempt_limit,
            completion_timeout_us: self.config.completion_timeout_us,
            he_txop_limit: self.config.aggregate_he_txop_limit,
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

    /// Validate all value policy before consuming the peer's rate-control
    /// owner, pairwise key, sequences or pinned descriptor storage.
    #[allow(clippy::result_large_err)]
    pub fn prepare<const AGGREGATE_SLOTS: usize>(
        peer: Esp32s31ConnectedStaPeer,
        config: Esp32s31ConnectedStaConfig,
    ) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
        Self::prepare_with_storage::<AGGREGATE_SLOTS, RX_REORDER_BACKING_SLOT_COUNT>(peer, config)
    }

    /// Validate connected policy against the concrete TX aggregate and RX
    /// reorder storage selected by the board composition.
    ///
    /// Compact SRAM profiles must use this entry point. It prevents a runtime
    /// Block Ack window from retaining more MPDUs than the statically allocated
    /// reorder backing can own.
    #[allow(clippy::result_large_err)]
    pub fn prepare_with_storage<const AGGREGATE_SLOTS: usize, const RX_REORDER_SLOTS: usize>(
        peer: Esp32s31ConnectedStaPeer,
        config: Esp32s31ConnectedStaConfig,
    ) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
        let interface = BoundVirtualInterface::new(
            VirtualInterface::new(VifId::PRIMARY, VifRole::Station, peer.link.station_address),
            ChannelContextId::PRIMARY,
        );
        Self::prepare_for_interface_with_storage::<AGGREGATE_SLOTS, RX_REORDER_SLOTS>(
            peer, config, interface,
        )
    }

    /// Prepare one explicitly identified STA VIF on a hardware channel
    /// context. This is the multi-interface entry point; the compatibility
    /// `prepare*` methods bind the existing station to primary VIF/context.
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
        if config.aggregate_frame_limit == 0
            || usize::from(config.aggregate_frame_limit) > AGGREGATE_SLOTS
        {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::AggregateFrameLimit {
                    limit: config.aggregate_frame_limit,
                    capacity: AGGREGATE_SLOTS,
                },
                peer,
            });
        }
        if usize::from(config.rx_block_ack_maximum_window) > RX_REORDER_SLOTS {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::RxBlockAckWindowExceedsStorage {
                    window: config.rx_block_ack_maximum_window,
                    capacity: RX_REORDER_SLOTS,
                },
                peer,
            });
        }
        if config.unicast_attempt_limit == 0 {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::ZeroUnicastAttemptLimit,
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
            config.tx_block_ack_window,
            config.tx_block_ack_negotiation_timeout_us,
            config.tid0_amsdu,
        ) {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::TxBlockAck(error),
                peer,
            });
        }
        if let Err(error) =
            StaRxBlockAckSessions::with_maximum_window(config.rx_block_ack_maximum_window)
        {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::RxBlockAck(error),
                peer,
            });
        }
        let beacon_loss = match StaBeaconLossConfig::new(
            peer.link.beacon_interval_tu,
            config.beacon_miss_limit,
        ) {
            Ok(beacon_loss) => beacon_loss,
            Err(error) => {
                return Err(Esp32s31ConnectedStaPrepareFailure {
                    error: Esp32s31ConnectedStaConfigError::BeaconLoss(error),
                    peer,
                });
            }
        };

        let data_policy = sta_tx_rate_policy(peer.link, config.rate, false);
        let aggregate_policy = sta_tx_rate_policy(peer.link, config.rate, true);
        Ok(Esp32s31ConnectedStaPlan {
            interface,
            link: peer.link,
            config,
            data_tx_rate: data_policy.fallback_rate(),
            aggregate_tx_rate: peer.rate_control.ampdu_tx_rate(aggregate_policy),
            beacon_loss,
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
        peer_supports_ldpc: use_peer_capabilities && link.peer_supports_ldpc,
        peer_dcm_receive: if use_peer_capabilities {
            link.peer_dcm_receive
        } else {
            HeDcmConstellation::NotSupported
        },
    }
}
