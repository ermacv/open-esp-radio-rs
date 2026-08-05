#![no_std]
#![no_main]

mod console;

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, Ordering},
};

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "s31-open-radio-vendor-oracle-hil",
    "00:00:00",
    "2026-07-26",
    "6.2",
    64 * 1024,
    0,
    u16::MAX,
    0
);

use console::{emergency_log, panic_report};
use embassy_time::Timer;
use esp_hal::{
    efuse::{self, InterfaceMacAddress},
    interrupt::software::SoftwareInterruptControl,
    timer::systimer::SystemTimer,
};
use open_esp_radio::esp32s31::{
    hal::{
        ColdRadioRegisters, Radio, RadioRegisters, phy_i2c::PhyI2cMasterControl,
        phy_temperature::PhyTemperatureSystemControl, wifi_bb::PhyWifiBbControl,
    },
    wifi::lmac::{
        descriptor::{Descriptor, rx_done},
        init::{configure_sta_link_receive_policy, initialize_promiscuous_receive},
        registers::{
            MAC_INT_RAW, Mmio, RX_CONTROL, RX_DESCRIPTOR_BASE, RX_LAST_DESCRIPTOR,
            RX_LAST_DESCRIPTOR_HIGH, RX_NEXT_DESCRIPTOR,
        },
        rx::{RxIngressConfig, RxSegment, build_cold_ring, extract_management, publish_cold_ring},
        tx::{LegacyTxConfig, TxSlot},
    },
    phy::{
        phy_channel::{
            PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
            PhyChipChannelRequest, PhyChipChannelTransition,
        },
        phy_cold::{
            PhyColdI2cAction, PhyColdI2cError, PhyColdI2cObservation, PhyColdMmioBinding,
            PhyColdState,
        },
        phy_i2c::{PhyI2cAddress, PhyRfInitPrefixAction},
        phy_temperature::{
            PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureExternalBinding,
            PhyTemperatureI2cBinding,
        },
    },
};
use open_esp_radio::wifi::ieee80211::station::{
    OpenAuthenticationRequest, parse_open_authentication_response,
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use static_cell::StaticCell;

unsafe extern "C" {
    fn ets_install_usb_printf();
    fn phy_i2c_readReg(block: u32, host: u32, register: u32) -> u32;
    fn phy_i2c_writeReg(block: u32, host: u32, register: u32, value: u32);
    fn phy_change_channel(channel: u32, do_calibration: u32, reserved: u32, cbw: u32) -> i32;
    fn phy_wifi_enable_set(enable: u8);
    fn phy_get_max_pwr(rate: u32, output: *mut i8) -> u32;
    fn phy_txdc_cal_init(output: *mut u16, tx_power: u32, tx_bb_gain: u32, force_path_one: u32);
    fn __real_phy_txdc_cal_init(
        output: *mut u16,
        tx_power: u32,
        tx_bb_gain: u32,
        force_path_one: u32,
    );
    fn __real_phy_rf_init();
    fn __real_phy_set_chan_freq_hw_init(crystal_selector: u32, frequency_code: u32);
    fn __real_phy_xtal_duty_cal_init(force_calibration: u32);
    fn __real_phy_fe_reg_update();
    fn __real_phy_i2cmst_reg_init();
    fn __real_phy_pwdet_reg_init();
    fn __real_phy_fe_reg_init();
    fn __real_phy_tx_pwctrl_bg_init();
    fn __real_phy_rc_cal_init();
    fn __real_phy_filter_dcap_set();
    fn __real_phy_i2c_init1();
    fn __real_phy_rfpll_chgp_cal();
    fn __real_phy_i2c_master_cmd_mem_init();
    fn __real_phy_bias_reg_set(enabled: u32);
    fn __real_phy_open_i2c_xpd_new(mode: u32);
    fn __real_phy_tsens_read_init(enabled: u32, calibration: u32);
    static mut phy_param: [u8; 0x1fc];
}

static TXDC_ORACLE_CALLS: AtomicU32 = AtomicU32::new(0);

/// Record the exact hardware state at the blob TXDC entry, then invoke it.
///
/// SOURCE: `_oracles/libphy.a[phy_tx_cal.o]`, `phy_bb_init` calls
/// `phy_txdc_cal_init(output, 15, 0, 0)`. The latter wrapper enters
/// `_oracles/esp32s31_rev0_rom.elf::phy_txdc_cal` once for each of five gain
/// rows. Linker `--wrap` lets the vendor cold-init path remain byte-for-byte
/// unchanged while exposing the state inherited from its earlier ROM/blob
/// calls.
#[unsafe(export_name = "__wrap_phy_txdc_cal_init")]
unsafe extern "C" fn wrap_phy_txdc_cal_init(
    output: *mut u16,
    tx_power: u32,
    tx_bb_gain: u32,
    force_path_one: u32,
) {
    let call = TXDC_ORACLE_CALLS.fetch_add(1, Ordering::Relaxed);
    log_txdc_entry_mmio("vendor-before-txdc", call);
    unsafe {
        __real_phy_txdc_cal_init(output, tx_power, tx_bb_gain, force_path_one);
    }
}

/// SOURCE: `_oracles/libphy.a[phy_init.o]::register_chipv7_phy` calls
/// `phy_rf_init` immediately before `phy_bb_init`. Capture both sides of the
/// complete RF prefix without changing its implementation.
#[unsafe(export_name = "__wrap_phy_rf_init")]
unsafe extern "C" fn wrap_phy_rf_init() {
    log_rf_boundary_mmio("vendor-before-rf-init");
    unsafe {
        __real_phy_rf_init();
    }
    log_rf_boundary_mmio("vendor-after-rf-init");
}

/// SOURCE: `_oracles/libphy.a[phy_init.o]::phy_rf_init` tail-calls
/// `phy_set_chan_freq_hw_init(2, 4)`. This isolates whether that final
/// ROM/blob graph creates the PBus result-window state required by TXDC.
#[unsafe(export_name = "__wrap_phy_set_chan_freq_hw_init")]
unsafe extern "C" fn wrap_phy_set_chan_freq_hw_init(crystal_selector: u32, frequency_code: u32) {
    log_rf_boundary_mmio("vendor-before-chan-freq-init");
    unsafe {
        __real_phy_set_chan_freq_hw_init(crystal_selector, frequency_code);
    }
    log_rf_boundary_mmio("vendor-after-chan-freq-init");
}

/// SOURCE: `_oracles/libphy.a[phy_init.o]::phy_rf_init` calls the archive
/// implementation in `phy_rx_cal.o` immediately before the final
/// `phy_fe_reg_update`. Capture whether the crystal-duty calibration is the
/// operation that first leaves the analog PBus windows populated.
#[unsafe(export_name = "__wrap_phy_xtal_duty_cal_init")]
unsafe extern "C" fn wrap_phy_xtal_duty_cal_init(force_calibration: u32) {
    log_rf_boundary_mmio("vendor-before-xtal-duty");
    unsafe {
        __real_phy_xtal_duty_cal_init(force_calibration);
    }
    log_rf_boundary_mmio("vendor-after-xtal-duty");
}

/// SOURCE: `_oracles/libphy.a[phy_init.o]::phy_rf_init` calls the complete
/// `phy_fe_reg_update` leaf after crystal-duty calibration and before channel
/// frequency initialization.
#[unsafe(export_name = "__wrap_phy_fe_reg_update")]
unsafe extern "C" fn wrap_phy_fe_reg_update() {
    log_rf_boundary_mmio("vendor-before-fe-reg-update");
    unsafe {
        __real_phy_fe_reg_update();
    }
    log_rf_boundary_mmio("vendor-after-fe-reg-update");
}

macro_rules! wrap_rf_leaf {
    (
        $wrapper:ident,
        $export:literal,
        $real:ident,
        $before:literal,
        $after:literal
    ) => {
        #[unsafe(export_name = $export)]
        unsafe extern "C" fn $wrapper() {
            log_rf_boundary_mmio($before);
            unsafe {
                $real();
            }
            log_rf_boundary_mmio($after);
        }
    };
}

// SOURCE: ordered no-argument calls in
// `_oracles/libphy.a[phy_init.o]::phy_rf_init`. These wrappers only sample
// read-only PBus result windows and forward to the unmodified blob/ROM leaf.
wrap_rf_leaf!(
    wrap_phy_i2cmst_reg_init,
    "__wrap_phy_i2cmst_reg_init",
    __real_phy_i2cmst_reg_init,
    "vendor-before-i2cmst-reg-init",
    "vendor-after-i2cmst-reg-init"
);
wrap_rf_leaf!(
    wrap_phy_pwdet_reg_init,
    "__wrap_phy_pwdet_reg_init",
    __real_phy_pwdet_reg_init,
    "vendor-before-pwdet-reg-init",
    "vendor-after-pwdet-reg-init"
);
wrap_rf_leaf!(
    wrap_phy_fe_reg_init,
    "__wrap_phy_fe_reg_init",
    __real_phy_fe_reg_init,
    "vendor-before-fe-reg-init",
    "vendor-after-fe-reg-init"
);
wrap_rf_leaf!(
    wrap_phy_tx_pwctrl_bg_init,
    "__wrap_phy_tx_pwctrl_bg_init",
    __real_phy_tx_pwctrl_bg_init,
    "vendor-before-tx-pwctrl-bg-init",
    "vendor-after-tx-pwctrl-bg-init"
);
wrap_rf_leaf!(
    wrap_phy_rc_cal_init,
    "__wrap_phy_rc_cal_init",
    __real_phy_rc_cal_init,
    "vendor-before-rc-cal-init",
    "vendor-after-rc-cal-init"
);
wrap_rf_leaf!(
    wrap_phy_filter_dcap_set,
    "__wrap_phy_filter_dcap_set",
    __real_phy_filter_dcap_set,
    "vendor-before-filter-dcap-set",
    "vendor-after-filter-dcap-set"
);
wrap_rf_leaf!(
    wrap_phy_i2c_init1,
    "__wrap_phy_i2c_init1",
    __real_phy_i2c_init1,
    "vendor-before-i2c-init1",
    "vendor-after-i2c-init1"
);
wrap_rf_leaf!(
    wrap_phy_rfpll_chgp_cal,
    "__wrap_phy_rfpll_chgp_cal",
    __real_phy_rfpll_chgp_cal,
    "vendor-before-rfpll-chgp-cal",
    "vendor-after-rfpll-chgp-cal"
);
wrap_rf_leaf!(
    wrap_phy_i2c_master_cmd_mem_init,
    "__wrap_phy_i2c_master_cmd_mem_init",
    __real_phy_i2c_master_cmd_mem_init,
    "vendor-before-i2c-command-memory",
    "vendor-after-i2c-command-memory"
);

#[unsafe(export_name = "__wrap_phy_bias_reg_set")]
unsafe extern "C" fn wrap_phy_bias_reg_set(enabled: u32) {
    log_rf_boundary_mmio("vendor-before-bias-reg-set");
    unsafe {
        __real_phy_bias_reg_set(enabled);
    }
    log_rf_boundary_mmio("vendor-after-bias-reg-set");
}

#[unsafe(export_name = "__wrap_phy_open_i2c_xpd_new")]
unsafe extern "C" fn wrap_phy_open_i2c_xpd_new(mode: u32) {
    log_rf_boundary_mmio("vendor-before-open-i2c-xpd");
    unsafe {
        __real_phy_open_i2c_xpd_new(mode);
    }
    log_rf_boundary_mmio("vendor-after-open-i2c-xpd");
}

#[unsafe(export_name = "__wrap_phy_tsens_read_init")]
unsafe extern "C" fn wrap_phy_tsens_read_init(enabled: u32, calibration: u32) {
    log_rf_boundary_mmio("vendor-before-tsens-read-init");
    unsafe {
        __real_phy_tsens_read_init(enabled, calibration);
    }
    log_rf_boundary_mmio("vendor-after-tsens-read-init");
}

const HARDWARE_EDGE_LIMIT: u16 = 10_000;
const FREQUENCY_READY_LIMIT: u32 = 10_000;
const MAC_HANDSHAKE_SAMPLE_LIMIT: u32 = 100_000;
const RX_DESCRIPTOR_COUNT: usize = 32;
const RX_BUFFER_SIZE: usize = 1_700;
const RX_OBSERVATION_LIMIT: u32 = 10_000;
const TX_BUFFER_SIZE: usize = 88;
const TX_METADATA_SIZE: usize = 8;
const TX_FCS_SIZE: usize = 4;
const TX_COMPLETION_SAMPLE_LIMIT: u32 = 200_000;
const TARGET_BSSID: [u8; 6] = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
const TARGET_CHANNEL: u16 = 11;
const FLASH_TUNING_REFERENCE_WORDS: usize = 64 * 1024 / size_of::<u32>();

// The shared S31 bootstrap linker reserves the same deterministic cold-XIP
// reference span as the source-only HIL. This oracle does not consume it, but
// retaining the span keeps the flash image compatible with the board's
// 120-MHz bootstrap contract without changing any vendor PHY operation.
#[used]
#[unsafe(link_section = ".flash.tuning.reference")]
static FLASH_TUNING_REFERENCE: [u32; FLASH_TUNING_REFERENCE_WORDS] = flash_tuning_reference();

const fn flash_tuning_reference() -> [u32; FLASH_TUNING_REFERENCE_WORDS] {
    let mut words = [0_u32; FLASH_TUNING_REFERENCE_WORDS];
    let mut state = 0x31a5_c33c_u32;
    let mut index = 0;
    while index < words.len() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        words[index] = state ^ (index as u32).wrapping_mul(0x9e37_79b9);
        index += 1;
    }
    words
}

