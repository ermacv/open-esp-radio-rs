#![no_main]
#![no_std]
// Embassy moves each generated task future once into its static task arena.
// Those values are intentionally large and do not live on a CPU stack. Owned
// driver crates remain under the 4 KiB `large_assignments` build lint, while
// this final image is guarded by authoritative post-LTO `.stack_sizes` frames
// and runtime high-water qualification.
#![allow(large_assignments)]

#[cfg(not(any(feature = "boot-smoke", feature = "open-radio-hil")))]
compile_error!("select a HIL scenario feature: boot-smoke or open-radio-hil");
#[cfg(all(feature = "boot-smoke", feature = "open-radio-hil"))]
compile_error!("boot-smoke and open-radio-hil are mutually exclusive scenarios");
#[cfg(all(feature = "code-flash", feature = "code-psram"))]
compile_error!("code-flash and code-psram are mutually exclusive");
#[cfg(not(any(feature = "code-flash", feature = "code-psram")))]
compile_error!("select code-flash or code-psram");
#[cfg(all(feature = "profile-psram-data", feature = "profile-sram-data"))]
compile_error!("profile-psram-data and profile-sram-data are mutually exclusive");
#[cfg(not(any(feature = "profile-psram-data", feature = "profile-sram-data")))]
compile_error!("select profile-psram-data or profile-sram-data");
#[cfg(all(
    feature = "psram-task-stack",
    not(all(feature = "code-psram", feature = "profile-psram-data"))
))]
compile_error!("psram-task-stack requires code-psram and profile-psram-data");

#[cfg(feature = "open-radio-hil")]
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use core::{
    arch::{asm, global_asm},
    ffi::CStr,
    ptr,
};

#[cfg(feature = "open-radio-hil")]
use embassy_executor::SendSpawner;
#[cfg(feature = "boot-smoke")]
use embassy_time::{Duration, Timer};
#[cfg(feature = "open-radio-hil")]
use esp_hal::system::{CpuControl, Stack};
use esp_hal::{
    interrupt::software::{SoftwareInterrupt, SoftwareInterruptControl},
    timer::{OneShotTimer, timg::TimerGroup},
};
use open_esp_radio_esp32s31_embassy_runtime::Executor;
use static_cell::StaticCell;

#[cfg(feature = "open-radio-hil")]
mod console;
#[cfg(feature = "open-radio-hil")]
mod phy_calibration_artifact;
#[cfg(feature = "open-radio-hil")]
mod product_hil;
#[cfg(feature = "psram-task-stack")]
mod psram_task_stack;

const DATA_SENTINEL: u32 = 0x5353_31d2;
const INTERNAL_SRAM_START: u32 = 0x2f00_0000;
const INTERNAL_SRAM_END: u32 = 0x2f07_afc0;
#[cfg(not(feature = "psram-task-stack"))]
const INTERNAL_STACK_END: u32 = INTERNAL_SRAM_END;
#[cfg(feature = "open-radio-hil")]
const STACK_PAINT_WORD: u32 = 0xa55a_a55a;
#[cfg(feature = "open-radio-hil")]
const STACK_PAINT_MARGIN_BYTES: u32 = 256;
#[cfg(feature = "open-radio-hil")]
const STACK_PAINT_BOTTOM_RESERVE_BYTES: u32 = 256;
#[cfg(feature = "open-radio-hil")]
// CPU1 runs the Embassy network executor in split images. Its nested async call
// graph needs more than 10 KiB under sustained A-MPDU traffic. The single-core
// task-poll image keeps only the idle control executor on CPU1, reclaiming SRAM
// without moving any radio/network work away from CPU0. Both variants retain
// the same independently checked 4-KiB runtime reserve.
#[cfg(not(feature = "single-core-diagnostic"))]
pub(crate) const APP_CORE_TASK_STACK_BYTES: usize = 16 * 1024;
#[cfg(feature = "single-core-diagnostic")]
pub(crate) const APP_CORE_TASK_STACK_BYTES: usize = 8 * 1024;
#[cfg(feature = "psram-task-stack")]
const APP_CORE_BOOTSTRAP_STACK_BYTES: usize = 8 * 1024;
#[cfg(not(feature = "psram-task-stack"))]
const APP_CORE_BOOTSTRAP_STACK_BYTES: usize = APP_CORE_TASK_STACK_BYTES;

