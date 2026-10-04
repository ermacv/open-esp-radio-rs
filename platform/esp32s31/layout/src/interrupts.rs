//! The interrupt contract the image's interrupt-stack gate checks: where the
//! image's interrupt table, vectors and dispatcher are, the levels every hart
//! takes, and the stack those interrupts run on.
//!
//! The trap entries learn the interrupt stack as the lower of `sp` and
//! `mscratch` because every interrupt stack lies in [`SRAM`], below every task
//! stack in [`PSRAM`]; the assertion below keeps that true of the map.

use crate::memory::{IRQ_STACK_BYTES, PSRAM, SRAM};

/// The exported slice of the image's interrupt table.
pub const TABLE_SYMBOL: &str = "__OER_INTERRUPT_TABLE";
/// Bytes of one entry, `Binding<Interrupt, Priority, Cpu>`, whose field
/// offsets the SoC adapter asserts: source `u16` at 0, level `u8` at 2, core
/// `u32` at 4, handler word at 8.
pub const TABLE_ENTRY_BYTES: u32 = 12;
pub const TABLE_SOURCE: (u32, u32) = (0, 2);
pub const TABLE_LEVEL: (u32, u32) = (2, 1);
pub const TABLE_CORE: (u32, u32) = (4, 4);
pub const TABLE_HANDLER: u32 = 8;

/// The source of the hardware-vector entries every hart's MTVT holds.
pub const VECTOR_TABLE_SYMBOL: &str = "_runtime_psram_mtvt_source";
/// The entry of synchronous exceptions.
pub const EXCEPTION_ENTRY_SYMBOL: &str = "_start_trap";
/// esp-hal's per-source handler table, through which the dispatcher calls.
pub const SOURCE_TABLE_SYMBOL: &str = "__EXTERNAL_INTERRUPTS";
/// The harts, by the table's core numbers.
pub const HARTS: [u32; 2] = [0, 1];
/// Levels every hart takes whatever the table holds: esp-hal's IPC line.
pub const ALWAYS_LEVELS: [u32; 1] = [1];
/// The only writer of the IPC callback, whose `handler` argument (`a1`) is
/// every function the IPC dispatch runs.
pub const IPC_POST: &str = "<esp_hal::interrupt::ipc::Ipc>::call_function";
pub const IPC_POST_HANDLER_ARGUMENT: usize = 1;
/// The functions that call the posted IPC callback.
pub const IPC_DISPATCH: [&str; 2] = [
    "esp_hal::interrupt::ipc::implem::dispatch",
    "esp_hal::interrupt::ipc::implem::callback_handler",
];

/// The functions that set an interrupt line's CLIC level or route a source
/// to a line, each with the only functions it may run inside: start-up and
/// the IPC line's install for levels (each line's from esp-hal's constant
/// priority map, the IPC line's the lowest), start-up and the table's routing
/// capability for routes (each source to the line of its table level). A copy
/// anywhere else could give a source a level other than its table entry's.
pub const LEVEL_WRITERS: [(&str, &[&str]); 2] = [
    (
        "esp_hal::interrupt::arch::cpu_int::set_priority_raw",
        &[
            "esp_hal::interrupt::arch::init_vectoring",
            "esp_hal::interrupt::ipc::implem::install_core",
        ],
    ),
    (
        "esp_hal::interrupt::map_raw",
        &[
            "esp_hal::interrupt::setup_interrupts",
            "<esp_hal::interrupt::InterruptRoutes>::enable",
            "<esp_hal::interrupt::InterruptRoutes>::disable",
        ],
    ),
];

/// Aligned bytes at the bottom of each interrupt stack whose stores trap.
pub const IRQ_STACK_GUARD_BYTES: u32 = 1024;
/// The margin the interrupt-stack bound keeps below the usable stack, in
/// percent of the bound.
pub const IRQ_STACK_MARGIN_PERCENT: u32 = 10;
/// The bytes an interrupt stack may use: below them lies the guard.
pub const IRQ_STACK_USABLE_BYTES: u32 = IRQ_STACK_BYTES - IRQ_STACK_GUARD_BYTES;

const _: () = assert!(SRAM.end() <= PSRAM.origin);