// Rust 1.97's prebuilt target archive is tagged soft-float, while the S31
// objects are ilp32f. `hal_init_tx_pwr` reaches these compiler helpers, so keep
// the tiny implementations in this hard-float oracle image.
#[inline(never)]
fn hard_float_popcount32(value: u32) -> u32 {
    let count: u32;
    unsafe {
        core::arch::asm!(
            "li {count}, 0",
            "2:",
            "beqz {value}, 3f",
            "addi {count}, {count}, 1",
            "addi {temporary}, {value}, -1",
            "and {value}, {value}, {temporary}",
            "j 2b",
            "3:",
            value = inout(reg) value => _,
            count = lateout(reg) count,
            temporary = lateout(reg) _,
            options(nomem, nostack),
        );
    }
    count
}

#[unsafe(no_mangle)]
extern "C" fn __popcountsi2(value: u32) -> i32 {
    hard_float_popcount32(value) as i32
}

#[unsafe(no_mangle)]
extern "C" fn __popcountdi2(value: u64) -> i32 {
    let halves: [u32; 2] = unsafe { core::mem::transmute(value) };
    hard_float_popcount32(halves[0]).wrapping_add(hard_float_popcount32(halves[1])) as i32
}

#[unsafe(no_mangle)]
extern "C" fn __bswapsi2(value: u32) -> u32 {
    let result: u32;
    unsafe {
        core::arch::asm!(
            "andi {temporary}, {value}, 0xff",
            "slli {result}, {temporary}, 24",
            "srli {temporary}, {value}, 8",
            "andi {temporary}, {temporary}, 0xff",
            "slli {temporary}, {temporary}, 16",
            "or {result}, {result}, {temporary}",
            "srli {temporary}, {value}, 16",
            "andi {temporary}, {temporary}, 0xff",
            "slli {temporary}, {temporary}, 8",
            "or {result}, {result}, {temporary}",
            "srli {temporary}, {value}, 24",
            "or {result}, {result}, {temporary}",
            value = in(reg) value,
            result = out(reg) result,
            temporary = lateout(reg) _,
            options(nomem, nostack),
        );
    }
    result
}
// The PHY-I2C banks are not visible in the ordinary MMIO snapshot. Keep this
// list identical to the fully open HIL so the two cold-init paths can be
// compared register-for-register after both have selected the HIL AP channel.
const ANALOG_FINGERPRINT: [(u8, u8); 64] = [
    (0x61, 0x07),
    (0x61, 0x08),
    (0x61, 0x09),
    (0x61, 0x0a),
    (0x62, 0x00),
    (0x62, 0x01),
    (0x62, 0x02),
    (0x62, 0x04),
    (0x62, 0x0b),
    (0x62, 0x0d),
    (0x62, 0x0f),
    (0x62, 0x11),
    (0x62, 0x12),
    (0x62, 0x13),
    (0x62, 0x14),
    (0x62, 0x15),
    (0x63, 0x00),
    (0x63, 0x06),
    (0x66, 0x02),
    (0x66, 0x04),
    (0x67, 0x00),
    (0x67, 0x02),
    (0x67, 0x03),
    (0x67, 0x04),
    (0x67, 0x05),
    (0x67, 0x06),
    (0x67, 0x07),
    (0x67, 0x0c),
    (0x67, 0x0d),
    (0x67, 0x0e),
    (0x67, 0x0f),
    (0x67, 0x14),
    (0x67, 0x15),
    (0x67, 0x16),
    (0x67, 0x17),
    (0x67, 0x18),
    (0x67, 0x19),
    (0x67, 0x1c),
    (0x67, 0x1d),
    (0x67, 0x1e),
    (0x67, 0x1f),
    (0x6a, 0x00),
    (0x6a, 0x01),
    (0x6b, 0x01),
    (0x6b, 0x02),
    (0x6b, 0x03),
    (0x6b, 0x04),
    (0x6b, 0x05),
    (0x6b, 0x06),
    (0x6b, 0x07),
    (0x6b, 0x08),
    (0x6b, 0x09),
    (0x6b, 0x0a),
    (0x6b, 0x0b),
    (0x6b, 0x0c),
    (0x6b, 0x0d),
    (0x6b, 0x0e),
    (0x6b, 0x0f),
    (0x6b, 0x10),
    (0x6b, 0x11),
    (0x6b, 0x12),
    (0x6b, 0x13),
    (0x6b, 0x14),
    (0x6d, 0x00),
];

