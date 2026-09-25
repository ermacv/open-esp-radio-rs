//! Channel restoration over the captured archive's installed ROM callbacks
//! against compiled production.
//!
//! Full-root cases establish ordered channel effects, TX gain publication and
//! the committed channel, bandwidth and temperature. Temperature-prefix cases
//! cover every sensor window; their ordered-effect claim ends where the gain
//! bank is first read and is retained as separate evidence, never as full-root
//! equivalence. Stuck readiness is production containment, not vendor timing
//! equivalence. Software comparison under explicit peripheral inputs, never
//! hardware qualification.
use crate::calibration_prefix::delay_calls;
use crate::evidence::{events, output, stop};
use crate::harness::{Budget, Buffer, Input, Result, case, evidence, selection};
use crate::i2c::{all_complete, returned_low};
use crate::layout::*;
use crate::phy::{PhyImage, Right, image_layout, select};
use crate::session::{Session, request};
use crate::{I2C_LIBRARY_SHA, ROM_SHA};
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceBehavior, DeviceDeclaration, ExecutionCase,
    ExecutionEvent, ExecutionEvidence, ExecutionStop, Invocation, LinkRequest, RegionLifetime,
    RegisterCell, SessionReset,
};
use std::path::PathBuf;

/// Channel readiness status; bit 8 reports a completed frequency switch.
const CHANNEL_STATUS: u32 = 0x2010_0028;
const CHANNEL_READY: u32 = 0x100;
/// Temperature-sensor code read once per channel transition.
const TEMPERATURE: u32 = 0x2081_8000;
/// Gain-bank base read that starts TX gain publication.
const GAIN_BASE: u32 = 0x2010_0408;
/// Three gain-memory data words written per published entry.
const GAIN_DATA: [u32; 3] = [0x2010_0848, 0x2010_084c, 0x2010_0850];
/// Two TX capacitance command words at the start of the analog command RAM.
const TX_CAPACITANCE: [u32; 2] = [COMMAND_RAM, COMMAND_RAM + 4];
/// PLL control word retained beside the analog I2C transport controls.
const BBPLL_CONTROL: u32 = 0x2010_f818;
/// Baseband work mode, the one retained register that starts cleared.
const WORK_MODE: u32 = 0x2010_9c18;
/// Retained channel, AGC, baseband and gain registers; initial values are the
/// fill pattern except the work mode.
const RETAINED: [u32; 13] = [
    0x2010_7030,
    0x2010_001c,
    0x2010_7848,
    0x2010_4400,
    0x2010_7ce0,
    0x2010_7ce4,
    0x2010_702c,
    0x2010_70a0,
    WORK_MODE,
    0x2010_0874,
    GAIN_BASE,
    0x2010_0844,
    0x2010_703c,
];
/// Offsets of the committed channel, temperature and bandwidth in `phy_param`.
const PARAMETER_CHANNEL: usize = 284;
const PARAMETER_TEMPERATURE: usize = 0;
const PARAMETER_BANDWIDTH: usize = 287;
/// Production output: channel, temperature and bandwidth halfwords.
const SEMANTIC_BYTES: u32 = 6;
/// Untouched production output bytes.
const OUTPUT_FILL: u8 = 0xa5;
/// Case index of the transition after parameter setup and callback installation.
const TRANSITION: u32 = 2;
/// Requested delays while readiness never arrives: the frequency-switch
/// settle before bounded polling and the DAC read's transport delay. No
/// temperature-dependent command follows the failed switch.
const STUCK_DELAYS: usize = SETTLE_DELAYS + 1;
/// Retained temperature-sensor DAC analog selector (block 0x69, register 6).
const TEMPERATURE_DAC: u32 = 0x0669;
/// Retained PLL analog selector (block 0x6b, register 2).
const PLL_REGISTER: u32 = 0x026b;

pub struct Options {
    pub binary: PathBuf,
    pub library: PathBuf,
    pub rom: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: Budget,
}

