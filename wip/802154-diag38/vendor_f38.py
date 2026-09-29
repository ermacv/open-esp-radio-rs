import subprocess, sys, time
exec(open("/home/ermacv/.claude/jobs/6dcee4f0/tmp/onair_ab.py").read().split("s31, c5 = Board")[0])
OCD = "/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/tools/openocd-esp32/v0.12.0-esp32-20260831/openocd-esp32"
def rd(tag):
    out = subprocess.run([OCD + "/bin/openocd", "-s", OCD + "/share/openocd/scripts", "-f", "interface/esp_usb_jtag.cfg",
                          "-c", "adapter serial 30:ED:A0:F3:F6:D0", "-f", "target/esp32s31.cfg",
                          "-c", "init", "-c", "halt", "-c", "echo [format {F38 %08x} [lindex [read_memory 0x20102f38 32 1] 0]]",
                          "-c", "resume", "-c", "shutdown"], capture_output=True, text=True, timeout=120)
    vals = [l for l in (out.stdout + out.stderr).splitlines() if l.startswith("F38")]
    print("VENDOR", tag, vals, flush=True)
s31 = Board("S31", S31)
s31.wait("@READY")
rd("after-boot")
s31.cmd("CFG 15 4f45 0001 015431533332654f 0 12")
rd("after-CFG-idle")
s31.cmd("TX 0 " + frame(1, 0x0002, 0x0001, False)); s31.lines(0.3)
rd("after-TX")
time.sleep(5)
rd("after-5s-idle")
s31.cmd("RX"); s31.lines(0.5)
rd("in-RX")
s31.cmd("TX 0 " + frame(2, 0x0002, 0x0001, False)); s31.lines(0.3)
rd("after-2nd-TX")