#[repr(C, align(4))]
struct DmaBuffer(UnsafeCell<[u8; RX_BUFFER_SIZE]>);

// The single HIL task observes a buffer only after its descriptor has returned
// from the hardware owner.
unsafe impl Send for DmaBuffer {}

impl DmaBuffer {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; RX_BUFFER_SIZE]))
    }

    fn address(&self) -> u32 {
        self.0.get().addr() as u32
    }

    unsafe fn as_slice(&self) -> &[u8] {
        unsafe { &*self.0.get() }
    }
}

struct RxStorage {
    descriptors: [Descriptor; RX_DESCRIPTOR_COUNT],
    buffers: [DmaBuffer; RX_DESCRIPTOR_COUNT],
}

impl RxStorage {
    const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; RX_DESCRIPTOR_COUNT],
            buffers: [const { DmaBuffer::new() }; RX_DESCRIPTOR_COUNT],
        }
    }
}

static RX_STORAGE: StaticCell<RxStorage> = StaticCell::new();
static TX_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
fn read_diagnostic_mmio(address: usize) -> u32 {
    // SAFETY: diagnostic-only oracle reads; the open MAC path itself accepts
    // only PAC-described registers.
    unsafe { (address as *const u32).read_volatile() }
}

fn log_rf_boundary_mmio(source: &str) {
    emergency_log(format_args!(
        "OPEN_RADIO_RF_BOUNDARY source={source} \
         pbus={:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x} \
         dac_scale={:#010x}",
        read_diagnostic_mmio(0x2010_0884),
        read_diagnostic_mmio(0x2010_088c),
        read_diagnostic_mmio(0x2010_0890),
        read_diagnostic_mmio(0x2010_0898),
        read_diagnostic_mmio(0x2010_089c),
        read_diagnostic_mmio(0x2010_08a0),
        read_diagnostic_mmio(0x2010_08a4),
        read_diagnostic_mmio(0x2010_0c04),
    ));
}

