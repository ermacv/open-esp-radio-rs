#![no_main]
#![no_std]
#![recursion_limit = "256"]
#[cfg(feature = "owned-network")]
extern crate embassy_net_owned as embassy_net;
#[cfg(all(feature = "open-radio-hil", not(feature = "owned-network")))]
compile_error!("owned Xarxa is the only network integration: enable the owned-network feature");
#[cfg(not(any(
    feature = "boot-smoke",
    feature = "open-radio-hil",
    feature = "bluetooth-hil",
    feature = "bluetooth-gatt",
    feature = "bluetooth-secure-gatt",
    feature = "system-watchdog",
    feature = "system-panic-reset"
)))]
compile_error!(
    "select boot-smoke, open-radio-hil, bluetooth-hil, bluetooth-gatt, bluetooth-secure-gatt, system-watchdog or system-panic-reset"
);
#[cfg(any(
    all(feature = "bluetooth-hil", feature = "bluetooth-gatt"),
    all(feature = "bluetooth-hil", feature = "bluetooth-secure-gatt"),
    all(feature = "bluetooth-gatt", feature = "bluetooth-secure-gatt")
))]
compile_error!(
    "select one Bluetooth image: bluetooth-hil, bluetooth-gatt or bluetooth-secure-gatt"
);
#[cfg(all(
    feature = "system-watchdog",
    any(
        feature = "boot-smoke",
        feature = "open-radio-hil",
        feature = "bluetooth-radio"
    )
))]
compile_error!("system-watchdog requires an exclusive radio-free image");
#[cfg(all(
    feature = "system-panic-reset",
    any(
        feature = "panic-hook",
        feature = "boot-smoke",
        feature = "system-watchdog",
        feature = "open-radio-hil",
        feature = "bluetooth-radio"
    )
))]
compile_error!(
    "system-panic-reset requires an exclusive radio-free image with the product panic entry"
);
#[cfg(all(
    feature = "bluetooth-radio",
    any(feature = "boot-smoke", feature = "open-radio-hil")
))]
compile_error!("Bluetooth requires exclusive radio composition");
#[cfg(all(feature = "boot-smoke", feature = "open-radio-hil"))]
compile_error!("boot-smoke and open-radio-hil are mutually exclusive scenarios");
#[cfg(any(
    all(
        feature = "ieee802154-event-status-probe",
        feature = "ieee802154-ed-event-probe"
    ),
    all(
        feature = "ieee802154-event-status-probe",
        feature = "ieee802154-radio"
    ),
    all(feature = "ieee802154-ed-event-probe", feature = "ieee802154-radio"),
    all(
        feature = "ieee802154-route-probe",
        any(
            feature = "ieee802154-event-status-probe",
            feature = "ieee802154-ed-event-probe",
            feature = "ieee802154-radio"
        )
    )
))]
compile_error!("IEEE 802.15.4 diagnostic images are mutually exclusive");

#[cfg(feature = "open-radio-hil")]
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use core::{arch::asm, ffi::CStr, ptr};

#[cfg(feature = "open-radio-hil")]
use embassy_executor::SendSpawner;
#[cfg(feature = "boot-smoke")]
use embassy_time::{Duration, Timer};
#[cfg(feature = "open-radio-hil")]
use esp_hal::system::{CpuControl, Stack};
use esp_hal::timer::{OneShotTimer, timg::TimerGroup};
use oer_esp32s31_executor_embassy::Executor;
use static_cell::StaticCell;

