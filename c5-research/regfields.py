import json,collections,os,glob,tomllib
exec(open("idxmap.py").read().split("pairs=collections")[0])
exec(open("mmio.py").read().split("report={}")[0].split("def mmio_accesses")[0])
touch=collections.defaultdict(lambda: collections.defaultdict(set))  # s31 off -> class -> fns
for lib in ("libpp","libnet80211"):
    d=libs[lib]; fa,fb=functions(f"{S}{d}/esp32s31/{lib}.a"),functions(f"{V}{d}/esp32c5/{lib}.a")
    cl=json.load(open(f"diff/{lib}.body.json"))["classes"]
    for n in cl:
        for _,a,_ in mmio_accesses(fa[n],0x20100000,0x20110000): touch[a-0x20100000][cl[n]].add(n)
        for a,s,*_ in indexed(fa[n],0x20100000,0x20110000):
            for q in range(8): touch[a-0x20100000+s*q][cl[n]].add(n)
json.dump({hex(o):{c:sorted(f) for c,f in v.items()} for o,v in touch.items()},open("diff/register-touch.json","w"),indent=0)
os.chdir("../..")
res=collections.Counter(); per=collections.defaultdict(collections.Counter)
for f in sorted(glob.glob("registers/esp32s31/model/peripherals/wifi-mac-*.toml")):
    b=os.path.basename(f)[9:-5]
    for p in tomllib.load(open(f,"rb"))["peripherals"]:
        for r in p["registers"]:
            R=r["register"]; a=p["baseAddress"]+R["addressOffset"]-0x20100000
            for i in range(R.get("dim",1)):
                t=touch.get(a+i*R.get("dimIncrement",0),{})
                k="untouched" if not t else ("same-code" if set(t)<= {"identical","lui-only"} else "differs")
                res[k]+=1; per[b][k]+=1
print(dict(res))
for b,c in per.items(): print(f"  {b:32} {dict(c)}")
