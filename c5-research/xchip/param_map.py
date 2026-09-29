"""Readers and writers of each phy_param offset in the C5 libphy (static)."""
import re, subprocess, os, collections, json
lib="../../vendor/esp-phy-lib/20f1db053a0e6cb9f1c09d255c43bf42483041d0/esp32c5/libphy.a"
tmp="c5o"
acc=collections.defaultdict(lambda: {"r":set(),"w":set()})
LOAD={"lb":1,"lbu":1,"lh":2,"lhu":2,"lw":4}; STORE={"sb":1,"sh":2,"sw":4}
for o in sorted(os.listdir(tmp)):
    if not o.endswith(".o"): continue
    dis=subprocess.run(["llvm-objdump","-dr","--no-show-raw-insn","--mattr=+c,+m,+a",os.path.join(tmp,o)],capture_output=True,text=True).stdout
    fn=None; base={}; last=None
    for l in dis.splitlines():
        m=re.match(r"^[0-9a-f]+ <([^>]+)>:",l)
        if m:
            if not m.group(1).startswith(".L"): fn=m.group(1); base={}
            continue
        m=re.match(r"\s*[0-9a-f]+:\s+(R_RISCV_\w+)\s+phy_param([+-]0x[0-9a-f]+)?",l)
        if m and last:
            add=int(m.group(2),16) if m.group(2) else 0
            op,args=last
            regs=[a.strip() for a in args.split(",")]
            if op=="lui": base[regs[0]]=("hi",add)
            elif op in ("addi","mv"): base[regs[0]]=("full",add)
            elif op in LOAD or op in STORE:
                w=LOAD.get(op) or STORE.get(op)
                acc[(add,w)]["r" if op in LOAD else "w"].add(fn)
            last=None; continue
        m=re.match(r"\s*[0-9a-f]+:\s+(\S+)\s*(.*)",l)
        if not m or not fn: continue
        op,args=m.group(1),re.sub(r"\s*<[^>]*>","",m.group(2))
        last=(op,args)
        mm=re.match(r"(\w+), (-?0x[0-9a-f]+|-?\d+)\((\w+)\)",args)
        if (op in LOAD or op in STORE) and mm and mm.group(3) in base and base[mm.group(3)][0]=="full":
            off=base[mm.group(3)][1]+int(mm.group(2),0); w=LOAD.get(op) or STORE.get(op)
            acc[(off,w)]["r" if op in LOAD else "w"].add(fn)
        if op=="mv":
            d,sr=[a.strip() for a in args.split(",")]
            if sr in base and base[sr][0]=="full": base[d]=base[sr]
        elif args.split(",")[0].strip() in base and op not in STORE and not (op in("addi","mv")):
            base.pop(args.split(",")[0].strip(),None)
rows=sorted(acc.items())
with open("phy_param_map.txt","w") as f:
    f.write("offset width | writers | readers\n")
    for (off,w),d in rows:
        f.write(f"0x{off:03x} {w} | W: {', '.join(sorted(d['w'])) or '-'} | R: {', '.join(sorted(d['r'])) or '-'}\n")
print(len(rows),"offset/width entries;", sum(1 for _,d in rows if d['w']),"written")
