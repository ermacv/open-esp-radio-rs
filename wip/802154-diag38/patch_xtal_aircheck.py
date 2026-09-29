"""On top of patch_sdm.py + patch_freq.py: write xtal duty 0x0a right after Enable."""
import sys
path = sys.argv[1]
s = open(path).read()
old = '    oer38_pll(c"enabled");\n    oer38_freq(radio, c"enabled").await;\n'
assert s.count(old) == 1
new = old + '''    {
        let ok = {
            let mut guard = radio.lock().await;
            oer_esp32s31_phy::diag_set_xtal_duty(guard.lease(), 0x0a)
        };
        #[allow(unsafe_code)]
        // SAFETY: temporary diagnostic print.
        unsafe {
            unsafe extern "C" { fn ets_printf(format: *const u8, ...) -> i32; }
            ets_printf(c"OER38 xtal duty write ok=%d\\n".as_ptr().cast(), ok as i32);
        }
    }
    oer38_pll(c"after-xtal-write");
'''
s = s.replace(old, new, 1)
open(path, "w").write(s)
