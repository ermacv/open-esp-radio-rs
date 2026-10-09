//! Optional operations of the lower-MAC port.
//!
//! A structural feature a backend may lack is an extension trait over
//! [`Ieee80211LowerMacPort`], not a capability flag: an upper layer that
//! needs the feature requires the trait bound, and a backend without it
//! does not implement the trait, so the missing feature cannot be requested
//! at all. Each extension states its own parametric limits. Why a backend
//! lacks a feature is recorded in its qualification catalog, not in code.

use oer_ieee80211_mac::tsf::TsfInstant;
use oer_radio_port::CancelError;
use oer_time::Duration;

use crate::{
    Channel,
    capabilities::PhyFormatSet,
    control::{SettingError, VifId, VifRoleSet},
    port::{Ieee80211LowerMacPort, SubmitResult},
    tx::{TxBody, TxId},
};

/// Memory for the MPDUs of one A-MPDU, lent by the backend.
///
/// The caller appends the MPDUs in transmission order, each without FCS;
/// the backend adds delimiters and padding. It is released when the
/// attempt's completion is reported, as a [`TxBuffer`](crate::TxBuffer) is;
/// the bodies it holds stay with the backend until
/// [`Ieee80211LowerMacPort::reclaim_tx_bodies`] after the attempt ended. A
/// buffer released unsubmitted drops its bodies.
pub trait AmpduBuffer {
    /// The owner of an MPDU's body.
    type Body: TxBody;

    /// Append one MPDU of `len` bytes that ends with `body`, which the
    /// buffer takes, and return its octets before the body for writing; the
    /// body back when the buffer cannot hold another MPDU of that length or
    /// the body is longer than the MPDU.
    fn push_mpdu(
        &mut self,
        len: usize,
        body: Option<Self::Body>,
    ) -> Result<&mut [u8], Option<Self::Body>>;

    /// The MPDUs appended so far.
    fn subframes(&self) -> usize;
}

/// The payload of one A-MPDU attempt, answered by a BlockAck.
#[derive(Debug, Eq, PartialEq)]
pub struct AmpduPayload<A> {
    pub subframes: A,
    /// The traffic identifier all subframes share.
    pub tid: u8,
    /// The recipient's Minimum MPDU Start Spacing, the IEEE encoding 0-7 of
    /// its HT Capabilities A-MPDU Parameters.
    pub min_mpdu_start_spacing: u8,
}

/// The limits of A-MPDU attempts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AmpduCapabilities {
    /// Subframes of one A-MPDU attempt.
    pub max_subframes: u16,
    /// The PPDU formats an A-MPDU attempt is sent in; an attempt at a rate
    /// of another format is refused as `Unsupported`.
    pub formats: PhyFormatSet,
    /// The longest A-MPDU in octets, delimiters and padding included, that
    /// the backend sends at every rate of `formats`. The recipient's own
    /// Maximum A-MPDU Length stays the caller's to respect, as does the
    /// TXOP limit of the attempt's access category: the caller keeps the
    /// aggregate and its BlockAck within it, and a backend refuses no
    /// aggregate by it.
    pub max_length: u32,
}

/// One A-MPDU attempt.
pub type AmpduAttempt<A> = crate::TxAttempt<AmpduPayload<A>>;

/// A-MPDU transmission: one attempt sends one aggregate and reports the
/// recipient's BlockAck in its completion's
/// [`block_ack`](crate::TxCompletion::block_ack).
///
/// The attempt occupies its access category's queue like a single MPDU;
/// selecting which MPDUs a later aggregate retransmits stays above the port
/// unless the backend reports
/// [`HardwareServices::AMPDU_RETRY_SELECTION`](crate::HardwareServices::AMPDU_RETRY_SELECTION).
pub trait LowerMacAmpdu: Ieee80211LowerMacPort {
    type AmpduBuffer: AmpduBuffer<Body = Self::TxBody>;

    fn ampdu_capabilities(&self) -> AmpduCapabilities;

    /// Lend an aggregate buffer; `Ok(None)` when every one is in use.
    fn ampdu_buffer(&self) -> Result<Option<Self::AmpduBuffer>, Self::Error>;

