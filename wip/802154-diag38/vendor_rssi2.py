import sys, time, serial
power = sys.argv[1]
sys.argv = [sys.argv[0]]
exec(open("/home/ermacv/.claude/jobs/6dcee4f0/tmp/onair_ab.py").read().split("s31, c5 = Board")[0])
class PlainBoard(Board):
    def __init__(self, name, path):
        self.name, self.buf = name, b""
        self.port = serial.Serial()
        self.port.port, self.port.baudrate, self.port.timeout = path, 115200, 0.05
        self.port.dtr = False
        self.port.rts = False
        self.port.open()
s31 = Board("S31", S31)
s31.wait("@READY")
c5 = PlainBoard("C5", C5)
time.sleep(1)
c5.port.write(b"\n")
for attempt in range(5):
    try:
        c5.cmd("SYNC"); break
    except SystemExit:
        time.sleep(1)
else:
    raise SystemExit("C5 never answered SYNC")
s31.cmd(f"CFG 15 4f45 0001 015431533332654f 0 {power}")
c5.cmd("CFG 15 4f45 0002 025431433570654f 0 0")
c5.cmd("RX")
for seq in range(1, 11):
    s31.cmd(f"TX 0 {frame(seq, 0x0002, 0x0001, False)}")
    for line in c5.lines(0.3):
        if line.startswith("@RX"):
            print("VENDOR-RSSI", line, flush=True)
