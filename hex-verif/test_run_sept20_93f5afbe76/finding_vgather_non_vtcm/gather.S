// gather to ordinary RAM (not VTCM). Hardware/hexagon-sim raise exception 0x26.
.globl main
.text
main:
{ r26 = ##buf }
{ r7 = #4095 }
{ m0 = r7 }
{ v30 = vxor(v30,v30) }
{ vtmp.h = vgather(r26,m0,v30.h).h ; vmem(r26+#3) = vtmp.new }
{ r0 = #0
  jumpr r31 }
.data
.balign 4096
buf:
.space 16384