    /// Take back an aggregate buffer the caller will not submit.
    fn release_ampdu_buffer(&self, buffer: Self::AmpduBuffer);

    /// Admit one A-MPDU attempt, with the base port's admission rules. An
    /// empty aggregate is `InvalidLength`; one above
    /// [`AmpduCapabilities::max_subframes`] or
    /// [`AmpduCapabilities::max_length`], or at a rate outside
    /// [`AmpduCapabilities::formats`], is `Unsupported`.
    fn submit_ampdu(
        &self,
        attempt: AmpduAttempt<Self::AmpduBuffer>,
    ) -> SubmitResult<AmpduAttempt<Self::AmpduBuffer>>;
}

/// The air an interface reserves for itself: the backend sends a CTS
/// addressed to the interface's own address whose Duration is `duration`,
/// so every station that hears it defers for that long (IEEE Std
/// 802.11-2020 10.3.2.4, the NAV).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AirReservation {
    pub duration: Duration,
}

/// The longest air reservation: the NAV a Duration field sets, 32 767 µs.
pub const MAX_AIR_RESERVATION: Duration = Duration::from_micros(32_767);

/// One air reservation attempt: it contends as its access category, at a
/// non-HT rate, with no key and no response.
pub type AirReservationAttempt = crate::TxAttempt<AirReservation>;

/// Air reservation: one attempt sends a CTS-to-self that keeps the
/// interface's medium for a while, as an owner that takes the radio off the
/// channel does. Its completion reports when the CTS went out.
pub trait LowerMacAirReservation: Ieee80211LowerMacPort {
    /// Admit one reservation, with the base port's admission rules. A
    /// duration above [`MAX_AIR_RESERVATION`], a rate other than a non-HT
    /// one, a key or an interface without an address is `Unsupported`.
    fn submit_air_reservation(
        &self,
        attempt: AirReservationAttempt,
    ) -> SubmitResult<AirReservationAttempt>;
}

/// Temporary channel changes without a port lifecycle transition.
///
/// A receive-only absence requires this independently of
/// [`LowerMacAirReservation`]. A backend that can tune only while disabled
/// does not implement it; ordinary channel moves may still use the base
/// port's channel setting and lifecycle.
pub trait LowerMacLiveRetune: Ieee80211LowerMacPort {
    /// Retune an enabled port after all admitted transmissions have ended.
    /// Success means the receiver is on `channel` when the call returns;
    /// it never just queues work for a runner. Refusal changes nothing;
    /// an outstanding transmission or a disabled port is `Busy`.
    ///
    /// Keep interface configuration, keys, RX BlockAck agreements, TX queue
    /// configuration and ownership, pending events, lent RX buffers and the
    /// TX gate intact. Preserve the radio-clock generation, each interface's
    /// advancing TSF and its relation generation, and absolute TBTT schedules.
    /// Do not hide Disable/Enable, aborted attempts or a TSF restart.
    /// The operation is synchronous and must not wait indefinitely; its
    /// settling latency must be qualified for the owner's window budget.
    fn retune_live(&self, channel: Channel) -> Result<Result<(), SettingError>, Self::Error>;
}

/// A value of one interface's TSF. Two interfaces count different TSFs (a
/// station follows its access point's, an access point keeps its own), so
/// arithmetic between values of different interfaces is an error.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VifTsf {
    pub vif: VifId,
    pub at: TsfInstant,
}

/// Two TSF values of different interfaces were combined.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TsfVifMismatch {
    pub left: VifId,
    pub right: VifId,
}

impl VifTsf {
    pub const fn new(vif: VifId, at: TsfInstant) -> Self {
        Self { vif, at }
    }

    /// The same interface's TSF `duration` later; `None` past the end of
    /// the TSF.
    pub const fn checked_add(self, duration: Duration) -> Option<Self> {
        match self.at.as_micros().checked_add(duration.as_micros()) {
            Some(at) => Some(Self {
                vif: self.vif,
                at: TsfInstant::from_micros(at),
            }),
            None => None,
        }
    }

