// Auto-generated Hexagon test program
// Do not edit manually

.text
.globl main
.type main, @function
main:
    { allocframe(#64) }
    { memd(r29+#-8) = r17:16 }
    { memd(r29+#-16) = r19:18 }
    { memd(r29+#-24) = r21:20 }
    { memd(r29+#-32) = r23:22 }
    { memd(r29+#-40) = r25:24 }
    { memd(r29+#-48) = r27:26 }
    { call init }
    { call body }
    { r17:16 = memd(r29+#-8) }
    { r19:18 = memd(r29+#-16) }
    { r21:20 = memd(r29+#-24) }
    { r23:22 = memd(r29+#-32) }
    { r25:24 = memd(r29+#-40) }
    { r27:26 = memd(r29+#-48) }
    { dealloc_return }

.globl init
.type init, @function
init:
{ r0 = ##0x00000040 }
{ r1 = ##0x00000001 }
{ r2 = ##0xd58707cb }
{ r3 = ##0x2b9ce13b }
{ r4 = ##0x2d6e6f8f }
{ r5 = ##0x00000100 }
{ r6 = ##0x0000007f }
{ r7 = ##0x78315a6f }
{ r8 = ##0x00008000 }
{ r9 = ##0x00000200 }
{ r10 = ##0x0000ffff }
{ r11 = ##0x0000ffff }
{ r12 = ##0x309ef412 }
{ r13 = ##0x00008000 }
{ r14 = ##0x00008000 }
{ r15 = ##0x9baa47b9 }
{ r16 = ##0x00008000 }
{ r17 = ##0x24d0632a }
{ r18 = ##0x00000010 }
{ r19 = ##0x9c5526e4 }
{ r20 = ##0x4f50f5a3 }
{ r21 = ##0xffffff7f }
{ r22 = ##0xbd613dca }
{ r23 = ##0x00007fff }
{ r24 = ##0x17e472dc }
{ r25 = ##0x19f568a5 }
{ r26 = ##0xc4afe487 }
{ r27 = ##0x5ded2b41 }
{ p0 = cmp.eq(r0,r0) }
    { p1 = cmp.gt(r0,r1) }
    { p2 = cmp.eq(r2,r3) }
    { p3 = cmp.gt(r4,r5) }
    { r28 = #3 }
    { r27 = ##mem_region }
{ jumpr r31 }

.globl body
.type body, @function
body:
    { call steps }
    { r28 = add(r28,#-1) }
    { p0 = cmp.gt(r28,#0) }
    { if (p0) jump body }
    { jumpr r31 }

.globl steps
.type steps, @function
steps:
    { r19:18 = vavgw(r27:26,r5:4) ; r9:8 = vmaxuw(r25:24,r7:6) ; if (p3) r21:20 = combine(r21,r9) ; r27:26 = combine(#-16,#25) }
    { r11:10 = vaslw(r21:20,#29) }
    { r5:4 -= mpyu(r27.l,r6.l) ; p2 = tlbmatch(r11:10,r19) ; r23:22 = combine(#-29,#40) }
.globl test_end
test_end:
{ jumpr r31 }
.data
.balign 4096
.globl mem_region
mem_region:
    .set lcg, 0x2545f491
    .rept 16384
    .word lcg
    .set lcg, (lcg * 1664525 + 1013904223) & 0xffffffff
    .endr