use core::{cell::UnsafeCell, mem::size_of, ptr};

const PHY_FREQUENCY_OFFSET: usize = 0x20;
const PHY_CHANNEL_14_MIC: usize = 0x26;
const PHY_11P_ENABLE: usize = 0x28;
const PHY_11P_CONFIG: usize = 0x29;
const PHY_XTAL_SELECTOR: usize = 0x4f;
const PHY_TX_GAIN_SKIP: usize = 0x07;
const PHY_TX_GAIN_SEED: usize = 0xa8;
const PHY_TX_GAIN_CONFIG: usize = 0xd0;
const PHY_TX_GAIN_CURVE: usize = 0xf1;
const PHY_TX_GAIN_CORRECTION: usize = 0xf7;
const PHY_TX_GAIN_BASE: usize = 0x123;
const PHY_TX_GAIN_DELTA: usize = 0x1b2;
const PHY_CURRENT_CHANNEL: usize = 0x11c;
const PHY_INIT_COMPLETE: usize = 0x11e;
const PHY_CURRENT_CBW: usize = 0x11f;

// Pinned `phy_tx_gain.o` `.rodata` slices at offsets 0x6c, 0x90 and 0xb4.
// Their semantic units are not public; the ROM oracle consumes 18 aligned
// little-endian halfwords per table.
const WIFI_TX_GAIN_TABLE_LOW: [u16; 18] = [
    0x003f, 0x0037, 0x002f, 0x0027, 0x0027, 0x001f, 0x0017, 0x000f, 0x000f, 0x000d, 0x000c, 0x0007,
    0x0006, 0x0005, 0x0004, 0x0003, 0x0002, 0x0001,
];
const WIFI_TX_GAIN_TABLE_MID: [u16; 18] = [
    0x0100, 0x0100, 0x0100, 0x0100, 0x8000, 0x8000, 0x8000, 0x8000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const WIFI_TX_GAIN_TABLE_HIGH: [u16; 18] = [
    0x001b, 0x0018, 0x0014, 0x000e, 0x0006, 0x0000, 0xfff6, 0xffe9, 0xffe1, 0xffd7, 0xffd0, 0xffc9,
    0xffc4, 0xffbe, 0xffb8, 0xffb0, 0xffa5, 0xff97,
];

unsafe extern "C" {
    static mut phy_param: u8;

    fn phy_chan_to_freq(channel: u16) -> u16;
    fn phy_mhz2ieee(frequency_mhz: u16) -> u16;
    fn phy_disable_agc();
    fn phy_bbpll_cal(enable: u32);
    fn phy_tsens_temp_read();
    fn phy_set_channel_rfpll_freq(frequency_mhz: u16, xtal_selector: u8, offset: i16);
    fn phy_set_chan_reg(enable: u32);
    fn phy_wifi_get_tx_gain(
        channel: u16,
        calibration_curve: *const u8,
        correction: i32,
        base_and_delta: i32,
        low: *const u16,
        mid: *const u16,
        high: *const u16,
        output_32: *mut u32,
        output_64: *mut u32,
        output_72: *mut u32,
        mode: u32,
    );
    fn phy_set_tx_gain_mem_new(
        bank: u32,
        entries: u32,
        output_72: *const u32,
        output_64: *const u32,
        output_32: *const u32,
        seed: *const u32,
        config: *const u16,
    );
    fn phy_i2c_master_mem_txcap();
    fn phy_bb_cbw_chan_cfg(cbw: u8);
    fn phy_set_rx_comp_new();
    fn phy_dc_mem_clr();
    fn phy_enable_agc();
}

#[derive(Clone, Copy)]
struct PhyChannelState {
    adopted: bool,
    frequency_offset: i16,
    xtal_selector: u8,
    dot11p_enable: u8,
    dot11p_config: u8,
    current_channel: u16,
    init_complete: bool,
    current_cbw: u8,
    tx_gain_skip: bool,
    tx_gain_seed: [u32; 6],
    tx_gain_config: u16,
    tx_gain_curve: [u8; 6],
    tx_gain_correction: i8,
    tx_gain_base: u8,
    tx_gain_delta: u8,
}

/// Exact stack layout passed by pinned `phy_wifi_set_tx_gain_new`.
///
/// `phy_set_tx_gain_mem_new` treats `seed` as the start of a 40-byte lookup
/// region: gain indices zero through two read `seed`, while indices three and
/// four deliberately continue into the adjacent `output_32` field.
#[repr(C)]
struct TxGainScratch {
    seed: [u32; 6],
    output_32: [u32; 8],
    output_64: [u32; 16],
    output_72: [u32; 18],
}

const _: () = {
    assert!(core::mem::offset_of!(TxGainScratch, seed) == 0);
    assert!(core::mem::offset_of!(TxGainScratch, output_32) == 24);
    assert!(core::mem::offset_of!(TxGainScratch, output_64) == 56);
    assert!(core::mem::offset_of!(TxGainScratch, output_72) == 120);
    assert!(size_of::<TxGainScratch>() == 192);
};

impl PhyChannelState {
    const fn new() -> Self {
        Self {
            adopted: false,
            frequency_offset: 0,
            xtal_selector: 0,
            dot11p_enable: 0,
            dot11p_config: 0,
            current_channel: 0,
            init_complete: false,
            current_cbw: 0,
            tx_gain_skip: false,
            tx_gain_seed: [0; 6],
            tx_gain_config: 0,
            tx_gain_curve: [0; 6],
            tx_gain_correction: 0,
            tx_gain_base: 0,
            tx_gain_delta: 0,
        }
    }
}

struct PhyChannelResources(UnsafeCell<PhyChannelState>);

// Adoption runs before strict handoff and every subsequent mutation belongs
// to the single radio owner. No interrupt handler reads this object.
unsafe impl Sync for PhyChannelResources {}

#[link_section = ".critical.bss.wifi_strict.phy_channel"]
static RESOURCES: PhyChannelResources =
    PhyChannelResources(UnsafeCell::new(PhyChannelState::new()));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelStateAdoptionError {
    /// The qualified basic AP/STA profile supports channels 1 through 13.
    ///
    /// Channel 14 has a separate maximum-power mutation whose complete ROM
    /// calibration contract has not yet been recovered.
    Channel14MicEnabled,
}

/// Adopt only the fields read or written by the pinned
/// `libphy.a[phy_rfpll.o]::phy_chip_set_chan` and
/// `libphy.a[phy_tx_gain.o]::phy_wifi_set_tx_gain_new` bodies.
///
/// The offsets are instruction operands in that object. Unknown bytes in the
/// 508-byte `phy_param` object deliberately remain outside this Rust type.
///
/// # Safety
/// PHY cold initialization must be complete and no channel transition may run
/// concurrently.
pub(crate) unsafe fn adopt_vendor_phy_channel_state() -> Result<(), PhyChannelStateAdoptionError> {
    let source = ptr::addr_of!(phy_param);
    if source.add(PHY_CHANNEL_14_MIC).read_volatile() != 0 {
        return Err(PhyChannelStateAdoptionError::Channel14MicEnabled);
    }

    let state = &mut *RESOURCES.0.get();
    state.frequency_offset = source
        .add(PHY_FREQUENCY_OFFSET)
        .cast::<i16>()
        .read_volatile();
    state.dot11p_enable = source.add(PHY_11P_ENABLE).read_volatile();
    state.dot11p_config = source.add(PHY_11P_CONFIG).read_volatile();
    state.xtal_selector = source.add(PHY_XTAL_SELECTOR).read_volatile();
    state.current_channel = source
        .add(PHY_CURRENT_CHANNEL)
        .cast::<u16>()
        .read_volatile();
    state.init_complete = source.add(PHY_INIT_COMPLETE).read_volatile() != 0;
    state.current_cbw = source.add(PHY_CURRENT_CBW).read_volatile();
    state.tx_gain_skip = source.add(PHY_TX_GAIN_SKIP).read_volatile() != 0;
    for (index, word) in state.tx_gain_seed.iter_mut().enumerate() {
        *word = source
            .add(PHY_TX_GAIN_SEED + index * size_of::<u32>())
            .cast::<u32>()
            .read_volatile();
    }
    state.tx_gain_config = source.add(PHY_TX_GAIN_CONFIG).cast::<u16>().read_volatile();
    for (index, byte) in state.tx_gain_curve.iter_mut().enumerate() {
        *byte = source.add(PHY_TX_GAIN_CURVE + index).read_volatile();
    }
    state.tx_gain_correction = source
        .add(PHY_TX_GAIN_CORRECTION)
        .cast::<i8>()
        .read_volatile();
    state.tx_gain_base = source.add(PHY_TX_GAIN_BASE).read_volatile();
    state.tx_gain_delta = source.add(PHY_TX_GAIN_DELTA).read_volatile();
    state.adopted = true;
    Ok(())
}

#[inline(never)]
unsafe fn trap_invalid_phy_channel_state() -> ! {
    core::arch::asm!("ebreak", options(noreturn))
}

unsafe fn set_wifi_tx_gain(channel: u16, state: &PhyChannelState) {
    let mut scratch = TxGainScratch {
        seed: state.tx_gain_seed,
        output_32: [0; 8],
        output_64: [0; 16],
        output_72: [0; 18],
    };
    phy_wifi_get_tx_gain(
        channel,
        state.tx_gain_curve.as_ptr(),
        i32::from(state.tx_gain_correction),
        i32::from(state.tx_gain_base.wrapping_add(state.tx_gain_delta) as i8),
        WIFI_TX_GAIN_TABLE_LOW.as_ptr(),
        WIFI_TX_GAIN_TABLE_MID.as_ptr(),
        WIFI_TX_GAIN_TABLE_HIGH.as_ptr(),
        scratch.output_32.as_mut_ptr(),
        scratch.output_64.as_mut_ptr(),
        scratch.output_72.as_mut_ptr(),
        0,
    );
    if !state.tx_gain_skip {
        phy_set_tx_gain_mem_new(
            0,
            32,
            scratch.output_72.as_ptr(),
            scratch.output_64.as_ptr(),
            scratch.output_32.as_ptr(),
            scratch.seed.as_ptr(),
            ptr::addr_of!(state.tx_gain_config),
        );
    }
}

/// Strict channel-programming sequence recovered from the pinned
/// `phy_rfpll.o` implementation.
///
/// The two vendor I2C critical-section callbacks are intentionally absent:
/// the qualified S31 image binds both to a single `ret`, while strict runtime
/// serializes this whole sequence in the Rust radio owner. The ROM function
/// table call at slot `+0x14` is the cold-published
/// `phy_set_rx_comp_new` leaf and is therefore direct here.
pub(crate) unsafe fn program_channel(frequency_mhz: u16, cbw: u8) {
    if !crate::critical::strict_wifi_hart_armed() || !crate::critical::on_strict_wifi_hart() {
        trap_invalid_phy_channel_state();
    }

    let state = &mut *RESOURCES.0.get();
    if !state.adopted {
        trap_invalid_phy_channel_state();
    }

    let channel = phy_mhz2ieee(frequency_mhz);
    let frequency_mhz = phy_chan_to_freq(channel);
    state.current_channel = channel;
    state.init_complete = cbw != 0;
    state.current_cbw = cbw;

    phy_disable_agc();
    phy_bbpll_cal(1);
    phy_tsens_temp_read();
    phy_set_channel_rfpll_freq(frequency_mhz, state.xtal_selector, state.frequency_offset);
    phy_set_chan_reg(1);
    set_wifi_tx_gain(channel, state);
    phy_i2c_master_mem_txcap();
    phy_bb_cbw_chan_cfg(cbw);
    // The pinned `phy_11p_set` body only writes these same two values back to
    // `phy_param[0x28..=0x29]`; Rust already owns them after handoff.
    phy_set_rx_comp_new();
    phy_bbpll_cal(0);
    phy_dc_mem_clr();
    phy_enable_agc();
}
