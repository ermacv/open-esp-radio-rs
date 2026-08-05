#![allow(unsafe_code, reason = "ESP-HAL executor and interrupt binding")]

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

/// Scheduler-free thread-mode Embassy executor.
///
/// One software interrupt is reserved per executor so a waker on another CPU
/// can wake the sleeping owner without introducing a scheduler or RTOS.
pub struct Executor<const SWI: u8> {
    inner: raw::Executor,
    interrupt: SoftwareInterrupt<'static, SWI>,
    not_send: PhantomData<*mut ()>,
}

impl<const SWI: u8> Executor<SWI> {
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
            ESP32S31_EMBASSY_WORK_PENDING[SWI as usize].store(false, Ordering::Release);
            if SWI == 0 {
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
    riscv::interrupt::free(|| {
        if !ESP32S31_EMBASSY_WORK_PENDING[SWI as usize].load(Ordering::Acquire) {
            esp_hal::interrupt::wait_for_interrupt();
        }
    });
}

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
