import subprocess, sys
exec(open("/home/ermacv/.claude/jobs/6dcee4f0/tmp/onair_ab.py").read().split("s31, c5 = Board")[0])
OCD = "/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/tools/openocd-esp32/v0.12.0-esp32-20260831/openocd-esp32"
NM = "/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/tools/riscv32-esp-elf/esp-16.1.0_20260609/riscv32-esp-elf/bin/riscv32-esp-elf-nm"
ELF = "/home/ermacv/dev/open-esp-radio-rs-wifi/target/vendor-firmware/esp32s31/ieee802154-reference/build/oer_ieee802154_vendor_reference.elf"
syms = {}
for l in subprocess.run([NM, "-S", ELF], capture_output=True, text=True).stdout.splitlines():
    f = l.split()
    if f[-1] in ("phy_param", "phy_param_addr"): syms[f[-1]] = f[0]
print("SYMS", syms, flush=True)
s31 = Board("S31", S31)
s31.wait("@READY")
s31.cmd("CFG 15 4f45 0001 015431533332654f 0 12")
s31.cmd("TX 0 " + frame(1, 0x0002, 0x0001, False)); s31.lines(0.3)
log = open("/home/ermacv/.claude/jobs/6dcee4f0/tmp/vendor_fe2_ocd.txt", "w")
cmd = [OCD + "/bin/openocd", "-s", OCD + "/share/openocd/scripts", "-f", "interface/esp_usb_jtag.cfg",
       "-c", "adapter serial 30:ED:A0:F3:F6:D0", "-f", "target/esp32s31.cfg",
       "-f", "/home/ermacv/.claude/jobs/6dcee4f0/tmp/dump_fe2.tcl"]
p = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT)
try:
    print("OCD RC", p.wait(timeout=900), flush=True)
except subprocess.TimeoutExpired:
    p.kill(); p.wait(); print("OCD TIMEOUT", flush=True)
log.close()