oer_esp32s31_platform_runtime::interrupt_table! {
    /// Wakes the core-0 Embassy executor.
    wake: ExecutorWake = FROM_CPU_INTR0 => oer_esp32s31_executor_embassy::wake_handler::<0>, Priority1, ProCpu;
    /// The Embassy time driver's alarm (TIMG0 timer 0).
    alarm: TimeAlarm = TG0_T0_LEVEL => oer_esp32s31_executor_embassy::timer_interrupt, Priority1, ProCpu;
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    /// The Wi-Fi MAC's interrupt.
    wifi_mac: WifiMac = MODEM_WIFI_MAC => oer_esp32s31_ieee80211_system::mac_interrupt, Priority1, ProCpu;
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    /// The Wi-Fi power interrupt.
    wifi_power: WifiPower = MODEM_WIFI_PWR => oer_esp32s31_ieee80211_system::power_interrupt, Priority1, ProCpu;
    #[cfg(any(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
    /// The Bluetooth Controller's primary interrupt.
    bluetooth_primary: BluetoothPrimary = MODEM_BT_MAC => oer_esp32s31_radio_esp_hal::bluetooth_primary_interrupt_handler, Priority3, ProCpu;
    #[cfg(any(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
    /// The modem low-power timer, the Controller's deadline source.
    bluetooth_timer: BluetoothTimer = MODEM_LP_TIMER => oer_esp32s31_radio_esp_hal::bluetooth_modem_lp_timer_interrupt_handler, Priority3, ProCpu;
    #[cfg(any(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
    /// The Bluetooth Controller's NRT interrupt.
    bluetooth_nrt: BluetoothNrt = MODEM_BT_MAC_INT1 => oer_esp32s31_radio_esp_hal::bluetooth_nrt_default_interrupt_handler, Priority3, ProCpu;
    #[cfg(feature = "ieee802154-radio")]
    /// The IEEE 802.15.4 MAC's interrupt.
    ieee802154: Ieee802154Mac = MODEM_ZB_MAC => oer_esp32s31_ieee802154_system::ieee802154_interrupt, Priority1, ProCpu;
    #[cfg(feature = "ieee802154-route-probe")]
    /// The IEEE 802.15.4 MAC's interrupt, to the route probe.
    ieee802154_probe: Ieee802154Probe = MODEM_ZB_MAC => product_hil::route_probe_interrupt, Priority1, ProCpu;
    #[cfg(any(feature = "memory-benchmark", feature = "gdma-mem2mem-probe"))]
    /// AXI GDMA channel 0's inbound interrupt.
    dma_input: DmaInput = AXI_PDMA_IN_CH0 => oer_esp32s31_soc_esp_hal::axi_gdma_mem2mem_interrupt, Priority1, ProCpu;
    #[cfg(any(feature = "memory-benchmark", feature = "gdma-mem2mem-probe"))]
    /// AXI GDMA channel 0's outbound interrupt.
    dma_output: DmaOutput = AXI_PDMA_OUT_CH0 => oer_esp32s31_soc_esp_hal::axi_gdma_mem2mem_interrupt, Priority1, ProCpu;
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    /// The hang watchdog's period (SYSTIMER alarm 0).
    hang_check: HangWatchdogCheck = SYSTIMER_TARGET0 => hang_watchdog::check, Priority8, ProCpu;
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    /// The hang watchdog's request that core 1 samples its context.
    hang_sample: HangWatchdogSample = FROM_CPU_INTR2 => hang_watchdog::sample_core1, Priority8, AppCpu;
    #[cfg(feature = "pc-profile")]
    /// The PC profiler's period (SYSTIMER alarm 1).
    profile_sample: ProfileSample = SYSTIMER_TARGET1 => pc_profile::sample_core0, Priority8, ProCpu;
    #[cfg(feature = "pc-profile")]
    /// The PC profiler's request that core 1 samples its PC.
    profile_core1: ProfileCore1Sample = FROM_CPU_INTR3 => pc_profile::sample_core1, Priority8, AppCpu;
    #[cfg(feature = "open-radio-hil")]
    /// Wakes the core-1 Embassy executor.
    app_wake: AppExecutorWake = FROM_CPU_INTR1 => oer_esp32s31_executor_embassy::wake_handler::<1>, Priority1, AppCpu;
    #[cfg(any(
        feature = "system-watchdog",
        feature = "system-panic-reset",
        feature = "open-radio-hil",
        feature = "bluetooth-radio"
    ))]
    /// The USB Serial/JTAG console, to esp-hal's async driver.
    console_usb: ConsoleUsb = USB_DEVICE => esp_hal::usb::usb_serial_jtag::handle_async_interrupt, Priority1, ProCpu;
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    /// System timer alarm 2, the interrupt-table probe's source.
    source_gate: SourceGateToken = SYSTIMER_TARGET2 => source_gate::on_alarm, Priority1, ProCpu;
}

#[cfg(any(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
mod bluetooth;
#[cfg(all(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
compile_error!("the joint Wi-Fi/Bluetooth image runs Bluetooth as a shared-radio client");
#[cfg(feature = "boot-smoke")]
mod boot_smoke_console;
#[cfg(feature = "open-radio-hil")]
mod console;
mod exception;
mod fatal;
#[cfg(feature = "gdma-mem2mem-probe")]
mod gdma_mem2mem_probe;
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
mod hang_watchdog;
#[cfg(any(
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "open-radio-hil",
    feature = "bluetooth-radio"
))]
mod image_features;
#[cfg(feature = "open-radio-hil")]
mod limits;
#[cfg(feature = "memory-benchmark")]
mod memory_benchmark;
#[cfg(feature = "pc-profile")]
mod pc_profile;
#[cfg(feature = "open-radio-hil")]
mod phy_fault;
mod software_interrupt;
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
mod source_gate;
#[cfg(any(feature = "open-radio-hil", feature = "bluetooth-radio"))]
mod stack_evidence;
#[cfg(any(
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "open-radio-hil",
    feature = "bluetooth-radio"
))]
mod system;
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
mod trace;
#[cfg(any(
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "open-radio-hil",
    feature = "bluetooth-radio"
))]
mod transport;
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
mod watchdog;
#[cfg(all(feature = "memory-benchmark", feature = "gdma-mem2mem-probe"))]
compile_error!(
    "memory-benchmark and the startup GDMA probe require exclusive DMA channel ownership"
);
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
mod phy_calibration_artifact;
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
mod phy_tracking;
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
mod product_hil;
use oer_esp32s31_platform_runtime::stacks as psram_task_stack;

const DATA_SENTINEL: u32 = 0x5353_31d2;
const INTERNAL_SRAM_START: u32 = 0x2f00_0000;
const INTERNAL_SRAM_END: u32 = 0x2f07_afc0;
#[cfg(any(
    feature = "open-radio-hil",
    feature = "bluetooth-gatt",
    feature = "bluetooth-secure-gatt"
))]
const STACK_PAINT_WORD: u32 = 0xa55a_a55a;
#[cfg(any(
    feature = "open-radio-hil",
    feature = "bluetooth-gatt",
    feature = "bluetooth-secure-gatt"
))]
const STACK_PAINT_MARGIN_BYTES: u32 = 256;
#[cfg(any(
    feature = "open-radio-hil",
    feature = "bluetooth-gatt",
    feature = "bluetooth-secure-gatt"
))]
const STACK_PAINT_BOTTOM_RESERVE_BYTES: u32 = 256;
#[cfg(feature = "open-radio-hil")]
// CPU1 runs the Embassy network executor in split images. Its nested async call
// graph needs more than 10 KiB under sustained A-MPDU traffic. All images keep
// the same 16-KiB owner stack so runtime-selected placement cannot change the
// executable resource graph or weaken the independently checked 4-KiB reserve.
pub(crate) const APP_CORE_TASK_STACK_BYTES: usize = 16 * 1024;
#[cfg(feature = "open-radio-hil")]
const APP_CORE_BOOTSTRAP_STACK_BYTES: usize = 8 * 1024;

