import json,collections,os,glob,tomllib
V=json.load(open("diff/function-verdicts.json"))["verdict"]
T=json.load(open("diff/register-touch.json"))
M={}; MSRC={}
for o,l in T["fixedpairs"].items():
    lib=[x for x in l if x[1]=="lib"]; pool=lib or l
    votes=collections.Counter()
    for c,k,n in pool: votes[c]+=n
    top=votes.most_common(2)
    M[int(o,16)]=int(top[0][0],16); MSRC[int(o,16)]=("lib" if lib else "rom")+("" if len(top)==1 or top[0][1]>=2*top[1][1] else "-contested")
EQ=json.load(open("diff/modem-offset-map.json"))
for o,v in EQ.items():
    o=int(o,16); c=int(max(v,key=v.get),16)
    if M.get(o)!=c: MSRC[o]="equal-length"+("-overrides-aligned" if o in M else "")
    else: MSRC[o]="equal-length"
    M[o]=c
# the MPDU-length link table: hal_he_init clears 0x55f0..0x57cc on both chips (loop bounds identical)
for o in range(0x55f0,0x57d0,4): M.setdefault(o,o); MSRC.setdefault(o,"lib-loop")
best={}
for sb,ss,cb,cs,n in T["pairs"]:
    if (sb,ss) not in best or best[(sb,ss)][2]<n: best[(sb,ss)]=(cb,cs,n)
def bank(o):
    for i in range(8):
        b=0x54d8-0x7c*i
        if b<=o<b+0x7c:
            r=o-b
            return (0x54b0-0x78*i+r if r<=0x68 else (0x54b0-0x78*i+r-4 if r>=0x74 else None)),"queue-bank"
    return None,None
def within(o,sb,ss,lo,hi):
    # the element lies on the pair's progression inside the array's span
    return lo<=o<hi and (o-sb)%ss==0 and lo<=sb<hi
def xlate(o,dim,inc,lo=None,hi=None):
    c,why=bank(o)
    if why: return c,why
    if lo is None: lo,hi=o,o+4
    for (sb,ss),(cb,cs,n) in best.items():
        if within(o,sb,ss,lo,hi): return cb+cs*((o-sb)//ss),"indexed"
    if o in M: return M[o],"fixed-"+MSRC[o]
    return None,None
def touching(o,lo,hi):
    fns=set()
    for c,l in T["fixed"].get(hex(o),{}).items(): fns|=set(l)
    for sb,ss,v in T["indexed"]:
        if within(o,sb,ss,lo,hi) or (bank(o)[1] and bank(sb)[1] and (o-sb)%ss==0 and abs(ss) in(0x7c,)):
            for c,l in v.items(): fns|=set(l)
    return fns
OK=("identical","same-modulo-map","struct-layout")
os.chdir("../..")
rows=[]; tot=collections.Counter(); per=collections.defaultdict(collections.Counter); review=collections.defaultdict(set)
for f in sorted(glob.glob("registers/esp32s31/model/peripherals/wifi-mac-*.toml")):
    b=os.path.basename(f)[9:-5]
    for p in tomllib.load(open(f,"rb"))["peripherals"]:
        for r in p["registers"]:
            R=r["register"]; a=p["baseAddress"]+R["addressOffset"]-0x20100000; dim=R.get("dim",1); inc=R.get("dimIncrement",0)
            for i in range(dim):
                o=a+i*inc; lo,hi=a,a+max(dim*inc,4); fns=touching(o,lo,hi); c5,how=xlate(o,dim,inc,lo,hi)
                if not fns: k="unobserved"
                elif c5 is None: k="no-address"
                elif all(V.get(n) in OK for n in fns): k="confirmed"
                elif any(V.get(n) in OK for n in fns): k="partly"; review[b]|={n for n in fns if V.get(n) not in OK}
                else: k="review"; review[b]|=fns
                tot[k]+=1; per[b][k]+=1
                rows.append(dict(file=b,peripheral=p["name"],name=R["name"],index=i,s31=hex(o),c5=hex(c5) if c5 is not None else None,how=how,status=k,functions=sorted(fns)))
json.dump(rows,open("target/c5-research/diff/wifi-mac-c5-map.json","w"),indent=0)
print(dict(tot))
for b,c in per.items():
    if set(c)!={"confirmed"}: print(f"  {b:28} {dict(c)}  {sorted(review[b])[:5]}")
print("fully confirmed:",[b for b,c in per.items() if set(c)=={"confirmed"}])
