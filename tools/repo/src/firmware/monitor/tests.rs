use super::*;
use std::{cell::Cell, collections::VecDeque};

struct Input(VecDeque<io::Result<Vec<u8>>>);
impl Read for Input {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let next = self.0.pop_front().expect("no reads after cancellation")?;
        buffer[..next.len()].copy_from_slice(&next);
        Ok(next.len())
    }
}

#[test]
fn serial_timeouts_and_interrupted_reads_preserve_output_until_cancelled() {
    let mut input = Input(VecDeque::from([
        Err(io::ErrorKind::TimedOut.into()),
        Ok(b"boot\r\n".to_vec()),
        Err(io::ErrorKind::Interrupted.into()),
        Ok(b"ready\r\n".to_vec()),
    ]));
    let mut output = Vec::new();
    let calls = Cell::new(0);
    stream(&mut input, &mut output, || {
        let count = calls.get();
        calls.set(count + 1);
        count == 4
    })
    .unwrap();
    assert_eq!(output, b"boot\r\nready\r\n");
    assert!(input.0.is_empty());
}

#[test]
fn disconnect_is_reported_and_prior_cancellation_performs_no_read() {
    let mut input = Input(VecDeque::from([Err(io::ErrorKind::BrokenPipe.into())]));
    let mut output = Vec::new();
    stream(&mut input, &mut output, || true).unwrap();
    assert_eq!(
        stream(&mut input, &mut output, || false)
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[test]
fn eof_does_not_spin() {
    let mut input = Input(VecDeque::from([Ok(Vec::new())]));
    assert_eq!(
        stream(&mut input, &mut Vec::new(), || false)
            .unwrap_err()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
}
