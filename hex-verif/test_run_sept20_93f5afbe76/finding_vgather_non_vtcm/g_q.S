.globl main
.text
main:
{ r26 = ##buf }
{ r6 = ##0x0ffe0ffe }
{ r7 = #4095 }
{ m0 = r7 }
{ v31 = vsplat(r6) }
{ v30 = vand(v3,v31) }
{ if (q1) vtmp.w = vgather(r26,m0,v30.w).w ; vmem(r26+#-2) = vtmp.new }
{ r0 = #0
  jumpr r31 }
.data
.balign 4096
buf:
.space 16384
