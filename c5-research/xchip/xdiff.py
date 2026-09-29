"""Cross-chip semantic comparison of RV32 vendor functions.

Inputs: C5 libphy.a (and optionally C5 ROM) vs S31 ROM ELF + S31 libphy.a.
Each function becomes a token list of normalized instructions: aliases
expanded by llvm-objdump's default printing, prologue/epilogue stack traffic
and cm.* push/pop dropped, addresses and call targets symbolized, the S31
modem window 0x2010_xxxx mapped onto the C5 window 0x600a_xxxx.
"""
import re, subprocess, sys, json, difflib, collections, os
def run(*a): return subprocess.run(a, capture_output=True, text=True).stdout

def functions_elf(path):
    """Functions of a linked ELF from its symbol table."""
    syms=[]
    for l in run("llvm-nm","-S","--defined-only",path).splitlines():
        p=l.split()
        if len(p)==4 and p[2] in "Tt" and int(p[1],16)>0: syms.append((int(p[0],16),int(p[1],16),p[3]))
    addr2name={a:n for a,_,n in syms}
    out={}
    dis=run("llvm-objdump","-d","--no-show-raw-insn","--mattr=+c,+m,+a,+zba,+zbb,+zbs,+zcb,+zcmp,+f",path)
    cur=None; body=collections.defaultdict(list)
    ranges=sorted(syms)
    import bisect
    starts=[r[0] for r in ranges]
    for l in dis.splitlines():
        m=re.match(r"\s*([0-9a-f]+):\s+(\S+)\s*(.*)",l)
        if not m: continue
        a=int(m.group(1),16); i=bisect.bisect_right(starts,a)-1
        if i<0: continue
        s,sz,n=ranges[i]
        if a>=s+sz: continue
        body[n].append((m.group(2),m.group(3)))
    return {n:(b,addr2name) for n,b in body.items()}

def functions_archive(path, tmp):
    os.makedirs(tmp,exist_ok=True)
    subprocess.run(["ar","x",os.path.abspath(path)],cwd=tmp)
    out={}
    for o in sorted(os.listdir(tmp)):
        if not o.endswith(".o"): continue
        dis=run("llvm-objdump","-dr","--no-show-raw-insn","--mattr=+c,+m,+a,+zba,+zbb,+zbs,+zcb,+zcmp,+f",os.path.join(tmp,o))
        cur=None; pending=None
        for l in dis.splitlines():
            m=re.match(r"^[0-9a-f]+ <([^>]+)>:",l)
            if m:
                n=m.group(1)
                if n.startswith(".L"): continue
                cur=n; out[cur]=([],{},o); continue
            m=re.match(r"\s*[0-9a-f]+:\s+(R_RISCV_\w+)\s+(\S+)",l)
            if m and cur:
                if m.group(1) in ("R_RISCV_CALL","R_RISCV_CALL_PLT","R_RISCV_JAL","R_RISCV_HI20","R_RISCV_LO12_I","R_RISCV_LO12_S"):
                    ins=out[cur][0]
                    if ins: ins[-1]=(ins[-1][0],ins[-1][1],re.sub(r"[+-]0x[0-9a-f]+$","",m.group(2)))
                continue
            m=re.match(r"\s*([0-9a-f]+):\s+(\S+)\s*(.*)",l)
            if m and cur: out[cur][0].append((m.group(2),m.group(3),None))
    return out

SAVE=re.compile(r"^(ra|s\d+|s1[01]|fp), (-?0x[0-9a-f]+|-?\d+)\(sp\)$")
def normalize(ins, addr2name=None):
    toks=[]; carried=None
    for item in ins:
        op,args=item[0],item[1]; sym=item[2] if len(item)>2 else None
        if op=="auipc":
            carried=sym; continue
        if op in("jalr","jr") and sym is None and carried: sym=carried
        if op not in("jalr","jr"): carried=None
        args=re.sub(r"\s*<[^>]*>","",args).strip()
        if op.startswith("cm.") or (op=="addi" and args.startswith("sp, sp,")): continue
        if op in("sw","lw","c.swsp","c.lwsp") and SAVE.match(args): continue
        if op in("ret","jr") and args in("","ra"): toks.append("ret"); continue
        if op=="jr" and sym: toks.append("jump "+sym); continue
        # call / jal / auipc-jalr pairs
        if op in("auipc",) : continue
        if op in ("jalr",) and "ra" in args.split(",")[0]:
            toks.append("call "+(sym or "?")); continue
        if op in("jal","j","c.j","c.jal","tail"):
            t=sym
            if t is None and addr2name is not None:
                mm=re.search(r"(0x[0-9a-f]+)$",args)
                if mm: t=addr2name.get(int(mm.group(1),16))
            toks.append(("call " if op in("jal","c.jal") else "jump ")+(t or "LOCAL")); continue
        if op.startswith("b") and op not in ("bset","bclr","binv","bext"):
            regs=",".join(a for a in args.split(", ")[:-1]); toks.append(f"{op} {regs}"); continue
        if op=="lui":
            r,imm=args.split(", "); v=int(imm,16)<<12
            if 0x20100000<=v<0x20110000: v=v-0x20100000+0x600a0000
            toks.append(f"lui {r},{sym or hex(v)}"); continue
        if sym: args=re.sub(r"-?0x[0-9a-f]+(\(\w+\))?$",lambda m:"SYM"+(m.group(1) or ""),args)
        toks.append(f"{op} {args}")
    return alpha(toks)

REG=re.compile(r"\b(a[0-7]|s[0-9]|s1[01]|t[0-6])\b")
def alpha(toks):
    """Rename registers by order of first use, keeping argument registers
    a0..a7 at entry distinct from temporaries."""
    names={}
    def sub(m):
        r=m.group(1)
        if r not in names: names[r]=f"r{len(names)}"
        return names[r]
    return [REG.sub(sub,t) for t in toks]

def main():
    c5_lib, s31_rom, s31_lib, out = sys.argv[1:5]
    c5=functions_archive(c5_lib,out+"/c5o")
    s31=functions_archive(s31_lib,out+"/s31o")
    rom=functions_elf(s31_rom)
    rows=[]
    for name,(ins,_,obj) in sorted(c5.items()):
        a=normalize(ins)
        cands=[]
        if name in s31: cands.append(("s31-libphy",normalize(s31[name][0])))
        for alt in (name, name+"_new", name.rstrip("_") , re.sub(r"_new$","",name)):
            if alt in rom: cands.append((f"s31-rom:{alt}",normalize(rom[alt][0],rom[alt][1]))); break
        best=None
        for src,b in cands:
            r=difflib.SequenceMatcher(None,a,b,autojunk=False).ratio()
            if best is None or r>best[1]: best=(src,r,len(a),len(b))
        rows.append((name,obj,len(a),best))
    json.dump(rows,open(out+"/xdiff.json","w"),indent=0)
    cls=collections.Counter()
    for n,o,l,b in rows:
        k="absent" if b is None else ("identical" if b[1]==1 else "near>=0.9" if b[1]>=0.9 else "similar>=0.6" if b[1]>=0.6 else "different")
        cls[k]+=1
    print(dict(cls))
main()