/// One channel transition: requested channel, bandwidth, sensor DAC, sensor
/// code and stack/register fill.
#[derive(Clone, Copy, Debug)]
pub struct Transition {
    pub channel: u32,
    pub cbw: u32,
    pub dac: u32,
    pub code: u32,
    pub fill: u8,
}

impl Transition {
    fn label(&self) -> String {
        format!(
            "channel-{}-cbw{}-dac{:x}-code{}-fill{:x}",
            self.channel, self.cbw, self.dac, self.code, self.fill
        )
    }
}

/// Full-root matrix: every tracked channel class at both bandwidths.
pub fn full_transitions() -> Vec<Transition> {
    transitions(
        [1, 6, 11, 13]
            .into_iter()
            .flat_map(|channel| [0, 1].map(|cbw| (channel, cbw, 5, 100))),
    )
}

/// Temperature-prefix matrix: every sensor DAC range with boundary codes.
pub fn prefix_transitions() -> Vec<Transition> {
    transitions(
        [5, 7, 15, 11, 10]
            .into_iter()
            .flat_map(|dac| [0, 64, 100, 255].map(move |code| (13, 1, dac, code))),
    )
}

fn transitions(specs: impl Iterator<Item = (u32, u32, u32, u32)>) -> Vec<Transition> {
    specs
        .flat_map(|(channel, cbw, dac, code)| {
            FILLS.map(|fill| Transition {
                channel,
                cbw,
                // The DAC field is the low nibble; retained high bits follow the fill.
                dac: dac | u32::from(fill & 0xf0),
                code,
                fill,
            })
        })
        .collect()
}

/// Explicit peripheral inputs of one transition; no channel algorithm.
pub fn channel_models(transition: &Transition, ready: bool) -> Vec<DeviceDeclaration> {
    let fill = fill_word(transition.fill);
    let bank = |id: &str, applicability: &str, cells: Vec<(u32, u32)>| DeviceDeclaration {
        id: id.into(),
        applicability: applicability.into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: cells
                .into_iter()
                .map(|(address, value)| RegisterCell {
                    address,
                    width: 4,
                    value,
                })
                .collect(),
        },
    };
    let constant = |id: &str, address: u32, value: u32| DeviceDeclaration {
        id: id.into(),
        applicability: "explicit constant peripheral observation".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::ConstantRead {
            address,
            width: 4,
            value,
        },
    };
    let mut retained: Vec<(u32, u32)> = RETAINED
        .iter()
        .map(|a| (*a, if *a == WORK_MODE { 0 } else { fill }))
        .chain(GAIN_DATA.map(|a| (a, 0)))
        .chain(TX_CAPACITANCE.map(|a| (a, 0)))
        .collect();
    retained.sort();
    vec![
        analog_bank(
            "channel-analog",
            "explicit sensor DAC and PLL analog registers",
            0,
            [0, 0],
            [(TEMPERATURE_DAC, transition.dac), (PLL_REGISTER, 0)]
                .map(|(selector, initial)| CommandCell {
                    selector,
                    initial,
                    reads: None,
                })
                .to_vec(),
        ),
        bank(
            "transport-controls",
            "explicit retained PLL control, read-mask and host-map words",
            vec![(BBPLL_CONTROL, fill), (I2C_READ_MASK, 0), (I2C_HOST_MAP, 0)],
        ),
        bank(
            "channel-registers",
            "explicit retained channel, AGC, baseband and gain registers",
            retained,
        ),
        constant("temperature-code", TEMPERATURE, transition.code),
        constant(
            "channel-ready",
            CHANNEL_STATUS,
            if ready { CHANNEL_READY } else { 0 },
        ),
    ]
}

/// Channel image and its production probe.
pub struct Channel {
    pub image: PhyImage,
    rom_delay: u32,
    production_delay: u32,
}