fn log_txdc_entry_mmio(source: &str, call: u32) {
    const ADDRESSES: [usize; 18] = [
        0x2010_001c,
        0x2010_0028,
        0x2010_040c,
        0x2010_0418,
        0x2010_041c,
        0x2010_0420,
        0x2010_0428,
        0x2010_0800,
        0x2010_081c,
        0x2010_0820,
        0x2010_0830,
        0x2010_0848,
        0x2010_084c,
        0x2010_0850,
        0x2010_0870,
        0x2010_0884,
        0x2010_088c,
        0x2010_0890,
    ];
    let values: [u32; ADDRESSES.len()] =
        core::array::from_fn(|index| read_diagnostic_mmio(ADDRESSES[index]));
    emergency_log(format_args!(
        "OPEN_RADIO_TXDC_ENTRY source={source} call={call} addresses={ADDRESSES:08x?}"
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_TXDC_ENTRY source={source} call={call} values={values:08x?}"
    ));
    log_phy_mmio_page_hashes_with_call(source, call);
    log_txdc_differing_pages(source, call);
}

fn log_txdc_differing_pages(source: &str, call: u32) {
    // These are the only page hashes that differed between the vendor cold
    // entry and the open cold entry on the 2026-07-29 HIL run. Dumping the
    // exact words turns that observation into addresses which can be traced
    // back to the preceding blob/ROM writers.
    const OFFSETS: [u16; 8] = [
        0x0400, 0x0800, 0x0c00, 0x4400, 0x5500, 0x7000, 0x7c00, 0xd800,
    ];
    for page in OFFSETS {
        let base = 0x2010_0000_usize + usize::from(page);
        for chunk in 0..4_u16 {
            let offset = page + chunk * 0x40;
            let values: [u32; 16] = core::array::from_fn(|word| {
                read_diagnostic_mmio(base + usize::from(chunk) * 0x40 + word * 4)
            });
            emergency_log(format_args!(
                "OPEN_RADIO_TXDC_WORDS source={source} call={call} \
                 offset={offset:#06x} values={values:08x?}"
            ));
        }
    }
}

fn log_vendor_analog_fingerprint() {
    for (block, register) in ANALOG_FINGERPRINT {
        let address = PhyI2cAddress::new(block, register).unwrap();
        let value =
            unsafe { phy_i2c_readReg(block.into(), address.host().into(), register.into()) };
        emergency_log(format_args!(
            "OPEN_RADIO_ANALOG source=vendor block={block:#04x} \
             register={register:#04x} value={value:#04x}"
        ));
    }
}

fn log_parameter_image(source: &str, parameter: &[u8; 0x1fc]) {
    for (chunk, values) in parameter.chunks(16).enumerate() {
        emergency_log(format_args!(
            "OPEN_RADIO_PARAMETER source={source} offset={:#05x} values={values:02x?}",
            chunk * 16
        ));
    }
}

const PHY_MMIO_PAGE_OFFSETS: [u16; 28] = [
    0x0000, 0x0400, 0x0800, 0x0c00, 0x4000, 0x4100, 0x4200, 0x4300, 0x4400, 0x4800, 0x4c00, 0x4d00,
    0x5100, 0x5500, 0x5700, 0x7000, 0x7100, 0x7400, 0x7800, 0x7900, 0x7a00, 0x7c00, 0x7d00, 0x8000,
    0x9c00, 0xd800, 0xf000, 0xf800,
];

fn log_phy_mmio_page_hashes(source: &str) {
    log_phy_mmio_page_hashes_with_call(source, u32::MAX);
}

fn log_phy_mmio_page_hashes_with_call(source: &str, call: u32) {
    for offset in PHY_MMIO_PAGE_OFFSETS {
        let base = 0x2010_0000_usize + usize::from(offset);
        let mut hash = 0x811c_9dc5_u32;
        let mut word = 0_usize;
        while word != 64 {
            let value = unsafe { ((base + word * 4) as *const u32).read_volatile() };
            hash ^= value;
            hash = hash.wrapping_mul(0x0100_0193);
            word += 1;
        }
        emergency_log(format_args!(
            "OPEN_RADIO_MMIO_PAGE source={source} call={call} \
             offset={offset:#06x} hash={hash:#010x}"
        ));
    }
}

fn log_full_mac_words(source: &str) {
    for chunk in 0..128_u16 {
        let offset = 0x4000 + chunk * 0x40;
        let base = 0x2010_0000_usize + usize::from(offset);
        let values: [u32; 16] = core::array::from_fn(|word| unsafe {
            ((base + word * 4) as *const u32).read_volatile()
        });
        emergency_log(format_args!(
            "FULL_MAC_WORDS source={source} offset={offset:#06x} values={values:08x?}"
        ));
    }
}

fn log_mac_tx_rx_boundary(source: &str) {
    emergency_log(format_args!(
        "OPEN_RADIO_MAC_BOUNDARY source={source} \
         power_tail={:#010x}/{:#010x}/{:#010x}/{:#010x} \
         ack={:#010x}/{:#010x}/{:#010x}/{:#010x} \
         delay={:#010x}/{:#010x} rx={:#010x} mac_control={:#010x}",
        read_diagnostic_mmio(0x2010_443c),
        read_diagnostic_mmio(0x2010_4440),
        read_diagnostic_mmio(0x2010_4444),
        read_diagnostic_mmio(0x2010_4448),
        read_diagnostic_mmio(0x2010_444c),
        read_diagnostic_mmio(0x2010_4450),
        read_diagnostic_mmio(0x2010_4458),
        read_diagnostic_mmio(0x2010_445c),
        read_diagnostic_mmio(0x2010_4c54),
        read_diagnostic_mmio(0x2010_4c58),
        read_diagnostic_mmio(0x2010_4080),
        read_diagnostic_mmio(0x2010_4cac),
    ));
}

/// Capture the phase-aligned PHY/MAC words used by the full-vendor TX oracle.
///
/// Keep this address order identical to `wifi_scan::log_tx_state_vector`.
/// Most words are still dynamic or semantically unknown; this diagnostic is
/// an address-for-address comparison aid, not a register-name source.
fn log_tx_state_vector(source: &str) {
    const ADDRESSES: [usize; 56] = [
        0x2010_001c,
        0x2010_0024,
        0x2010_0028,
        0x2010_0400,
        0x2010_0408,
        0x2010_0440,
        0x2010_0448,
        0x2010_0800,
        0x2010_081c,
        0x2010_0820,
        0x2010_0830,
        0x2010_0848,
        0x2010_084c,
        0x2010_0850,
        0x2010_0870,
        0x2010_0890,
        0x2010_4400,
        0x2010_4404,
        0x2010_4408,
        0x2010_443c,
        0x2010_4440,
        0x2010_4448,
        0x2010_447c,
        0x2010_4480,
        0x2010_4c04,
        0x2010_4c1c,
        0x2010_4c40,
        0x2010_4c44,
        0x2010_4c5c,
        0x2010_4c60,
        0x2010_4c7c,
        0x2010_4c80,
        0x2010_4c8c,
        0x2010_4cac,
        0x2010_4cb4,
        0x2010_4dd4,
        0x2010_4dd8,
        0x2010_4ddc,
        0x2010_7000,
        0x2010_705c,
        0x2010_7064,
        0x2010_70a0,
        0x2010_7104,
        0x2010_7114,
        0x2010_7428,
        0x2010_7848,
        0x2010_78c8,
        0x2010_7c80,
        0x2010_9c18,
        0x2010_d814,
        0x2010_d820,
        0x2010_d824,
        0x2010_d830,
        0x2010_d83c,
        0x2010_d858,
        0x2010_d878,
    ];
    let values: [u32; ADDRESSES.len()] =
        core::array::from_fn(|index| read_diagnostic_mmio(ADDRESSES[index]));
    let (first, second) = values.split_at(28);
    emergency_log(format_args!(
        "TX_STATE_VECTOR source={source} part=0 values={first:08x?}"
    ));
    emergency_log(format_args!(
        "TX_STATE_VECTOR source={source} part=1 values={second:08x?}"
    ));
}

