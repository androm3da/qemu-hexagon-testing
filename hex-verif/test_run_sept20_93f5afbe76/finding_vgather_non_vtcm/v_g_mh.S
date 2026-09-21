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
{ vtmp.h = vgather(r26,m0,v30.h).h ; vmem(r26+#3) = vtmp.new }
{ v4 = vmem(r26+#3) }
{ r0 = #0
  jumpr r31 }
.data
.balign 4096
buf:
.space 16384