impl std::ops::Deref for Channel {
    type Target = PhyImage;
    fn deref(&self) -> &PhyImage {
        &self.image
    }
}
impl std::ops::DerefMut for Channel {
    fn deref_mut(&mut self) -> &mut PhyImage {
        &mut self.image
    }
}

/// One ordered channel effect of the runner-side comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelEffect {
    Read(u32, u32),
    Write(u32, u32),
    Delay(u32),
}

fn is_transport_port(address: u32) -> bool {
    I2C_PORTS.contains(&address)
}

/// Ordered MMIO writes, non-transport reads and requested delays. Transport
/// port reads, transport control accesses and the single-microsecond delay
/// before a transport read are implementation plumbing, not channel effects.
pub fn channel_effects(observed: &[ExecutionEvent]) -> Vec<ChannelEffect> {
    let ordered: Vec<ChannelEffect> = observed
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Read {
                address,
                width: 4,
                value,
            } => Some(ChannelEffect::Read(*address, *value)),
            ExecutionEvent::Write {
                address,
                width: 4,
                value,
            } => Some(ChannelEffect::Write(*address, *value)),
            ExecutionEvent::Read { .. } | ExecutionEvent::Write { .. } => {
                panic!("non-word channel MMIO {event:?}")
            }
            ExecutionEvent::DelayMicros { value } => Some(ChannelEffect::Delay(*value)),
            _ => None,
        })
        .collect();
    ordered
        .iter()
        .enumerate()
        .filter(|(i, effect)| match effect {
            ChannelEffect::Read(address, _) => {
                !is_transport_port(*address) && !matches!(*address, I2C_READ_MASK | I2C_HOST_MAP)
            }
            ChannelEffect::Write(address, _) => !matches!(*address, I2C_READ_MASK | I2C_HOST_MAP),
            ChannelEffect::Delay(1) => !matches!(
                ordered.get(i + 1),
                Some(ChannelEffect::Read(address, _)) if is_transport_port(*address)
            ),
            ChannelEffect::Delay(_) => true,
        })
        .map(|(_, effect)| *effect)
        .collect()
}

/// Effects before the gain bank is first read; exactly one sensor read.
pub fn temperature_prefix(effects: &[ChannelEffect]) -> &[ChannelEffect] {
    let end = effects
        .iter()
        .position(|e| matches!(e, ChannelEffect::Read(GAIN_BASE, _)))
        .expect("channel must reach gain publication after temperature sampling");
    let prefix = &effects[..end];
    assert_eq!(
        prefix
            .iter()
            .filter(|e| matches!(e, ChannelEffect::Read(TEMPERATURE, _)))
            .count(),
        1
    );
    prefix
}

impl Channel {
    pub fn new(options: &Options) -> Result<Self> {
        let inputs = [
            Input {
                role: "phy",
                path: &options.library,
                sha256: Some(I2C_LIBRARY_SHA),
            },
            Input {
                role: "rom",
                path: &options.rom,
                sha256: Some(ROM_SHA),
            },
            Input {
                role: "production",
                path: &options.production,
                sha256: None,
            },
        ];
        let session = Session::start(
            &options.binary,
            &options.output,
            options.budget,
            &inputs,
            "captured channel restoration, temperature prefix and stuck readiness; no RF qualification",
        )?;
        let companions = COMPANIONS
            .iter()
            .map(|n| select(&session, 1, n))
            .collect::<Result<_>>()?;
        let link = LinkRequest {
            companions,
            revision: Some(session.revision.clone()),
            inputs: vec![0],
            entry: select(&session, 0, "phy_chip_set_chan")?,
            roots: vec![select(&session, 0, "phy_get_romfunc_addr")?],
            layout: image_layout(),
        };
        let image = PhyImage::link(session, &link, &options.linker, "phy_chip_set_chan")?;
        Ok(Self {
            rom_delay: image.sym(1, "ets_delay_us"),
            production_delay: image.sym(2, "ets_delay_us"),
            image,
        })
    }

