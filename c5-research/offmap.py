import json,collections,sys
exec(open("mmio.py").read().split("report={}")[0])
pairs=collections.defaultdict(collections.Counter)
for lib,d in libs.items():
    fa,fb=functions(f"{S}{d}/esp32s31/{lib}.a"),functions(f"{V}{d}/esp32c5/{lib}.a")
    cl=json.load(open(f"diff/{lib}.body.json"))["classes"]
    for n in cl:
        if cl[n]=="structural": continue
        a=mmio_accesses(fa[n],0x20100000,0x20110000); b=mmio_accesses(fb[n],0x600a0000,0x600b0000)
        if len(a)!=len(b): continue
        for x,y in zip(a,b): pairs[x[1]-0x20100000][y[1]-0x600a0000]+=1
conf=[o for o,c in pairs.items() if len(c)>1]
print("offsets",len(pairs),"conflicting",len(conf), [ (hex(o),dict((hex(k),v) for k,v in pairs[o].items())) for o in conf[:5]])
# segments by delta
seg=[];
for o in sorted(pairs):
    d=pairs[o].most_common(1)[0][0]-o
    if seg and seg[-1][2]==d: seg[-1][1]=o; seg[-1][3]+=1
    else: seg.append([o,o,d,1])
for s,e,d,n in seg: print(f"  s31 {s:#06x}..{e:#06x}  delta {d:+#x}  ({n} regs)")
json.dump({hex(o):{hex(k):v for k,v in c.items()} for o,c in sorted(pairs.items())},open("diff/modem-offset-map.json","w"),indent=1)
