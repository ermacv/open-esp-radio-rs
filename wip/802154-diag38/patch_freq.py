"""Temporary #38 patch on top of patch_sdm.py: PHY diag_frequency_report at quiet air-check points."""
import sys
path = sys.argv[1]
s = open(path).read()
helper = '''
// TEMPORARY #38: PHY frequency report, never committed.
async fn oer38_freq(radio: &'static oer_esp32s31_radio_system::SharedRadio, tag: &core::ffi::CStr) {
    if OER38_DONE.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
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
        let mut guard = radio.lock().await;
        oer_esp32s31_phy::diag_frequency_report(guard.lease(), &mut text);
    }
    #[allow(unsafe_code)]
    // SAFETY: temporary diagnostic; the buffer is NUL-terminated.
    unsafe {
        unsafe extern "C" { fn ets_printf(format: *const u8, ...) -> i32; }
        ets_printf(c"OER38FREQ %s begin\\n%sOER38FREQ end\\n".as_ptr().cast(), tag.as_ptr(), text.buf.as_ptr());
    }
}
'''
a = "async fn run_cycle("
assert s.count(a) == 1
s = s.replace(a, helper + "\n" + a, 1)
def rep(old, new):
    global s
    assert s.count(old) == 1, old
    s = s.replace(old, new, 1)
rep("        let outcome = run_cycle(&system, channel, request, &mut evidence.cycles[index]).await;",
    "        let outcome = run_cycle(client.radio, &system, channel, request, &mut evidence.cycles[index]).await;")
rep("async fn run_cycle(\n    system: &Ieee802154System,",
    "async fn run_cycle(\n    radio: &'static oer_esp32s31_radio_system::SharedRadio,\n    system: &Ieee802154System,")
rep('    oer38_pll(c"enabled");\n', '    oer38_pll(c"enabled");\n    oer38_freq(radio, c"enabled").await;\n')
rep('    oer38_pll(c"before-tx");\n', '    oer38_pll(c"before-tx");\n    oer38_freq(radio, c"before-tx").await;\n')
rep('    oer38_pll(c"after-tx");\n', '    oer38_pll(c"after-tx");\n    oer38_freq(radio, c"after-tx").await;\n')
rep('    oer38_pll(c"after-sleep");\n', '    oer38_pll(c"after-sleep");\n    oer38_freq(radio, c"after-sleep").await;\n')
open(path, "w").write(s)
