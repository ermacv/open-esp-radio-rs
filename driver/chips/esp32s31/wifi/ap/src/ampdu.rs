//! AP adapter for the shared retained-DMA HT A-MPDU mechanism.
//!
//! The MAC crate owns descriptor assembly, BlockAck sampling and retained
//! retry compaction. This module supplies only AP peer identity, pairwise key
//! authority and interface-specific publication configuration.

use open_esp_radio_dma::StableDmaBacking;
use open_esp_radio_esp32s31_hal::types::MacInterface;
use open_esp_radio_esp32s31_wifi::ampdu_tx::{
    AmpduTxRoleAdapter, HtAmpduTxRolePolicy, HtAmpduTxRolePolicyError,
};
use open_esp_radio_esp32s31_wifi_mac::{
    tx::{HtRate, LegacyTxQueue, TxCookie},
    tx_ampdu::{
        AmpduFrameLayout, AmpduFrameSize, HtAmpduFrameRequest, HtAmpduHardware, HtAmpduTxError,
        HtAmpduTxResources, RetainedAmpduDmaStorage, RetainedAmpduRetryCompletionError,
        RetainedDmaAmpduTx, TX_AMPDU_METADATA_SIZE,
    },
    tx_protection::TxProtectionAdmissionError,
    tx_runtime::{AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState},
};
use open_esp_radio_wifi_ap::ApAssociationIdentity;

use crate::{
    engine::{Esp32s31ApAggregateBinding, Esp32s31ApAggregateFrame},
    tx::Esp32s31ApTx,
};

/// AP peer decision captured before consuming a network lease into an
/// aggregate transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApAggregateAdmission {
    binding: Esp32s31ApAggregateBinding,
    association: ApAssociationIdentity,
    rate: HtRate,
    block_ack_window: u16,
    amsdu: bool,
}

impl Esp32s31ApAggregateAdmission {
    pub(crate) const fn new(
        binding: Esp32s31ApAggregateBinding,
        association: ApAssociationIdentity,
        rate: HtRate,
        block_ack_window: u16,
        amsdu: bool,
    ) -> Self {
        Self {
            binding,
            association,
            rate,
            block_ack_window,
            amsdu,
        }
    }

    pub const fn peer(self) -> [u8; 6] {
        self.binding.peer()
    }

    pub const fn association(self) -> ApAssociationIdentity {
        self.association
    }

    pub const fn binding(self) -> Esp32s31ApAggregateBinding {
        self.binding
    }

    pub const fn rate(self) -> HtRate {
        self.rate
    }

    /// Whether the exact operational TID-0 agreement echoed A-MSDU support.
    pub const fn amsdu(self) -> bool {
        self.amsdu
    }

    pub fn accepts_ethernet(self, ethernet: &[u8]) -> bool {
        ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
            == Some(self.peer())
    }

