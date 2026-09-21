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
{ r0 = ##0x00000200 }
{ r1 = ##0xc8d860b7 }
{ r2 = ##0x00010000 }
{ r3 = ##0x80000000 }
{ r4 = ##0x7fffffff }
{ r5 = ##0xffffff80 }
{ r6 = ##0xcba22bff }
{ r7 = ##0x00000001 }
{ r8 = ##0xc17d15b4 }
{ r9 = ##0x7fffffff }
{ r10 = ##0xfa0000fa }
{ r11 = ##0xffff7fff }
{ r12 = ##0x00007fff }
{ r13 = ##0x000000ff }
{ r14 = ##0xfa0000fa }
{ r15 = ##0x65435e12 }
{ r16 = ##0x0b5e95d7 }
{ r17 = ##0x00000040 }
{ r18 = ##0x269b8e0c }
{ r19 = ##0x092f987f }
{ r20 = ##0x816df507 }
{ r21 = ##0x78f2c2ac }
{ r22 = ##0x00000400 }
{ r23 = ##0xc15535e9 }
{ r24 = ##0x00010000 }
{ r25 = ##0x00008000 }
{ r26 = ##0xffffff7f }
{ r27 = ##0x00000100 }
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
{ allocframe(#8) }
{ loop1(.Lho_0, #4) }
.Lho_0:
{ loop0(.Lhi_0, #4) }
.Lhi_0:
{ nop }:endloop01
.Lcf_4:
.Lcf_5:
.Lcf_11:
.Lcf_13:
    { r5:4 = vtrunewh(r27:26,r11:10) }
    { r17:16 += mpy(r9.h,r1.h) ; r15 = asl(r5,r5):sat ; r21:20 = membh(r27+#952) }
.Lcf_17:
    { r4 |= lsl(r17,r17) ; p3 = cmpb.eq(r15,r2) ; r13:12 = memubh(r27+#784) }
    { r17:16 = vlslw(r27:26,r13) ; r14 &= asl(r8,#31) ; p2 = !cmp.gtu(r1,r24) }
    { r17:16 = add(r25:24,r23:22,p1):carry ; p2 = tlbmatch(r15:14,r16) ; p3 = cmp.gtu(r9,#342) ; memh(r27+#54) = #99 }
.Lcf_28:
{ loop1(.Lhl_1, #4) }
.Lhl_1:
{ nop }:endloop1
.Lcf_29:
{ loop1(.Lhl_2, #3) }
.Lhl_2:
    { r10 = cmp.eq(r14,#83) ; r19:18 &= asr(r1:0,r2) ; r9 -= mpy(r16.l,r19.l):<<1:sat }:endloop1
{ r25 = #2 }
.Lsl_3:
{ r25 = add(r25,#-1) }
{ loop1(.Lhl_4, #1) }
.Lhl_4:
{ nop }:endloop1
.Lcf_48:
.Lcf_53:
.Lcf_56:
.globl test_end
test_end:
{ dealloc_return }
.Lfn_0:
    { allocframe(#8) }
    { r5:4 = bitsplit(r0,#1) ; r17:16 += mpyu(r6.h,r21.h):<<1 }
    { if (p1) dealloc_return }
    { dealloc_return }
.Lfn_1:
    { allocframe(#8) }
    { r4 -= asl(r8,#29) ; p1 = cmp.gt(r15:14,r5:4) ; r1 = vavgh(r8,r14) ; if (p2) r19 = sxth(r7) }
    { r5:4 = and(r5:4,~r15:14) ; r17 = cmpyiwh(r17:16,r23):<<1:rnd:sat ; r8 = zxth(r0) ; r24 = vaddh(r20,r18):sat }
    { dealloc_return }
.data
.balign 4096
.globl mem_region
mem_region:
    .set lcg, 0x2545f491
    .rept 16384
    .word lcg
    .set lcg, (lcg * 1664525 + 1013904223) & 0xffffffff
    .endr