const SELECTED_PHY_MMIO_ADDRESSES: [usize; 14] = [
    0x2010_001c,
    0x2010_002c,
    0x2010_0438,
    0x2010_081c,
    0x2010_0820,
    0x2010_0830,
    0x2010_0848,
    0x2010_084c,
    0x2010_0850,
    0x2010_0870,
    0x2010_0890,
    0x2010_7050,
    0x2010_7c80,
    0x2010_9c18,
];

fn log_selected_phy_mmio(source: &str) {
    for address in SELECTED_PHY_MMIO_ADDRESSES {
        let value = unsafe { (address as *const u32).read_volatile() };
        emergency_log(format_args!(
            "OPEN_RADIO_MMIO_SELECTED source={source} address={address:#010x} value={value:#010x}"
        ));
    }
}

fn read_phy_frequency_memory(address: u16) -> u32 {
    let address_control = 0x2010_001c_usize as *mut u32;
    let read_control = 0x2010_0020_usize as *mut u32;
    let mode_control = 0x2010_0030_usize as *mut u32;
    let data = 0x2010_0040_usize as *const u32;
    unsafe {
        let saved_address = address_control.read_volatile();
        let saved_read = read_control.read_volatile();
        let saved_mode = mode_control.read_volatile();
        address_control.write_volatile(
            (saved_address & 0xff80_00ff) | ((u32::from(address) << 8) & 0x007f_ff00),
        );
        mode_control.write_volatile((saved_mode & !0x3) | 2);
        read_control.write_volatile(saved_read | (1 << 16));
        read_control.write_volatile(saved_read & !(1 << 16));
        let value = data.read_volatile();
        mode_control.write_volatile(saved_mode);
        read_control.write_volatile(saved_read);
        address_control.write_volatile(saved_address);
        value
    }
}

fn log_listen_frequency_memory(source: &str) {
    // Channel 6 is 2437 MHz, index 37 in the table beginning at 2400 MHz.
    // `phy_get_freq_mem_param(2)` resolves to RF-record base 0x20, stride 7
    // and the three record words at offsets 0, 3 and 6.
    const BASE: u16 = 0x20 + 37 * 7;
    let words = [
        read_phy_frequency_memory(BASE),
        read_phy_frequency_memory(BASE + 3),
        read_phy_frequency_memory(BASE + 6),
    ];
    emergency_log(format_args!(
        "OPEN_RADIO_FREQ_MEMORY source={source} base={BASE:#05x} words={words:08x?}"
    ));
}

const DIFFERING_PHY_MMIO_PAGE_OFFSETS: [u16; 8] = [
    0x0000, 0x0400, 0x0800, 0x0c00, 0x7000, 0x7c00, 0x9c00, 0xf800,
];

fn log_differing_phy_mmio_pages(source: &str) {
    for offset in DIFFERING_PHY_MMIO_PAGE_OFFSETS {
        let base = 0x2010_0000_usize + usize::from(offset);
        let mut values = [0_u32; 64];
        let mut word = 0_usize;
        while word != values.len() {
            values[word] = unsafe { ((base + word * 4) as *const u32).read_volatile() };
            word += 1;
        }
        emergency_log(format_args!(
            "OPEN_RADIO_MMIO_WORDS source={source} offset={offset:#06x} values={values:08x?}"
        ));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HilError {
    I2cDeadline,
    UnexpectedBinding,
}

async fn complete_i2c<P: PhyI2cMasterControl>(
    mut binding: open_esp_radio::esp32s31::phy::phy_channel::PhyChipChannelI2cBinding,
    platform: &mut P,
) -> Result<PhyChipChannelCompletion, HilError> {
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match binding.action() {
            PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                match binding.start_target(platform) {
                    Ok(()) => {}
                    Err(PhyColdI2cError::BusyAtStart) => Timer::after_micros(1).await,
                    Err(_) => return Err(HilError::UnexpectedBinding),
                }
            }
            PhyColdI2cAction::AwaitReadCompletionEdge { .. }
            | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                Timer::after_micros(1).await;
                match binding
                    .observe_target_edge(platform)
                    .map_err(|_| HilError::UnexpectedBinding)?
                {
                    PhyColdI2cObservation::EdgeConsumed | PhyColdI2cObservation::StillPending => {}
                }
            }
            PhyColdI2cAction::Complete(_) => {
                return binding
                    .into_completion()
                    .map_err(|_| HilError::UnexpectedBinding);
            }
        }
    }
    Err(HilError::I2cDeadline)
}

async fn complete_temperature_i2c<P: PhyI2cMasterControl>(
    mut binding: open_esp_radio::esp32s31::phy::phy_temperature::PhyTemperatureI2cBinding,
    platform: &mut P,
) -> Result<PhyTemperatureCompletion, HilError> {
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match binding.action() {
            PhyColdI2cAction::StartRead { .. } => match binding.start_target(platform) {
                Ok(()) => {}
                Err(PhyColdI2cError::BusyAtStart) => Timer::after_micros(1).await,
                Err(_) => return Err(HilError::UnexpectedBinding),
            },
            PhyColdI2cAction::StartWrite { address, value } => {
                emergency_log(format_args!(
                    "OPEN_RADIO_ORACLE_HIL probe=temperature-start-write block={:#04x} \
                     register={:#04x} value={value:#04x}",
                    address.block(),
                    address.register()
                ));
                match binding.start_target(platform) {
                    Ok(()) => {}
                    Err(PhyColdI2cError::BusyAtStart) => Timer::after_micros(1).await,
                    Err(_) => return Err(HilError::UnexpectedBinding),
                }
            }
            PhyColdI2cAction::AwaitReadCompletionEdge { .. }
            | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                Timer::after_micros(1).await;
                match binding
                    .observe_target_edge(platform)
                    .map_err(|_| HilError::UnexpectedBinding)?
                {
                    PhyColdI2cObservation::EdgeConsumed | PhyColdI2cObservation::StillPending => {}
                }
            }
            PhyColdI2cAction::Complete(_) => {
                return binding
                    .into_completion()
                    .map_err(|_| HilError::UnexpectedBinding);
            }
        }
    }
    Err(HilError::I2cDeadline)
}

