import json,collections,re,sys
exec(open("mmio.py").read().split("report={}")[0])
MEMOP=re.compile(r"^(lw|sw|lh|lhu|lb|lbu|sh|sb) (\w+), (-?0x[0-9a-f]+|-?\d+)\((\w+)\)$")
def num(s): return int(s,0)
def indexed(ins, lo, hi):
    R={}; out=[]
    for idx,x in enumerate(ins):
        p=x.split(" ",1); op=p[0]; a=[s.strip() for s in p[1].split(",")] if len(p)>1 else []
        m=MEMOP.match(x)
        if m:
            o,rd,off,b=m.groups(); v=R.get(b)
            if v and v[0]=="p" and lo<=v[1]<hi: out.append((v[1]+num(off),v[2],o,idx))
            if o[0]=="l": R.pop(rd,None)
            continue
        if not a: continue
        rd=a[0]; new=None
        try:
            if op=="lui": new=("c",num(a[1])<<12 & 0xffffffff)
            elif op=="li": new=("c",num(a[1]))
            elif op=="mv": new=R.get(a[1])
            elif op=="addi":
                v=R.get(a[1])
                if v and v[0]=="c": new=("c",(v[1]+num(a[2]))&0xffffffff)
                elif v and v[0]=="p": new=("p",v[1]+num(a[2]),v[2])
            elif op=="mul":
                x1,x2=R.get(a[1]),R.get(a[2])
                if x2 and x2[0]=="c" and not(x1 and x1[0]=="c"): new=("s",x2[1] if x2[1]<0x80000000 else x2[1]-(1<<32))
                elif x1 and x1[0]=="c" and not(x2 and x2[0]=="c"): new=("s",x1[1] if x1[1]<0x80000000 else x1[1]-(1<<32))
            elif op=="slli":
                x1=R.get(a[1])
                if not x1: new=("s",1<<num(a[2]))
                elif x1[0]=="s": new=("s",x1[1]<<num(a[2]))
            elif op in("add","sub"):
                x1,x2=R.get(a[1]),R.get(a[2])
                if op=="sub" and x2 and x2[0]=="s": x2=("s",-x2[1])
                for u,w in ((x1,x2),(x2,x1)):
                    if u and w and u[0]=="c" and w[0]=="s": new=("p",u[1],w[1])
                    elif u and w and u[0]=="p" and w[0]=="c": new=("p",u[1]+w[1],u[2])
        except Exception: new=None
        if new: R[rd]=new
        else: R.pop(rd,None)
    return out
pairs=collections.defaultdict(collections.Counter); nf=0; mism=[]
import difflib
IMMR=re.compile(r"-?0x[0-9a-f]+|-?\b\d+\b")
def align(A,B):
    sm=difflib.SequenceMatcher(None,[IMMR.sub("#",x) for x in A],[IMMR.sub("#",x) for x in B],autojunk=False)
    m={}
    for a,b,n in sm.get_matching_blocks():
        for k in range(n): m[a+k]=b+k
    return m
for lib in ("libpp","libnet80211"):
    d=libs[lib]; fa,fb=functions(f"{S}{d}/esp32s31/{lib}.a"),functions(f"{V}{d}/esp32c5/{lib}.a")
    for n in set(fa)&set(fb):
        a=indexed(fa[n],0x20100000,0x20110000); b=indexed(fb[n],0x600a0000,0x600b0000)
        if not a and not b: continue
        nf+=1; m=align(fa[n],fb[n]); bi={y[3]:y for y in b}; un=0
        for x in a:
            y=bi.get(m.get(x[3]))
            if y and y[2]==x[2]: pairs[(x[0]-0x20100000,x[1])][(y[0]-0x600a0000,y[1])]+=1
            else: un+=1
        if un: mism.append((n,un))
print("functions",nf,"with unaligned accesses",len(mism),mism[:20])
for k in sorted(pairs): print(f"s31 {k[0]:#x} step {k[1]:#x} -> "+", ".join(f"c5 {c[0]:#x} step {c[1]:#x} x{v}" for c,v in pairs[k].items()))
json.dump([[k[0],k[1],c[0],c[1],v] for k in pairs for c,v in pairs[k].items()],open("diff/indexed-map.json","w"))
