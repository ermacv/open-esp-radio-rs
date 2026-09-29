import sys
p="hil/targets/esp32s31/runtime/src/product_hil/ieee802154/session.rs"
s=open(p).read()
a="    publish_event_reliably(0, request_id, HilEvent::Ieee802154SessionStarted(started)).await;\n"
assert s.count(a)==1
s=s.replace(a,'''    // TEMPORARY #38 A/B: vendor value of ZBBB radio control +0x338.
    {
        let _guard = client.radio.lock().await;
        #[allow(unsafe_code)]
        // SAFETY: temporary experiment on one PHY MMIO word while the radio is up.
        unsafe {
            unsafe extern "C" { fn ets_printf(format: *const u8, ...) -> i32; }
            let word = 0x2010_2f38 as *mut u32;
            let before = core::ptr::read_volatile(word);
            core::ptr::write_volatile(word, 0xa8b9_9001);
            ets_printf(c"OER38 f38 before=%08x after=%08x\\n".as_ptr().cast(), before, core::ptr::read_volatile(word));
        }
    }
'''+a,1)
open(p,"w").write(s)