    /// How long after `earlier` this value lies, saturating at zero; an
    /// error for a value of another interface.
    pub const fn saturating_duration_since(
        self,
        earlier: Self,
    ) -> Result<Duration, TsfVifMismatch> {
        if self.vif.0 != earlier.vif.0 {
            return Err(TsfVifMismatch {
                left: self.vif,
                right: earlier.vif,
            });
        }
        Ok(Duration::from_micros(
            self.at.as_micros().saturating_sub(earlier.at.as_micros()),
        ))
    }
}

/// When the backend reports target beacon transmission times.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TbttSchedule {
    /// A target beacon transmission time of the schedule, in the TSF of the
    /// interface the schedule is for.
    pub next: VifTsf,
    /// The beacon interval ([`time_units`](oer_ieee80211_mac::tsf::time_units) of the beacon's interval field).
    pub beacon_interval: Duration,
    /// How long before each TBTT its event is reported.
    pub lead: Duration,
}

/// One target beacon transmission time of an interface's schedule.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TbttEvent {
    /// The TBTT the event announces, in its interface's TSF.
    pub tbtt: VifTsf,
}

/// The largest rate difference between an interface's TSF and the port's
/// radio clock, in parts per million.
///
/// IEEE 802.11-2020 11.1.3 bounds every station's timer at ±0.01 %
/// (100 ppm). A station's TSF follows the access point's timer while the
/// radio clock runs on the station's own crystal, and the two may err in
/// opposite directions: the bound is the sum of both sides. A narrower
/// bound needs a cited tolerance of the station's crystal.
pub const TSF_DRIFT_PPM: u32 = 200;

/// One paired reading of an interface's TSF and the port's radio clock,
/// taken back to back by the backend, in the generation of their relation.
///
/// The relation breaks, and the generation changes, when the TSF jumps (a
/// new BSS, a reassociation, a channel change, a TSF set beyond the
/// backend's tolerance, an access point's TSF restart) and with every break
/// of the radio clock itself (its stamp's generation).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TsfSample {
    pub tsf: VifTsf,
    /// The port's radio clock beside it.
    pub local: crate::Ieee80211Stamp,
    /// How far apart the two readings may lie.
    pub uncertainty: Duration,
    /// The TSF relation the sample belongs to.
    pub generation: TsfGeneration,
}

/// The generation of a TSF relation: the epoch its owner took when it was
/// created, from a source shared by every owner of one radio start, and the
/// jumps since. Two owners never share an epoch, so a generation names one
/// stretch of one relation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TsfGeneration {
    pub epoch: u32,
    pub jump: u32,
}

/// The relation of one interface's TSF to the radio clock across TSF sets:
/// its generation and its last sample, the value the TSF was last set to.
///
/// The backend that owns an interface's TSF writes keeps one and records
/// every write. A set within what the relation predicts since its last
/// sample keeps the generation: [`TSF_DRIFT_PPM`] of the time since then,
/// rounded up, plus the sample's uncertainty. A beacon follow after missed
/// beacons corrects more drift and stays within it; a larger set, and the
/// first one, is a jump and starts a new generation. The TSF is not modular
/// here: a set to a value across 2^64, or one after the counter passed 2^64
/// since the last sample (a reading below it), is a jump, so the instants of
/// one generation always compare in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TsfRelation {
    generation: TsfGeneration,
    last_sample: Option<TsfInstant>,
    sample_uncertainty: Duration,
}

/// What a TSF set did to its relation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfSetKind {
    /// Within the drift since the last sample: the generation stays.
    Drift,
    /// Beyond it, or the first set: a new generation.
    Jump,
}

impl TsfRelation {
    /// A relation without a sample in `epoch`, a number its owner took from
    /// the shared source, whose samples are each uncertain by
    /// `sample_uncertainty` (the backend's TSF counter resolution).
    pub const fn new(epoch: u32, sample_uncertainty: Duration) -> Self {
        Self {
            generation: TsfGeneration { epoch, jump: 0 },
            last_sample: None,
            sample_uncertainty,
        }
    }

    /// The current generation.
    pub const fn generation(&self) -> TsfGeneration {
        self.generation
    }

    /// Start a new generation without a sample: the TSF jumped (a new
    /// association, a channel change, a timer restart).
    pub fn break_relation(&mut self) {
        self.generation.jump = self.generation.jump.wrapping_add(1);
        self.last_sample = None;
    }