    fn vendor_phase(&self, t: &Transition, ready: bool, delays: usize) -> Invocation {
        let mut phase = self.enter(
            self.root("phy_chip_set_chan"),
            &[t.channel, t.cbw],
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            channel_models(t, ready),
        );
        phase.calls = delay_calls("channel-delay", self.rom_delay, delays);
        phase
    }

    pub fn production_phase(
        &self,
        t: &Transition,
        ready: bool,
        delays: usize,
    ) -> Result<Invocation> {
        let probe = self.probes.invoke(
            "open_phy_channel_trace_state",
            vec![
                ("channel_or_frequency", i64::from(t.channel).into()),
                ("cbw", i64::from(t.cbw).into()),
                (
                    "output",
                    Buffer::new(OUTPUT, [OUTPUT_FILL; SEMANTIC_BYTES as usize]).into(),
                ),
            ],
            channel_models(t, ready),
            vec![selection(OUTPUT, SEMANTIC_BYTES)],
        )?;
        let mut phase = self.enter_probe(probe);
        phase.calls = delay_calls("channel-delay", self.production_delay, delays);
        Ok(phase)
    }

    /// Zeroed parameters, captured callback installation, then the transition.
    fn rows(
        &self,
        t: &Transition,
        delays: (usize, usize),
        install: bool,
    ) -> Result<Vec<ExecutionCase>> {
        let zero = vec![0u8; PHY_PARAM_BYTES as usize];
        let noop = self.noop();
        let mut transition = case(
            t.label(),
            self.vendor_phase(t, true, delays.0),
            Some(self.production_phase(t, true, delays.1)?),
            SessionReset::Warm,
            false,
        );
        let relation = transition.relation.as_mut().unwrap();
        // Polling counts and transport reads are implementation plumbing; the
        // ordered effect comparison below reviews every remaining read.
        relation.events.mmio_read = false;
        relation.events.delay = false;
        relation.returns.low = false;
        Ok(vec![
            case(
                "initialize",
                self.setup(&zero, false),
                Some(self.setup(&zero, true)),
                SessionReset::Cold,
                false,
            ),
            case(
                "install-captured-callbacks",
                if install {
                    self.install_callbacks(INSTALLED_CALLBACK_SLOT)?
                } else {
                    noop.clone()
                },
                Some(noop),
                SessionReset::Warm,
                false,
            ),
            transition,
        ])
    }
}

/// Captured ROM definitions closing the channel root's physical references.
const COMPANIONS: [&str; 26] = [
    "memcpy",
    "ets_delay_us",
    "phy_set_chan_reg",
    "phy_chan_to_freq",
    "phy_mhz2ieee",
    "phy_disable_agc",
    "phy_bbpll_cal",
    "phy_tsens_temp_read",
    "phy_set_channel_rfpll_freq",
    "phy_i2c_master_mem_txcap",
    "phy_bb_cbw_chan_cfg",
    "phy_enable_agc",
    "phy_param_addr",
    "phy_get_romfuncs",
    "phy_i2c_writeReg",
    "memset",
    "phy_get_data_sat",
    "phy_get_i2c_mst0_mask",
    "phy_i2c_paral_write_num",
    "phy_wait_i2c_sdm_stable",
    "phy_tsens_dac_cal",
    "phy_tsens_temp_read_local",
    "phy_txbbgain_to_index",
    "phy_write_gain_mem",
    "phy_bt_get_tx_gain",
    "phy_wifi_get_tx_gain",
];

/// Committed vendor channel, temperature and bandwidth as production output bytes.
pub fn vendor_semantic(parameters: &[u8]) -> [u8; SEMANTIC_BYTES as usize] {
    [
        parameters[PARAMETER_CHANNEL],
        parameters[PARAMETER_CHANNEL + 1],
        parameters[PARAMETER_TEMPERATURE],
        parameters[PARAMETER_TEMPERATURE + 1],
        parameters[PARAMETER_BANDWIDTH],
        0,
    ]
}

