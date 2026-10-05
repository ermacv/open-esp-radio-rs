//! Station power save over the lower-MAC port.

use core::marker::PhantomData;
use oer_ieee80211_upper_mac_service::TxMpdu;
use oer_ieee80211_upper_mac_service::client::PortError;

use oer_ieee80211_lower_mac::{
    KeySelector, LowerMacBeaconTiming, LowerMacSetting, MacAddress, RxBeaconPriority, TbttSchedule,
    VifTsf,
};
use oer_ieee80211_mac::{
    qos::WmmAccessCategory,
    station::StaSequenceCounter,
    station_beacon::StaBeaconObservation,
    station_power_save::{STA_NULL_DATA_FRAME_LEN, StaNullDataFrame, StaPowerManagement},
};
use oer_ieee80211_sta::modem_sleep::{
    CoexView, ModemSleep, PmAction, PmActions, PmBeacon, PmClock, PmState, PmTbttSchedule, PmTim,
    PmTimer, PmTraffic, SleepType,
};
use oer_ieee80211_upper_mac::TxReport;
use oer_ieee80211_upper_mac_service::UpperMacTxError;
use oer_time::{Clock, Duration, Instant};

use super::link::{PortCoexistence, PortLink, PortLinkError, PortStationEnv};

/// Effect lists one input of the power manager may chain through Null
/// completions.
const ACTION_QUEUE: usize = 4;

const TIMERS: [PmTimer; 5] = [
    PmTimer::SliceEnd,
    PmTimer::Active,
    PmTimer::SleepDelay,
    PmTimer::Dream,
    PmTimer::Preemption,
];

const fn timer_index(timer: PmTimer) -> usize {
    match timer {
        PmTimer::SliceEnd => 0,
        PmTimer::Active => 1,
        PmTimer::SleepDelay => 2,
        PmTimer::Dream => 3,
        PmTimer::Preemption => 4,
    }
}

/// The station's modem sleep over the port.
///
/// [`ModemSleep`] decides; this driver performs its effects on the port:
///
/// | Effect | Port operation |
/// | --- | --- |
/// | `SendNull` | A Null Data frame with the Power Management bit, through the transmit planner; its completion feeds `null_done` |
/// | `BlockTx`, `RfSleep` | `LowerMacSetting::TxGate { open: false }`: the station dozes and no attempt is published |
/// | `UnblockTx`, `RfWake` | `TxGate { open: true }` |
/// | `StartTbtt`, `SetTbttInterval`, `SetTbttAhead`, `StopTbtt` | [`LowerMacBeaconTiming::set_tbtt`](oer_ieee80211_lower_mac::LowerMacBeaconTiming::set_tbtt) of the station interface, the TBTT lead being the TBTT and wake leads together |
/// | `Arm`, `Disarm` | Deadlines on the station's monotonic clock, due through [`Self::next_deadline`] |
/// | `ReleaseHeldFrames` | the connection is told to send the frame it held |
///
/// | `RxBeaconPriority`, `ClearRxBeaconPriority` | `LowerMacSetting::RxBeaconPriority`: the radio system's beacon-window priority, zero, or cleared |
/// | `Coex` | [`PortCoexistence::perform`] of the environment's radio system, whose [`PortCoexistence::view`] every input decides with |
///
/// Each beacon of the access point also sets the
/// station TSF to the beacon's timestamp, so the TBTTs follow the access
/// point.
pub struct PortPowerSave<P: LowerMacBeaconTiming> {
    modem: ModemSleep,
    port: PhantomData<fn(&P)>,
    deadlines: [Option<Instant>; 5],
    schedule: Option<PmTbttSchedule>,
    gate_open: bool,
    release: bool,
}

impl<P: LowerMacBeaconTiming> PortPowerSave<P> {
    pub(crate) const fn new(sleep_type: SleepType) -> Self {
        Self {
            modem: ModemSleep::new(sleep_type),
            port: PhantomData,
            deadlines: [None; 5],
            schedule: None,
            gate_open: true,
            release: false,
        }
    }

    pub const fn state(&self) -> PmState {
        self.modem.state()
    }

    /// Whether attempts leave the station: the transmit gate is open.
    pub const fn awake(&self) -> bool {
        self.gate_open
    }

    /// The earliest armed timer.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadlines.iter().flatten().copied().min()
    }

    /// Whether the manager asked for the held frame to be sent.
    pub(crate) fn take_release(&mut self) -> bool {
        core::mem::take(&mut self.release)
    }
}

/// What one input of the power manager needs from its connection.
pub(crate) struct PowerContext<'a, 'p, X: PortStationEnv> {
    pub link: &'a mut PortLink<'p, X>,
    pub timer: &'a X::Timer,
    pub sequence: &'a mut StaSequenceCounter,
    pub bssid: MacAddress,
    pub traffic: PmTraffic,
}

impl<X: PortStationEnv> PowerContext<'_, '_, X> {
    fn clock(&self) -> PmClock {
        PmClock {
            now: self.timer.now(),
        }
    }

