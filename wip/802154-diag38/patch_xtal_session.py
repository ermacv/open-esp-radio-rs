"""peer-exchange: write xtal duty 0x0a after session start (I2C only)."""
p = "hil/targets/esp32s31/runtime/src/product_hil/ieee802154/session.rs"
s = open(p).read()
a = "    publish_event_reliably(\n        0,\n        request_id,\n        oer_hil_protocol::ieee802154::SessionStarted(started),\n    )\n    .await;\n"
assert s.count(a) == 1
s = s.replace(a, '''    // TEMPORARY #38 A/B: vendor crystal duty.
    {
        let ok = {
            let mut guard = client.radio.lock().await;
            oer_esp32s31_phy::diag_set_xtal_duty(guard.lease(), 0x0a)
        };
        #[allow(unsafe_code)]
        // SAFETY: temporary diagnostic print.
        unsafe {
            unsafe extern "C" { fn ets_printf(format: *const u8, ...) -> i32; }
            ets_printf(c"OER38 xtal duty write ok=%d\\n".as_ptr().cast(), ok as i32);
        }
    }
''' + a, 1)
open(p, "w").write(s)
