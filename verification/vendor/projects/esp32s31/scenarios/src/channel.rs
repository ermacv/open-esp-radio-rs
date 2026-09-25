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
use crate::evidence::{PhyEffect, environment_reads, events, output, phy_effects, stop};
use crate::harness::{Buffer, Result, case, evidence, selection};
use crate::i2c::{all_complete, returned_low};
use crate::layout::*;
use crate::phy::delay_calls;
use crate::phy::{PhyImage, PhyOptions, Right, image_layout, select, start_session};
use crate::session::request;
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceDeclaration, ExecutionCase, ExecutionEvidence,
    ExecutionStop, Invocation, LinkRequest, SessionReset,
};

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
/// Retained temperature-sensor DAC analog selector (block 0x69, register 6).
const TEMPERATURE_DAC: u32 = 0x0669;
/// Retained PLL analog selector (block 0x6b, register 2).
const PLL_REGISTER: u32 = 0x026b;

pub type Options = PhyOptions;

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
/// Explicit peripheral inputs of one transition; every other radio register
/// is retained storage starting with the fill pattern. No channel algorithm.
pub fn channel_models(transition: &Transition, ready: bool) -> Vec<DeviceDeclaration> {
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
        register_bank(
            "channel-inputs",
            "cleared work mode and transport read-mask and host-map words",
            vec![(WORK_MODE, 0), (I2C_READ_MASK, 0), (I2C_HOST_MAP, 0)],
        ),
        constant_read("temperature-code", TEMPERATURE_CODE, transition.code),
        constant_read(
            "channel-ready",
            CHANNEL_STATUS,
            if ready { CHANNEL_READY } else { 0 },
        ),
        radio_aperture(transition.fill),
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

/// Effects before the gain bank is first read; exactly one sensor read.
pub fn temperature_prefix(effects: &[PhyEffect]) -> &[PhyEffect] {
    let end = effects
        .iter()
        .position(|e| matches!(e, PhyEffect::Read(GAIN_BASE, _)))
        .expect("channel must reach gain publication after temperature sampling");
    let prefix = &effects[..end];
    assert_eq!(
        prefix
            .iter()
            .filter(|e| matches!(e, PhyEffect::Read(TEMPERATURE_CODE, _)))
            .count(),
        1
    );
    prefix
}

impl Channel {
    pub fn new(options: &Options) -> Result<Self> {
        let session = start_session(
            options,
            &[],
            "captured channel restoration, temperature prefix and stuck readiness; no RF qualification",
        )?;
        let link = LinkRequest {
            companions: vec![],
            revision: Some(session.revision.clone()),
            inputs: vec![0],
            entry: select(&session, 0, "phy_chip_set_chan")?,
            roots: vec![select(&session, 0, "phy_get_romfunc_addr")?],
            layout: image_layout(),
        };
        let image = PhyImage::link(
            session,
            &link,
            &options.linker,
            "phy_chip_set_chan",
            &[ROM_INPUT],
        )?;
        Ok(Self {
            rom_delay: image.sym(1, "ets_delay_us"),
            production_delay: image.sym(2, "ets_delay_us"),
            image,
        })
    }

    fn vendor_phase(&self, t: &Transition, ready: bool) -> Invocation {
        let mut phase = self.enter(
            self.root("phy_chip_set_chan"),
            &[t.channel, t.cbw],
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            channel_models(t, ready),
        );
        phase.calls = delay_calls("channel-delay", self.rom_delay);
        phase
    }

    pub fn production_phase(&self, t: &Transition, ready: bool) -> Result<Invocation> {
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
        phase.calls = delay_calls("channel-delay", self.production_delay);
        Ok(phase)
    }

    /// Zeroed parameters, captured callback installation, then the transition.
    fn rows(&self, t: &Transition, install: bool) -> Result<Vec<ExecutionCase>> {
        let zero = vec![0u8; PHY_PARAM_BYTES as usize];
        let noop = self.noop();
        let mut transition = case(
            t.label(),
            self.vendor_phase(t, true),
            Some(self.production_phase(t, true)?),
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
) -> (Vec<PhyEffect>, Vec<PhyEffect>) {
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
    let (vendor, production) = (
        events(records, TRANSITION, false),
        events(records, TRANSITION, true),
    );
    assert_eq!(
        environment_reads(&vendor),
        environment_reads(&production),
        "{label}: environment-supplied registers"
    );
    (phy_effects(&vendor, &[]), phy_effects(&production, &[]))
}

fn gain_writes(effects: &[PhyEffect]) -> usize {
    effects
        .iter()
        .filter(|e| matches!(e, PhyEffect::Write(a, _) if GAIN_DATA.contains(a)))
        .count()
}

pub fn exercise(ctx: &mut Channel) -> Result<()> {
    // Full-root evidence: every channel class and bandwidth, including TX gain publication.
    for t in full_transitions() {
        let label = format!("full-{}", t.label());
        let rows = ctx.rows(&t, true)?;
        let executed = ctx.compare(&label, rows, t.fill)?;
        let (vendor, production) = check_transition(&label, &t, &executed.records);
        assert_eq!(vendor, production, "{label}: channel effects");
        assert!(gain_writes(&vendor) > 0, "{label}: no gain publication");
    }
    // Temperature-prefix evidence: every sensor DAC window at boundary codes.
    // The claim is limited to effects before the gain bank is first read.
    for t in prefix_transitions() {
        let label = format!("prefix-{}", t.label());
        let rows = ctx.rows(&t, true)?;
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
            ctx.production_phase(&t, false)?,
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
        let effects = phy_effects(&observed, &[]);
        assert_eq!(gain_writes(&effects), 0, "{label}: gain published");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, PhyEffect::Read(CHANNEL_STATUS, 0))),
            "{label}: readiness never sampled"
        );
    }
    Ok(())
}

/// A changed production channel is a DIFF, uninstalled callbacks leave the
/// vendor INCOMPLETE, and an undersized event capacity publishes nothing.
fn negative(ctx: &mut Channel) -> Result<()> {
    let t = full_transitions()[0];
    let mut changed = ctx.rows(&t, true)?;
    let other = Transition { channel: 6, ..t };
    changed[TRANSITION as usize].replacement = Some(ctx.production_phase(&other, true)?);
    ctx.image.execute(
        "negative-changed-channel",
        changed,
        t.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        MAX_EVENTS,
    )?;
    let uninstalled = ctx.rows(&t, false)?;
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
    let rows = ctx.rows(&t, true)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn prefix_ends_at_gain_base_after_one_sample() {
        let effects = [
            PhyEffect::Read(TEMPERATURE_CODE, 1),
            PhyEffect::Write(WORK_MODE, 4),
            PhyEffect::Read(GAIN_BASE, 0),
            PhyEffect::Read(TEMPERATURE_CODE, 2),
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