    /// How far a set `elapsed` after the last sample may move the TSF
    /// without breaking the relation.
    pub fn tolerance(&self, elapsed: Duration) -> Duration {
        let drift =
            (u128::from(elapsed.as_micros()) * u128::from(TSF_DRIFT_PPM)).div_ceil(1_000_000);
        Duration::from_micros(
            u64::try_from(drift)
                .unwrap_or(u64::MAX)
                .saturating_add(self.sample_uncertainty.as_micros()),
        )
    }

    /// Record a set of the TSF from `current`, the value read just before
    /// it, to `value`.
    pub fn set(&mut self, current: TsfInstant, value: TsfInstant) -> TsfSetKind {
        let kind = match self.last_sample {
            // The counter only advances from the last set: a reading below
            // it passed 2^64.
            Some(last) if current < last => TsfSetKind::Jump,
            Some(last) => {
                let elapsed = Duration::from_micros(current.as_micros() - last.as_micros());
                let moved = value.as_micros().abs_diff(current.as_micros());
                if moved <= self.tolerance(elapsed).as_micros() {
                    TsfSetKind::Drift
                } else {
                    TsfSetKind::Jump
                }
            }
            None => TsfSetKind::Jump,
        };
        if kind == TsfSetKind::Jump {
            self.break_relation();
        }
        self.last_sample = Some(value);
        kind
    }
}

/// Why a TSF projection has no value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfProjectionError {
    /// The radio stamp belongs to another generation of the radio clock.
    StaleStamp,
    /// The sample belongs to another generation of the TSF relation.
    StaleSample,
    /// The projected value lies outside the representable range.
    OutOfRange,
}

impl TsfSample {
    /// The TSF of this sample's interface when the port's radio clock read
    /// `local`, with the sample's uncertainty plus the drift over the
    /// distance from the sample; `current` is the relation's generation now.
    pub fn tsf_at(
        &self,
        local: crate::Ieee80211Stamp,
        current: TsfGeneration,
    ) -> Result<oer_radio_port::Projected<VifTsf>, TsfProjectionError> {
        self.check(current)?;
        if local.generation != self.local.generation {
            return Err(TsfProjectionError::StaleStamp);
        }
        let (at, distance) = shift(
            self.tsf.at.as_micros(),
            self.local.at.as_micros(),
            local.at.as_micros(),
        )?;
        Ok(oer_radio_port::Projected {
            at: VifTsf::new(self.tsf.vif, TsfInstant::from_micros(at)),
            uncertainty: drift_bound(self.uncertainty, distance)?,
        })
    }

    /// The port's radio clock when this sample's interface's TSF reads
    /// `tsf`, in the sample's radio-clock generation; the inverse of
    /// [`Self::tsf_at`]. An error for a TSF of another interface.
    pub fn local_at(
        &self,
        tsf: VifTsf,
        current: TsfGeneration,
    ) -> Result<
        Result<oer_radio_port::Projected<crate::Ieee80211Stamp>, TsfProjectionError>,
        TsfVifMismatch,
    > {
        if tsf.vif.0 != self.tsf.vif.0 {
            return Err(TsfVifMismatch {
                left: tsf.vif,
                right: self.tsf.vif,
            });
        }
        Ok((|| {
            self.check(current)?;
            let (at, distance) = shift(
                self.local.at.as_micros(),
                self.tsf.at.as_micros(),
                tsf.at.as_micros(),
            )?;
            Ok(oer_radio_port::Projected {
                at: crate::Ieee80211Stamp {
                    at: crate::Ieee80211Instant::from_micros(at),
                    generation: self.local.generation,
                },
                uncertainty: drift_bound(self.uncertainty, distance)?,
            })
        })())
    }

    /// Whether the sample belongs to the relation's `current` generation.
    fn check(&self, current: TsfGeneration) -> Result<(), TsfProjectionError> {
        if self.generation == current {
            Ok(())
        } else {
            Err(TsfProjectionError::StaleSample)
        }
    }
}

