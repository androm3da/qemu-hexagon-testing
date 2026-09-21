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
    { nop ; r15 = add(r22.h,r11.l):sat:<<16 ; if (!p2) r4 = sub(r23,r10) ; r23 = add(r20.h,r14.l):<<16 }
    { p0 = !cmp.eq(r8,#336) ; p3 = tlbmatch(r15:14,r22) }
.Lcf_4:
.Lcf_7:
.Lcf_12:
.Lcf_13:
.Lcf_16:
.Lcf_18:
.globl test_end
test_end:
    { jumpr r31 }

.data
.balign 4096
.globl mem_region
mem_region:
    .space 65536