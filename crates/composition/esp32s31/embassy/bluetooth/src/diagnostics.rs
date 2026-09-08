//! Boot-lifetime observations of Controller execution boundaries.
//!
//! Counters describe software publication, not peer reception or link readiness.
//! This observer never acknowledges hardware or grants protocol authority.
//! Snapshots are coherent across tasks; counters saturate explicitly and text
//! truncation preserves UTF-8. The first terminal cause is retained until reboot.

use core::fmt::{self, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothExecutionEvent {
    ConnectableAdvertisingRun,
    PeripheralRun,
    Retry,
    Terminal,
}

#[derive(Clone, Copy)]
pub struct BluetoothExecutionSnapshot {
    pub advertising_runs: u32,
    pub peripheral_runs: u32,
    pub retries: u32,
    pub terminal: bool,
    pub saturated: bool,
    pub detail_truncated: bool,
    detail: [u8; 128],
    detail_len: usize,
}

impl BluetoothExecutionSnapshot {
    const fn new() -> Self {
        Self {
            advertising_runs: 0,
            peripheral_runs: 0,
            retries: 0,
            terminal: false,
            saturated: false,
            detail_truncated: false,
            detail: [0; 128],
            detail_len: 0,
        }
    }

    pub fn detail(&self) -> &str {
        core::str::from_utf8(&self.detail[..self.detail_len])
            .expect("diagnostic writer preserves UTF-8")
    }

    fn record(&mut self, event: BluetoothExecutionEvent, detail: fmt::Arguments<'_>) {
        if self.terminal {
            return;
        }
        let counter = match event {
            BluetoothExecutionEvent::ConnectableAdvertisingRun => Some(&mut self.advertising_runs),
            BluetoothExecutionEvent::PeripheralRun => Some(&mut self.peripheral_runs),
            BluetoothExecutionEvent::Retry => Some(&mut self.retries),
            BluetoothExecutionEvent::Terminal => {
                self.terminal = true;
                None
            }
        };
        if let Some(counter) = counter {
            match counter.checked_add(1) {
                Some(value) => *counter = value,
                None => self.saturated = true,
            }
        }
        self.detail_len = 0;
        self.detail_truncated = false;
        let _ = self.write_fmt(detail);
    }
}

impl Write for BluetoothExecutionSnapshot {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut len = text.len().min(self.detail.len() - self.detail_len);
        while !text.is_char_boundary(len) {
            len -= 1;
        }
        self.detail[self.detail_len..self.detail_len + len]
            .copy_from_slice(&text.as_bytes()[..len]);
        self.detail_len += len;
        if len < text.len() {
            self.detail_truncated = true;
            return Err(fmt::Error);
        }
        Ok(())
    }
}

#[cfg(target_arch = "riscv32")]
static STATE: embassy_sync::blocking_mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    core::cell::RefCell<BluetoothExecutionSnapshot>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(
    BluetoothExecutionSnapshot::new(),
));

#[cfg(target_arch = "riscv32")]
pub(crate) fn record(event: BluetoothExecutionEvent, detail: fmt::Arguments<'_>) {
    STATE.lock(|state| state.borrow_mut().record(event, detail));
}

#[cfg(target_arch = "riscv32")]
pub fn snapshot() -> BluetoothExecutionSnapshot {
    STATE.lock(|state| *state.borrow())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_retains_first_reason_and_counters() {
        let mut state = BluetoothExecutionSnapshot::new();
        state.record(
            BluetoothExecutionEvent::ConnectableAdvertisingRun,
            format_args!("advertising"),
        );
        state.record(
            BluetoothExecutionEvent::PeripheralRun,
            format_args!("first"),
        );
        state.record(
            BluetoothExecutionEvent::PeripheralRun,
            format_args!("successor"),
        );
        state.record(BluetoothExecutionEvent::Retry, format_args!("busy"));
        state.record(
            BluetoothExecutionEvent::Terminal,
            format_args!("timing failure"),
        );
        state.record(
            BluetoothExecutionEvent::Terminal,
            format_args!("generic quarantine"),
        );
        assert_eq!(
            (state.advertising_runs, state.peripheral_runs, state.retries),
            (1, 2, 1)
        );
        assert!(state.terminal);
        assert_eq!(state.detail(), "timing failure");
    }
    #[test]
    fn truncation_and_counter_saturation_are_explicit() {
        let mut state = BluetoothExecutionSnapshot::new();
        state.peripheral_runs = u32::MAX;
        state.record(
            BluetoothExecutionEvent::PeripheralRun,
            format_args!("{:я<129}", ""),
        );
        assert_eq!(state.peripheral_runs, u32::MAX);
        assert!(state.saturated && state.detail_truncated);
        assert!(state.detail().chars().all(|c| c == 'я'));
    }
}