const PROFILE_CODE_START: u32 = 0x5001_0000;
const PROFILE_CODE_END: u32 = 0x5100_0000;
const PROFILE_DATA_START: u32 = 0x5000_0000;
const PROFILE_DATA_END: u32 = 0x5100_0000;
const PROFILE_NAME: &core::ffi::CStr = c"psram-code-psram-data-psram-stack";

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
#[cfg(feature = "open-radio-hil")]
static APP_EXECUTOR: StaticCell<Executor<1>> = StaticCell::new();
/// Core 1's interrupt tokens, which core 0 leaves for it before starting it.
#[cfg(feature = "open-radio-hil")]
struct AppCoreInterrupts {
    wake: AppExecutorWake,
    #[cfg(not(feature = "memory-benchmark"))]
    hang_sample: HangWatchdogSample,
    #[cfg(feature = "pc-profile")]
    profile_sample: ProfileCore1Sample,
}

#[cfg(feature = "open-radio-hil")]
static APP_INTERRUPTS: critical_section::Mutex<core::cell::RefCell<Option<AppCoreInterrupts>>> =
    critical_section::Mutex::new(core::cell::RefCell::new(None));

/// Leave core 1 its interrupt tokens. A function of its own, so that
/// `runtime_main`'s closures keep the names the stack coverage reviews.
#[cfg(feature = "open-radio-hil")]
fn leave_app_core_interrupts(interrupts: AppCoreInterrupts) {
    critical_section::with(|cs| APP_INTERRUPTS.borrow_ref_mut(cs).replace(interrupts));
}
// The hardware entropy source is a process-lifetime owner. Keeping it in a
// named static prevents task cancellation or panic cleanup from trying to
// disable the source while a nested radio future still owns `Trng`.
#[cfg(all(feature = "open-radio-hil", not(feature = "wifi-ble-coex")))]
static TRNG_SOURCE: StaticCell<esp_hal::rng::TrngSource<'static>> = StaticCell::new();
// The joint image's Bluetooth client owns the same source through its entropy
// service; the Wi-Fi readers borrow it for the process lifetime.
#[cfg(feature = "wifi-ble-coex")]
static BLUETOOTH_ENTROPY: StaticCell<oer_esp32s31_bluetooth_system::BluetoothEntropy<'static>> =
    StaticCell::new();
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
static L1_CACHE_PERFORMANCE: StaticCell<oer_esp32s31_soc_esp_hal::L1CachePerformanceCounters> =
    StaticCell::new();
#[cfg(feature = "open-radio-hil")]
static APP_SEND_SPAWNER: StaticCell<SendSpawner> = StaticCell::new();
#[cfg(feature = "open-radio-hil")]
static APP_SEND_SPAWNER_PTR: AtomicPtr<SendSpawner> = AtomicPtr::new(ptr::null_mut());
#[cfg(feature = "open-radio-hil")]
static APP_STACK_PAINT_END: AtomicU32 = AtomicU32::new(0);
oer_memory::zeroed_static! {
    #[cfg(feature = "open-radio-hil")]
    static mut APP_CORE_STACK: AppCoreStack =
        zeroed in ".critical.bss.open_radio_app_core_bootstrap_stack";
}

/// esp-hal's application-core stack in a zeroed region.
#[cfg(feature = "open-radio-hil")]
#[repr(transparent)]
struct AppCoreStack(Stack<APP_CORE_BOOTSTRAP_STACK_BYTES>);

#[cfg(feature = "open-radio-hil")]
#[allow(unsafe_code, reason = "esp-hal's stack is uninitialized memory")]
// REVIEWED-LAYOUT: esp-hal d61af210
// SAFETY: at the pinned esp-hal revision `Stack<SIZE>` is
// `repr(C, align(16))` with one field, `mem: MaybeUninit<[u8; SIZE]>`, valid
// for any bytes; `Stack::new` leaves it uninitialized.
unsafe impl bytemuck::Zeroable for AppCoreStack {}
static mut INITIALIZED_DATA: u32 = DATA_SENTINEL;
static mut BSS_PROBE: u32 = 0;

#[used]
#[unsafe(link_section = ".isr.rodata.profile_probe")]
static ISR_RODATA_PROBE: u32 = 0x4953_5231;
#[used]
#[unsafe(link_section = ".critical.data.profile_probe")]
static mut CRITICAL_DATA_PROBE: u32 = 0x4352_5431;
#[used]
#[unsafe(link_section = ".dma.data.profile_probe")]
static mut DMA_DATA_PROBE: u32 = 0x444d_4131;
oer_memory::zeroed_static! {
    #[used]
    static mut DMA_BSS_PROBE: u32 =
        zeroed in ".dma.bss.profile_probe";
}

