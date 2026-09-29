import json,collections,sys,re
exec(open("mmio.py").read().split("S=\"../../../")[0])
S="../../../open-esp-radio-rs/target/vendor/"; V="../vendor/"
P="esp-phy-lib/20f1db053a0e6cb9f1c09d255c43bf42483041d0"
def load(paths):
    out={}
    for p in paths: out.update(functions(p))
    return out
s31={"rom":functions("rom/esp32s31_rev0_rom.elf"),
     "lib":load([f"{S}{P}/esp32s31/{l}.a" for l in ["libphy","librftest","libbtbb","libbttestmode"]]),
     "ble":load([f"{S}esp32s31-bt-lib/10c507788e9da0993709cf82e405c896561172d8/{l}.a" for l in ["libble_app","libbtdm_common"]])}
c5={"rom":functions(f"{V}esp-rom-elfs/20260528/esp32c5_rev100_rom.elf"),
    "lib":load([f"{V}{P}/esp32c5/{l}.a" for l in ["libphy","librftest","libbtbb","libbttestmode"]]),
    "ble":functions(f"{V}esp32c5-bt-lib/0b5cb2d7cfb4078e951da36a695bc4fb37e52391/libble_app.a")}
A=lambda d:{**d["rom"],**d["lib"]}
pairs=[]  # (source, s31 body, c5 body)
sa,ca=A(s31),A(c5)
for n in set(sa)&set(ca): pairs.append(("phy/rom-by-name",n,sa[n],ca[n]))
lin=json.load(open("lineage-ble.json"))["functions"]
for f in lin:
    for st in f["chain"]:
        cn,sn=st["previous_name"],f["name"]
        if cn in c5["ble"] and sn in s31["ble"]: pairs.append(("ble/lineage",sn,s31["ble"][sn],c5["ble"][cn]))
        # BLE ROM functions share names r_ble_* in both ROMs too (already in by-name)
off=collections.defaultdict(collections.Counter); pages=collections.Counter(); used=collections.Counter()
for src,n,a,b in pairs:
    ma=mmio_accesses(a,0x20100000,0x20110000); mb=mmio_accesses(b,0x600a0000,0x600b0000)
    if not ma or len(ma)!=len(mb): continue
    if [x[2] for x in ma]!=[x[2] for x in mb]: continue
    used[src]+=1
    for x,y in zip(ma,mb):
        off[x[1]-0x20100000][y[1]-0x600a0000]+=1; pages[((x[1]-0x20100000)>>12,(y[1]-0x600a0000)>>12)]+=1
print("pairs",collections.Counter(p[0] for p in pairs),"used",used)
print("page map:",sorted((hex(a),hex(b),n) for (a,b),n in pages.items()))
conf={o:c for o,c in off.items() if len(c)>1}
print("offsets",len(off),"conflicting",len(conf))
for o,c in list(conf.items())[:15]: print("  conflict",hex(o),{hex(k):v for k,v in c.items()})
json.dump({hex(o):{hex(k):v for k,v in c.items()} for o,c in sorted(off.items())},open("diff/modem-offset-map-phy-ble.json","w"),indent=1)
