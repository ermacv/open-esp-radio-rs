import re,sys
root="/home/ermacv/dev/oer-phy-diag38/"
p=root+"crates/composition/esp32s31/embassy/ieee802154/src/system.rs"
s=open(p).read()
a="    if acquired == ConcurrentAcquire::TrackingDue {\n"
assert s.count(a)==1
s=s.replace(a,"    diag38_write(|w| { let _ = core::fmt::Write::write_fmt(w, format_args!(\"diag38 acquired={:?}\\n\", acquired)); });\n"+a,1)
b="            Ok(proof) => {\n                maintain_concurrent_phy::<P, EmbassyPhyTime, _>(\n"
i=s.index(a); j=s.index(b,i)
s=s[:j]+s[j:].replace(b,"            Ok(proof) => {\n                diag38_write(|w| { let _ = core::fmt::Write::write_str(w, \"diag38 --- before join tracking\\n\"); oer_esp32s31_phy::diag_tracking_report(&*lease, w); });\n                maintain_concurrent_phy::<P, EmbassyPhyTime, _>(\n",1)
c="            Ok(_outcome) => {}\n"
j=s.index(c,i)
s=s[:j]+s[j:].replace(c,"            Ok(outcome) => {\n                diag38_write(|w| { let _ = core::fmt::Write::write_fmt(w, format_args!(\"diag38 outcome={:?}\\ndiag38 --- after join tracking\\n\", outcome)); oer_esp32s31_phy::diag_tracking_report(&*lease, w); });\n            }\n",1)
s+='''
/// TEMPORARY #38 diagnostic text buffer.
pub struct Diag38Text { buf: [u8; 4096], len: usize }
impl core::fmt::Write for Diag38Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len + 1 < self.buf.len() { self.buf[self.len] = b; self.len += 1; }
        }
        self.buf[self.len] = 0;
        Ok(())
    }
}
static DIAG38: embassy_sync::blocking_mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    core::cell::RefCell<Diag38Text>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(Diag38Text { buf: [0; 4096], len: 0 }));
fn diag38_write(f: impl FnOnce(&mut Diag38Text)) {
    DIAG38.lock(|cell| f(&mut cell.borrow_mut()));
}
/// TEMPORARY #38: NUL-terminated collected text.
pub fn diag38_with(f: impl FnOnce(&[u8])) {
    DIAG38.lock(|cell| { let t = cell.borrow(); f(&t.buf[..=t.len]) });
}
'''
open(p,"w").write(s)
p=root+"crates/composition/esp32s31/embassy/ieee802154/src/lib.rs"
s=open(p).read()
a="    Ieee802154System, Ieee802154SystemRuntime, start,\n};\n"
assert s.count(a)==1
s=s.replace(a,a+"#[cfg(target_arch = \"riscv32\")]\npub use system::diag38_with;\n",1)
open(p,"w").write(s)
p=root+"hil/targets/esp32s31/runtime/src/product_hil/ieee802154/session.rs"
s=open(p).read()
a="    publish_event_reliably(0, request_id, HilEvent::Ieee802154SessionStarted(started)).await;\n"
assert s.count(a)==1
s=s.replace(a,'''    {
        unsafe extern "C" { fn ets_printf(format: *const u8, ...) -> i32; }
        oer_esp32s31_ieee802154_system::diag38_with(|t| {
            // SAFETY: TEMPORARY #38 diagnostic; t is NUL-terminated.
            unsafe { ets_printf(b"OER38 report begin\\n%sOER38 report end\\n\\0".as_ptr(), t.as_ptr()) };
        });
    }
'''+a,1)
open(p,"w").write(s)
print("ok")