/// Independent semantic expectations and vendor/production agreement of one
/// completed transition. Returns the ordered effects of both sides.
fn check_transition(
    label: &str,
    t: &Transition,
    records: &[ExecutionEvidence],
) -> (Vec<ChannelEffect>, Vec<ChannelEffect>) {
    assert_eq!(returned_low(records, TRANSITION, true), Some(0), "{label}");
    for side in [false, true] {
        assert!(all_complete(records, TRANSITION, side), "{label} {side}");
    }
    let committed = vendor_semantic(&output(records, TRANSITION, false));
    let (temperature, _) = sensor_expectation(t.dac)(t.code);
    let mut expected = [0u8; SEMANTIC_BYTES as usize];
    expected[..2].copy_from_slice(&(t.channel as u16).to_le_bytes());
    expected[2..4].copy_from_slice(&(temperature as i16).to_le_bytes());
    expected[4] = t.cbw as u8;
    assert_eq!(committed, expected, "{label}: vendor commit");
    assert_eq!(
        output(records, TRANSITION, true),
        committed,
        "{label}: production commit"
    );
    (
        channel_effects(&events(records, TRANSITION, false)),
        channel_effects(&events(records, TRANSITION, true)),
    )
}

fn gain_writes(effects: &[ChannelEffect]) -> usize {
    effects
        .iter()
        .filter(|e| matches!(e, ChannelEffect::Write(a, _) if GAIN_DATA.contains(a)))
        .count()
}

pub fn exercise(ctx: &mut Channel) -> Result<()> {
    // Full-root evidence: every channel class and bandwidth, including TX gain publication.
    for t in full_transitions() {
        let label = format!("full-{}", t.label());
        let rows = ctx.rows(&t, delays(&t), true)?;
        let executed = ctx.compare(&label, rows, t.fill)?;
        let (vendor, production) = check_transition(&label, &t, &executed.records);
        assert_eq!(vendor, production, "{label}: channel effects");
        assert!(gain_writes(&vendor) > 0, "{label}: no gain publication");
    }
    // Temperature-prefix evidence: every sensor DAC window at boundary codes.
    // The claim is limited to effects before the gain bank is first read.
    for t in prefix_transitions() {
        let label = format!("prefix-{}", t.label());
        let rows = ctx.rows(&t, delays(&t), true)?;
        let executed = ctx.compare(&label, rows, t.fill)?;
        let (vendor, production) = check_transition(&label, &t, &executed.records);
        assert_eq!(
            temperature_prefix(&vendor),
            temperature_prefix(&production),
            "{label}: temperature prefix"
        );
    }
    stuck_readiness(ctx)?;
    negative(ctx)
}

/// Production containment: a channel that never reports readiness fails
/// without publishing gain or semantic output. Not vendor timing equivalence.
fn stuck_readiness(ctx: &mut Channel) -> Result<()> {
    for fill in FILLS {
        let t = Transition {
            channel: 13,
            cbw: 1,
            dac: 5,
            code: 100,
            fill,
        };
        let label = format!("stuck-readiness-fill{fill:x}");
        let mut row = case(
            &label,
            ctx.production_phase(&t, false, STUCK_DELAYS)?,
            None,
            SessionReset::Cold,
            false,
        );
        row.relation = None;
        let production = ctx.production.clone();
        let request = request(&production, None, Some(fill), vec![row], MAX_EVENTS);
        let records = evidence(&ctx.submit(&label, &request, None)?.document);
        assert_eq!(returned_low(&records, 0, false), Some(1), "{label}");
        assert!(all_complete(&records, 0, false), "{label}");
        assert_eq!(
            output(&records, 0, false),
            [OUTPUT_FILL; SEMANTIC_BYTES as usize],
            "{label}: semantic output published"
        );
        let observed = events(&records, 0, false);
        let effects = channel_effects(&observed);
        assert_eq!(gain_writes(&effects), 0, "{label}: gain published");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, ChannelEffect::Read(CHANNEL_STATUS, 0))),
            "{label}: readiness never sampled"
        );
    }
    Ok(())
}

