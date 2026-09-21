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
{ r0 = ##0x00000001 }
{ r1 = ##0x00000080 }
{ r2 = ##0xfa0000fa }
{ r3 = ##0xd4784bb7 }
{ r4 = ##0xffffff7f }
{ r5 = ##0x7dafe650 }
{ r6 = ##0x00000080 }
{ r7 = ##0x83e5ad05 }
{ r8 = ##0x0000007f }
{ r9 = ##0xbc359fca }
{ r10 = ##0x34df71e9 }
{ r11 = ##0x05ffff05 }
{ r12 = ##0x5b979b72 }
{ r13 = ##0xa2ab900d }
{ r14 = ##0xffffff80 }
{ r15 = ##0x7fffffff }
{ r16 = ##0x83f72902 }
{ r17 = ##0x00001000 }
{ r18 = ##0x84c5e868 }
{ r19 = ##0x00000200 }
{ r20 = ##0xdc7afece }
{ r21 = ##0xc924332d }
{ r22 = ##0x9298e595 }
{ r23 = ##0xffffff80 }
{ r24 = ##0x18947ca7 }
{ r25 = ##0xffffff80 }
{ r26 = ##0xc3ae3184 }
{ r27 = ##0x135384d6 }
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
    { p1 = cmp.eq(r24,#22) ; r3:2 = max(r15:14,r9:8) ; r17:16 = vsubh(r5:4,r19:18):sat }
.Lcf_10:
.Lcf_14:
    { r17:16 = min(r25:24,r11:10) ; p3 = !cmp.eq(r26,r21) ; r11:10 = vavguh(r25:24,r19:18) }
    { r0 = combine(r23.l,r11.l) ; r7:6 = vminuw(r13:12,r3:2) ; r15:14 = vsubh(r17:16,r27:26):sat ; if (p2) r26 = add(r23,#39) }
    { r14 = add(r8.h,r6.h):<<16 ; r17:16 = and(r9:8,~r27:26) ; r23:22 = combine(#17,#-48) ; r26 = mux(p3,#41,r18) }
.Lcf_31:
.Lcf_33:
.Lcf_35:
    { r9 = or(r10,~r2) ; p0 = vcmph.eq(r9:8,#8) ; p2 = tlbmatch(r15:14,r2) }
.Lcf_47:
.globl test_end
test_end:
    { jumpr r31 }

.data
.balign 4096
.globl mem_region
mem_region:
    .space 65536