unsafe extern "C" {
    fn ets_install_usb_printf();
    fn ets_printf(format: *const core::ffi::c_char, ...) -> i32;

    static __runtime_image_start: u8;
    static __runtime_payload_end: u8;
    static __runtime_bss_start: u8;
    static __runtime_bss_end: u8;
    static __runtime_data_load_start: u8;
    static __runtime_data_start: u8;
    static __runtime_data_end: u8;
    static __runtime_data_bss_start: u8;
    static __runtime_data_bss_end: u8;
    static __runtime_isr_start: u8;
    static __runtime_isr_end: u8;
    static __runtime_dma_data_start: u8;
    static __runtime_dma_data_end: u8;
    static __runtime_dma_bss_start: u8;
    static __runtime_dma_bss_end: u8;
    static __runtime_cpu0_irq_stack_bottom: u8;
    static __runtime_cpu0_irq_stack_top: u8;
    static __runtime_cpu1_irq_stack_bottom: u8;
    static __runtime_cpu1_irq_stack_top: u8;
    static _stack_end: u8;
    static _stack_start: u8;
}

use oer_esp32s31_platform_runtime as _;

/// The image's record of a panic, which the platform's panic entry calls
/// after its own record and before it resets the chip (feature `panic-hook`):
/// it freezes the trace and records the post-mortem fault and the machine
/// state, formatting nothing, so the panic path stays a leaf in every stack
/// bound. The next boot reports what it recorded.
#[cfg(feature = "panic-hook")]
#[unsafe(no_mangle)]
fn oer_platform_panic_hook(info: &core::panic::PanicInfo<'_>) {
    // The trace keeps what happened before the panic.
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    oer_trace::freeze(<oer_hil_trace::Panic as oer_trace::Event>::KIND, 0);
    #[cfg(any(
        feature = "system-watchdog",
        feature = "open-radio-hil",
        feature = "bluetooth-radio"
    ))]
    system::postmortem::record_panic(info);
    #[cfg(not(any(
        feature = "system-watchdog",
        feature = "open-radio-hil",
        feature = "bluetooth-radio"
    )))]
    let _ = info;
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    let stage = product_hil::diagnostic_snapshot().0;
    #[cfg(not(all(feature = "open-radio-hil", not(feature = "memory-benchmark"))))]
    let stage = 0;
    fatal::record_panic(stage);
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    unsafe { ets_install_usb_printf() };
    print(c"OPEN_RADIO_HIL runtime=START profile=");
    print(PROFILE_NAME);
    print(c"\r\n");
    // Before anything can fail and overwrite it.
    fatal::report_previous();

    validate_runtime_layout();

    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));
    // Before anything records a checkpoint over the previous boot's record.
    #[cfg(any(
        feature = "system-watchdog",
        feature = "open-radio-hil",
        feature = "bluetooth-radio"
    ))]
    system::postmortem::begin();
    // Before any boot evidence is served: taking it clears the record.
    #[cfg(any(
        feature = "system-watchdog",
        feature = "system-panic-reset",
        feature = "open-radio-hil",
        feature = "bluetooth-radio"
    ))]
    system::take_platform_panic();
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    trace::install();
    // The bootstrap configured PSRAM before entering this separately linked
    // runtime. `esp_hal::init()` cannot carry process-local mapping metadata
    // across that ELF boundary, so stage two explicitly adopts the live
    // hardware mapping without reinitializing the PSRAM device or MMU.
    let _psram =
        unsafe { oer_esp32s31_platform_runtime::adopt_psram(peripherals.PSRAM, INTERRUPT_TABLE) };
    exception::install_stack_guard(ptr::addr_of!(_stack_end) as usize);
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    let l1_cache = L1_CACHE_PERFORMANCE.init(
        oer_esp32s31_soc_esp_hal::L1CachePerformanceCounters::new(peripherals.CACHE),
    );

    let interrupts = Interrupts::take().expect("the image takes its interrupt tokens once");
    #[cfg(feature = "ieee802154-radio")]
    assert!(
        oer_esp32s31_ieee802154_system::install_interrupt_route(
            oer_esp32s31_ieee802154_system::Ieee802154InterruptSource::new(interrupts.ieee802154),
        )
        .is_ok(),
        "the image installs its IEEE 802.15.4 route once"
    );
    #[cfg(feature = "ieee802154-route-probe")]
    assert!(
        oer_esp32s31_ieee802154_esp_hal::install(
            oer_esp32s31_ieee802154_esp_hal::EspHalIeee802154Source::new(
                interrupts.ieee802154_probe,
            ),
        )
        .is_ok(),
        "the image installs its IEEE 802.15.4 route once"
    );
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    oer_esp32s31_executor_embassy::init(OneShotTimer::new(timer_group.timer0), interrupts.alarm);
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    {
        let systimer = esp_hal::timer::systimer::SystemTimer::new(peripherals.SYSTIMER);
        hang_watchdog::start(systimer.alarm0, interrupts.hang_check);
        #[cfg(feature = "pc-profile")]
        pc_profile::init(systimer.alarm1, interrupts.profile_sample);
        source_gate::install(OneShotTimer::new(systimer.alarm2), interrupts.source_gate);
    }
    #[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
    let watchdog_service = watchdog::init(peripherals.TIMG1);

    #[cfg(feature = "open-radio-hil")]
    let _app_spawner = {
        leave_app_core_interrupts(AppCoreInterrupts {
            wake: interrupts.app_wake,
            #[cfg(not(feature = "memory-benchmark"))]
            hang_sample: interrupts.hang_sample,
            #[cfg(feature = "pc-profile")]
            profile_sample: interrupts.profile_core1,
        });
        let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
        let app_interrupt = software_interrupt::executor1(peripherals.FROM_CPU_INTR1);
        let guard = cpu_control
            .start_app_core(
                unsafe { &mut (*ptr::addr_of_mut!(APP_CORE_STACK)).0 },
                move || {
                    // The ROM/ESP-HAL second-core entry requires its initial
                    // stack in SRAM. Consume the captured zero-drop token,
                    // abandon that bootstrap call chain, and enter the
                    // non-returning PSRAM task-stack trampoline.
                    let _ = app_interrupt;
                    unsafe { psram_task_stack::enter_cpu1_task_context() }
                },
            )
            .unwrap_or_else(|_| fail(c"OPEN_RADIO_HIL runtime=FAIL reason=app-core-start\r\n"));
        // The HIL runtime owns both cores until reset. Dropping this guard
        // would park Core 1 while its Embassy executor still owns tasks.
        core::mem::forget(guard);
        system::ipc_call::install(esp_hal::interrupt::ipc::Ipc::new(peripherals.IPC));

        loop {
            let pointer = APP_SEND_SPAWNER_PTR.load(Ordering::Acquire);
            if let Some(spawner) = unsafe { pointer.as_ref() } {
                break *spawner;
            }
            #[allow(
                clippy::disallowed_methods,
                reason = "CPU0 has not entered its executor; this boot handoff waits for the CPU1 spawner publication"
            )]
            core::hint::spin_loop();
        }
    };

    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupt::executor0(
        peripherals.FROM_CPU_INTR0,
    )));

    // SAFETY: bootstrap intentionally hands MIE over clear, and timer and
    // software wake interrupt ownership is complete at this point.
    unsafe { oer_esp32s31_platform_runtime::enable_interrupts_after_handoff() };

    #[cfg(feature = "bluetooth-radio")]
    bluetooth::start(
        executor,
        interrupts.wake,
        oer_esp32s31_radio_esp_hal::EspHalBluetoothInterruptRoutes::new(
            interrupts.bluetooth_primary,
            interrupts.bluetooth_timer,
            interrupts.bluetooth_nrt,
        ),
        oer_esp32s31_radio_esp_hal::EspHalRadioPlatform::new(
            peripherals.MODEM_SYSCON,
            peripherals.MODEM_LPCON,
            peripherals.HP_SYS_CLKRST,
            peripherals.PMU,
            peripherals.LP_AON_CLK_RST,
            peripherals.LP_PERI,
            peripherals.LP_TSENS,
            peripherals.I2C_ANA_MST,
        ),
        transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb),
        peripherals.RNG,
    );

    #[cfg(feature = "system-panic-reset")]
    system::panic_reset::start(
        executor,
        interrupts.wake,
        transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb),
        peripherals.RNG,
    );

    #[cfg(feature = "system-watchdog")]
    system::console::start(
        executor,
        interrupts.wake,
        transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb),
        peripherals.RNG,
        peripherals.TIMG1,
    );

    #[cfg(feature = "boot-smoke")]
    executor.run(interrupts.wake, move |spawner| {
        let Ok(task) = boot_smoke(boot_smoke_console::BootSmokeConsole::new(
            peripherals.USB_DEVICE,
        )) else {
            fail(c"OPEN_RADIO_HIL runtime=FAIL reason=task-allocation\r\n");
        };
        spawner.spawn(task);
    });

    #[cfg(feature = "open-radio-hil")]
    {
        transport::init_logger();
        use esp_hal::rng::Trng;
        #[cfg(not(feature = "wifi-ble-coex"))]
        let _trng_source = TRNG_SOURCE.init(esp_hal::rng::TrngSource::new(peripherals.RNG));
        #[cfg(feature = "wifi-ble-coex")]
        let bluetooth_entropy =
            BLUETOOTH_ENTROPY.init(oer_esp32s31_bluetooth_system::BluetoothEntropy::new(
                oer_esp32s31_soc_esp_hal::entropy::Entropy::new(peripherals.RNG),
            ));
        let trng = Trng::try_new()
            .unwrap_or_else(|_| fail(c"OPEN_RADIO_HIL runtime=FAIL reason=trng-ownership\r\n"));
        check_entropy_source();
        let boot_id = (u64::from(trng.random()) << 32) | u64::from(trng.random());
        transport::CONSOLE.start(boot_id);
        #[cfg(not(feature = "memory-benchmark"))]
        let radio = product_hil::RadioPlatforms {
            radio: oer_esp32s31_ieee80211_system::EspHalRadioPlatform::new(
                peripherals.MODEM_SYSCON,
                peripherals.MODEM_LPCON,
                peripherals.HP_SYS_CLKRST,
                peripherals.PMU,
                peripherals.LP_AON_CLK_RST,
                peripherals.LP_PERI,
                peripherals.LP_TSENS,
                peripherals.I2C_ANA_MST,
            ),
            wifi: oer_esp32s31_ieee80211_system::EspHalWifiPlatform::new(
                peripherals.WIFI,
                interrupts.wifi_mac,
                interrupts.wifi_power,
            ),
            #[cfg(feature = "wifi-ble-coex")]
            bluetooth_entropy,
            #[cfg(feature = "wifi-ble-coex")]
            bluetooth_interrupts: oer_esp32s31_radio_esp_hal::EspHalBluetoothInterruptRoutes::new(
                interrupts.bluetooth_primary,
                interrupts.bluetooth_timer,
                interrupts.bluetooth_nrt,
            ),
        };
        let usb = transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb);
        executor.run(interrupts.wake, |spawner| {
            let Ok(logger) = console::console_task(usb, boot_id) else {
                fail(c"OPEN_RADIO_HIL runtime=FAIL reason=logger-allocation\r\n");
            };
            spawner.spawn(logger);
            let Ok(protocol) = console::protocol_task() else {
                fail(c"OPEN_RADIO_HIL runtime=FAIL reason=protocol-allocation\r\n");
            };
            spawner.spawn(protocol);
            #[cfg(not(feature = "memory-benchmark"))]
            if let Ok(hold) = trace::hold_limit_task() {
                spawner.spawn(hold);
            }
            #[cfg(feature = "memory-benchmark")]
            spawner.spawn(
                memory_benchmark::task(oer_esp32s31_soc_esp_hal::AxiGdmaMem2MemChannel::new(
                    peripherals.DMA_AXI_CH0,
                    interrupts.dma_input,
                    interrupts.dma_output,
                ))
                .unwrap_or_else(|_| {
                    fail(c"OPEN_RADIO_HIL runtime=FAIL reason=memory-benchmark-allocation\r\n")
                }),
            );
            #[cfg(not(feature = "memory-benchmark"))]
            {
                let Ok(hil) = open_radio_hil_task(
                    spawner,
                    _app_spawner,
                    radio,
                    trng,
                    l1_cache,
                    watchdog_service,
                    #[cfg(feature = "gdma-mem2mem-probe")]
                    oer_esp32s31_soc_esp_hal::AxiGdmaMem2MemChannel::new(
                        peripherals.DMA_AXI_CH0,
                        interrupts.dma_input,
                        interrupts.dma_output,
                    ),
                ) else {
                    fail(c"OPEN_RADIO_HIL runtime=FAIL reason=radio-task-allocation\r\n");
                };
                spawner.spawn(hil);
            }
        })
    }
}

