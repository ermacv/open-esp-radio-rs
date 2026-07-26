#![cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]

use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CriticalSectionSnapshot {
    pub interrupt_entries: usize,
    pub active_interrupt_sections: usize,
    pub max_interrupt_nesting: usize,
    pub unmatched_restores: usize,
    pub other_core_stalls: usize,
    pub wrong_hart_entries: usize,
}

/// Allocation-free counters for auditing critical sections reached on real
/// hardware. Interrupt masking always preserves the hardware adapter's
/// semantics. Other-core stalls delegate only during initialization; the
/// strict runtime phase records and rejects them without entering the stall.
pub struct CriticalSectionProbe {
    interrupt_entries: AtomicUsize,
    active_interrupt_sections: AtomicUsize,
    max_interrupt_nesting: AtomicUsize,
    unmatched_restores: AtomicUsize,
    other_core_stalls: AtomicUsize,
    wrong_hart_entries: AtomicUsize,
}

impl CriticalSectionProbe {
    pub const fn new() -> Self {
        Self {
            interrupt_entries: AtomicUsize::new(0),
            active_interrupt_sections: AtomicUsize::new(0),
            max_interrupt_nesting: AtomicUsize::new(0),
            unmatched_restores: AtomicUsize::new(0),
            other_core_stalls: AtomicUsize::new(0),
            wrong_hart_entries: AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn enter_interrupt(&self) {
        self.interrupt_entries.fetch_add(1, Ordering::Relaxed);
        let depth = self
            .active_interrupt_sections
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.max_interrupt_nesting
            .fetch_max(depth, Ordering::Relaxed);
    }

    #[inline(always)]
    fn exit_interrupt(&self) {
        let depth = self.active_interrupt_sections.load(Ordering::Acquire);
        if depth == 0
            || self
                .active_interrupt_sections
                .compare_exchange(depth, depth - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            // This is a diagnostic counter, not synchronization required by
            // the driver. A racing sample is reported and never retried.
            self.unmatched_restores.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    fn enter_other_core_stall(&self) {
        self.other_core_stalls.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    fn enter_wrong_hart(&self) {
        self.wrong_hart_entries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> CriticalSectionSnapshot {
        CriticalSectionSnapshot {
            interrupt_entries: self.interrupt_entries.load(Ordering::Acquire),
            active_interrupt_sections: self.active_interrupt_sections.load(Ordering::Acquire),
            max_interrupt_nesting: self.max_interrupt_nesting.load(Ordering::Acquire),
            unmatched_restores: self.unmatched_restores.load(Ordering::Acquire),
            other_core_stalls: self.other_core_stalls.load(Ordering::Acquire),
            wrong_hart_entries: self.wrong_hart_entries.load(Ordering::Acquire),
        }
    }

    pub fn clear(&self) {
        self.interrupt_entries.store(0, Ordering::Release);
        self.max_interrupt_nesting.store(0, Ordering::Release);
        self.unmatched_restores.store(0, Ordering::Release);
        self.other_core_stalls.store(0, Ordering::Release);
        self.wrong_hart_entries.store(0, Ordering::Release);
    }
}

impl Default for CriticalSectionProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.critical_probe"
)]
static PROBE: CriticalSectionProbe = CriticalSectionProbe::new();

pub fn critical_section_probe() -> &'static CriticalSectionProbe {
    &PROBE
}

#[cfg(target_arch = "riscv32")]
mod target {
    use core::{
        ffi::c_void,
        mem,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use esp_wifi_sys_esp32s31::include::wifi_osi_funcs_t;

    use super::PROBE;

    type IntDisable = unsafe extern "C" fn(*mut c_void) -> u32;
    type IntRestore = unsafe extern "C" fn(*mut c_void, u32);
    type Stall = unsafe extern "C" fn();

    #[link_section = ".critical.bss.wifi_strict.critical_callbacks"]
    static INT_DISABLE: AtomicUsize = AtomicUsize::new(0);
    #[link_section = ".critical.bss.wifi_strict.critical_callbacks"]
    static INT_RESTORE: AtomicUsize = AtomicUsize::new(0);
    #[link_section = ".critical.bss.wifi_strict.critical_callbacks"]
    static STALL_START: AtomicUsize = AtomicUsize::new(0);
    #[link_section = ".critical.bss.wifi_strict.critical_callbacks"]
    static STALL_END: AtomicUsize = AtomicUsize::new(0);
    #[link_section = ".critical.bss.wifi_strict.critical_state"]
    static CALLBACKS_PATCHED: AtomicUsize = AtomicUsize::new(0);
    #[link_section = ".critical.bss.wifi_strict.critical_state"]
    static RUNTIME_CORE_STALL_FORBIDDEN: AtomicUsize = AtomicUsize::new(0);
    const NO_STRICT_HART: usize = usize::MAX;
    #[link_section = ".critical.data.wifi_strict.critical_hart"]
    static STRICT_WIFI_HART: AtomicUsize = AtomicUsize::new(NO_STRICT_HART);

    unsafe extern "C" {
        static mut g_osi_funcs_p: *const wifi_osi_funcs_t;
    }

    /// Wrap hardware critical-section callbacks with counters while preserving
    /// their original implementation.
    ///
    /// Installation must be serialized with Wi-Fi initialization. Calling it
    /// twice on the same table would capture the wrappers themselves and is
    /// therefore unsupported.
    ///
    /// # Safety
    /// The table must not be registered or invoked concurrently with this
    /// update, and its original callbacks must remain valid permanently.
    pub unsafe fn patch_critical_section_probes(table: &mut wifi_osi_funcs_t) {
        if let (Some(disable), Some(restore)) = (table._wifi_int_disable, table._wifi_int_restore) {
            INT_DISABLE.store(disable as usize, Ordering::Release);
            INT_RESTORE.store(restore as usize, Ordering::Release);
            table._wifi_int_disable = Some(wifi_int_disable);
            table._wifi_int_restore = Some(wifi_int_restore);
        }
        if let Some(start) = table._dport_access_stall_other_cpu_start_wrap {
            STALL_START.store(start as usize, Ordering::Release);
            table._dport_access_stall_other_cpu_start_wrap = Some(stall_other_cpu_start);
        }
        if let Some(end) = table._dport_access_stall_other_cpu_end_wrap {
            STALL_END.store(end as usize, Ordering::Release);
            table._dport_access_stall_other_cpu_end_wrap = Some(stall_other_cpu_end);
        }
        CALLBACKS_PATCHED.store(1, Ordering::Release);
    }

    pub(crate) fn critical_callbacks_patched() -> bool {
        if CALLBACKS_PATCHED.load(Ordering::Acquire) == 0 {
            return false;
        }
        let table = unsafe { core::ptr::addr_of!(g_osi_funcs_p).read().as_ref() };
        let Some(table) = table else {
            return false;
        };
        table._wifi_int_disable.is_some_and(|registered| {
            core::ptr::eq(registered as *const (), wifi_int_disable as *const ())
        }) && table._wifi_int_restore.is_some_and(|registered| {
            core::ptr::eq(registered as *const (), wifi_int_restore as *const ())
        }) && table
            ._dport_access_stall_other_cpu_start_wrap
            .is_some_and(|registered| {
                core::ptr::eq(registered as *const (), stall_other_cpu_start as *const ())
            })
            && table
                ._dport_access_stall_other_cpu_end_wrap
                .is_some_and(|registered| {
                    core::ptr::eq(registered as *const (), stall_other_cpu_end as *const ())
                })
    }

    pub(crate) fn forbid_runtime_core_stalls(expected_hart: usize) -> bool {
        if !critical_callbacks_patched() {
            return false;
        }
        if current_hart() != expected_hart {
            return false;
        }
        STRICT_WIFI_HART.store(expected_hart, Ordering::Release);
        RUNTIME_CORE_STALL_FORBIDDEN.store(1, Ordering::Release);
        true
    }

    /// Restore initialization-time core-stall behavior after every strict
    /// callback and the radio executor have stopped.
    ///
    /// # Safety
    /// No strict runtime callback may execute concurrently or afterwards.
    pub unsafe fn allow_core_stalls_for_wifi_teardown() {
        RUNTIME_CORE_STALL_FORBIDDEN.store(0, Ordering::Release);
        STRICT_WIFI_HART.store(NO_STRICT_HART, Ordering::Release);
    }

    #[inline(always)]
    fn runtime_core_stall_forbidden() -> bool {
        RUNTIME_CORE_STALL_FORBIDDEN.load(Ordering::Acquire) != 0
    }

    #[inline(always)]
    pub(crate) fn strict_wifi_hart_armed() -> bool {
        STRICT_WIFI_HART.load(Ordering::Acquire) != NO_STRICT_HART
    }

    #[inline(always)]
    pub(crate) fn on_strict_wifi_hart() -> bool {
        let expected = STRICT_WIFI_HART.load(Ordering::Acquire);
        expected != NO_STRICT_HART && current_hart() == expected
    }

    #[inline(always)]
    pub(crate) fn current_hart() -> usize {
        let hart: usize;
        unsafe {
            core::arch::asm!(
                "csrr {hart}, mhartid",
                hart = out(reg) hart,
                options(nomem, nostack, preserves_flags)
            );
        }
        hart
    }

    #[inline(always)]
    unsafe fn disable_local_interrupts() -> u32 {
        let previous: usize;
        core::arch::asm!(
            "csrrci {previous}, mstatus, 8",
            previous = out(reg) previous,
            options(nomem, nostack, preserves_flags)
        );
        previous as u32
    }

    #[inline(always)]
    unsafe fn restore_local_interrupts(previous: u32) {
        if previous & 8 != 0 {
            core::arch::asm!("csrsi mstatus, 8", options(nomem, nostack, preserves_flags));
        }
    }

    #[link_section = ".rwtext.wifi_strict.critical"]
    pub(crate) unsafe fn strict_wifi_int_disable() -> u32 {
        if current_hart() != STRICT_WIFI_HART.load(Ordering::Acquire) {
            PROBE.enter_wrong_hart();
        }
        let state = disable_local_interrupts();
        PROBE.enter_interrupt();
        state
    }

    #[link_section = ".rwtext.wifi_strict.critical"]
    pub(crate) unsafe fn strict_wifi_int_restore(state: u32) {
        restore_local_interrupts(state);
        PROBE.exit_interrupt();
    }

    /// Mask only the current hart while a cold-to-strict callback slot and its
    /// backing ownership are transferred together.
    ///
    /// Unlike `strict_wifi_int_disable`, this is valid immediately before the
    /// strict hart marker is published and deliberately does not claim a
    /// runtime critical-section sample.
    #[link_section = ".rwtext.wifi_strict.critical"]
    pub(crate) unsafe fn handoff_local_interrupts_disable() -> u32 {
        disable_local_interrupts()
    }

    #[link_section = ".rwtext.wifi_strict.critical"]
    pub(crate) unsafe fn handoff_local_interrupts_restore(state: u32) {
        restore_local_interrupts(state);
    }

    #[link_section = ".rwtext.wifi_strict.critical"]
    unsafe extern "C" fn wifi_int_disable(mux: *mut c_void) -> u32 {
        if strict_wifi_hart_armed() {
            return strict_wifi_int_disable();
        }
        let original = mem::transmute::<usize, IntDisable>(INT_DISABLE.load(Ordering::Acquire));
        let state = original(mux);
        PROBE.enter_interrupt();
        state
    }

    #[link_section = ".rwtext.wifi_strict.critical"]
    unsafe extern "C" fn wifi_int_restore(mux: *mut c_void, state: u32) {
        if strict_wifi_hart_armed() {
            strict_wifi_int_restore(state);
            return;
        }
        let original = mem::transmute::<usize, IntRestore>(INT_RESTORE.load(Ordering::Acquire));
        original(mux, state);
        PROBE.exit_interrupt();
    }

    #[link_section = ".rwtext.wifi_strict.critical"]
    unsafe extern "C" fn stall_other_cpu_start() {
        PROBE.enter_other_core_stall();
        if runtime_core_stall_forbidden() {
            return;
        }
        let original = mem::transmute::<usize, Stall>(STALL_START.load(Ordering::Acquire));
        original();
    }

    #[link_section = ".rwtext.wifi_strict.critical"]
    unsafe extern "C" fn stall_other_cpu_end() {
        if runtime_core_stall_forbidden() {
            return;
        }
        let original = mem::transmute::<usize, Stall>(STALL_END.load(Ordering::Acquire));
        original();
    }
}

#[cfg(target_arch = "riscv32")]
pub use target::{allow_core_stalls_for_wifi_teardown, patch_critical_section_probes};

#[cfg(target_arch = "riscv32")]
pub(crate) use target::{critical_callbacks_patched, forbid_runtime_core_stalls};
#[cfg(target_arch = "riscv32")]
pub(crate) use target::{
    current_hart, handoff_local_interrupts_disable, handoff_local_interrupts_restore,
    on_strict_wifi_hart, strict_wifi_hart_armed, strict_wifi_int_disable, strict_wifi_int_restore,
};

#[cfg(test)]
mod tests {
    use super::CriticalSectionProbe;

    #[test]
    fn probe_tracks_nesting_and_unmatched_restore() {
        let probe = CriticalSectionProbe::new();
        probe.enter_interrupt();
        probe.enter_interrupt();
        probe.exit_interrupt();
        probe.exit_interrupt();
        probe.exit_interrupt();
        probe.enter_other_core_stall();
        assert_eq!(probe.snapshot().interrupt_entries, 2);
        assert_eq!(probe.snapshot().max_interrupt_nesting, 2);
        assert_eq!(probe.snapshot().active_interrupt_sections, 0);
        assert_eq!(probe.snapshot().unmatched_restores, 1);
        assert_eq!(probe.snapshot().other_core_stalls, 1);
        assert_eq!(probe.snapshot().wrong_hart_entries, 0);
    }
}
