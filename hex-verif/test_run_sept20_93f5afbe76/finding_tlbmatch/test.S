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
{ r0 = ##0x561f8156 }
{ r1 = ##0xf7cbd3cd }
{ r2 = ##0x00008000 }
{ r3 = ##0x00000001 }
{ r4 = ##0x53905156 }
{ r5 = ##0x8aeab844 }
{ r6 = ##0x438d6159 }
{ r7 = ##0x97e49187 }
{ r8 = ##0x048fba10 }
{ r9 = ##0x00000100 }
{ r10 = ##0xffff7fff }
{ r11 = ##0xffff8000 }
{ r12 = ##0xf3fc775a }
{ r13 = ##0x00000010 }
{ r14 = ##0x00000400 }
{ r15 = ##0x6dc6e77b }
{ r16 = ##0x1d135dce }
{ r17 = ##0x00000010 }
{ r18 = ##0x00000000 }
{ r19 = ##0x00000100 }
{ r20 = ##0xb0b299db }
{ r21 = ##0x3e59c1be }
{ r22 = ##0x6483dc90 }
{ r23 = ##0xfa0000fa }
{ r24 = ##0xcce0d948 }
{ r25 = ##0xde804bb7 }
{ r26 = ##0xffffff7f }
{ r27 = ##0xb978a08e }
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
    { r7 = add(r4.l,r5.l):sat:<<16 ; r5 = sub(r11,r3):sat ; r17:16 = vsubh(r15:14,r25:24):sat ; if (p3) r3:2 = combine(r4,r25) }
    { nop ; r15 = add(r22.h,r11.l):sat:<<16 ; if (!p2) r4 = sub(r23,r10) ; r23 = add(r20.h,r14.l):<<16 }
    { p1 = vcmpw.gt(r3:2,#83) ; p2 = dfcmp.ge(r15:14,r13:12) ; p3 = !cmp.gt(r21,#-359) }
    { p0 = !cmp.eq(r8,#336) ; p3 = tlbmatch(r15:14,r22) }
    { r16 = and(r7,#-372) ; r24 = vavgh(r1,r10):rnd ; p2 = cmph.gtu(r20,#24) ; r23 = and(r11,~r2) }
{ p1 = cmp.eq(r26,r20) ; if (p1.new) jump:nt .Lcf_4 }
    { r24 = add(r9.l,r5.h):<<16 ; r13:12 = vavgub(r19:18,r27:26):rnd ; r3 = and(r11,~r27) }
.Lcf_4:
    { r27:26 = vsubw(r23:22,r9:8):sat ; r3 = !cmp.eq(r12,#87) ; r22 = sxtb(r16) }
    { r15:14 = vnavgh(r25:24,r25:24):rnd:sat }
{ jump .Lcf_7 }
    { r21:20 = vmaxub(r21:20,r27:26) ; p2 = vcmpw.gt(r15:14,r9:8) ; p0 = !cmp.eq(r21,r0) ; r3 = combine(r11.l,r15.l) }
.Lcf_7:
    { if (!p3) r14 = #200 ; r25:24 = sub(r11:10,r9:8) ; r20 = cmp.eq(r22,r19) ; r3:2 = vminuw(r3:2,r3:2) }
    { r24 = sub(r23.l,r5.l):sat ; r6 = add(r22.l,r12.l):sat ; p0 = !cmp.eq(r15,r8) }
    { r15 = add(r10.l,r21.h) ; r9:8 = vminh(r3:2,r21:20) ; r10 = sub(r18,r16):sat }
    { p0 = dfclass(r19:18,#31) ; r17 = vadduh(r24,r19):sat ; r21:20 = vsubub(r25:24,r25:24):sat ; r12.h = #48762 }
{ if (!p3) jump:nt .Lcf_12 }
    { p1 = vcmph.gt(r13:12,r19:18) ; p3 = !cmp.gtu(r26,#60) }
{ jump .Lcf_13 }
    { r21 = or(r1,r25) ; r9:8 = and(r11:10,~r11:10) ; r15 = xor(r1,r24) ; r2 = sub(r17,r23):sat }
.Lcf_12:
    { r25:24 = dfmake(#825):neg ; r21:20 = combine(#-98,#-68) }
.Lcf_13:
    { p1 = cmpb.gtu(r8,#44) ; if (p0) r7 = add(r26,r5) ; r1:0 = and(r19:18,~r13:12) }
{ p1 = cmp.gtu(r4,r9) ; if (p1.new) jump:nt .Lcf_16 }
    { p3 = vcmpw.eq(r13:12,r19:18) }
    { r22 = vsubuh(r13,r23):sat ; r3:2 = dfmake(#891):pos ; p0 = vcmpw.eq(r23:22,#-103) ; if (!p1) r5 = zxth(r16) }
{ if (p1) jump:t .Lcf_18 }
.Lcf_16:
    { r5:4 = vsubh(r21:20,r17:16) ; r7:6 = vaddh(r17:16,r7:6) ; if (!p2) r15:14 = combine(r15,r26) }
.Lcf_18:
    { p2 = vcmph.gtu(r7:6,#57) }
    { r7 = maxu(r6,r9) ; p3 = !cmp.gt(r25,r14) ; p0 = vcmpw.gtu(r13:12,r21:20) ; if (!p2) r1 = aslh(r12) }
    { r10 = sub(r9.l,r6.h):sat:<<16 ; r14 = add(r1.l,r23.h) ; if (p0) r7 = sub(r22,r6) ; if (!p0) r3:2 = combine(r6,r3) }
    { if (p0) r14 = add(r7,#-7) ; r27:26 = vnavgw(r1:0,r27:26):crnd:sat ; r3 = sub(r13.l,r16.h) }
    { r19:18 = vnavgh(r13:12,r15:14):rnd:sat ; r27 = #17463 ; r23:22 = vmaxuw(r23:22,r9:8) ; r9 = mux(p0,r26,#69) }
    { r25:24 = vmaxub(r13:12,r23:22) }
    { if (p1) r21 = and(r20,r20) ; r13 = sub(r0,r12):sat ; p2 = vcmph.gt(r23:22,r21:20) ; r23 = sfmake(#429):pos }
    { if (p1) r6 = add(r13,r10) ; r11:10 = vavgw(r19:18,r23:22):rnd ; r17 = combine(r13.h,r17.l) }
    { r4 |= and(r17,#-139) ; p3 = cmp.gt(r15:14,r5:4) ; r24.h = #65235 ; r13 = sub(r19,r12) }
    { r13 = asrh(r27) ; r1 = cmp.eq(r24,#42) ; r7:6 = minu(r11:10,r1:0) }
.globl test_end
test_end:
    { jumpr r31 }

.data
.balign 4096
.globl mem_region
mem_region:
    .space 65536
