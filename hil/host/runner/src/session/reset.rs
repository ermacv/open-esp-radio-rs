//! USB-Serial/JTAG reset boundary: drain the old boot while reset is asserted.
use std::{thread, time::Duration};

#[derive(Clone, Copy)]
enum Step {
    Dtr(bool),
    Rts(bool),
    ClearInput,
}

pub(super) fn reset_usb_serial_jtag(
    serial: &mut dyn serialport::SerialPort,
) -> serialport::Result<()> {
    sequence(
        |step| match step {
            Step::Dtr(level) => serial.write_data_terminal_ready(level),
            Step::Rts(level) => serial.write_request_to_send(level),
            Step::ClearInput => serial.clear(serialport::ClearBuffer::Input),
        },
        || thread::sleep(Duration::from_millis(100)),
    )
}

fn sequence<E>(
    mut apply: impl FnMut(Step) -> Result<(), E>,
    mut settle: impl FnMut(),
) -> Result<(), E> {
    // espflash's USB-Serial/JTAG reset sequence, with old-boot input drained
    // after the chip has stopped transmitting and before the new boot can speak.
    settle();
    apply(Step::Dtr(false))?;
    settle();
    apply(Step::Rts(true))?;
    apply(Step::Dtr(false))?;
    apply(Step::Rts(true))?;
    settle();
    apply(Step::ClearInput)?;
    apply(Step::Rts(false))
}

#[cfg(test)]
mod tests;
