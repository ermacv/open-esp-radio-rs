import json,collections,os,glob,tomllib,re
exec(open("idxmap.py").read().split("pairs=collections")[0])
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
out={}
for lib in ("libpp","libnet80211"):
    d=libs[lib]; fa,fb=functions(f"{S}{d}/esp32s31/{lib}.a"),functions(f"{V}{d}/esp32c5/{lib}.a")
    for n in set(fa)&set(fb):
        if fa[n]==fb[n]: out[n]="identical"; continue
        v=verdict(fa[n],fb[n]); out[n]=v if isinstance(v,str) else v[0]
        if not isinstance(v,str): out[n+"#why"]=[list(t) for t in v[1]]
json.dump(out,open("diff/function-verdicts.json","w"),indent=0)
print(collections.Counter(v for k,v in out.items() if not k.endswith("#why")))
