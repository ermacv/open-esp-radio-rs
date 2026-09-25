//! Serial output without changing the terminal or re-entering the ROM loader.
use crate::{Result, process};
use std::{
    io::{self, Read, Write},
    path::Path,
};

pub(super) fn run(port: &Path) -> Result<()> {
    let mut serial = serialport::new(port.to_string_lossy(), 115_200)
        .flow_control(serialport::FlowControl::None)
        .timeout(std::time::Duration::from_millis(100))
        .open()?;
    let mut stdout = io::stdout().lock();
    eprintln!("monitor: {} (Ctrl-C to stop)", port.display());
    stream(&mut serial, &mut stdout, process::cancellation_requested)?;
    Ok(())
}

fn stream(
    reader: &mut impl Read,
    writer: &mut impl Write,
    cancelled: impl Fn() -> bool,
) -> io::Result<()> {
    let mut buffer = [0; 1024];
    while !cancelled() {
        match reader.read(&mut buffer) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => {
                writer.write_all(&buffer[..count])?;
                writer.flush()?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
