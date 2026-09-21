// scatter to ordinary RAM (not VTCM). Hardware/hexagon-sim raise exception 0x26.
.globl main
.text
main:
{ r26 = ##buf }
{ r7 = #4095 }
{ m0 = r7 }
{ v30 = vxor(v30,v30) }
{ vscatter(r26,m0,v30.h).h = v5 }
{ vmem(r26+#0):scatter_release }
{ r0 = #0
  jumpr r31 }
.data
.balign 4096
buf:
.space 16384