#[cfg(feature = "open-radio-hil")]
fn run_app_core(
    app_interrupt: esp_hal::interrupt::software::SoftwareInterrupt<
        'static,
        { software_interrupt::Line::Executor1 as u8 },
    >,
) -> ! {
    unsafe {
        psram_task_stack::install_current_hart_interrupt_stack();
    }
    paint_app_core_stack();
    let interrupts = critical_section::with(|cs| APP_INTERRUPTS.borrow_ref_mut(cs).take())
        .expect("core 0 leaves core 1 its interrupt tokens");
    #[cfg(not(feature = "memory-benchmark"))]
    hang_watchdog::enable_core1_sampler(interrupts.hang_sample);
    #[cfg(feature = "pc-profile")]
    pc_profile::enable_core1_sampler(interrupts.profile_sample);
    // SAFETY: Core 1 enters directly from ROM rather than through
    // `_runtime_start` with MIE clear; its per-hart vector state and stack
    // ownership are complete, so hand interrupt enable to its executor.
    unsafe { oer_esp32s31_platform_runtime::enable_interrupts_after_handoff() };
    APP_EXECUTOR
        .init(Executor::<1>::new(app_interrupt))
        .run(interrupts.wake, |spawner| {
            spawner.spawn(stack_evidence::cpu1_sampler().expect("CPU1 stack sampler allocation"));
            #[cfg(not(feature = "memory-benchmark"))]
            {
                let Ok(network) = product_hil::secondary_network_task(spawner) else {
                    fail(c"OPEN_RADIO_HIL runtime=FAIL reason=app-network-allocation\r\n");
                };
                spawner.spawn(network);
            }
            let send_spawner = APP_SEND_SPAWNER.init(spawner.make_send());
            APP_SEND_SPAWNER_PTR.store(send_spawner, Ordering::Release);
        })
}

