import json,collections,sys,re,glob
sys.argv=["x","","",""]
exec(open("bodycmp.py").read().split("old, new, tag")[0])
MEM=re.compile(r"^(lw|sw|lh|lhu|lb|lbu|sh|sb|addi) (\w+), (?:(\w+), )?(-?0x[0-9a-f]+|-?\d+)(?:\((\w+)\))?$")
def mmio_accesses(ins, lo, hi):
    """(index, abs address, op) for loads/stores/addis based on a lui in [lo,hi) page range (straight-line approx)."""
    regs={}; out=[]
    for i,x in enumerate(ins):
        p=x.split(" ",1); op=p[0]
        if op=="lui":
            r,imm=[s.strip() for s in p[1].split(",")]
            v=int(imm,0)<<12
            if lo<=v<hi: regs[r]=v
            else: regs.pop(r,None)
            continue
        m=MEM.match(x)
        if m:
            o,rd,rs1,off,base=m.groups()
            b = base if base else rs1
            if b in regs:
                out.append((i,regs[b]+int(off,0),o))
            if o in("lw","lh","lhu","lb","lbu") or o=="addi":
                if o=="addi" and b in regs: regs[rd]=regs[b]+int(off,0)
                else: regs.pop(rd,None)
        elif p[0] in ("jal","jalr","ret","j","call","tail"):
            pass
        else:
            d=p[1].split(",")[0].strip() if len(p)>1 else None
            if d: regs.pop(d,None)
    return out
S="../../../open-esp-radio-rs/target/vendor/"; V="../vendor/"
libs={"libpp":"esp32-wifi-lib/af55a0ca258ce9d791d1661d7c2bbb65f08c0c21","libnet80211":"esp32-wifi-lib/af55a0ca258ce9d791d1661d7c2bbb65f08c0c21",
"libcoexist":"esp-coex-lib/c758e7b56e0fa22177a0539796e1df59978dc322","libphy":"esp-phy-lib/20f1db053a0e6cb9f1c09d255c43bf42483041d0",
"librftest":"esp-phy-lib/20f1db053a0e6cb9f1c09d255c43bf42483041d0","libbtbb":"esp-phy-lib/20f1db053a0e6cb9f1c09d255c43bf42483041d0"}
report={}
for lib,d in libs.items():
    fa,fb=functions(f"{S}{d}/esp32s31/{lib}.a"),functions(f"{V}{d}/esp32c5/{lib}.a")
    cl=json.load(open(f"diff/{lib}.body.json"))["classes"]
    same=moved=0; mv=[]
    regs_s31=set()
    for n in cl:
        a=mmio_accesses(fa[n],0x20100000,0x20110000); b=mmio_accesses(fb[n],0x600a0000,0x600b0000)
        regs_s31|={x[1] for x in a}
        if cl[n]=="structural" or not a: continue
        ra=[x[1]-0x20100000 for x in a]; rb=[x[1]-0x600a0000 for x in b]
        if ra==rb: same+=1
        else: moved+=1; mv.append((n,[(hex(x),hex(y)) for x,y in zip(ra,rb) if x!=y][:4]))
    print(f"{lib}: mmio-functions same-offsets={same} moved-offsets={moved} distinct-s31-modem-regs={len(regs_s31)}")
    for m in mv[:6]: print("   ",m)
    report[lib]=mv
json.dump(report,open("diff/mmio-moved.json","w"),indent=1)