    pub fn bind_policy(
        self,
        hardware_key_selector: u8,
        arena_capacity: usize,
    ) -> Result<HtAmpduTxRolePolicy, HtAmpduTxRolePolicyError> {
        HtAmpduTxRolePolicy::new(
            AmpduTxRoleAdapter {
                interface: MacInterface::AccessPoint,
                hardware_key_selector,
            },
            self.rate,
            self.block_ack_window,
            u8::try_from(arena_capacity).unwrap_or(u8::MAX),
            arena_capacity,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApAmpduError {
    Busy,
    Idle,
    PeerChanged,
    KeyChanged,
    SequenceDiscontinuity,
    TooFewFrames,
    Geometry,
    HardwareDidNotDetach,
    DeadlineOverflow,
    ConflictingInterruptEvents(u32),
    CompletionInterruptWithoutState,
    Hardware(HtAmpduTxError),
    Retry(AmpduRetryError),
    RolePolicy(HtAmpduTxRolePolicyError),
    Protection(TxProtectionAdmissionError),
}

impl From<HtAmpduTxError> for Esp32s31ApAmpduError {
    fn from(error: HtAmpduTxError) -> Self {
        Self::Hardware(error)
    }
}

impl From<AmpduRetryError> for Esp32s31ApAmpduError {
    fn from(error: AmpduRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<RetainedAmpduRetryCompletionError> for Esp32s31ApAmpduError {
    fn from(error: RetainedAmpduRetryCompletionError) -> Self {
        match error {
            RetainedAmpduRetryCompletionError::Hardware(error) => Self::Hardware(error),
            RetainedAmpduRetryCompletionError::Retry(error) => Self::Retry(error),
        }
    }
}

impl From<HtAmpduTxRolePolicyError> for Esp32s31ApAmpduError {
    fn from(error: HtAmpduTxRolePolicyError) -> Self {
        Self::RolePolicy(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApPreparedAmpdu {
    pub peer: [u8; 6],
    pub rate: HtRate,
    pub first_sequence: u16,
    pub subframes: u8,
    pub aggregate_length: u16,
    pub hardware_key_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApAmpduCompletion {
    pub tx_status: u8,
    pub block_ack_received: bool,
    pub block_ack_control: u8,
    pub first_sequence: u16,
    pub starting_sequence: u16,
    pub subframes: u8,
    pub missing: u8,
    pub acknowledged: u8,
    pub aggregate_attempts: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApAmpduProgress {
    Pending,
    Republished(Esp32s31ApAmpduCompletion),
    /// Hardware/BlockAck processing is terminal, while the detached DMA
    /// backing remains retained until the caller performs the explicit
    /// release edge.
    CompletionReady(Esp32s31ApAmpduCompletion),
}

enum ApAmpduState<const SLOTS: usize> {
    Idle,
    Building {
        cookie: TxCookie,
        peer: [u8; 6],
        rate: HtRate,
        first_sequence: u16,
        next_sequence: u16,
        hardware_key_selector: u8,
    },
    Hardware {
        cookie: TxCookie,
        rate: HtRate,
        hardware_key_selector: u8,
        retry: AmpduRetryState<SLOTS>,
    },
    Completed {
        cookie: TxCookie,
    },
}

/// One AP publication owner over the same retained-DMA mechanism used by STA.
pub struct Esp32s31ApAmpduTx<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize> {
    inner: RetainedDmaAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>,
    state: ApAmpduState<SLOTS>,
    attempt_limit: u8,
}

impl<'storage, B: StableDmaBacking + 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    Esp32s31ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        resources: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        maximum_aggregate_bytes: u16,
        attempt_limit: u8,
    ) -> Result<Self, Esp32s31ApAmpduError> {
        if SLOTS < 2 || SLOTS > 32 || attempt_limit == 0 {
            return Err(Esp32s31ApAmpduError::TooFewFrames);
        }
        let mut inner = RetainedDmaAmpduTx::new(resources, retention);
        inner.configure_max_aggregate_bytes(maximum_aggregate_bytes)?;
        Ok(Self {
            inner,
            state: ApAmpduState::Idle,
            attempt_limit,
        })
    }

    pub fn begin(
        &mut self,
        peer: [u8; 6],
        rate: HtRate,
        first_sequence: u16,
        hardware_key_selector: u8,
    ) -> Result<(), Esp32s31ApAmpduError> {
        if !matches!(self.state, ApAmpduState::Idle) {
            return Err(Esp32s31ApAmpduError::Busy);
        }
        let cookie = self.inner.begin()?;
        let first_sequence = first_sequence & 0x0fff;
        self.state = ApAmpduState::Building {
            cookie,
            peer,
            rate,
            first_sequence,
            next_sequence: first_sequence,
            hardware_key_selector,
        };
        Ok(())
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.state, ApAmpduState::Idle)
    }

    pub fn push(
        &mut self,
        peer: [u8; 6],
        backing: B,
        frame: Esp32s31ApAggregateFrame,
    ) -> Result<(), Esp32s31ApAmpduError> {
        let ApAmpduState::Building {
            cookie,
            peer: expected_peer,
            rate,
            next_sequence,
            hardware_key_selector,
            ..
        } = &mut self.state
        else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        if peer != *expected_peer {
            return Err(Esp32s31ApAmpduError::PeerChanged);
        }
        if frame.hardware_key_selector != *hardware_key_selector {
            return Err(Esp32s31ApAmpduError::KeyChanged);
        }
        if frame.sequence_number != *next_sequence {
            return Err(Esp32s31ApAmpduError::SequenceDiscontinuity);
        }
        let dma_offset = frame
            .encoded
            .offset
            .checked_sub(TX_AMPDU_METADATA_SIZE)
            .ok_or(Esp32s31ApAmpduError::Geometry)?;
        let layout = AmpduFrameLayout::new(
            dma_offset,
            AmpduFrameSize::new(
                frame.encoded.length,
                open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE as u8,
            ),
        )
        .ok_or(Esp32s31ApAmpduError::Geometry)?;
        self.inner
            .commit_ht(*cookie, backing, HtAmpduFrameRequest::new(layout, 0, *rate))?;
        *next_sequence = (*next_sequence + 1) & 0x0fff;
        Ok(())
    }

    pub fn prepared(&self) -> Result<Esp32s31ApPreparedAmpdu, Esp32s31ApAmpduError> {
        let ApAmpduState::Building {
            cookie,
            peer,
            rate,
            first_sequence,
            hardware_key_selector,
            ..
        } = self.state
        else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        let aggregate = self.inner.prepared_aggregate(cookie)?;
        if aggregate.subframes < 2 {
            return Err(Esp32s31ApAmpduError::TooFewFrames);
        }
        Ok(Esp32s31ApPreparedAmpdu {
            peer,
            rate,
            first_sequence,
            subframes: aggregate.subframes,
            aggregate_length: aggregate.bytes,
            hardware_key_selector,
        })
    }

    pub fn publish<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H: HtAmpduHardware>(
        &mut self,
        ordinary: &mut Esp32s31ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
    ) -> Result<Esp32s31ApPreparedAmpdu, Esp32s31ApAmpduError>
    where
        P: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerProfile,
        E: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxEntropy,
        T: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxTimer,
    {
        let prepared = self.prepared()?;
        ordinary
            .require_unprotected_ht_aggregate(prepared.rate)
            .map_err(Esp32s31ApAmpduError::Protection)?;
        let config = ordinary
            .ht_ampdu_config(
                prepared.rate,
                prepared.aggregate_length,
                prepared.subframes,
                prepared.hardware_key_selector,
            )
            .ok_or(Esp32s31ApAmpduError::Geometry)?;
        let ApAmpduState::Building { cookie, .. } = self.state else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        self.inner
            .submit(hardware, cookie, LegacyTxQueue::BestEffort, config)?;
        self.state = ApAmpduState::Hardware {
            cookie,
            rate: prepared.rate,
            hardware_key_selector: prepared.hardware_key_selector,
            retry: AmpduRetryState::new(
                prepared.first_sequence,
                prepared.subframes,
                AmpduRetryPolicy {
                    attempt_limit: self.attempt_limit,
                    retain_single_mpdu: true,
                },
            )?,
        };
        Ok(prepared)
    }

    /// Process one hardware observation without conflating an absent
    /// completion with a retained retry publication.
    pub fn service_completion<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H: HtAmpduHardware>(
        &mut self,
        ordinary: &mut Esp32s31ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
    ) -> Result<Esp32s31ApAmpduProgress, Esp32s31ApAmpduError>
    where
        P: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerProfile,
        E: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxEntropy,
        T: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxTimer,
    {
        let ApAmpduState::Hardware {
            cookie,
            rate,
            hardware_key_selector,
            mut retry,
        } = core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        let Some(observed) = self
            .inner
            .observe_retry_completion(hardware, cookie, &mut retry)?
        else {
            self.state = ApAmpduState::Hardware {
                cookie,
                rate,
                hardware_key_selector,
                retry,
            };
            return Ok(Esp32s31ApAmpduProgress::Pending);
        };
        let completion = observed.completion;
        let current_subframes = observed.subframes;
        let current_first_sequence = observed.first_sequence;
        let decision = observed.decision;
        let observation = Esp32s31ApAmpduCompletion {
            tx_status: completion.tx.status(),
            block_ack_received: completion.block_ack_received,
            block_ack_control: completion.block_ack.control,
            first_sequence: current_first_sequence,
            starting_sequence: completion.block_ack.block_ack.starting_sequence,
            subframes: current_subframes,
            missing: decision.missing(),
            acknowledged: retry.acknowledged(),
            aggregate_attempts: retry.aggregate_attempts(),
        };
        if let AmpduRetryDecision::RetainAggregate { retry_mask } = decision {
            let aggregate = self.inner.retain_for_ampdu_retry(cookie, retry_mask)?;
            ordinary.record_aggregate_retry_failure();
            let refreshed = ordinary
                .ht_ampdu_config(
                    rate,
                    aggregate.bytes,
                    aggregate.subframes,
                    hardware_key_selector,
                )
                .ok_or(Esp32s31ApAmpduError::Geometry)?;
            self.inner
                .submit(hardware, cookie, LegacyTxQueue::BestEffort, refreshed)?;
            self.state = ApAmpduState::Hardware {
                cookie,
                rate,
                hardware_key_selector,
                retry,
            };
            return Ok(Esp32s31ApAmpduProgress::Republished(observation));
        }
        if decision.missing() == 0 {
            ordinary.record_aggregate_success();
        } else {
            ordinary.reset_aggregate_contention();
        }
        self.state = ApAmpduState::Completed { cookie };
        Ok(Esp32s31ApAmpduProgress::CompletionReady(observation))
    }

    /// Release the exact detached terminal batch after the caller has
    /// finished completion classification and observation.
    pub fn release_completed(&mut self) -> Result<(), Esp32s31ApAmpduError> {
        let ApAmpduState::Completed { cookie } =
            core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        self.inner.release_completed(cookie)?;
        Ok(())
    }

    pub fn cancel_build(&mut self) -> Result<(), Esp32s31ApAmpduError> {
        let ApAmpduState::Building { cookie, .. } =
            core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        self.inner.cancel(cookie)?;
        Ok(())
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<bool, Esp32s31ApAmpduError> {
        let ApAmpduState::Hardware { cookie, .. } = self.state else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        Ok(self.inner.begin_timeout_abort(hardware, cookie)?)
    }

    /// Finish a timeout abort after the caller-owned hardware settle delay.
    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31ApAmpduError> {
        let ApAmpduState::Hardware { cookie, .. } =
            core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        self.inner.finish_timeout_abort(hardware, cookie)?;
        Ok(())
    }

    pub fn abort_collision<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<bool, Esp32s31ApAmpduError> {
        let ApAmpduState::Hardware { cookie, .. } = self.state else {
            return Err(Esp32s31ApAmpduError::Idle);
        };
        if !self.inner.abort_collision(hardware, cookie)? {
            return Ok(false);
        }
        self.state = ApAmpduState::Idle;
        Ok(true)
    }

    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(
        self,
    ) -> Result<
        (
            HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
            &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        ),
        Self,
    > {
        if !matches!(self.state, ApAmpduState::Idle) {
            return Err(self);
        }
        let Self {
            inner,
            state: _,
            attempt_limit,
        } = self;
        inner.try_into_resources().map_err(|inner| Self {
            inner,
            state: ApAmpduState::Idle,
            attempt_limit,
        })
    }
}