#[cfg(feature = "code-flash")]
const PROFILE_CODE_START: u32 = 0x4000_0140;
#[cfg(feature = "code-flash")]
const PROFILE_CODE_END: u32 = 0x4400_0000;
#[cfg(feature = "code-psram")]
const PROFILE_CODE_START: u32 = 0x5001_0000;
#[cfg(feature = "code-psram")]
const PROFILE_CODE_END: u32 = 0x5100_0000;

#[cfg(feature = "profile-psram-data")]
const PROFILE_DATA_START: u32 = 0x5000_0000;
#[cfg(feature = "profile-psram-data")]
const PROFILE_DATA_END: u32 = 0x5100_0000;
#[cfg(feature = "profile-sram-data")]
const PROFILE_DATA_START: u32 = INTERNAL_SRAM_START;
#[cfg(feature = "profile-sram-data")]
const PROFILE_DATA_END: u32 = INTERNAL_SRAM_END;

#[cfg(all(feature = "code-flash", feature = "profile-psram-data"))]
const PROFILE_NAME: &core::ffi::CStr = c"flash-code-psram-data";
#[cfg(all(
    feature = "code-psram",
    feature = "profile-psram-data",
    not(feature = "psram-task-stack")
))]
const PROFILE_NAME: &core::ffi::CStr = c"psram-code-psram-data";
#[cfg(all(
    feature = "code-psram",
    feature = "profile-psram-data",
    feature = "psram-task-stack"
))]
const PROFILE_NAME: &core::ffi::CStr = c"psram-code-psram-data-psram-stack";
#[cfg(all(feature = "code-psram", feature = "profile-sram-data"))]
const PROFILE_NAME: &core::ffi::CStr = c"psram-code-sram-data";

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
#[cfg(feature = "open-radio-hil")]
static APP_EXECUTOR: StaticCell<Executor<1>> = StaticCell::new();
// The hardware entropy source is a process-lifetime owner. Keeping it in a
// named static prevents task cancellation or panic cleanup from trying to
// disable the source while a nested radio future still owns `Trng`.
#[cfg(feature = "open-radio-hil")]
static TRNG_SOURCE: StaticCell<esp_hal::rng::TrngSource<'static>> = StaticCell::new();
#[cfg(feature = "open-radio-hil")]
static APP_SEND_SPAWNER: StaticCell<SendSpawner> = StaticCell::new();
#[cfg(feature = "open-radio-hil")]
static APP_SEND_SPAWNER_PTR: AtomicPtr<SendSpawner> = AtomicPtr::new(ptr::null_mut());
#[cfg(feature = "open-radio-hil")]
static APP_STACK_PAINT_END: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "open-radio-hil")]
#[unsafe(link_section = ".critical.bss.open_radio_app_core_bootstrap_stack")]
static mut APP_CORE_STACK: Stack<APP_CORE_BOOTSTRAP_STACK_BYTES> = Stack::new();
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
#[used]
#[unsafe(link_section = ".dma.bss.profile_probe")]
static mut DMA_BSS_PROBE: u32 = 0;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.stack_guard")]
static mut __stack_chk_guard: u32 = 0xDEED_BAAD;

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
    #[cfg(feature = "psram-task-stack")]
    static __runtime_cpu0_irq_stack_bottom: u8;
    #[cfg(feature = "psram-task-stack")]
    static __runtime_cpu0_irq_stack_top: u8;
    #[cfg(feature = "psram-task-stack")]
    static __runtime_cpu1_irq_stack_bottom: u8;
    #[cfg(feature = "psram-task-stack")]
    static __runtime_cpu1_irq_stack_top: u8;
    static _stack_end: u8;
    static _stack_start: u8;
}

