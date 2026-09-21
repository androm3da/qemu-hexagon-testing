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
{ r0 = ##0xd980024e }
{ r1 = ##0x00000020 }
{ r2 = ##0x00000100 }
{ r3 = ##0x56d795d2 }
{ r4 = ##0x000000ff }
{ r5 = ##0x0000ffff }
{ r6 = ##0xffffffff }
{ r7 = ##0xd34928db }
{ r8 = ##0x00000010 }
{ r9 = ##0x6ebf0c43 }
{ r10 = ##0xffff7fff }
{ r11 = ##0x0000007f }
{ r12 = ##0x956d24c8 }
{ r13 = ##0x00000400 }
{ r14 = ##0x00001000 }
{ r15 = ##0xffffff80 }
{ r16 = ##0x00000400 }
{ r17 = ##0xdf582cac }
{ r18 = ##0xaa33fc11 }
{ r19 = ##0xffff8000 }
{ r20 = ##0x0d5e79f1 }
{ r21 = ##0xe8b5d910 }
{ r22 = ##0xa26788b5 }
{ r23 = ##0x91dbd4c6 }
{ r24 = ##0x00000020 }
{ r25 = ##0xffffff7f }
{ r26 = ##0x28523e4f }
{ r27 = ##0x7309d885 }
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
.Lcf_5:
{ r25 = #3 }
.Lsl_0:
{ r25 = add(r25,#-1) }
{ p0 = cmp.gt(r25,#0) ; if (p0.new) jump:t .Lsl_0 }
.Lcf_6:
.Lcf_7:
    { r24 = mpy(r15,r25) ; r15:14 |= lsl(r25:24,r3) ; if (!p0) r11 = add(r23,r17) ; p1 = cmp.gt(r10,#381) }
    { r0 = convert_df2w(r19:18):chop ; r15:14 = vmpywoh(r13:12,r19:18):<<1:rnd:sat ; r3 = combine(r2.l,r13.l) ; r11 = xor(r14,r19) }
.Lcf_14:
    { r21:20 = vavguh(r25:24,r9:8) ; r4 = add(r1,r3):sat ; r15:14 = mpy(r25.h,r12.l):<<1 }
    { r21:20 += cmpy(r6,r14*):sat ; r13 = vasrhub(r7:6,#7):raw ; r19 = !cmp.eq(r3,#-64) }
.Lcf_23:
.Lcf_26:
{ loop1(.Lho_1, #4) }
.Lho_1:
{ loop0(.Lhi_1, #2) }
.Lhi_1:
{ nop }:endloop01
.Lcf_38:
    { r19 -= mpyu(r25.h,r19.h):<<1 ; r24 += mpy(r11.h,r27.l):<<1:sat ; r15 = aslh(r19) ; r1 = cmp.eq(r16,r12) }
    { r1:0 += vrcmpyi(r19:18,r9:8) ; r15:14 = dfsub(r25:24,r15:14) ; r5 = vadduh(r13,r1):sat }
    { p2 = cmpb.gtu(r5,r14) ; p1 = tlbmatch(r15:14,r24) ; r16 = vadduh(r17,r3):sat }
{ loop1(.Lho_2, #1) }
.Lho_2:
{ loop0(.Lhi_2, #4) }
.Lhi_2:
{ nop }:endloop01
.Lcf_44:
.Lcf_49:
.Lcf_50:
.Lcf_51:
.Lcf_53:
.globl test_end
test_end:
{ dealloc_return }
.Lfn_0:
    { allocframe(#8) }
    { r22 = togglebit(r0,r15) ; r7:6 -= lsr(r9:8,r1) ; p2 = !cmp.gt(r24,r17) }
    { r15:14 = brev(r19:18) ; r6 = sub(r22.l,r1.l):<<16 ; r10 = sub(#-214,r17) }
    { r19:18 = vmpywoh(r7:6,r1:0):rnd:sat ; r23:22 = vaslh(r13:12,r24) ; nop ; if (p1) r3 = sxth(r22) }
    { if (p3) dealloc_return }
    { dealloc_return }
.Lfn_1:
    { allocframe(#8) }
    { r12 = xor(r2,r8) ; r3:2 += cmpy(r5,r21*):sat ; r11:10 = vrmpyweh(r15:14,r21:20):<<1 }
    { r11:10 = mpy(r15.l,r26.l):rnd ; r4 -= add(r4,#-30) ; if (p3) r13 = and(r8,r23) ; if (!p0) r16 = sxtb(r0) }
    { r22 |= lsr(r18,r2) ; r21:20 += vmpyhsu(r27,r25):sat ; p2 = cmp.gtu(r21,#463) ; r4 = vadduh(r20,r11):sat }
    { dealloc_return }
.Lfn_2:
    { p0 = dfcmp.eq(r25:24,r3:2) ; if (!p3) r10 = sxtb(r17) }
    { r7:6 += vmpyh(r18,r25):<<1:sat ; r2 = cround(r22,r13) ; r23:22 = combine(#-24,#20) }
    { if (!p0) jumpr:nt r31 }
    { r9:8 = vmpyweh(r7:6,r13:12):<<1:sat }
    { if (p0) memb(r27+#58) = #27 ; r15:14 -= mpyu(r13.l,r18.h):<<1 ; r11:10 += vrcmpyr(r9:8,r21:20) ; r1:0 = combine(#99,#-61) }
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