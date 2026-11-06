// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! HMX (matrix coprocessor) instructions in packets.
//!
//! Rules (HMX chapter; coprocessor configuration chapter):
//!  * HMX instructions raise 0x18 (illegal second-coprocessor instruction)
//!    when SSR.XE2 is clear; XE2 is evaluated before anything else.
//!  * A write to SSR.XE2 takes effect after the packet that contains it, so
//!    an HMX instruction in the same packet is judged by the old value.
//!  * Activation and weight instructions share one packet (weight in slot 0,
//!    activation in slot 1).  Scalar and HVX instructions may share a packet
//!    with HMX operations and behave as usual.
//!  * A packet that raises an exception changes nothing.
//!
//! The accumulator state itself is not observable without convert/store
//! instructions; the tests here concentrate on the packet-level rules.  The
//! operands follow the 3x3 byte convolution example of the HMX chapter.

use super::*;

fn v() -> Vector {
    Vector::new()
}

fn dv(n: usize) -> Vec<u8> {
    (0..VLEN).map(|i| def_vbyte(n, i)).collect()
}

fn vaddw(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.chunks(4)
        .zip(b.chunks(4))
        .flat_map(|(x, y)| {
            u32::from_le_bytes([x[0], x[1], x[2], x[3]])
                .wrapping_add(u32::from_le_bytes([y[0], y[1], y[2], y[3]]))
                .to_le_bytes()
        })
        .collect()
}

/// Operands of the activation / weight pair: VTCM addresses and modifiers.
fn pair_operands(vv: Vector) -> Vector {
    vv.set_vt(0, 0x0000)              // activation crouton, 2KB aligned
        .set(1, (0b111000 << 5) | 0b11111) // spatial mask, channel stop 31
        .set_vt(2, 0x1000)            // weights, 128B aligned
        .set(3, 72 * 128 - 1)
}

const PAIR: &str = "activation.ub = mxmem(r0, r1); weight.b = mxmem(r2, r3)";

/// Enable HMX (SSR.XE2 = bit 26) in packets of their own.
const ENABLE: &str = "{ r27 = ssr }\n{ r26 = setbit(r27, #26) }\n{ ssr = r26 }\n{ isync }";

fn hmx_case(name: &str, body: String) -> Case {
    Case::new(name, format!("{ENABLE}\n{body}")).vtcm().restore_enables()
}

pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let sum = vaddw(&dv(0), &dv(1));

    // ---- XE2 clear: the second-coprocessor exception ---------------------------
    for (name, ins) in [
        ("pair", PAIR),
        ("mxclracc", "mxclracc"),
        ("mxclracc_hf", "mxclracc.hf"),
        ("mxswapacc", "mxswapacc"),
        ("mxswapacc_hf", "mxswapacc.hf"),
    ] {
        out.push(Case::new(format!("xe2_clear_{name}_faults"), format!("{{ {ins}; r4 = add(r4, #1) }}"))
            .vtcm().restore_enables().resume_at_end().elr_at(PROLOGUE)
            .vec(pair_operands(v()).ev(Ev::new(EvKind::Err, 0x18))));
        // The same packet runs when XE2 is set.
        out.push(hmx_case(&format!("xe2_set_{name}_runs"), format!("{{ {ins}; r4 = add(r4, #1) }}"))
            .vec(pair_operands(v()).ignore(26).ignore(27).exp(4, d(4).wrapping_add(1))));
    }

    // ---- Same-packet writes of SSR.XE2 -------------------------------------------
    // Set in the HMX packet: still illegal, and nothing commits.
    out.push(Case::new("xe2_set_in_hmx_packet_still_faults",
        format!("{{ r27 = ssr }}\n{{ r26 = setbit(r27, #26) }}\n{{ ssr = r26; {PAIR}; r4 = add(r4, #1) }}"))
        .vtcm().restore_enables().resume_at_end().elr_at(PROLOGUE + 4 + 4)
        .vec(pair_operands(v()).ignore(26).ignore(27).ev(Ev::new(EvKind::Err, 0x18))));
    // Cleared in the HMX packet: it runs; the next HMX packet faults.
    out.push(Case::new("xe2_clear_in_hmx_packet_runs_next_faults",
        format!("{ENABLE}\n{{ r26 = clrbit(r26, #26) }}\n{{ ssr = r26; {PAIR}; r4 = add(r4, #1) }}\n{{ mxclracc }}"))
        .vtcm().restore_enables().resume_at_end().elr_at(PROLOGUE + 16 + 4 + 16)
        .vec(pair_operands(v()).ignore(26).ignore(27).exp(4, d(4).wrapping_add(1))
            .ev(Ev::new(EvKind::Err, 0x18))));

    // ---- HMX together with HVX and scalar work ------------------------------------
    out.push(hmx_case("pair_with_hvx_and_scalar", format!("{{ {PAIR}; v2.w = vadd(v0.w, v1.w); r5 = add(r4, #2) }}"))
        .hvx().vec(pair_operands(v()).ignore(26).ignore(27)
            .expv(2, &sum).exp(5, d(4).wrapping_add(2))));
    out.push(hmx_case("mxclracc_with_hvx_load_and_alu",
        "{ mxclracc; v4 = vmem(r6+#0); v2.w = vadd(v0.w, v1.w) }".to_string())
        .hvx().vec(pair_operands(v()).ignore(26).ignore(27).set_vt(6, 0x0800)
            .expv(4, &(0x0800..0x0800 + VLEN).map(def_vt_byte).collect::<Vec<_>>())
            .expv(2, &sum)));

    // ---- HMX memory operands must be in the VTCM ---------------------------------
    // A weight operand outside the VTCM is a coprocessor memory address error
    // (0x26) for the slot that holds it; the packet commits nothing.
    out.push(hmx_case("pair_with_non_vtcm_weight_faults",
        format!("{{ {PAIR}; r4 = add(r4, #1) }}"))
        .resume_at_end().elr_at(PROLOGUE + 16)
        .vec(pair_operands(v()).set_win(2, 0x80).ignore(26).ignore(27)
            .ev(Ev::new(EvKind::Err, 0x26))));

    out
}
