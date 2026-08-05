use core::marker::PhantomData;

use embassy_executor::{Spawner, raw};
use esp_hal::{
    interrupt::{InterruptHandler, Priority, software::SoftwareInterrupt},
    system::Cpu,
};
use esp_sync::NonReentrantMutex;
use portable_atomic::{AtomicBool, AtomicUsize, Ordering};

const SOFTWARE_INTERRUPT_COUNT: usize = 4;
const THREAD_MODE_CONTEXT: usize = 16;
const UNASSIGNED_CORE: usize = usize::MAX;

#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor wake-state section"
)]
#[unsafe(link_section = ".critical.bss.embassy_executor")]
static ESP32S31_EMBASSY_WORK_PENDING: [AtomicBool; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicBool::new(false) }; SOFTWARE_INTERRUPT_COUNT];
#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor core-state section"
)]
#[unsafe(link_section = ".critical.data.embassy_executor")]
static ESP32S31_EMBASSY_EXECUTOR_CORE: [AtomicUsize; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicUsize::new(UNASSIGNED_CORE) }; SOFTWARE_INTERRUPT_COUNT];

enum OwnedSoftwareInterrupt {
    Zero(SoftwareInterrupt<'static, 0>),
    One(SoftwareInterrupt<'static, 1>),
    Two(SoftwareInterrupt<'static, 2>),
    Three(SoftwareInterrupt<'static, 3>),
}

impl OwnedSoftwareInterrupt {
    fn reset(&self) {
        match self {
            Self::Zero(interrupt) => interrupt.reset(),
            Self::One(interrupt) => interrupt.reset(),
            Self::Two(interrupt) => interrupt.reset(),
            Self::Three(interrupt) => interrupt.reset(),
        }
    }

    fn raise(&self) {
        match self {
            Self::Zero(interrupt) => interrupt.raise(),
            Self::One(interrupt) => interrupt.raise(),
            Self::Two(interrupt) => interrupt.raise(),
            Self::Three(interrupt) => interrupt.raise(),
        }
    }

    fn set_interrupt_handler(&mut self, handler: InterruptHandler) {
        match self {
            Self::Zero(interrupt) => interrupt.set_interrupt_handler(handler),
            Self::One(interrupt) => interrupt.set_interrupt_handler(handler),
            Self::Two(interrupt) => interrupt.set_interrupt_handler(handler),
            Self::Three(interrupt) => interrupt.set_interrupt_handler(handler),
        }
    }
}

#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns the runtime software-interrupt token section"
)]
#[unsafe(link_section = ".critical.data.embassy_executor")]
static ESP32S31_EMBASSY_INTERRUPTS: NonReentrantMutex<
    [Option<OwnedSoftwareInterrupt>; SOFTWARE_INTERRUPT_COUNT],
> = NonReentrantMutex::new([const { None }; SOFTWARE_INTERRUPT_COUNT]);

/// Scheduler-free thread-mode Embassy executor.
///
/// One software interrupt is reserved per executor so a waker on another CPU
/// can wake the sleeping owner without introducing a scheduler or RTOS.
pub struct Executor<const SWI: u8> {
    inner: raw::Executor,
    interrupt: Option<OwnedSoftwareInterrupt>,
    not_send: PhantomData<*mut ()>,
}

impl<const SWI: u8> Executor<SWI> {
    fn with_interrupt(interrupt: OwnedSoftwareInterrupt) -> Self {
        Self {
            inner: raw::Executor::new((THREAD_MODE_CONTEXT + SWI as usize) as *mut ()),
            interrupt: Some(interrupt),
            not_send: PhantomData,
        }
    }

    pub fn run(&'static mut self, init: impl FnOnce(Spawner)) -> ! {
        let current_core = Cpu::current() as usize;
        let mut interrupt = self
            .interrupt
            .take()
            .expect("executor software interrupt was already installed");
        interrupt.reset();
        interrupt.set_interrupt_handler(InterruptHandler::new(
            wake_handler::<SWI>,
            Priority::Priority1,
        ));
        ESP32S31_EMBASSY_INTERRUPTS.with(|interrupts| {
            assert!(
                interrupts[SWI as usize].is_none(),
                "software interrupt {SWI} is already installed"
            );
            interrupts[SWI as usize] = Some(interrupt);
        });
        ESP32S31_EMBASSY_EXECUTOR_CORE[SWI as usize]
            .compare_exchange(
                UNASSIGNED_CORE,
                current_core,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .unwrap_or_else(|_| panic!("software interrupt {SWI} is already used by an executor"));
        init(self.inner.spawner());

        loop {
            ESP32S31_EMBASSY_WORK_PENDING[SWI as usize].store(false, Ordering::Release);
            if SWI == 0 {
                crate::time_driver::dispatch_pending();
            }
            #[allow(
                unsafe_code,
                reason = "the static executor owner is polled only by its run loop"
            )]
            unsafe {
                self.inner.poll()
            };
            wait_for_work::<SWI>();
        }
    }
}

macro_rules! impl_executor_constructor {
    ($number:literal, $variant:ident) => {
        impl Executor<$number> {
            pub fn new(interrupt: SoftwareInterrupt<'static, $number>) -> Self {
                Self::with_interrupt(OwnedSoftwareInterrupt::$variant(interrupt))
            }
        }
    };
}

impl_executor_constructor!(0, Zero);
impl_executor_constructor!(1, One);
impl_executor_constructor!(2, Two);
impl_executor_constructor!(3, Three);

#[esp_hal::ram]
extern "C" fn wake_handler<const SWI: u8>() {
    ESP32S31_EMBASSY_INTERRUPTS.with(|interrupts| {
        interrupts[SWI as usize]
            .as_ref()
            .expect("software interrupt fired before executor installation")
            .reset();
    });
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
        ESP32S31_EMBASSY_INTERRUPTS.with(|interrupts| {
            interrupts[SWI as usize]
                .as_ref()
                .expect("executor core assigned before software interrupt installation")
                .raise();
        });
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
#[allow(
    unsafe_code,
    reason = "Embassy requires this unique global pender ABI symbol"
)]
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
