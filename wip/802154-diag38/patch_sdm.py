"""Temporary #38 patch: print RF PLL regi2c bytes at several air-check points (first cycle only)."""
import sys
path = sys.argv[1]
s = open(path).read()
helper = '''
// TEMPORARY #38: RF PLL regi2c snapshot, never committed.
static OER38_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
fn oer38_pll(tag: &core::ffi::CStr) {
    if OER38_DONE.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    #[allow(unsafe_code)]
    // SAFETY: temporary experiment; the radio is powered, the analog master is idle between commands.
    unsafe {
        unsafe extern "C" {
            fn ets_printf(format: *const u8, ...) -> i32;
        }
        let conf2 = 0x2010_F820 as *mut u32;
        let saved = core::ptr::read_volatile(conf2);
        if (saved >> 4) & 0x3fff != 0x3fa0 {
            core::ptr::write_volatile(conf2, (saved & !(0x3fff << 4)) | (0x3fa0 << 4));
        }
        let mut v = [0u32; 10];
        for (i, (block, reg, low)) in [
            (0x63u32, 0x03u32, 0x00ff_ffefu32), (0x63, 0x04, 0x00ff_ffef), (0x63, 0x05, 0x00ff_ffef), (0x63, 0x06, 0x00ff_ffef),
            (0x62, 0x01, 0x00ff_ffdf), (0x62, 0x02, 0x00ff_ffdf), (0x62, 0x05, 0x00ff_ffdf), (0x62, 0x07, 0x00ff_ffdf),
            (0x61, 0x09, 0x00ff_feff), (0x61, 0x0a, 0x00ff_feff),
        ].into_iter().enumerate() {
            let ctrl = (0x2010_F800 + 4) as *mut u32;
            while core::ptr::read_volatile(ctrl) & (1 << 25) != 0 {}
            core::ptr::write_volatile(0x2010_F81C as *mut u32, 0xff00_0000 | low);
            core::ptr::write_volatile(ctrl, block | (reg << 8));
            while core::ptr::read_volatile(ctrl) & (1 << 25) != 0 {}
            v[i] = (core::ptr::read_volatile(ctrl) >> 16) & 0xff;
        }
        core::ptr::write_volatile(conf2, saved);
        ets_printf(
            c"OER38PLL %s sdm=%02x %02x %02x %02x vco=%02x %02x %02x %02x p61=%02x %02x f38=%08x\\r\\n".as_ptr().cast(),
            tag.as_ptr(), v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9],
            core::ptr::read_volatile(0x2010_2f38 as *const u32),
        );
    }
}
'''
anchor_fn = "async fn run_cycle("
assert s.count(anchor_fn) == 1
s = s.replace(anchor_fn, helper + "\n" + anchor_fn, 1)
def ins_after(a, text):
    global s
    assert s.count(a) == 1, a
    s = s.replace(a, a + text, 1)
def ins_before(a, text):
    global s
    assert s.count(a) == 1, a
    s = s.replace(a, text + a, 1)
ins_after("    submit(RadioCommand::Enable { id: id() })?;\n", '    oer38_pll(c"enabled");\n')
ins_before("    submit(RadioCommand::ClearChannelAssessment { id: id(), channel })?;\n", '    oer38_pll(c"after-energy-scan");\n')
ins_before("    let requested_at_micros = now_micros();\n", '    oer38_pll(c"before-tx");\n')
ins_after("    cycle.direct = transmitted(system, requested_at_micros).await?;\n", '    oer38_pll(c"after-tx");\n')
ins_after("    submit(RadioCommand::Receive { id: id(), channel })?;\n", '    Timer::after(Duration::from_millis(20)).await;\n    oer38_pll(c"in-rx-20ms");\n')
ins_before("    let start_micros = now_micros() + u64::from(request.scheduled_lead_micros);\n", '    oer38_pll(c"after-sleep");\n    OER38_DONE.store(true, core::sync::atomic::Ordering::Relaxed);\n')
open(path, "w").write(s)
