// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Privileged instructions that may share a packet with other work, and the
//! rule that a write to a coprocessor-enable bit takes effect only after the
//! packet that contains it.
//!
//! Rules (coprocessor configuration chapter, "changing enable bits on a
//! coprocessor packet"; control-register transfer semantics):
//!  * A packet that writes SSR[XE] (or CCR[XE3], SSR[XE2]) and also holds a
//!    coprocessor instruction runs that instruction under the enable value
//!    from BEFORE the packet.  Clearing XE leaves the instruction working
//!    and makes the next coprocessor packet raise "illegal execution of a
//!    coprocessor instruction" (0x16); setting XE in the packet does not
//!    rescue an instruction that was already illegal, and since the packet
//!    faults none of its writes (including the SSR one) are committed.
//!  * Register transfers between a general register and a monitor/system
//!    register obey ordinary packet semantics: sources are read before any
//!    result is written.

use super::*;

fn v() -> Vector {
    Vector::new()
}

fn dv(n: usize) -> Vec<u8> {
    (0..VLEN).map(|i| def_vbyte(n, i)).collect()
}

fn addw(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.chunks(4)
        .zip(b.chunks(4))
        .flat_map(|(x, y)| {
            u32::from_le_bytes([x[0], x[1], x[2], x[3]])
                .wrapping_add(u32::from_le_bytes([y[0], y[1], y[2], y[3]]))
                .to_le_bytes()
        })
        .collect()
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let sum = addw(&dv(0), &dv(1));
    let wb = BASE as usize;

    // ---- SSR[XE]: clear it in a packet with a vector instruction ------------
    // Packets: read SSR, derive a value with XE clear, then the packet under
    // test, then a second vector instruction.  Offsets from the function
    // start: prologue 8, three 4-byte packets, then the packet under test is
    // at +20 (it is two words), the following one at +28.
    let pre = "{ r2 = ssr }\n{ r1 = clrbit(r2, #31) }";
    out.push(Case::new("xe_clear_in_packet_op_still_runs_next_packet_faults",
        format!("{pre}\n{{ ssr = r1; v2.w = vadd(v0.w, v1.w) }}\n{{ v3.w = vadd(v0.w, v1.w) }}"))
        .hvx().restore_enables().resume_at_end().elr_at(PROLOGUE + 4 + 4 + 8)
        .vec(v().ignore(1).ignore(2)
            .expv(2, &sum)
            .ev(Ev::new(EvKind::Err, 0x16))));
    // Several coprocessor instructions, including memory, in the packet that
    // clears XE: they all run.
    out.push(Case::new("xe_clear_in_packet_vector_load_alu_store_all_run",
        format!("{pre}\n{{ ssr = r1; v4 = vmem(r20+#0); v2.w = vadd(v0.w, v1.w); vmem(r21+#0) = v3 }}\n{{ v5.w = vadd(v0.w, v1.w) }}"))
        .hvx().restore_enables().resume_at_end().elr_at(PROLOGUE + 4 + 4 + 16)
        .vec(v().ignore(1).ignore(2).set_win(20, BASE).set_win(21, 0x100)
            .expv(2, &sum).expv(4, &(0..VLEN).map(|i| def_mem_byte(wb + i)).collect::<Vec<_>>())
            .exp_mem(0x100, &dv(3))
            .ev(Ev::new(EvKind::Err, 0x16))));

    // ---- SSR[XE]: set it in the packet that uses the coprocessor -----------
    // First clear XE in its own packet, then try to set it together with a
    // vector instruction: the instruction faults and nothing in the packet is
    // committed (v2 keeps its value, SSR keeps XE clear until the hook puts
    // it back).
    let clear = "{ r2 = ssr }\n{ r1 = clrbit(r2, #31) }\n{ ssr = r1 }";
    out.push(Case::new("xe_set_in_packet_op_still_faults",
        format!("{clear}\n{{ ssr = r2; v2.w = vadd(v0.w, v1.w) }}"))
        .hvx().restore_enables().resume_at_end().elr_at(PROLOGUE + 4 + 4 + 4)
        .vec(v().ignore(1).ignore(2)
            .ev(Ev::new(EvKind::Err, 0x16))));
    // A coprocessor-free packet that sets XE works, and the next vector
    // packet then runs.
    out.push(Case::new("xe_set_then_next_packet_runs",
        format!("{clear}\n{{ ssr = r2 }}\n{{ v2.w = vadd(v0.w, v1.w) }}"))
        .hvx().restore_enables().vec(v().ignore(1).ignore(2).expv(2, &sum)));

    // ---- Control-register transfers with ordinary work in the packet --------
    // crswap with a scratch system register (sgp1): sources read first.
    out.push(Case::new("crswap_packet_reads_old_gpr",
        "{ crswap(r0, sgp1); r2 = r0 }\n{ r3 = sgp1 }\n{ crswap(r0, sgp1) }")
        .vec(v().exp(2, d(0)).exp(3, d(0))));
    out.push(Case::new("crswap_pair",
        "{ crswap(r1:0, sgp1:0); r4 = add(r0, r1) }\n{ crswap(r1:0, sgp1:0) }")
        .vec(v().ignore(4)));
    // User GP register moves: write the register, read it back next packet; the
    // reader in the same packet sees the old value through a register op.
    out.push(Case::new("ugp_write_with_alu",
        "{ ugp = r0; r2 = add(r3, r4) }\n{ r5 = ugp }")
        .vec(v().set(0, 0x1234_5600).exp(2, d(3).wrapping_add(d(4))).exp(5, 0x1234_5600)));
    out.push(Case::new("htid_with_alu",
        "{ r2 = htid; r3 = add(r0, r1) }")
        .vec(v().exp(2, 0).exp(3, d(0).wrapping_add(d(1)))));
    out
}
