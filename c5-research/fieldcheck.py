import json,collections,re
exec(open("wifimap.py").read().split("verd={}")[0])
LOGIC=re.compile(r"^(andi|ori|xori|slli|srli|srai|lui|li|bseti|bclri|bexti|binvi) ")
def logic_window(ins,i,span=6):
    """Immediates of logic ops within +-span instructions of access i (register names dropped)."""
    out=[]
    for k in range(max(0,i-span),min(len(ins),i+span+1)):
        x=ins[k]
        if LOGIC.match(x):
            op=x.split()[0]; imm=x.split(",")[-1].strip()
            if op=="lui" and (imm.startswith("0x2010") or imm.startswith("0x600a")): continue
            out.append((op,imm))
    return out
res=collections.defaultdict(lambda: collections.Counter()); ex={}
for src,(pa,pb) in sources.items():
    fa,fb=functions(pa),functions(pb)
    for n in set(fa)&set(fb):
        A=mmio_accesses(fa[n],0x20100000,0x20110000)
        if not A: continue
        B={i:(a,o) for i,a,o in mmio_accesses(fb[n],0x600a0000,0x600b0000)}; mm=align(fa[n],fb[n])
        for i,a,o in A:
            j=mm.get(i); y=B.get(j)
            if not y: res[a-0x20100000]["unaligned"]+=1; continue
            wa,wb=logic_window(fa[n],i),logic_window(fb[n],j)
            k="same-bits" if wa==wb else "bits-differ"
            res[a-0x20100000][k]+=1
            if k=="bits-differ": ex.setdefault(hex(a-0x20100000),[]).append((f"{src}:{n}",wa,wb))
json.dump({"counts":{hex(o):dict(c) for o,c in res.items()},"differ":ex},open("diff/field-check.json","w"),indent=0)
print("registers",len(res),"with bits-differ",len(ex))
