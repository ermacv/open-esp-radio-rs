import json,collections,os,glob,tomllib,re
exec(open("idxmap.py").read().split("pairs=collections")[0].replace("pairs=",""))
m=json.load(open("diff/modem-offset-map.json")); M={int(k,16):int(max(v,key=v.get),16) for k,v in m.items()}
def bank(o):
    # 8 reverse-stride queue banks: S31 0x54d8-0x7c*i -> C5 0x54b0-0x78*i
    for i in range(8):
        b=0x54d8-0x7c*i
        if b<=o<b+0x7c:
            r=o-b
            if r<=0x68: return 0x54b0-0x78*i+r
            if r>=0x74: return 0x54b0-0x78*i+r-4
    return None
def xlate(o):
    c=bank(o)
    return c if c is not None else M.get(o)
def annot(ins, lo):
    """Per instruction: modem address it forms/uses, if any (straight-line)."""
    R={}; out=[None]*len(ins)
    for i,x in enumerate(ins):
        p=x.split(" ",1); op=p[0]; a=[s.strip() for s in p[1].split(",")] if len(p)>1 else []
        mm=MEMOP.match(x)
        if mm:
            o,rd,off,b=mm.groups(); v=R.get(b)
            if v is not None: out[i]=("addr",v+num(off))
            if o[0]=="l": R.pop(rd,None)
            continue
        if not a: continue
        rd=a[0]; new=None
        if op=="lui":
            v=num(a[1])<<12 & 0xffffffff
            if lo<=v<lo+0x10000: new=v; out[i]=("page",v)
        elif op=="addi" and a[1] in R: new=R[a[1]]+num(a[2]); out[i]=("addr",new)
        elif op=="li" and num(a[2] if len(a)>2 else a[1]) in (-0x7c,-0x78,0x7c,0x78): out[i]=("stride",None)
        elif op=="mv" and a[1] in R: new=R[a[1]]
        if new is not None: R[rd]=new
        else: R.pop(rd,None)
    return out
IMMR=re.compile(r"-?0x[0-9a-f]+|-?\b\d+\b")
def verdict(A,B):
    if len(A)!=len(B) or any(IMMR.sub("#",x)!=IMMR.sub("#",y) for x,y in zip(A,B)): return "structural"
    aa,bb=annot(A,0x20100000),annot(B,0x600a0000); bad=[]
    for k,(x,y) in enumerate(zip(A,B)):
        if x==y: continue
        u,w=aa[k],bb[k]
        if u and w and u[0]==w[0]=="page": continue
        if u and w and u[0]==w[0]=="stride": continue
        if u and w and u[0]==w[0]=="addr":
            o=u[1]-0x20100000; c=xlate(o)
            if c is None or c==w[1]-0x600a0000: continue   # unmapped addresses judged by the map, not here
        bad.append((x,y))
    return "same-modulo-map" if not bad else ("field-differs",bad[:3])

import difflib
IMMR2=IMMR

IMMR=re.compile(r"-?0x[0-9a-f]+|-?\b\d+\b")
def align(A,B):
    sm=difflib.SequenceMatcher(None,[IMMR.sub("#",x) for x in A],[IMMR.sub("#",x) for x in B],autojunk=False)
    m={}
    for a,b,n in sm.get_matching_blocks():
        for k in range(n): m[a+k]=b+k
    return m

import os
ROM="../vendor/esp-rom-elfs/20260528/"
sources={"libpp":(f"{S}{libs['libpp']}/esp32s31/libpp.a",f"{V}{libs['libpp']}/esp32c5/libpp.a"),
 "libnet80211":(f"{S}{libs['libnet80211']}/esp32s31/libnet80211.a",f"{V}{libs['libnet80211']}/esp32c5/libnet80211.a"),
 "libcoexist":(f"{S}{libs['libcoexist']}/esp32s31/libcoexist.a",f"{V}{libs['libcoexist']}/esp32c5/libcoexist.a"),
 "libphy":(f"{S}{libs['libphy']}/esp32s31/libphy.a",f"{V}{libs['libphy']}/esp32c5/libphy.a"),
 "rom":(ROM+"esp32s31_rev0_rom.elf",ROM+"esp32c5_rev100_rom.elf")}
