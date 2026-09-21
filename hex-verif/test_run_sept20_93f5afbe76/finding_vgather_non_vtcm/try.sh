#!/usr/bin/env bash
# usage: try.sh name 'asm body (after setup)'
n=$1; body=$2
cat > $n.S <<EOF
.globl main
.text
main:
{ r26 = ##0xd9000000 }
{ r6 = ##0x03fe03fe }
{ r7 = #1023 }
{ m0 = r7 }
{ v31 = vsplat(r6) }
{ v30 = vand(v3,v31) }
{ vmem(r26+#0) = v1 }
{ vmem(r26+#1) = v2 }
{ vmem(r26+#2) = v3 }
$body
{ r0 = #0
  jumpr r31 }
.data
.balign 4096
buf:
.space 16384
EOF
TB=/opt/Hexagon_SDK/6.4.0.2/tools/HEXAGON_Tools/19.0.04/Tools/bin
$TB/hexagon-clang -mv73 -mhvx -O2 -G0 $n.S -o $n.elf || exit 1
r() { grep -m1 'exception was' | sed 's/I think the exception was: //' ; }
s=$(timeout 60 $TB/hexagon-sim --mv73 $n.elf 2>&1 | r); a=$(timeout 15 /opt/Hexagon_SDK/6.4.0.2/tools/Tools/QEMUHexagon/bin/qemu-system-hexagon -machine V81QA_1 -kernel $n.elf -nographic -display none 2>&1 </dev/null | r); b=$(timeout 15 /home/brian/src/qemu-integ/build_integ/qemu-system-hexagon -machine V81QA_1 -kernel $n.elf -nographic -display none 2>&1 </dev/null | r)
printf '%-14s sim=[%s] sdk=[%s] integ=[%s]\n' "$n" "${s:-none}" "${a:-none}" "${b:-none}"