// Stage two does not return through the bootloader reset entry. It owns data,
// BSS, the SRAM interrupt closure and vector registers, and initializes each
// one before entering Rust or enabling interrupts.
global_asm!(
    r#"
    .section .text._start, "ax", @progbits
    .balign 4
    .global _runtime_start
    .type _runtime_start, @function
_runtime_start:
    .option push
    .option norelax
    la gp, __global_pointer$
    la a0, __runtime_data_load_start
    la a1, __runtime_data_start
    la a2, __runtime_data_end
1:
    beq a1, a2, 2f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 1b
2:
    la a0, __runtime_data_bss_start
    la a1, __runtime_data_bss_end
3:
    beq a0, a1, 4f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 3b
4:
    la a0, __runtime_isr_load_start
    la a1, __runtime_isr_start
    la a2, __runtime_isr_end
5:
    beq a1, a2, 6f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 5b
6:
    la a0, __runtime_critical_data_load_start
    la a1, __runtime_critical_data_start
    la a2, __runtime_critical_data_end
7:
    beq a1, a2, 8f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 7b
8:
    la a0, __runtime_critical_bss_start
    la a1, __runtime_critical_bss_end
9:
    beq a0, a1, 10f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 9b
10:
    la a0, __runtime_dma_data_load_start
    la a1, __runtime_dma_data_start
    la a2, __runtime_dma_data_end
11:
    beq a1, a2, 12f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 11b
12:
    la a0, __runtime_dma_bss_start
    la a1, __runtime_dma_bss_end
13:
    beq a0, a1, 14f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 13b
14:
    la a0, __runtime_hot_text_load_start
    la a1, __runtime_hot_text_start
    la a2, __runtime_hot_text_end
15:
    beq a1, a2, 16f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 15b
16:
    call _runtime_stack_bootstrap
    fence.i
    la t0, _vector_table
    ori t0, t0, 3
    csrw mtvec, t0
    la t0, _runtime_mtvt_table
    csrw 0x307, t0
    .option pop
    li t0, 0x6000
    csrrs zero, mstatus, t0
    fscsr zero
    tail runtime_main
    .size _runtime_start, . - _runtime_start

    # Control profile: paint the inherited SRAM stack. The PSRAM stack module
    # supplies a strong `_runtime_stack_bootstrap` implementation.
    .balign 4
    .global _runtime_default_stack_bootstrap
    .type _runtime_default_stack_bootstrap, @function
_runtime_default_stack_bootstrap:
    la t0, _stack_end
    addi t0, t0, 256
    mv t1, sp
    addi t1, t1, -256
    li t2, 0xa55aa55a
17:
    bgeu t0, t1, 18f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 17b
18:
    ret
    .size _runtime_default_stack_bootstrap, . - _runtime_default_stack_bootstrap
"#
);

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(feature = "open-radio-hil")]
    {
        let (stage, action) = product_hil::diagnostic_snapshot();
        console::emergency_log(format_args!(
            "OPEN_RADIO_HIL panic stage={stage} action={action} info={info}"
        ));
        let (mcause, mepc, mtval): (usize, usize, usize);
        unsafe {
            asm!("csrr {0}, mcause", out(reg) mcause);
            asm!("csrr {0}, mepc", out(reg) mepc);
            asm!("csrr {0}, mtval", out(reg) mtval);
        }
        console::panic_report(mcause, mepc, mtval);
    }
    #[cfg(feature = "boot-smoke")]
    let _ = info;
    print(c"OPEN_RADIO_HIL runtime=PANIC\r\n");
    halt()
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    unsafe { ets_install_usb_printf() };
    print(c"OPEN_RADIO_HIL runtime=START profile=");
    print(PROFILE_NAME);
    print(c"\r\n");

    validate_runtime_layout();

    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));
    unsafe { esp_hal::interrupt::reinitialize_vectoring_after_handoff() };
    #[cfg(feature = "psram-task-stack")]
    unsafe {
        psram_task_stack::install_current_hart_interrupt_stack();
    }

    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    open_esp_radio_esp32s31_embassy_runtime::init(OneShotTimer::new(timer_group.timer0));

    #[cfg(feature = "open-radio-hil")]
    let app_spawner = {
        let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
        let app_interrupt = software_interrupts.software_interrupt1;
        let guard = cpu_control
            .start_app_core(
                unsafe { &mut *ptr::addr_of_mut!(APP_CORE_STACK) },
                move || {
                    #[cfg(feature = "psram-task-stack")]
                    unsafe {
                        // The ROM/ESP-HAL second-core entry requires its initial
                        // stack in SRAM. Consume the captured token, abandon
                        // that bootstrap call chain, and enter the non-returning
                        // PSRAM task-stack trampoline.
                        core::mem::forget(app_interrupt);
                        psram_task_stack::enter_cpu1_task_context();
                    }
                    #[cfg(not(feature = "psram-task-stack"))]
                    run_app_core(app_interrupt)
                },
            )
            .unwrap_or_else(|_| fail(c"OPEN_RADIO_HIL runtime=FAIL reason=app-core-start\r\n"));
        // The HIL runtime owns both cores until reset. Dropping this guard
        // would park Core 1 while its Embassy executor still owns tasks.
        core::mem::forget(guard);

        loop {
            let pointer = APP_SEND_SPAWNER_PTR.load(Ordering::Acquire);
            if let Some(spawner) = unsafe { pointer.as_ref() } {
                break *spawner;
            }
            core::hint::spin_loop();
        }
    };

    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupts.software_interrupt0));

    // Bootstrap intentionally hands MIE over clear. Timer and software wake
    // interrupt ownership is complete at this point.
    unsafe { asm!("csrsi mstatus, 8", options(nomem, nostack)) };

    #[cfg(feature = "boot-smoke")]
    executor.run(|spawner| {
        let Ok(task) = boot_smoke() else {
            fail(c"OPEN_RADIO_HIL runtime=FAIL reason=task-allocation\r\n");
        };
        spawner.spawn(task);
    });

    #[cfg(feature = "open-radio-hil")]
    {
        console::init_logger();
        use esp_hal::rng::{Trng, TrngSource};
        let trng_source = TrngSource::new(peripherals.RNG);
        let trng = Trng::try_new()
            .unwrap_or_else(|_| fail(c"OPEN_RADIO_HIL runtime=FAIL reason=trng-ownership\r\n"));
        let _trng_source = TRNG_SOURCE.init(trng_source);
        let boot_id = (u64::from(trng.random()) << 32) | u64::from(trng.random());
        console::init_protocol(boot_id);
        let radio = open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral::new(
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
        let usb = peripherals.USB_DEVICE;
        executor.run(|spawner| {
            let Ok(logger) = console::logger_task(usb) else {
                fail(c"OPEN_RADIO_HIL runtime=FAIL reason=logger-allocation\r\n");
            };
            spawner.spawn(logger);
            let Ok(protocol) = console::protocol_task(product_hil::hil_capabilities()) else {
                fail(c"OPEN_RADIO_HIL runtime=FAIL reason=protocol-allocation\r\n");
            };
            spawner.spawn(protocol);
            let Ok(hil) = open_radio_hil_task(spawner, app_spawner, radio, trng) else {
                fail(c"OPEN_RADIO_HIL runtime=FAIL reason=radio-task-allocation\r\n");
            };
            spawner.spawn(hil);
        })
    }
}

