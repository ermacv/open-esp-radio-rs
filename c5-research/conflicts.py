import json,collections,sys
exec(open("evidence2.py").read().split("off=collections")[0])
want={0x2014,0x4db8,0x890,0x464,0x468,0x46c,0x2c34,0x2c2c}
# also same-lib pairs
W="esp32-wifi-lib/af55a0ca258ce9d791d1661d7c2bbb65f08c0c21"
for l in ["libpp"]:
    fa,fb=functions(f"{S}{W}/esp32s31/{l}.a"),functions(f"{V}{W}/esp32c5/{l}.a")
    for n in set(fa)&set(fb): pairs.append(("pp",n,fa[n],fb[n]))
hits=collections.defaultdict(list)
for src,n,a,b in pairs:
    ma=mmio_accesses(a,0x20100000,0x20110000); mb=mmio_accesses(b,0x600a0000,0x600b0000)
    if not ma or len(ma)!=len(mb) or [x[2] for x in ma]!=[x[2] for x in mb]: continue
    same=a==b or len(a)==len(b)
    for x,y in zip(ma,mb):
        o=x[1]-0x20100000
        if o in want: hits[o].append((hex(y[1]-0x600a0000),src,n,"samelen" if len(a)==len(b) else f"len {len(a)}/{len(b)}"))
for o in sorted(hits):
    print(hex(o)); [print("   ",h) for h in sorted(set(hits[o]))]
