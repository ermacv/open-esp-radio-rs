use core::marker::PhantomData;

use embassy_executor::{Spawner, raw};
use esp_hal::{
    interrupt::{InterruptHandler, Priority, software::SoftwareInterrupt},
    system::Cpu,
};
use portable_atomic::{AtomicBool, AtomicUsize, Ordering};

const SOFTWARE_INTERRUPT_COUNT: usize = 4;
const THREAD_MODE_CONTEXT: usize = 16;
const UNASSIGNED_CORE: usize = usize::MAX;

#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.embassy_executor")]
static ESP32S31_EMBASSY_WORK_PENDING: [AtomicBool; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicBool::new(false) }; SOFTWARE_INTERRUPT_COUNT];
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.embassy_executor")]
static ESP32S31_EMBASSY_EXECUTOR_CORE: [AtomicUsize; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicUsize::new(UNASSIGNED_CORE) }; SOFTWARE_INTERRUPT_COUNT];

/// A scheduler-free, thread-mode Embassy executor.
///
/// The executor polls futures cooperatively and executes `WFI` when no wake-up
/// was requested while polling. One software interrupt is reserved for each
/// executor so a waker running on the other CPU can wake the sleeping core.
pub struct Executor<const SWI: u8> {
    inner: raw::Executor,
    interrupt: SoftwareInterrupt<'static, SWI>,
    not_send: PhantomData<*mut ()>,
}

impl<const SWI: u8> Executor<SWI> {
    /// Creates an executor using the supplied software interrupt for remote
    /// wake-up.
    pub fn new(interrupt: SoftwareInterrupt<'static, SWI>) -> Self {
        assert!(
            (SWI as usize) < SOFTWARE_INTERRUPT_COUNT,
            "invalid software interrupt"
        );

        Self {
            inner: raw::Executor::new((THREAD_MODE_CONTEXT + SWI as usize) as *mut ()),
            interrupt,
            not_send: PhantomData,
        }
    }

    /// Runs the executor forever on the current CPU.
    pub fn run(&'static mut self, init: impl FnOnce(Spawner)) -> ! {
        let current_core = Cpu::current() as usize;
        ESP32S31_EMBASSY_EXECUTOR_CORE[SWI as usize]
            .compare_exchange(
                UNASSIGNED_CORE,
                current_core,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .unwrap_or_else(|_| panic!("software interrupt {SWI} is already used by an executor"));

        self.interrupt.reset();
        self.interrupt.set_interrupt_handler(InterruptHandler::new(
            wake_handler::<SWI>,
            Priority::Priority1,
        ));

        init(self.inner.spawner());

        loop {
            // Any wake-up racing with poll sets this back to true. Clearing it
            // before poll ensures we never sleep after such a wake-up.
            ESP32S31_EMBASSY_WORK_PENDING[SWI as usize].store(false, Ordering::Release);
            if SWI == 0 {
                // The hardware ISR only acknowledges TIMG and records the
                // event. Walking Embassy's timer queue here keeps arbitrary
                // RawWaker vtable code in thread mode, so a PSRAM-resident
                // task waker is never called from an interrupt.
                crate::time_driver::dispatch_pending();
            }
            unsafe { self.inner.poll() };
            wait_for_work::<SWI>();
        }
    }
}

#[esp_hal::ram]
extern "C" fn wake_handler<const SWI: u8>() {
    unsafe { SoftwareInterrupt::<SWI>::steal() }.reset();
}

#[inline(always)]
pub(crate) fn mark_work<const SWI: u8>() {
    ESP32S31_EMBASSY_WORK_PENDING[SWI as usize].store(true, Ordering::Release);
}

#[inline(always)]
fn pend<const SWI: u8>() {
    mark_work::<SWI>();

    let target_core = ESP32S31_EMBASSY_EXECUTOR_CORE[SWI as usize].load(Ordering::Acquire);
    if target_core != UNASSIGNED_CORE && target_core != Cpu::current() as usize {
        unsafe { SoftwareInterrupt::<SWI>::steal() }.raise();
    }
}

fn wait_for_work<const SWI: u8>() {
    // Interrupts are disabled between the flag check and WFI. If a remote
    // wake-up races with this section, its software interrupt remains pending
    // and immediately wakes WFI. A local interrupt is itself sufficient to
    // wake the core and also sets WORK_PENDING through the task waker.
    riscv::interrupt::free(|| {
        if !ESP32S31_EMBASSY_WORK_PENDING[SWI as usize].load(Ordering::Acquire) {
            esp_hal::interrupt::wait_for_interrupt();
        }
    });
}

// A timer or peripheral ISR can wake an Embassy task, which reaches the
// executor through this callback before interrupt return. Keep the complete
// dispatch entry in internal SRAM for PSRAM-code and Flash-write profiles.
#[esp_hal::ram]
#[unsafe(export_name = "__pender")]
fn embassy_pender(context: *mut ()) {
    match context as usize {
        16 => pend::<0>(),
        17 => pend::<1>(),
        18 => pend::<2>(),
        19 => pend::<3>(),
        _ => unreachable!("invalid Embassy executor context"),
    }
}