#[cfg(feature = "open-radio-hil")]
#[unsafe(no_mangle)]
extern "C" fn runtime_cpu1_psram_main() -> ! {
    // The original singleton was consumed and forgotten by the bootstrap
    // closure immediately before the non-returning stack switch.
    let app_interrupt = software_interrupt::executor1_after_stack_switch();
    run_app_core(app_interrupt)
}

#[cfg(feature = "boot-smoke")]
#[embassy_executor::task]
async fn boot_smoke(mut console: boot_smoke_console::BootSmokeConsole) {
    console.embassy_started();
    Timer::after(Duration::from_millis(50)).await;
    console.timer_passed();
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the top-level HIL owner graph is moved once into its static Embassy task arena; runtime stack placement and linked-frame audits remain authoritative"
)]
async fn open_radio_hil_task(
    spawner: embassy_executor::Spawner,
    protocol_spawner: SendSpawner,
    radio: product_hil::RadioPlatforms,
    trng: esp_hal::rng::Trng,
    l1_cache: &'static oer_esp32s31_soc_esp_hal::L1CachePerformanceCounters,
    watchdog: &'static oer_esp32s31_soc_esp_hal::watchdog::DeadlineWatchdog,
    #[cfg(feature = "gdma-mem2mem-probe")]
    gdma_channel: oer_esp32s31_soc_esp_hal::AxiGdmaMem2MemChannel<'static>,
) {
    #[cfg(feature = "gdma-mem2mem-probe")]
    gdma_mem2mem_probe::run(gdma_channel).await;
    product_hil::run(spawner, protocol_spawner, radio, trng, l1_cache, watchdog).await;
}

