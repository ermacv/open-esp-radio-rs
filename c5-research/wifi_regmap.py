import json,tomllib,glob,os,collections
os.chdir("/home/ermacv/dev/open-esp-radio-rs-esp32c5")
m=json.load(open("target/c5-research/diff/modem-offset-map.json"))
known={int(k,16):int(max(v,key=v.get),16) for k,v in m.items()}
src={k:"fixed" for k in known}
best={}
for sb,ss,cb,cs,n in json.load(open("target/c5-research/diff/indexed-map.json")):
    if (sb,ss) not in best or best[(sb,ss)][2]<n: best[(sb,ss)]=(cb,cs,n)
idx=[(sb,ss,cb,cs) for (sb,ss),(cb,cs,n) in best.items()]
def indexed(o,dim,inc):
    for sb,ss,cb,cs in idx:
        if abs(ss)==inc and (o-sb)%ss==0 and abs((o-sb)//ss)<dim: return cb+cs*((o-sb)//ss)
ks=sorted(known)
def bracket(o):
    lo=[k for k in ks if k<=o]; hi=[k for k in ks if k>=o]
    if lo and hi and known[lo[-1]]-lo[-1]==known[hi[0]]-hi[0]: return known[lo[-1]]-lo[-1]
tot=collections.Counter(); rows=[]; unknown=[]
for f in sorted(glob.glob("registers/esp32s31/model/peripherals/wifi-mac-*.toml")):
    d=tomllib.load(open(f,"rb")); n=collections.Counter()
    for p in d["peripherals"]:
        for r in p["registers"]:
            R=r["register"]; a=p["baseAddress"]+R["addressOffset"]-0x20100000
            for i in range(R.get("dim",1)):
                o=a+i*R.get("dimIncrement",0)
                c=indexed(o,R.get("dim",1),R.get("dimIncrement",0)) if "dim" in R else None
                if o in known and c is not None and known[o]!=c: print("CONFLICT",hex(o),hex(known[o]),hex(c))
                if c is not None: n["indexed"]+=1; known.setdefault(o,c)
                elif o in known: n[src[o]]+=1
                elif bracket(o) is not None: n["bracketed"]+=1
                else: n["unknown"]+=1; unknown.append((hex(o),R["name"],i,os.path.basename(f)))
    tot+=n; rows.append((os.path.basename(f)[:-5],n))
for r,n in rows:
    if n["unknown"] or n["bracketed"]: print(f"{r:40}",dict(n))
print("TOTAL register instances",sum(tot.values()),dict(tot))
for u in unknown: print("  unknown",*u)