/// `target_base` moved by `at - source_base`, and that distance.
fn shift(target_base: u64, source_base: u64, at: u64) -> Result<(u64, u64), TsfProjectionError> {
    let shifted = if at >= source_base {
        target_base.checked_add(at - source_base)
    } else {
        target_base.checked_sub(source_base - at)
    };
    shifted
        .map(|shifted| (shifted, at.abs_diff(source_base)))
        .ok_or(TsfProjectionError::OutOfRange)
}

/// `uncertainty` plus [`TSF_DRIFT_PPM`] of `distance`, rounded up.
fn drift_bound(uncertainty: Duration, distance: u64) -> Result<Duration, TsfProjectionError> {
    let drift = (u128::from(distance) * u128::from(TSF_DRIFT_PPM)).div_ceil(1_000_000);
    u64::try_from(drift)
        .ok()
        .and_then(|drift| uncertainty.as_micros().checked_add(drift))
        .map(Duration::from_micros)
        .ok_or(TsfProjectionError::OutOfRange)
}

/// Which roles the beacon-timing operations serve.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BeaconTimingCapabilities {
    /// Roles whose TSF [`LowerMacBeaconTiming::tsf`] reads.
    pub tsf_read: VifRoleSet,
    /// Roles whose TSF [`LowerMacBeaconTiming::set_tsf`] sets to any value.
    pub tsf_set: VifRoleSet,
    /// Roles whose TSF [`LowerMacBeaconTiming::set_tsf`] restarts from zero.
    pub tsf_restart: VifRoleSet,
    /// Roles whose TBTT schedule [`LowerMacBeaconTiming::set_tbtt`]
    /// programs.
    pub tbtt: VifRoleSet,
}

/// The TSF and TBTT schedule of each interface.
///
/// An operation on an interface whose role the
/// [`BeaconTimingCapabilities`] exclude is refused as
/// [`SettingError::Unsupported`].
pub trait LowerMacBeaconTiming: Ieee80211LowerMacPort {
    fn beacon_timing_capabilities(&self) -> BeaconTimingCapabilities;

    /// The TSF of a configured interface.
    fn tsf(&self, vif: VifId) -> Result<Result<VifTsf, SettingError>, Self::Error>;

    /// The TSF of a configured interface and the port's radio clock read
    /// back to back, in the current generation of their relation.
    fn tsf_sample(&self, vif: VifId) -> Result<Result<TsfSample, SettingError>, Self::Error>;

    /// Set the TSF of the interface `tsf` names.
    fn set_tsf(&self, tsf: VifTsf) -> Result<Result<(), SettingError>, Self::Error>;

    /// Report the target beacon transmission times of the schedule's
    /// interface as events.
    fn set_tbtt(&self, schedule: TbttSchedule) -> Result<Result<(), SettingError>, Self::Error>;

    /// Stop reporting an interface's target beacon transmission times.
    fn stop_tbtt(&self, vif: VifId) -> Result<Result<(), SettingError>, Self::Error>;

    /// The TBTT an event reports; `None` for another event. Its base view
    /// is [`LowerMacEvent::Extension`](crate::LowerMacEvent::Extension).
    fn tbtt(event: &Self::Event) -> Option<TbttEvent>;
}

/// The limits of monitor reception.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MonitorCapabilities {
    /// Whether monitor reception runs while an interface has a receive
    /// filter other than `ReceiveFilter::NONE`.
    pub with_receiving_interfaces: bool,
}

/// Monitor reception: every frame the PHY decodes with a valid FCS is
/// delivered as [`LowerMacEvent::Received`](crate::LowerMacEvent::Received),
/// whatever its addresses and the interfaces' filters.
pub trait LowerMacMonitor: Ieee80211LowerMacPort {
    fn monitor_capabilities(&self) -> MonitorCapabilities;

    /// Start or stop monitor reception.
    fn set_monitor(&self, enabled: bool) -> Result<Result<(), SettingError>, Self::Error>;
}

/// Ending a published attempt on the air.
pub trait LowerMacCancelPublished: Ieee80211LowerMacPort {
    /// Withdraw an admitted attempt from the hardware, published or not.
    /// Its terminal event is its completion,
    /// [`TxStatus::Aborted`](crate::TxStatus::Aborted) unless it had already
    /// completed; the medium may have carried part of it.
    fn cancel_published(&self, id: TxId) -> Result<Result<(), CancelError>, Self::Error>;
}