fn validate_runtime_layout() {
    let image_start = symbol(ptr::addr_of!(__runtime_image_start));
    let payload_end = symbol(ptr::addr_of!(__runtime_payload_end));
    let bss_start = symbol(ptr::addr_of!(__runtime_bss_start));
    let bss_end = symbol(ptr::addr_of!(__runtime_bss_end));
    let data_load_start = symbol(ptr::addr_of!(__runtime_data_load_start));
    let data_start = symbol(ptr::addr_of!(__runtime_data_start));
    let data_end = symbol(ptr::addr_of!(__runtime_data_end));
    let runtime_bss_start = symbol(ptr::addr_of!(__runtime_data_bss_start));
    let runtime_bss_end = symbol(ptr::addr_of!(__runtime_data_bss_end));
    let isr_start = symbol(ptr::addr_of!(__runtime_isr_start));
    let isr_end = symbol(ptr::addr_of!(__runtime_isr_end));
    let dma_data_start = symbol(ptr::addr_of!(__runtime_dma_data_start));
    let dma_data_end = symbol(ptr::addr_of!(__runtime_dma_data_end));
    let dma_bss_start = symbol(ptr::addr_of!(__runtime_dma_bss_start));
    let dma_bss_end = symbol(ptr::addr_of!(__runtime_dma_bss_end));
    let stack_bottom = symbol(ptr::addr_of!(_stack_end));
    let stack_top = symbol(ptr::addr_of!(_stack_start));
    let stack = current_stack_pointer();

    let interrupt_stacks_valid = {
        let cpu0_bottom = symbol(ptr::addr_of!(__runtime_cpu0_irq_stack_bottom));
        let cpu0_top = symbol(ptr::addr_of!(__runtime_cpu0_irq_stack_top));
        let cpu1_bottom = symbol(ptr::addr_of!(__runtime_cpu1_irq_stack_bottom));
        let cpu1_top = symbol(ptr::addr_of!(__runtime_cpu1_irq_stack_top));
        range_in_internal_sram(cpu0_bottom, cpu0_top)
            && range_in_internal_sram(cpu1_bottom, cpu1_top)
            && cpu0_top - cpu0_bottom == psram_task_stack::IRQ_STACK_BYTES as u32
            && cpu1_top - cpu1_bottom == psram_task_stack::IRQ_STACK_BYTES as u32
    };
    let task_stack_valid = stack_bottom >= PROFILE_DATA_START
        && stack_top <= PROFILE_DATA_END
        && stack_top - stack_bottom == psram_task_stack::CPU0_TASK_STACK_BYTES as u32;

    let initialized_data = unsafe { ptr::addr_of!(INITIALIZED_DATA).read_volatile() };
    let bss_probe = unsafe { ptr::addr_of!(BSS_PROBE).read_volatile() };
    let isr_probe = unsafe { ptr::addr_of!(ISR_RODATA_PROBE).read_volatile() };
    let critical_probe = unsafe { ptr::addr_of!(CRITICAL_DATA_PROBE).read_volatile() };
    let dma_probe = unsafe { ptr::addr_of!(DMA_DATA_PROBE).read_volatile() };
    let dma_bss_probe = unsafe { ptr::addr_of!(DMA_BSS_PROBE).read_volatile() };

    if image_start != PROFILE_CODE_START
        || payload_end <= image_start
        || payload_end > PROFILE_CODE_END
        || data_load_start < PROFILE_CODE_START
        || data_load_start >= payload_end
        || data_end < data_start
        || runtime_bss_end < runtime_bss_start
        || bss_end < bss_start
        || !(PROFILE_DATA_START..PROFILE_DATA_END).contains(&data_start)
        || !(PROFILE_DATA_START..=PROFILE_DATA_END).contains(&runtime_bss_end)
        || !range_in_internal_sram(isr_start, isr_end)
        || !range_in_internal_sram(dma_data_start, dma_data_end)
        || !range_in_internal_sram(dma_bss_start, dma_bss_end)
        || !interrupt_stacks_valid
        || !task_stack_valid
        || !(stack_bottom..stack_top).contains(&stack)
        || !stack.is_multiple_of(16)
        || initialized_data != DATA_SENTINEL
        || bss_probe != 0
        || isr_probe != 0x4953_5231
        || critical_probe != 0x4352_5431
        || dma_probe != 0x444d_4131
        || dma_bss_probe != 0
    {
        fail(c"OPEN_RADIO_HIL runtime=FAIL reason=layout\r\n");
    }
    print(c"OPEN_RADIO_HIL placement=PASS isr=SRAM dma_probes=SRAM task_stack=PSRAM irq_stack=SRAM\r\n");
}

fn range_in_internal_sram(start: u32, end: u32) -> bool {
    start >= INTERNAL_SRAM_START && end >= start && end <= INTERNAL_SRAM_END
}

fn symbol(address: *const u8) -> u32 {
    address as usize as u32
}

