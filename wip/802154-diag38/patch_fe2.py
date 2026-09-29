"""Temporary #38 patch: dump PHY FE/TX windows and analog I2C blocks after the first direct TX."""
import sys
path, power = sys.argv[1], sys.argv[2]
src = open(path).read()
anchor = "    cycle.direct = transmitted(system, requested_at_micros).await?;\n"
assert anchor in src
dump = anchor + """    // TEMPORARY #38 FE/TX dump, never committed.
    {
        static DUMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !DUMPED.swap(true, core::sync::atomic::Ordering::Relaxed) {
            #[allow(unsafe_code)]
            // SAFETY: temporary experiment; the radio is powered, plain MMIO reads.
            unsafe {
                unsafe extern "C" {
                    fn ets_printf(format: *const u8, ...) -> i32;
                }
                let conf2 = 0x2010_F820 as *mut u32;
                let saved = core::ptr::read_volatile(conf2);
                if (saved >> 4) & 0x3fff != 0x3fa0 {
                    core::ptr::write_volatile(conf2, (saved & !(0x3fff << 4)) | (0x3fa0 << 4));
                }
                for (block, host, low) in [
                    (0x61u32, 1u32, 0x00ff_feffu32), (0x62, 1, 0x00ff_ffdf), (0x63, 1, 0x00ff_ffef),
                    (0x66, 0, 0x00ff_ff7f), (0x67, 1, 0x00ff_fffb), (0x69, 0, 0x00ff_f7ff),
                    (0x6a, 1, 0x00ff_ffbf), (0x6b, 1, 0x00ff_fff7), (0x6d, 0, 0x00ff_7fff),
                ] {
                    let ctrl = (0x2010_F800 + 4 * host) as *mut u32;
                    for register in 0..32u32 {
                        while core::ptr::read_volatile(ctrl) & (1 << 25) != 0 {}
                        core::ptr::write_volatile(0x2010_F81C as *mut u32, 0xff00_0000 | low);
                        core::ptr::write_volatile(ctrl, block | (register << 8));
                        while core::ptr::read_volatile(ctrl) & (1 << 25) != 0 {}
                        let value = (core::ptr::read_volatile(ctrl) >> 16) & 0xff;
                        ets_printf(c"OER38I2C %02x %02x %02x\\r\\n".as_ptr().cast(), block, register, value);
                    }
                }
                core::ptr::write_volatile(conf2, saved);
                let skipped = |a: u32| (0x2010_2fa0..0x2010_2fc0).contains(&a)
                    || [0x2010_70cc, 0x2010_8004, 0x2010_8050, 0x2010_8078].contains(&a);
                for (start, end) in [(0x2010_6000u32, 0x2010_8000u32)] {
                    let mut a = start;
                    while a < end {
                        if skipped(a) {
                            ets_printf(c"OER38W %08x SKIP\\r\\n".as_ptr().cast(), a);
                        } else {
                            ets_printf(c"OER38W %08x %08x\\r\\n".as_ptr().cast(), a, core::ptr::read_volatile(a as *const u32));
                        }
                        a += 4;
                    }
                }
                ets_printf(c"OER38W done\\r\\n".as_ptr().cast());
            }
        }
    }
"""
src = src.replace(anchor, dump, 1)
src = src.replace("        transmit_power_dbm: None,", f"        transmit_power_dbm: Some({power}),")
src = src.replace("            transmit_power_dbm: None,", f"            transmit_power_dbm: Some({power}),")
open(path, "w").write(src)
