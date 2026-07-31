//! Synchronous ROM-console output for the isolated vendor oracle.

use core::fmt::{Arguments, Write};

const MESSAGE_CAPACITY: usize = 384;

unsafe extern "C" {
    fn ets_printf(format: *const u8, ...) -> i32;
}

struct TextBuffer {
    bytes: [u8; MESSAGE_CAPACITY],
    len: usize,
}

impl TextBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; MESSAGE_CAPACITY],
            len: 0,
        }
    }
}

impl Write for TextBuffer {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = self.bytes.len() - 1 - self.len;
        let length = text.len().min(available);
        self.bytes[self.len..self.len + length].copy_from_slice(&text.as_bytes()[..length]);
        self.len += length;
        Ok(())
    }
}

pub fn emergency_log(args: Arguments<'_>) {
    let mut message = TextBuffer::new();
    let _ = message.write_fmt(args);
    unsafe {
        ets_printf(c"%s\r\n".as_ptr().cast(), message.bytes.as_ptr());
    }
}

pub fn panic_report(mcause: usize, mepc: usize, mtval: usize) {
    unsafe {
        ets_printf(
            c"panic mcause=%08x mepc=%08x mtval=%08x\r\n"
                .as_ptr()
                .cast(),
            mcause,
            mepc,
            mtval,
        );
    }
}