fn current_stack_pointer() -> u32 {
    let value: usize;
    unsafe { asm!("mv {value}, sp", value = out(reg) value, options(nomem, nostack)) };
    value as u32
}

#[cfg(feature = "open-radio-hil")]
fn paint_app_core_stack() {
    let bottom = psram_task_stack::cpu1_task_stack_bottom();
    let paint_start = bottom + STACK_PAINT_BOTTOM_RESERVE_BYTES;
    let paint_end = current_stack_pointer().saturating_sub(STACK_PAINT_MARGIN_BYTES);
    let maximum_end = bottom + APP_CORE_TASK_STACK_BYTES as u32;
    if paint_end <= paint_start || paint_end > maximum_end {
        fail(c"OPEN_RADIO_HIL runtime=FAIL reason=app-stack-layout\r\n");
    }
    let mut address = paint_start;
    while address < paint_end {
        unsafe { (address as usize as *mut u32).write_volatile(STACK_PAINT_WORD) };
        address += 4;
    }
    APP_STACK_PAINT_END.store(paint_end, Ordering::Release);
    exception::install_stack_guard(bottom as usize);
}

#[cfg(feature = "open-radio-hil")]
pub(crate) async fn stack_usage_snapshot() -> oer_hil_protocol::system::StackUsage {
    let (cpu1, cpu1_irq) = stack_evidence::cpu1_snapshot().await;
    oer_hil_protocol::system::StackUsage {
        cpu0: cpu0_stack_usage_snapshot(),
        cpu1,
        cpu0_irq: stack_evidence::current_irq_snapshot(),
        cpu1_irq,
    }
}

#[cfg(feature = "open-radio-hil")]
pub(crate) fn cpu1_stack_usage_snapshot() -> oer_hil_protocol::system::StackWatermark {
    let bottom = psram_task_stack::cpu1_task_stack_bottom();
    measure_stack(
        bottom,
        bottom + STACK_PAINT_BOTTOM_RESERVE_BYTES,
        APP_STACK_PAINT_END.load(Ordering::Acquire),
        bottom + APP_CORE_TASK_STACK_BYTES as u32,
        stack_minimum_free_bytes(1),
    )
}

#[cfg(any(
    feature = "open-radio-hil",
    feature = "bluetooth-gatt",
    feature = "bluetooth-secure-gatt"
))]
pub(crate) fn cpu0_stack_usage_snapshot() -> oer_hil_protocol::system::StackWatermark {
    let cpu0_bottom = symbol(ptr::addr_of!(_stack_end));
    let cpu0_top = symbol(ptr::addr_of!(_stack_start));
    let cpu0_paint_start = cpu0_bottom + STACK_PAINT_BOTTOM_RESERVE_BYTES;
    let cpu0_paint_end = cpu0_top.saturating_sub(STACK_PAINT_MARGIN_BYTES);

    measure_stack(
        cpu0_bottom,
        cpu0_paint_start,
        cpu0_paint_end,
        cpu0_top,
        stack_minimum_free_bytes(0),
    )
}

#[cfg(any(
    feature = "open-radio-hil",
    feature = "bluetooth-gatt",
    feature = "bluetooth-secure-gatt"
))]
fn measure_stack(
    bottom: u32,
    paint_start: u32,
    paint_end: u32,
    top: u32,
    minimum_free_bytes: u32,
) -> oer_hil_protocol::system::StackWatermark {
    if bottom >= paint_start || paint_start >= paint_end || paint_end > top {
        fail(c"OPEN_RADIO_HIL runtime=FAIL reason=stack-paint-layout\r\n");
    }
    let mut lowest_used = paint_end;
    let mut address = paint_start;
    while address < paint_end {
        let word = unsafe { (address as usize as *const u32).read_volatile() };
        if word != STACK_PAINT_WORD {
            lowest_used = address;
            break;
        }
        address += 4;
    }
    oer_hil_protocol::system::StackWatermark {
        capacity_bytes: top - bottom,
        free_bytes: lowest_used - paint_start,
        used_bytes: (top - bottom) - (lowest_used - paint_start),
        minimum_free_bytes,
    }
}

#[cfg(any(
    feature = "open-radio-hil",
    feature = "bluetooth-gatt",
    feature = "bluetooth-secure-gatt"
))]
fn stack_minimum_free_bytes(cpu: u8) -> u32 {
    let value = match cpu {
        0 => option_env!("OPEN_RADIO_CPU0_STACK_MINIMUM_FREE_BYTES"),
        1 => option_env!("OPEN_RADIO_CPU1_STACK_MINIMUM_FREE_BYTES"),
        _ => None,
    }
    .expect("HIL runner must provide each target stack headroom policy");
    value
        .parse::<u32>()
        .expect("stack headroom policy must be an unsigned byte count")
}

/// Halt the image unless the TRNG produces health-tested entropy: Wi-Fi and
/// Thread keys and nonces come from it.
pub(crate) fn check_entropy_source() {
    if oer_esp32s31_soc_esp_hal::entropy::source_status().is_healthy() {
        print(c"OPEN_RADIO_HIL entropy=PASS\r\n");
    } else {
        fail(c"OPEN_RADIO_HIL runtime=FAIL reason=entropy-source\r\n");
    }
}

fn print(message: &'static CStr) {
    unsafe { ets_printf(message.as_ptr()) };
}

fn fail(message: &'static CStr) -> ! {
    print(message);
    halt()
}

fn halt() -> ! {
    loop {
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