async fn complete_binding<
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
>(
    binding: PhyChipChannelExternalBinding,
    platform: &mut P,
    registers: &mut RadioRegisters,
) -> Result<PhyChipChannelCompletion, HilError> {
    match binding {
        PhyChipChannelExternalBinding::Mmio(binding) => {
            Ok(binding.execute_target(platform, registers))
        }
        PhyChipChannelExternalBinding::Timer(binding) => {
            Timer::after_micros(u64::from(binding.micros())).await;
            Ok(binding.into_completion())
        }
        PhyChipChannelExternalBinding::I2c(binding) => complete_i2c(binding, platform).await,
        PhyChipChannelExternalBinding::TxGain(binding) => Ok(binding.execute()),
        PhyChipChannelExternalBinding::Temperature(binding) => {
            let completion = match binding {
                PhyTemperatureExternalBinding::I2c(binding) => {
                    complete_temperature_i2c(binding, platform).await?
                }
                PhyTemperatureExternalBinding::Sample(binding) => binding.execute_target(platform),
            };
            Ok(PhyChipChannelCompletion::Temperature(completion))
        }
    }
}

async fn run_open_mac_rx(
    platform: &mut EspHalRadioPeripheral,
    mmio: &mut ColdRadioRegisters,
) -> bool {
    let storage = RX_STORAGE.init(RxStorage::new());
    let descriptor_base = storage.descriptors.as_ptr().addr() as u32;
    let buffer_addresses: [u32; RX_DESCRIPTOR_COUNT] =
        core::array::from_fn(|index| storage.buffers[index].address());

    if let Err(error) = build_cold_ring(
        &storage.descriptors,
        descriptor_base,
        &buffer_addresses,
        RX_BUFFER_SIZE as u32,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_ORACLE_HIL result=FAIL stage=rx-ring-build error={error:?}"
        ));
        return false;
    }

    let mut station_address = [0_u8; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0_u8; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let cold = match initialize_promiscuous_receive(
        platform,
        mmio,
        MAC_HANDSHAKE_SAMPLE_LIMIT,
        station_address,
        access_point_address,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_ORACLE_HIL result=FAIL stage=mac-cold-start error={error:?}"
            ));
            return false;
        }
    };
    if let Err(error) = publish_cold_ring(mmio, descriptor_base, true) {
        emergency_log(format_args!(
            "OPEN_RADIO_ORACLE_HIL result=FAIL stage=rx-ring-publish error={error:?}"
        ));
        return false;
    }
    emergency_log(format_args!("OPEN_RADIO_ORACLE_HIL stage=open-tx-power"));
    log_phy_mmio_page_hashes("vendor-phy-open-mac");
    let mut tx = TxSlot::pin_static(TxSlot::init_in_place(TX_STORAGE.uninit()));
    let frame_length = {
        let buffer = tx.as_mut().buffer_mut().unwrap();
        OpenAuthenticationRequest {
            source: station_address,
            sequence_number: 1,
            bssid: TARGET_BSSID,
        }
        .encode(&mut buffer[TX_METADATA_SIZE..])
        .unwrap()
    };
    let hardware_frame_length = frame_length + TX_FCS_SIZE;
    {
        let buffer = tx.as_mut().buffer_mut().unwrap();
        buffer[..4].copy_from_slice(&(hardware_frame_length as u32).to_le_bytes());
        buffer[4..TX_METADATA_SIZE].fill(0);
        buffer[TX_METADATA_SIZE + frame_length..TX_METADATA_SIZE + hardware_frame_length].fill(0);
    }
    let descriptor_address = tx.as_ref().descriptor_address();
    let transfer_length = TX_METADATA_SIZE + hardware_frame_length;
    let descriptor_capacity = (transfer_length + 3) & !3;
    let cookie = tx
        .as_mut()
        .reserve(descriptor_capacity as u32, transfer_length as u32)
        .unwrap();
    configure_sta_link_receive_policy(&mut **mmio, TARGET_BSSID);
    // This HIL submits a raw MPDU through the open q0 descriptor ABI, so PLCP
    // SIGNAL owns MPDU+FCS (30+4 = 0x22). The 0x00b6 value visible in vendor
    // submissions belongs to vendor-private metadata and cannot be replayed
    // as the raw PLCP length: the direct-q0 HIL then never completed.
    // SOURCE: `driver/esp32s31/wifi/lmac/src/tx.rs`
    // `management_1m_from_mpdu_length` and live `hybrid-power5.log`.
    let mut config = LegacyTxConfig::management_1m_from_mpdu_length(frame_length as u16).unwrap();
    config.data_power = 5;
    config.rts_power_low = 5;
    config.rts_power_high = 5;
    config.pti = 0;
    config.pti_count = 1;
    config.timeout = 10;
    // Phase-align this snapshot with the full-vendor hook: the descriptor,
    // link/BSSID policy and complete queue vector are prepared, but the queue
    // control edge has not yet been published.
    log_mac_tx_rx_boundary("open-before-submit");
    log_tx_state_vector("hybrid-open-before-submit");
    log_full_mac_words("hybrid-open-before-submit");
    tx.as_mut().submit_legacy_q0(mmio, cookie, config).unwrap();
    log_mac_tx_rx_boundary("open-after-submit");
    log_tx_state_vector("hybrid-open-after-submit");
    let mut tx_completion = None;
    for _ in 0..TX_COMPLETION_SAMPLE_LIMIT {
        if let Some(completion) = tx.as_mut().acknowledge_q0_completion(mmio).unwrap() {
            tx_completion = Some(completion);
            break;
        }
        Timer::after_micros(1).await;
    }
    match tx_completion {
        Some(completion) => emergency_log(format_args!(
            "OPEN_RADIO_ORACLE_HIL stage=open-mac-auth-tx \
             status={} descriptor={descriptor_address:#010x} word0={:#010x}",
            completion.status,
            tx.descriptor_word0(),
        )),
        None => {
            emergency_log(format_args!(
                "OPEN_RADIO_ORACLE_HIL result=FAIL stage=open-mac-tx-timeout \
                 descriptor={descriptor_address:#010x} word0={:#010x}",
                tx.descriptor_word0(),
            ));
            return false;
        }
    }

    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL stage=rx-active descriptor_base={descriptor_base:#010x} \
         handshake_samples={} control={:#010x} base={:#010x} next={:#010x} high={:#010x}",
        cold.handshake_samples,
        mmio.read32(RX_CONTROL),
        mmio.read32(RX_DESCRIPTOR_BASE),
        mmio.read32(RX_NEXT_DESCRIPTOR),
        mmio.read32(RX_LAST_DESCRIPTOR_HIGH),
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL probe=phy-rx-diff \
         frequency={:#010x}/{:#010x}/{:#010x} \
         clocks={:#010x}/{:#010x} agc={:#010x}/{:#010x} \
         rx={:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}",
        read_diagnostic_mmio(0x2010_001c),
        read_diagnostic_mmio(0x2010_0024),
        read_diagnostic_mmio(0x2010_0028),
        read_diagnostic_mmio(0x2010_0400),
        read_diagnostic_mmio(0x2010_0408),
        read_diagnostic_mmio(0x2010_705c),
        read_diagnostic_mmio(0x2010_7064),
        read_diagnostic_mmio(0x2010_70a0),
        read_diagnostic_mmio(0x2010_7104),
        read_diagnostic_mmio(0x2010_7114),
        read_diagnostic_mmio(0x2010_7848),
        read_diagnostic_mmio(0x2010_78c8),
    ));

    let mut frame = [0_u8; RX_BUFFER_SIZE];
    let mut observed_mask = 0_u32;
    let mut received_frames = 0_u32;
    for sample in 0..RX_OBSERVATION_LIMIT {
        for (index, descriptor) in storage.descriptors.iter().enumerate() {
            let word0 = descriptor.word0();
            let bit = 1_u32 << index;
            if !rx_done(word0) || observed_mask & bit != 0 {
                continue;
            }
            observed_mask |= bit;
            received_frames = received_frames.saturating_add(1);
            let segment = RxSegment {
                descriptor_address: descriptor_base + index as u32 * 12,
                descriptor_word0: word0,
                buffer: unsafe {
                    // The completed descriptor returned this buffer to the
                    // sole HIL task until the ring is rebuilt.
                    storage.buffers[index].as_slice()
                },
                next_descriptor_address: descriptor.next_address(),
            };
            let Ok(extracted) = extract_management(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                &mut frame,
            ) else {
                continue;
            };
            let subtype = frame[0] & 0xfc;
            if subtype == 0xb0 || received_frames <= 3 {
                emergency_log(format_args!(
                    "OPEN_RADIO_ORACLE_HIL probe=auth-rx sample={sample} \
                     frame={received_frames} subtype={subtype:#04x} \
                     length={} da={:02x?} sa={:02x?}",
                    extracted.length,
                    &frame[4..10],
                    &frame[10..16],
                ));
            }
            if let Some(response) = parse_open_authentication_response(
                &frame[..extracted.length],
                station_address,
                TARGET_BSSID,
            ) {
                emergency_log(format_args!(
                    "OPEN_RADIO_ORACLE_HIL result={} stage=vendor-phy-open-mac-auth \
                     status={} frames={received_frames}",
                    if response.status_code == 0 {
                        "PASS"
                    } else {
                        "FAIL"
                    },
                    response.status_code,
                ));
                return response.status_code == 0;
            }
        }
        Timer::after_millis(1).await;
    }

    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL result=FAIL stage=vendor-phy-open-mac-auth-timeout \
         frames={received_frames} \
         words={:#010x}/{:#010x}/{:#010x}/{:#010x} control={:#010x} \
         base={:#010x} next={:#010x} last={:#010x} int_raw={:#010x}",
        storage.descriptors[0].word0(),
        storage.descriptors[1].word0(),
        storage.descriptors[2].word0(),
        storage.descriptors[3].word0(),
        mmio.read32(RX_CONTROL),
        mmio.read32(RX_DESCRIPTOR_BASE),
        mmio.read32(RX_NEXT_DESCRIPTOR),
        mmio.read32(RX_LAST_DESCRIPTOR),
        mmio.read32(MAC_INT_RAW),
    ));
    false
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let (mcause, mepc, mtval): (usize, usize, usize);
    unsafe {
        core::arch::asm!("csrr {0}, mcause", out(reg) mcause);
        core::arch::asm!("csrr {0}, mepc", out(reg) mepc);
        core::arch::asm!("csrr {0}, mtval", out(reg) mtval);
    }
    panic_report(mcause, mepc, mtval);
    halt()
}

#[esp_hal::main]
async fn main(_spawner: embassy_executor::Spawner) {
    unsafe {
        ets_install_usb_printf();
    }
    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL schema=1 init=vendor channel=open-rust"
    ));

    let peripherals = esp_hal::init(esp_hal::Config::default());
    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let system_timer = SystemTimer::new(peripherals.SYSTIMER);
    esp_rtos::start(system_timer.alarm0, software_interrupts.software_interrupt0);
    let radio = EspHalRadioPeripheral::new(
        peripherals.WIFI,
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    );

    // The guard performs the known-good vendor cold PHY calibration and keeps
    // its clocks alive. No vendor Wi-Fi/MAC controller is created.
    let _oracle_guard = esp_phy::enable_phy();
    unsafe {
        phy_wifi_enable_set(1);
    }
    emergency_log(format_args!("OPEN_RADIO_ORACLE_HIL stage=vendor-phy-ready"));
    let mut repeated_txdc = [0_u16; 20];
    // SAFETY: this is an isolated oracle rerun after the same vendor PHY has
    // completed cold initialization. `libphy.a[phy_tx_cal.o]` proves that
    // `phy_txdc_cal_init(output, 15, 0, 0)` writes exactly five 8-byte rows.
    unsafe {
        phy_txdc_cal_init(repeated_txdc.as_mut_ptr(), 15, 0, 0);
    }
    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL probe=vendor-txdc-rerun values={repeated_txdc:?} \
         bb_init={:#010x} pbus={:#010x}/{:#010x}/{:#010x} \
         tone={:#010x}/{:#010x}/{:#010x}/{:#010x} control={:#010x}",
        read_diagnostic_mmio(0x2010_0800),
        read_diagnostic_mmio(0x2010_0884),
        read_diagnostic_mmio(0x2010_088c),
        read_diagnostic_mmio(0x2010_0890),
        read_diagnostic_mmio(0x2010_040c),
        read_diagnostic_mmio(0x2010_041c),
        read_diagnostic_mmio(0x2010_0420),
        read_diagnostic_mmio(0x2010_0428),
        read_diagnostic_mmio(0x2010_0418),
    ));
    let mut rate_zero_power = [0_i8; 2];
    let power_result = unsafe { phy_get_max_pwr(0, rate_zero_power.as_mut_ptr()) };
    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL probe=rate-zero-power result={power_result} \
         values={rate_zero_power:?}"
    ));
    let frequency_control = unsafe { (0x2010_001c as *const u32).read_volatile() };
    let frequency_parameter_0 = unsafe { (0x2010_0024 as *const u32).read_volatile() };
    let frequency_parameter_1 = unsafe { (0x2010_0028 as *const u32).read_volatile() };
    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL probe=vendor-frequency-registers \
         control={frequency_control:#010x} parameter0={frequency_parameter_0:#010x} \
         parameter1={frequency_parameter_1:#010x}"
    ));

    // SAFETY: vendor PHY initialization has returned, no Wi-Fi driver exists,
    // and the oracle guard serializes the parameter image for this HIL.
    let parameter_image = unsafe {
        let source = core::ptr::addr_of!(phy_param).cast::<u8>();
        let mut image = [0_u8; 0x1fc];
        let mut index = 0;
        while index != image.len() {
            image[index] = source.add(index).read_volatile();
            index += 1;
        }
        image
    };
    let mut state = PhyColdState::from_parameter_image(parameter_image);

    // SAFETY: the sole WIFI token is transferred to open-radio after the PHY
    // oracle completed. The returned guard only retains the already
    // established clocks and does not continue radio transactions. Replaying
    // `power_up` here would reset the calibrated oracle state, so this HIL
    // explicitly adopts the completed external prerequisites. The powered
    // owner remains live for the entire open channel graph.
    let mut owner = unsafe { Radio::claim(radio).assume_powered_after_external_initialization() };
    // Vendor PHY calibration powers the temperature sensor back down and
    // leaves its DAC field at zero. Channel programming samples temperature,
    // so establish the first valid ROM range through the open identity-bound
    // PHY-I2C path before handing off to that graph.
    let temperature_power =
        PhyColdMmioBinding::new(PhyRfInitPrefixAction::ConfigureTemperatureSensorRead).unwrap();
    if temperature_power.execute_target(&mut owner).is_err() {
        emergency_log(format_args!(
            "OPEN_RADIO_ORACLE_HIL result=FAIL stage=open-temperature-mmio"
        ));
        halt();
    }
    unsafe {
        phy_i2c_writeReg(0x69, 1, 0, 0);
    }
    let oracle_before = unsafe { phy_i2c_readReg(0x69, 1, 0) };
    let host_before = unsafe { (0x2010_f820 as *const u32).read_volatile() };
    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL probe=tsens-before oracle={oracle_before:#04x}"
    ));
    let temperature_sensor = PhyI2cAddress::new(0x69, 0).unwrap();
    let temperature_binding = PhyTemperatureI2cBinding::new(PhyTemperatureAction::WriteMasked {
        address: temperature_sensor,
        high_bit: 3,
        low_bit: 0,
        value: 5,
    })
    .unwrap();
    if let Err(error) = complete_temperature_i2c(temperature_binding, owner.parts_mut().0).await {
        emergency_log(format_args!(
            "OPEN_RADIO_ORACLE_HIL result=FAIL stage=open-temperature-power error={error:?}"
        ));
        halt();
    }
    let command_after_open = unsafe { (0x2010_f800 as *const u32).read_volatile() };
    let oracle_after_open = unsafe { phy_i2c_readReg(0x69, 1, 0) };
    let host_after_open = unsafe { (0x2010_f820 as *const u32).read_volatile() };
    emergency_log(format_args!(
        "OPEN_RADIO_ORACLE_HIL stage=open-temperature-power dac=5 \
         oracle_after_open={oracle_after_open:#04x} \
         command={command_after_open:#010x} \
         host={host_before:#010x}/{host_after_open:#010x}"
    ));
    if oracle_after_open != 5 {
        emergency_log(format_args!(
            "OPEN_RADIO_ORACLE_HIL result=FAIL stage=open-temperature-verify \
             expected=0x05 actual={oracle_after_open:#04x}"
        ));
        halt();
    }

    let mut transition = PhyChipChannelTransition::new(PhyChipChannelRequest {
        channel_or_frequency: TARGET_CHANNEL,
        cbw: 0,
        parameters: state.channel_parameters(),
    });
    let mut operations = 0_u32;

    loop {
        let action = transition.action();
        match action {
            PhyChipChannelAction::Complete(outcome) => {
                state.apply_channel_outcome(outcome);
                emergency_log(format_args!(
                    "OPEN_RADIO_ORACLE_HIL stage=open-channel \
                     channel={} frequency={} temperature={} operations={}",
                    outcome.channel,
                    outcome.frequency_mhz,
                    outcome.temperature.temperature,
                    operations
                ));
                log_phy_mmio_page_hashes("vendor");
                log_selected_phy_mmio("vendor");
                log_listen_frequency_memory("vendor");
                log_differing_phy_mmio_pages("vendor");
                log_parameter_image("vendor", state.parameter_image());
                log_vendor_analog_fingerprint();
                let vendor_channel_result =
                    unsafe { phy_change_channel(u32::from(TARGET_CHANNEL), 1, 0, 0) };
                emergency_log(format_args!(
                    "OPEN_RADIO_ORACLE_HIL stage=vendor-channel-after-open \
                     result={vendor_channel_result}"
                ));
                // Feed the Rust-owned profile derived from the vendor
                // parameter image into the open MAC table builder. A
                // quarter-dBm ceiling of 20 yields the vendor-observed gain
                // code 5; calling `hal_init_tx_pwr` and then patching only its
                // legacy table left the TB/RU table at code 20.
                // SOURCE: complete ROM `phy_get_max_pwr`, complete blob
                // `hal_init_tx_pwr`, and vendor-first-tx-snapshot.log.
                owner.parts_mut().0.install_phy_tx_power_profile(
                    state.tx_target_power_profile().with_maximum_quarter_dbm(20),
                );
                let (mut platform, mut registers) = owner.into_parts();
                let _ = run_open_mac_rx(&mut platform, &mut registers).await;
                break;
            }
            PhyChipChannelAction::Failed(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_ORACLE_HIL result=FAIL stage=open-channel \
                     error={error:?} operations={operations}"
                ));
                break;
            }
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                if samples >= FREQUENCY_READY_LIMIT =>
            {
                if transition
                    .advance(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                    .is_err()
                {
                    emergency_log(format_args!(
                        "OPEN_RADIO_ORACLE_HIL result=FAIL stage=transition-timeout"
                    ));
                    break;
                }
            }
            _ => {
                let binding = match PhyChipChannelExternalBinding::lower(action) {
                    Ok(binding) => binding,
                    Err(error) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_ORACLE_HIL result=FAIL stage=lowering \
                             action={action:?} error={error:?}"
                        ));
                        break;
                    }
                };
                let (platform, registers) = owner.parts_mut();
                let completion = match complete_binding(binding, platform, registers).await {
                    Ok(completion) => completion,
                    Err(error) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_ORACLE_HIL result=FAIL stage=port \
                             error={error:?} operations={operations}"
                        ));
                        break;
                    }
                };
                if transition.advance(completion).is_err() {
                    emergency_log(format_args!(
                        "OPEN_RADIO_ORACLE_HIL result=FAIL stage=transition \
                         operations={operations}"
                    ));
                    break;
                }
                operations = operations.wrapping_add(1);
            }
        }
    }
    halt()
}

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