def verdict2(A,B):
    if len(A)!=len(B) or any(IMMR.sub("#",x)!=IMMR.sub("#",y) for x,y in zip(A,B)): return "structural",[]
    aa,bb=annot(A,0x20100000),annot(B,0x600a0000); soft=[]; hard=[]
    for k,(x,y) in enumerate(zip(A,B)):
        if x==y: continue
        u,w=aa[k],bb[k]
        if u and w and u[0]==w[0] and u[0] in("page","stride"): continue
        if u and w and u[0]==w[0]=="addr":
            c=xlate(u[1]-0x20100000)
            if c is None or c==w[1]-0x600a0000: continue
            hard.append((x,y)); continue
        op=x.split()[0]
        if MEMOP.match(x) or op in("lui","auipc","jal","j","call","tail") or (op=="addi" and ("sp" in x or "s0" in x)) or op.startswith("b"): soft.append((x,y)); continue
        hard.append((x,y))
    return ("same-modulo-map" if not hard and not soft else "struct-layout" if not hard else "value-differs"), hard
verd={}; why={}; touch=collections.defaultdict(lambda: collections.defaultdict(set)); idxpairs=collections.defaultdict(collections.Counter); fixedpairs=collections.defaultdict(collections.Counter)
for src,(pa,pb) in sources.items():
    if not (os.path.exists(pa) and os.path.exists(pb)): print("missing",src,pa,pb); continue
    fa,fb=functions(pa),functions(pb)
    for n in set(fa)&set(fb):
        key=f"{src}:{n}"
        if fa[n]==fb[n]: verd[key]="identical"
        else:
            v,h=verdict2(fa[n],fb[n]); verd[key]=v
            if h: why[key]=h
        FA=mmio_accesses(fa[n],0x20100000,0x20110000)
        for _,a,_ in FA: touch[a-0x20100000][verd[key]].add(key)
        if FA:
            FB={i:(a,o) for i,a,o in mmio_accesses(fb[n],0x600a0000,0x600b0000)}; mm2=align(fa[n],fb[n])
            for i,a,o in FA:
                y=FB.get(mm2.get(i))
                if y and y[1]==o: fixedpairs[a-0x20100000][(y[0]-0x600a0000, 'rom' if src=='rom' else 'lib')]+=1
        A=indexed(fa[n],0x20100000,0x20110000); B=indexed(fb[n],0x600a0000,0x600b0000)
        if A:
            mm=align(fa[n],fb[n]); bi={y[3]:y for y in B}
            for x in A:
                touch[("i",x[0]-0x20100000,x[1])][verd[key]].add(key)
                y=bi.get(mm.get(x[3]))
                if y and y[2]==x[2]: idxpairs[(x[0]-0x20100000,x[1])][(y[0]-0x600a0000,y[1])]+=1
    print(src, collections.Counter(v for k,v in verd.items() if k.startswith(src+":")))
json.dump({"verdict":verd,"value_diffs":why},open("diff/function-verdicts.json","w"),indent=0)
json.dump({"fixed":{hex(o):{c:sorted(f) for c,f in v.items()} for o,v in touch.items() if isinstance(o,int)},
           "indexed":[[o[1],o[2],{c:sorted(f) for c,f in v.items()}] for o,v in touch.items() if not isinstance(o,int)],
           "fixedpairs":{hex(o):[[hex(c[0]),c[1],n] for c,n in v.items()] for o,v in fixedpairs.items()},
           "pairs":[[k[0],k[1],c[0],c[1],n] for k,v in idxpairs.items() for c,n in v.items()]},open("diff/register-touch.json","w"))