#[cfg(feature = "open-radio-hil")]
fn run_app_core(app_interrupt: SoftwareInterrupt<'static, 1>) -> ! {
    #[cfg(feature = "psram-task-stack")]
    unsafe {
        psram_task_stack::install_current_hart_interrupt_stack();
    }
    paint_app_core_stack();
    // Core 1 enters directly from ROM rather than through `_runtime_start`,
    // so hand global interrupt enable to its executor explicitly after the
    // per-hart vector state and stack ownership are complete.
    unsafe { asm!("csrsi mstatus, 8", options(nomem, nostack)) };
    APP_EXECUTOR
        .init(Executor::<1>::new(app_interrupt))
        .run(|spawner| {
            let Ok(network) = product_hil::secondary_network_task(spawner) else {
                fail(c"OPEN_RADIO_HIL runtime=FAIL reason=app-network-allocation\r\n");
            };
            spawner.spawn(network);
            let send_spawner = APP_SEND_SPAWNER.init(spawner.make_send());
            APP_SEND_SPAWNER_PTR.store(send_spawner, Ordering::Release);
        })
}

#[cfg(all(feature = "open-radio-hil", feature = "psram-task-stack"))]
#[unsafe(no_mangle)]
extern "C" fn runtime_cpu1_psram_main() -> ! {
    // The original singleton was consumed and forgotten by the bootstrap
    // closure immediately before the non-returning stack switch.
    let app_interrupt = unsafe { SoftwareInterrupt::<1>::steal() };
    run_app_core(app_interrupt)
}

