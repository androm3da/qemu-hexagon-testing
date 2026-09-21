#!/usr/bin/env python3
"""Greedy-reduce a generated test.S whose 'steps' body raises 'Invalid packet' (0x15)
on QEMU, to the minimal set of packets in `steps` that still does."""
import re, subprocess, sys, tempfile, os
CL="/opt/Hexagon_SDK/6.4.0.2/tools/HEXAGON_Tools/19.0.04/Tools/bin/hexagon-clang"
SDK="/opt/Hexagon_SDK/6.4.0.2/tools/Tools/QEMUHexagon/bin/qemu-system-hexagon"
INT="/home/brian/src/qemu-integ/build_integ/qemu-system-hexagon"
def run(q, asm, hvx=False):
    with tempfile.TemporaryDirectory() as d:
        s=f"{d}/t.S"; open(s,"w").write(asm)
        c=[CL,"-O2","-G0","-mv73"]+(["-mhvx"] if hvx else [])+[s,"-o",f"{d}/t.elf"]
        if subprocess.run(c,capture_output=True).returncode: return None
        try: o=subprocess.run([q,"-machine","V81QA_1","-kernel",f"{d}/t.elf","-nographic","-display","none"],capture_output=True,text=True,timeout=20)
        except subprocess.TimeoutExpired: return "timeout"
        m=re.search(r"exception was: (0x[0-9a-f]+), ([^\n]*)",o.stdout+o.stderr)
        return m.group(0) if m else f"rc={o.returncode}"
def split(asm):
    lines=asm.split("\n"); i=lines.index("steps:")+1
    j=next(k for k in range(i,len(lines)) if lines[k].startswith(".globl test_end"))
    body=[l for l in lines[i:j] if l.strip()]
    # packets: braces may span lines
    pk=[];cur=[]
    for l in body:
        cur.append(l)
        if "}" in l: pk.append("\n".join(cur));cur=[]
    return lines[:i],pk,lines[j:]
def build(h,p,t): return "\n".join(h+p+t)
asm=open(sys.argv[1]).read(); hvx="-mhvx" in sys.argv
h,pk,t=split(asm)
base=run(SDK,build(h,pk,t),hvx)
print("baseline SDK:",base)
target=base
i=0
while i<len(pk):
    trial=pk[:i]+pk[i+1:]
    if run(SDK,build(h,trial,t),hvx)==target: pk=trial
    else: i+=1
print("MINIMAL packets:"); print("\n".join(pk))
print("SDK  :",run(SDK,build(h,pk,t),hvx)); print("integ:",run(INT,build(h,pk,t),hvx))
open(sys.argv[1].replace("test.S","minimized_invalid.S"),"w").write(build(h,pk,t))
