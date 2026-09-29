import subprocess, sys
exec(open("/home/ermacv/.claude/jobs/6dcee4f0/tmp/onair_ab.py").read().split("s31, c5 = Board")[0])
OCD = "/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/tools/openocd-esp32/v0.12.0-esp32-20260831/openocd-esp32"
def rd(tag):
    out = subprocess.run([OCD + "/bin/openocd", "-s", OCD + "/share/openocd/scripts", "-f", "interface/esp_usb_jtag.cfg",
                          "-c", "adapter serial 30:ED:A0:F3:F6:D0", "-f", "target/esp32s31.cfg",
                          "-f", "/home/ermacv/.claude/jobs/6dcee4f0/tmp/dump_xtal.tcl"], capture_output=True, text=True, timeout=120)
    print("VENDOR", tag, [l for l in (out.stdout + out.stderr).splitlines() if l.startswith("I2C")], flush=True)
s31 = Board("S31", S31)
s31.wait("@READY")
for ch in (15, 11, 26):
    s31.cmd(f"CFG {ch} 4f45 0001 015431533332654f 0 12")
    s31.cmd("TX 0 " + frame(ch, 0x0002, 0x0001, False)); s31.lines(0.3)
    rd(f"ch{ch}-after-TX")