#[cfg(feature = "boot-smoke")]
#[embassy_executor::task]
async fn boot_smoke() {
    print(c"OPEN_RADIO_HIL embassy=START\r\n");
    Timer::after(Duration::from_millis(50)).await;
    print(c"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS\r\n");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[cfg(feature = "open-radio-hil")]
#[embassy_executor::task]
async fn open_radio_hil_task(
    spawner: embassy_executor::Spawner,
    protocol_spawner: SendSpawner,
    radio: open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral,
    trng: esp_hal::rng::Trng,
) {
    product_hil::run(spawner, protocol_spawner, radio, trng).await;
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

    #[cfg(feature = "psram-task-stack")]
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
    #[cfg(not(feature = "psram-task-stack"))]
    let interrupt_stacks_valid = true;

    #[cfg(feature = "psram-task-stack")]
    let task_stack_valid = stack_bottom >= PROFILE_DATA_START
        && stack_top <= PROFILE_DATA_END
        && stack_top - stack_bottom == psram_task_stack::CPU0_TASK_STACK_BYTES as u32;
    #[cfg(not(feature = "psram-task-stack"))]
    let task_stack_valid = stack_top == INTERNAL_STACK_END
        && stack_bottom < stack_top
        && stack_top - stack_bottom >= 64 * 1024;

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
    #[cfg(feature = "psram-task-stack")]
    print(c"OPEN_RADIO_HIL placement=PASS isr=SRAM dma_probes=SRAM task_stack=PSRAM irq_stack=SRAM\r\n");
    #[cfg(not(feature = "psram-task-stack"))]
    print(c"OPEN_RADIO_HIL placement=PASS isr=SRAM dma_probes=SRAM task_stack=SRAM\r\n");
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
    #[cfg(feature = "psram-task-stack")]
    let bottom = psram_task_stack::cpu1_task_stack_bottom();
    #[cfg(not(feature = "psram-task-stack"))]
    let bottom = ptr::addr_of_mut!(APP_CORE_STACK) as *mut u8 as usize as u32;
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
}

#[cfg(feature = "open-radio-hil")]
pub(crate) fn stack_usage_snapshot() -> open_esp_radio_hil_protocol::StackUsage {
    let cpu0_bottom = symbol(ptr::addr_of!(_stack_end));
    let cpu0_top = symbol(ptr::addr_of!(_stack_start));
    let cpu0_paint_start = cpu0_bottom + STACK_PAINT_BOTTOM_RESERVE_BYTES;
    let cpu0_paint_end = cpu0_top.saturating_sub(STACK_PAINT_MARGIN_BYTES);

    #[cfg(feature = "psram-task-stack")]
    let cpu1_bottom = psram_task_stack::cpu1_task_stack_bottom();
    #[cfg(not(feature = "psram-task-stack"))]
    let cpu1_bottom = ptr::addr_of!(APP_CORE_STACK) as *const u8 as usize as u32;
    let cpu1_top = cpu1_bottom + APP_CORE_TASK_STACK_BYTES as u32;
    let cpu1_paint_end = APP_STACK_PAINT_END.load(Ordering::Acquire);

    open_esp_radio_hil_protocol::StackUsage {
        cpu0: measure_stack(
            cpu0_bottom,
            cpu0_paint_start,
            cpu0_paint_end,
            cpu0_top,
            stack_minimum_free_bytes(0),
        ),
        cpu1: measure_stack(
            cpu1_bottom,
            cpu1_bottom + STACK_PAINT_BOTTOM_RESERVE_BYTES,
            cpu1_paint_end,
            cpu1_top,
            stack_minimum_free_bytes(1),
        ),
    }
}

#[cfg(feature = "open-radio-hil")]
fn measure_stack(
    bottom: u32,
    paint_start: u32,
    paint_end: u32,
    top: u32,
    minimum_free_bytes: u32,
) -> open_esp_radio_hil_protocol::StackWatermark {
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
    open_esp_radio_hil_protocol::StackWatermark {
        capacity_bytes: top - bottom,
        free_bytes: lowest_used - paint_start,
        used_bytes: (top - bottom) - (lowest_used - paint_start),
        minimum_free_bytes,
    }
}

#[cfg(feature = "open-radio-hil")]
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
