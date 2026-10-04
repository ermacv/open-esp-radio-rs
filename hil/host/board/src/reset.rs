//! USB-Serial/JTAG reset boundary: drain the old boot before the reset edge.
use std::{path::Path, thread, time::Duration};

/// Line rate and read timeout of a board console the stand opens.
const BAUD_RATE: u32 = 115_200;
const READ_TIMEOUT: Duration = Duration::from_millis(200);
/// How long RTS holds the chip in reset when resetting into the application.
const APPLICATION_RESET: Duration = Duration::from_millis(200);

/// The console at `port`, opened without a reset: RTS is released before
/// DTR, so the lines never pass through the reset-with-boot-strap state.
///
/// Opening a USB serial port with the default modem lines resets an
/// Espressif chip, and a UART bridge's lines drive EN and BOOT: this and
/// [`reset_into_application`] are the only ways stand tools open a board's
/// port, for one command or for a long interactive session alike.
pub fn open_without_reset(port: &Path) -> serialport::Result<Box<dyn serialport::SerialPort>> {
    let mut serial = serialport::new(port.to_string_lossy(), BAUD_RATE)
        .timeout(READ_TIMEOUT)
        .open()?;
    serial.write_request_to_send(false)?;
    serial.write_data_terminal_ready(false)?;
    Ok(serial)
}

/// Reset the board at `port` into its flashed application and return its
/// open console: RTS pulses the chip's reset while DTR keeps the boot strap
/// released. `espflash`'s own reset after connecting leaves an esp32c5 in its
/// ROM download mode.
pub fn reset_into_application(port: &Path) -> serialport::Result<Box<dyn serialport::SerialPort>> {
    let mut serial = serialport::new(port.to_string_lossy(), BAUD_RATE)
        .timeout(READ_TIMEOUT)
        .open()?;
    serial.write_data_terminal_ready(false)?;
    serial.write_request_to_send(true)?;
    thread::sleep(APPLICATION_RESET);
    serial.write_request_to_send(false)?;
    Ok(serial)
}

#[derive(Clone, Copy)]
enum Step {
    Dtr(bool),
    Rts(bool),
    ClearInput,
}

/// Reset the chip behind a USB-Serial/JTAG port and discard what the old
/// boot sent, so the next read starts with the new boot.
pub fn reset_usb_serial_jtag(serial: &mut dyn serialport::SerialPort) -> serialport::Result<()> {
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
    // just before the reset. The chip restarts on RTS's rising edge, not on
    // its release: it boots while RTS is still asserted, and an esp32c5
    // application has sent its hello before the release, so input drained
    // after the edge would discard the new boot's first words.
    settle();
    apply(Step::Dtr(false))?;
    settle();
    apply(Step::ClearInput)?;
    apply(Step::Rts(true))?;
    apply(Step::Dtr(false))?;
    apply(Step::Rts(true))?;
    settle();
    apply(Step::Rts(false))
}

#[cfg(test)]
mod tests;