/// A changed production channel is a DIFF, uninstalled callbacks leave the
/// vendor INCOMPLETE, and an undersized event capacity publishes nothing.
fn negative(ctx: &mut Channel) -> Result<()> {
    let t = full_transitions()[0];
    let mut changed = ctx.rows(&t, delays(&t), true)?;
    let other = Transition { channel: 6, ..t };
    changed[TRANSITION as usize].replacement =
        Some(ctx.production_phase(&other, true, delays(&other).1)?);
    ctx.image.execute(
        "negative-changed-channel",
        changed,
        t.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        MAX_EVENTS,
    )?;
    let uninstalled = ctx.rows(&t, delays(&t), false)?;
    let records = ctx
        .image
        .execute(
            "negative-uninstalled-callbacks",
            uninstalled,
            t.fill,
            Right::Production,
            ComparisonVerdict::Incomplete,
            MAX_EVENTS,
        )?
        .records;
    assert!(
        matches!(
            stop(&records, TRANSITION, false),
            ExecutionStop::Incomplete { .. }
        ),
        "uninstalled callbacks must not complete"
    );
    let rows = ctx.rows(&t, delays(&t), true)?;
    let limited = request(&ctx.vendor, Some(&ctx.production), Some(t.fill), rows, 1);
    ctx.capacity_failure("negative-capacity", &limited)
}

/// Sensor windows of the ROM `phy_tsens_attribute` object at 0x2f84_d9ec:
/// DAC, code calibration, and the inclusive accepted temperature range.
/// An independent research reading, used only as an expectation.
const SENSOR_WINDOWS: [(u32, i32, i32, i32); 5] = [
    (5, -2, 50, 125),
    (7, -1, 20, 100),
    (15, 0, -10, 80),
    (11, 1, -30, 50),
    (10, 2, -40, 20),
];

/// ROM `phy_code_to_temp`: saturating fixed-point code conversion.
pub fn temperature(code: u32, calibration: i32) -> i32 {
    let scaled = code as i32 * 44 - calibration * 2788;
    if scaled > 27_151 {
        250
    } else {
        ((scaled - 2052) / 100).max(-200)
    }
}

/// Expected temperature and whether the sample leaves the current DAC window.
pub fn sensor_expectation(dac: u32) -> impl Fn(u32) -> (i32, bool) {
    let (_, calibration, low, high) = *SENSOR_WINDOWS
        .iter()
        .find(|w| w.0 == dac & 0xf)
        .expect("tracked sensor DAC");
    move |code| {
        let value = temperature(code, calibration);
        (value, !(low..=high).contains(&value))
    }
}

/// Requested delays of one completed transition. Both sides settle the
/// frequency switch (1 and 10 us); production additionally waits one
/// microsecond before the transport read of each analog command: the DAC
/// read, the PLL write and the PLL read copied into TX capacitance memory,
/// plus the read-modify-write of an out-of-range DAC reselection.
const SETTLE_DELAYS: usize = 2;
const ANALOG_COMMANDS: usize = 3;
const RESELECT_COMMANDS: usize = 2;

