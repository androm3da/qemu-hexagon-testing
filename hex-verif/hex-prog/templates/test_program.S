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
{% for entry in init_regs -%}
    { r{{ entry.num }} = ##0x{{ entry.hex }} }
{% endfor -%}
    { p0 = cmp.eq(r0,r0) }
    { p1 = cmp.gt(r0,r1) }
    { p2 = cmp.eq(r2,r3) }
    { p3 = cmp.gt(r4,r5) }
    { r28 = #{{ num_iterations }} }
    { r27 = ##mem_region }
{% if hvx_mem -%}
    { r26 = ##hvx_mem_region+4096 }
{% endif -%}
{% if hvx -%}
    { v0 = vsplat(r0) }
    { v1 = vsplat(r1) }
    { v2 = vsplat(r2) }
    { v3 = vsplat(r3) }
{% endif -%}
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
{% if framed -%}
    { allocframe(#8) }
{% endif -%}
{% for pkt in packets -%}
{{ pkt }}
{% endfor -%}
.globl test_end
test_end:
{% if framed -%}
    { dealloc_return }
{% else -%}
    { jumpr r31 }
{% endif -%}
{% for f in functions -%}
{{ f }}
{% endfor -%}

.data
.balign 4096
.globl mem_region
mem_region:
    .set lcg, 0x2545f491
    .rept 16384
    .word lcg
    .set lcg, (lcg * 1664525 + 1013904223) & 0xffffffff
    .endr
{% if hvx || hvx_mem -%}

.balign 4096
.globl hvx_mem_region
hvx_mem_region:
    .set lcg2, 0x1badb002
    .rept 65536
    .word lcg2
    .set lcg2, (lcg2 * 1664525 + 1013904223) & 0xffffffff
    .endr
{% endif -%}