    fn coex(&self) -> CoexView {
        self.link.coex().view()
    }
}

impl<P: LowerMacBeaconTiming> PortPowerSave<P> {
    /// Start power management of the association; `join_beacon` is the
    /// access point's beacon or Probe Response the station joined from.
    pub(crate) async fn start<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
        join_beacon: PmBeacon,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let mut actions = PmActions::new();
        self.modem.start(join_beacon, context.coex(), &mut actions);
        self.perform(context, actions).await
    }

    /// Stop power management; the station stays awake.
    pub(crate) async fn stop<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let mut actions = PmActions::new();
        self.modem.stop(context.coex(), &mut actions);
        self.perform(context, actions).await?;
        self.deadlines = [None; 5];
        self.set_gate(context.link, true)
    }

    /// A TBTT of the station interface.
    pub(crate) async fn tbtt<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let mut actions = PmActions::new();
        self.modem.tbtt(
            context.clock(),
            context.coex(),
            context.traffic,
            &mut actions,
        );
        self.perform(context, actions).await
    }

    /// A beacon of the access point.
    pub(crate) async fn beacon<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
        beacon: &StaBeaconObservation,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let vif = context.link.config().vif;
        // The station TSF follows the access point's.
        let _ = context
            .link
            .port()
            .set_tsf(VifTsf::new(vif, beacon.timestamp_tsf))
            .map_err(PortLinkError::Port)?;
        let mut actions = PmActions::new();
        self.modem.beacon(
            PmBeacon {
                timestamp_tsf: beacon.timestamp_tsf,
                interval_tu: beacon.interval_tu,
                tim: beacon.tim.map(|tim| PmTim {
                    dtim_count: tim.dtim_count,
                    dtim_period: tim.dtim_period,
                    unicast: tim.unicast_buffered,
                    group: tim.group_buffered,
                }),
            },
            context.clock(),
            context.coex(),
            context.traffic,
            &mut actions,
        );
        self.perform(context, actions).await
    }

    /// A data frame of the access point arrived.
    pub(crate) async fn rx_data<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
        group: bool,
        more_data: bool,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let mut actions = PmActions::new();
        self.modem
            .rx_data(group, more_data, context.coex(), &mut actions);
        self.perform(context, actions).await
    }

    /// The connection offers a data frame; whether it must wait for the
    /// station to leave power save.
    pub(crate) async fn tx_data<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
    ) -> Result<bool, PortLinkError<PortError<X>>> {
        let mut actions = PmActions::new();
        let wait = self.modem.tx_data(
            true,
            context.clock(),
            context.coex(),
            context.traffic,
            &mut actions,
        );
        self.perform(context, actions).await?;
        Ok(wait || !self.gate_open)
    }

    /// A data frame completed.
    pub(crate) async fn tx_data_done<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
        acknowledged: bool,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let mut actions = PmActions::new();
        self.modem.tx_data_done(
            acknowledged,
            context.clock(),
            context.coex(),
            context.traffic,
            &mut actions,
        );
        self.perform(context, actions).await
    }

    /// Run every timer that is due.
    pub(crate) async fn expire<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let now = context.timer.now();
        for timer in TIMERS {
            let index = timer_index(timer);
            if self.deadlines[index].is_none_or(|deadline| deadline > now) {
                continue;
            }
            self.deadlines[index] = None;
            let mut actions = PmActions::new();
            let traffic = context.traffic;
            match timer {
                PmTimer::SliceEnd => {
                    self.modem
                        .slice_end_timer(context.coex(), traffic, &mut actions);
                }
                PmTimer::Active => {
                    self.modem
                        .active_timer(context.coex(), traffic, &mut actions);
                }
                PmTimer::SleepDelay => self.modem.sleep_delay_timer(
                    context.clock(),
                    context.coex(),
                    traffic,
                    &mut actions,
                ),
                PmTimer::Dream => self.modem.dream_timer(&mut actions),
                PmTimer::Preemption => {
                    self.modem
                        .preemption_timer(context.coex(), traffic, &mut actions);
                }
            }
            self.perform(context, actions).await?;
        }
        Ok(())
    }

    fn set_gate<X: PortStationEnv<Port = P>>(
        &mut self,
        link: &PortLink<'_, X>,
        open: bool,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        link.apply(LowerMacSetting::TxGate { open })?;
        self.gate_open = open;
        Ok(())
    }

    fn program_tbtt<X: PortStationEnv<Port = P>>(
        &self,
        link: &PortLink<'_, X>,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let vif = link.config().vif;
        let outcome = match self.schedule {
            Some(schedule) => link.port().set_tbtt(TbttSchedule {
                next: VifTsf::new(vif, schedule.first_tbtt),
                beacon_interval: Duration::from_micros(u64::from(schedule.interval_micros)),
                lead: Duration::from_micros(u64::from(
                    schedule
                        .ahead_micros
                        .saturating_add(schedule.wake_ahead_micros),
                )),
            }),
            None => link.port().stop_tbtt(vif),
        };
        outcome
            .map_err(PortLinkError::Port)?
            .map_err(PortLinkError::Setting)
    }

    /// Perform the effects of one input in order, and then those the Null
    /// completions they caused return.
    async fn perform<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
        actions: PmActions,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let mut queue: [Option<PmActions>; ACTION_QUEUE] = [None; ACTION_QUEUE];
        queue[0] = Some(actions);
        let mut next = 0;
        let mut queued = 1;
        while next < queued {
            let actions = queue[next].take().expect("a queued effect list");
            next += 1;
            for action in actions.iter() {
                let Some(followers) = self.perform_one(context, action).await? else {
                    continue;
                };
                if !followers.is_empty() && queued < ACTION_QUEUE {
                    queue[queued] = Some(followers);
                    queued += 1;
                }
            }
        }
        Ok(())
    }

    /// Perform one effect; a sent Null returns the effects of its
    /// completion.
    async fn perform_one<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
        action: PmAction,
    ) -> Result<Option<PmActions>, PortLinkError<PortError<X>>> {
        let now = context.timer.now();
        match action {
            PmAction::SendNull { power_save } => {
                let before = self.modem.null_flags();
                if !self.gate_open {
                    self.modem.null_not_sent(power_save, before);
                    return Ok(None);
                }
                let acknowledged = match self.send_null(context, power_save).await {
                    Ok(report) => acknowledged(&report),
                    Err(PortLinkError::Tx(
                        UpperMacTxError::Refused(_) | UpperMacTxError::NoBuffer,
                    )) => {
                        self.modem.null_not_sent(power_save, before);
                        return Ok(None);
                    }
                    Err(error) => return Err(error),
                };
                let mut followers = PmActions::new();
                self.modem.null_done(
                    power_save,
                    acknowledged,
                    context.clock(),
                    context.coex(),
                    context.traffic,
                    &mut followers,
                );
                return Ok(Some(followers));
            }
            PmAction::BlockTx | PmAction::RfSleep => self.set_gate(context.link, false)?,
            PmAction::UnblockTx | PmAction::RfWake => self.set_gate(context.link, true)?,
            PmAction::Arm { timer, after } => {
                self.deadlines[timer_index(timer)] = now.checked_add(after);
            }
            PmAction::Disarm(timer) => self.deadlines[timer_index(timer)] = None,
            PmAction::ReleaseHeldFrames => self.release = true,
            PmAction::StartTbtt(schedule) => {
                self.schedule = Some(schedule);
                self.program_tbtt(context.link)?;
            }
            PmAction::SetTbttInterval(interval_micros) => {
                if let Some(schedule) = &mut self.schedule {
                    schedule.interval_micros = interval_micros;
                }
                self.program_tbtt(context.link)?;
            }
            PmAction::SetTbttAhead {
                ahead_micros,
                wake_ahead_micros,
            } => {
                if let Some(schedule) = &mut self.schedule {
                    schedule.ahead_micros = ahead_micros;
                    schedule.wake_ahead_micros = wake_ahead_micros;
                    self.program_tbtt(context.link)?;
                }
            }
            PmAction::StopTbtt => {
                self.schedule = None;
                self.program_tbtt(context.link)?;
            }
            PmAction::Coex(action) => context
                .link
                .coex_mut()
                .perform(action)
                .map_err(|_| PortLinkError::Coexistence)?,
            PmAction::RxBeaconPriority(beacon_window) => {
                context
                    .link
                    .apply(LowerMacSetting::RxBeaconPriority(if beacon_window {
                        RxBeaconPriority::BeaconWindow
                    } else {
                        RxBeaconPriority::Zero
                    }))?;
            }
            PmAction::ClearRxBeaconPriority => {
                context
                    .link
                    .apply(LowerMacSetting::RxBeaconPriority(RxBeaconPriority::Cleared))?;
            }
            // The hardware meaning of the vendor's beacon receive time is
            // not established, so the port has no setting for it yet.
            PmAction::RxBeaconTime { .. } => {}
        }
        Ok(None)
    }

    async fn send_null<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut PowerContext<'_, '_, X>,
        power_save: bool,
    ) -> Result<TxReport, PortLinkError<PortError<X>>> {
        let config = *context.link.config();
        let mut frame = [0_u8; STA_NULL_DATA_FRAME_LEN];
        StaNullDataFrame {
            station_address: config.address,
            bssid: context.bssid,
            sequence_number: context.sequence.take(),
            power_management: if power_save {
                StaPowerManagement::PowerSave
            } else {
                StaPowerManagement::Active
            },
        }
        .encode(&mut frame)
        .map_err(PortLinkError::Frame)?;
        context
            .link
            .transmit(
                TxMpdu::whole(&frame),
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                config.management_rate,
            )
            .await
    }
}

pub(crate) fn acknowledged(report: &TxReport) -> bool {
    match report {
        TxReport::Mpdu(status) => status.acknowledged == Some(true),
        TxReport::Ampdu(status) => status.block_acknowledged_subframes != 0,
    }
}