fn delays(t: &Transition) -> (usize, usize) {
    let (_, reselect) = sensor_expectation(t.dac)(t.code);
    let commands = ANALOG_COMMANDS + if reselect { RESELECT_COMMANDS } else { 0 };
    (SETTLE_DELAYS, SETTLE_DELAYS + commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(dac: u32, fill: u8, code: u32) -> Transition {
        Transition {
            channel: 13,
            cbw: 1,
            dac: dac | u32::from(fill & 0xf0),
            code,
            fill,
        }
    }

    #[test]
    fn sensor_oracle_follows_rom_windows() {
        // DAC 5: 100 * 44 + 2 * 2788 - 2052 = 7924 -> 79 C, inside 50..=125.
        assert_eq!(sensor_expectation(5)(100), (79, false));
        // DAC 10: 64 * 44 - 2 * 2788 - 2052 = -4812 -> -48 C, below -40.
        assert_eq!(sensor_expectation(0xa0 | 10)(64), (-48, true));
        assert_eq!(sensor_expectation(11)(64), (-20, false));
        assert!(sensor_expectation(5)(255).1 && sensor_expectation(15)(0).1);
    }

    #[test]
    fn delay_budget_adds_reselection_commands() {
        assert_eq!(delays(&at(5, 0x5a, 100)), (2, 5));
        assert_eq!(delays(&at(5, 0x5a, 0)), (2, 7));
        assert_eq!(delays(&at(10, 0xa5, 64)), (2, 7));
    }

    #[test]
    fn matrices_cover_channels_windows_and_fills() {
        assert_eq!(full_transitions().len(), 4 * 2 * FILLS.len());
        let prefix = prefix_transitions();
        assert_eq!(prefix.len(), SENSOR_WINDOWS.len() * 4 * FILLS.len());
        for t in &prefix {
            assert_eq!(t.dac & 0xf0, u32::from(t.fill & 0xf0));
            assert!(SENSOR_WINDOWS.iter().any(|w| w.0 == t.dac & 0xf));
        }
    }

    #[test]
    fn effects_drop_transport_plumbing_only() {
        let read = |address, value| ExecutionEvent::Read {
            address,
            width: 4,
            value,
        };
        let write = |address, value| ExecutionEvent::Write {
            address,
            width: 4,
            value,
        };
        let delay = |value| ExecutionEvent::DelayMicros { value };
        let observed = [
            write(I2C_READ_MASK, 1),
            write(I2C_PORT_0, 0x0400_0669),
            delay(1),
            read(I2C_PORT_0, 0x0455_0669),
            read(TEMPERATURE, 100),
            delay(1),
            read(CHANNEL_STATUS, CHANNEL_READY),
            delay(10),
            read(I2C_HOST_MAP, 0),
        ];
        assert_eq!(
            channel_effects(&observed),
            [
                ChannelEffect::Write(I2C_PORT_0, 0x0400_0669),
                ChannelEffect::Read(TEMPERATURE, 100),
                ChannelEffect::Delay(1),
                ChannelEffect::Read(CHANNEL_STATUS, CHANNEL_READY),
                ChannelEffect::Delay(10),
            ]
        );
    }

    #[test]
    fn prefix_ends_at_gain_base_after_one_sample() {
        let effects = [
            ChannelEffect::Read(TEMPERATURE, 1),
            ChannelEffect::Write(WORK_MODE, 4),
            ChannelEffect::Read(GAIN_BASE, 0),
            ChannelEffect::Read(TEMPERATURE, 2),
        ];
        assert_eq!(temperature_prefix(&effects), &effects[..2]);
        let twice = [effects[0], effects[0], effects[2]];
        assert!(std::panic::catch_unwind(|| temperature_prefix(&twice).len()).is_err());
        assert!(std::panic::catch_unwind(|| temperature_prefix(&effects[..2]).len()).is_err());
    }

    #[test]
    fn semantic_bytes_select_channel_temperature_and_bandwidth() {
        let mut parameters = vec![0u8; PHY_PARAM_BYTES as usize];
        parameters[PARAMETER_CHANNEL..PARAMETER_CHANNEL + 2].copy_from_slice(&[13, 0]);
        parameters[..2].copy_from_slice(&(-48i16).to_le_bytes());
        parameters[PARAMETER_BANDWIDTH] = 1;
        assert_eq!(vendor_semantic(&parameters), [13, 0, 0xd0, 0xff, 1, 0]);
    }
}
