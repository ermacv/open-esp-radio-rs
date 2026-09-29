"""peer-exchange: print PHY diag_frequency_report (xtal duty initial/middle/outer) after session start."""
p = "hil/targets/esp32s31/runtime/src/product_hil/ieee802154/session.rs"
s = open(p).read()
a = "    publish_event_reliably(\n        0,\n        request_id,\n        oer_hil_protocol::ieee802154::SessionStarted(started),\n    )\n    .await;\n"
assert s.count(a) == 1
s = s.replace(a, '''    // TEMPORARY #38: PHY frequency report.
    {
        struct Text { buf: [u8; 1024], len: usize }
        impl core::fmt::Write for Text {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for &b in s.as_bytes() { if self.len + 1 < self.buf.len() { self.buf[self.len] = b; self.len += 1; } }
                self.buf[self.len] = 0;
                Ok(())
            }
        }
        let mut text = Text { buf: [0; 1024], len: 0 };
        {
            let mut guard = client.radio.lock().await;
            oer_esp32s31_phy::diag_frequency_report(guard.lease(), &mut text);
        }
        #[allow(unsafe_code)]
        // SAFETY: temporary diagnostic; the buffer is NUL-terminated.
        unsafe {
            unsafe extern "C" { fn ets_printf(format: *const u8, ...) -> i32; }
            ets_printf(c"OER38FREQ session begin\\n%sOER38FREQ end\\n".as_ptr().cast(), text.buf.as_ptr());
        }
    }
''' + a, 1)
open(p, "w").write(s)
