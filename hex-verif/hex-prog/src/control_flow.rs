//! Control-flow decoration of a straight-line packet list.
//!
//! The synthesizer produces independent packets. This module wraps and
//! interleaves them with control flow so that programs still terminate:
//!
//! - **Forward branches**: `jump`, `if ([!]pN) jump:[t|nt]` and compound
//!   compare-jumps, targeting labels 1--4 packets ahead.
//! - **Hardware loops**: `loop0` / `loop1` (and nested loops using
//!   `:endloop0`, `:endloop1` and `:endloop01`) around 1--4 packets.
//! - **Software loops**: a counted backward branch around 1--4 packets,
//!   using [`SW_LOOP_COUNTER`] as the counter register.
//! - **Calls**: `call`, `callr` and predicated forms to small generated
//!   functions that return with `jumpr r31` or `dealloc_return`.
//!
//! Loop bodies never contain branches or calls, and forward branches never
//! land inside a loop body, so every generated program terminates.
//!
//! Everything that is not a plain packet is emitted so that the minimizer
//! can tell it apart: labels sit on their own lines, and loop/frame
//! structure lines are recognised by [`is_structural`].

use rand::rngs::StdRng;
use rand::Rng;

/// Register used as the counter of software loops. Recipes that enable
/// software loops must keep the synthesizer from writing it.
pub const SW_LOOP_COUNTER: usize = 25;

/// Number of leading packets left free of control flow. The `steps`
/// breakpoint is placed after the prologue, so an early branch could skip it.
const LEADING_PACKETS: usize = 4;

/// Which control-flow constructs to insert.
#[derive(Debug, Clone, Copy, Default)]
pub struct CfOptions {
    pub forward: bool,
    pub hw_loops: bool,
    pub sw_loops: bool,
    pub calls: bool,
    /// HVX gather/scatter blocks operating on the machine's VTCM.
    pub hvx_gather_scatter: bool,
}

impl CfOptions {
    /// True if any construct is enabled.
    pub fn any(&self) -> bool {
        self.forward || self.hw_loops || self.sw_loops || self.calls || self.hvx_gather_scatter
    }
}

/// Result of decorating a packet list.
pub struct CfOutput {
    /// Lines of the `steps` body (packets, labels, structure).
    pub body: Vec<String>,
    /// Lines of generated functions, emitted after `test_end`.
    pub functions: Vec<String>,
}

/// True for lines the minimizer must never remove: loop setup/end and
/// frame management. (Removing them would change the program's structure
/// rather than shrink it.)
pub fn is_structural(line: &str) -> bool {
    let l = line.trim();
    l.contains("loop0(")
        || l.contains("loop1(")
        || l.contains(":endloop")
        || l.contains("allocframe")
        || l.contains("dealloc_return")
        || l.contains("jumpr r31")
        || SW_LOOP_LINES.iter().any(|s| l.contains(s))
}

/// Marker texts of the software-loop counter management packets.
const SW_LOOP_LINES: [&str; 2] = ["r25 = #", "r25 = add(r25,#-1)"];

/// Per-packet decorations, indexed by packet position.
struct Deco {
    pre: Vec<Vec<String>>,
    suffix: Vec<String>,
    post: Vec<Vec<String>>,
    standalone: Vec<Option<String>>,
    labels: Vec<Vec<String>>,
}

impl Deco {
    fn new(n: usize) -> Self {
        Self {
            pre: vec![Vec::new(); n + 1],
            suffix: vec![String::new(); n + 1],
            post: vec![Vec::new(); n + 1],
            standalone: vec![None; n + 1],
            labels: vec![Vec::new(); n + 1],
        }
    }

    /// Terminate a hardware loop at packet `i`, either as a suffix of the
    /// packet or as a dedicated `{ nop }` packet.
    fn end_loop(&mut self, i: usize, name: &str, rng: &mut StdRng) {
        if self.suffix[i].is_empty() && rng.gen_bool(0.5) {
            self.suffix[i] = format!(":{}", name);
        } else {
            self.post[i].push(format!("{{ nop }}:{}", name));
        }
    }
}

/// Decorate `packets` with control flow according to `opts`.
///
/// `gen_packet` produces a fresh packet for function bodies.
pub fn apply(
    mut packets: Vec<String>,
    opts: &CfOptions,
    rng: &mut StdRng,
    gen_packet: &mut dyn FnMut(&mut StdRng) -> String,
) -> CfOutput {
    let n = packets.len();
    let mut deco = Deco::new(n);
    let mut in_loop = vec![false; n];
    let mut ranges: Vec<(usize, usize)> = Vec::new();

    // Loops.
    if opts.hw_loops || opts.sw_loops {
        let mut i = LEADING_PACKETS;
        let mut id = 0;
        while i < n.saturating_sub(1) {
            if rng.gen_range(0..8) != 0 {
                i += 1;
                continue;
            }
            let len = rng.gen_range(1..=4usize).min(n - 1 - i);
            let (a, b) = (i, i + len - 1);
            let use_sw = opts.sw_loops && (!opts.hw_loops || rng.gen_bool(0.4));
            if use_sw {
                plan_sw_loop(&mut deco, a, b, id, rng);
            } else {
                plan_hw_loop(&mut deco, a, b, id, rng);
            }
            id += 1;
            for flag in &mut in_loop[a..=b] {
                *flag = true;
            }
            ranges.push((a, b));
            i = b + 2;
        }
    }

    // Forward branches.
    if opts.forward {
        for i in LEADING_PACKETS..n.saturating_sub(1) {
            if in_loop[i] || rng.gen_range(0..4) != 0 {
                continue;
            }
            let mut tgt = (i + 1 + rng.gen_range(1..=4usize)).min(n);
            if let Some(&(_, b)) = ranges.iter().find(|&&(a, b)| a < tgt && tgt <= b) {
                tgt = (b + 1).min(n);
            }
            let label = format!(".Lcf_{}", i);
            deco.labels[tgt].push(label.clone());
            let branch = forward_branch(&label, rng);

            // Compound compare-jumps and busy packets get their own packet;
            // short plain-ALU packets absorb a simple branch.
            let pkt = &packets[i];
            let slots = pkt.matches(';').count() + 1;
            let plain = !(pkt.contains("mem") || pkt.contains(".new") || pkt.contains(" v"));
            if branch.contains(".new") || slots > 2 || !plain {
                deco.standalone[i] = Some(format!("{{ {} }}", branch));
            } else {
                let body = pkt
                    .trim()
                    .trim_start_matches('{')
                    .trim_end_matches('}')
                    .trim();
                packets[i] = format!("{{ {} ; {} }}", body, branch);
            }
        }
    }

    // Calls and generated functions.
    let mut functions = Vec::new();
    if opts.calls {
        let nfn = rng.gen_range(1..=3usize);
        for k in 0..nfn {
            functions.extend(gen_function(k, rng, gen_packet));
        }
        for i in LEADING_PACKETS..n.saturating_sub(1) {
            if in_loop[i] || rng.gen_range(0..8) != 0 {
                continue;
            }
            let target = format!(".Lfn_{}", rng.gen_range(0..nfn));
            let neg = if rng.gen_bool(0.5) { "!" } else { "" };
            let pn = rng.gen_range(0..4);
            let reg = rng.gen_range(0..SW_LOOP_COUNTER);
            let lines = match rng.gen_range(0..4) {
                0 => vec![format!("{{ call {} }}", target)],
                1 => vec![format!("{{ if ({}p{}) call {} }}", neg, pn, target)],
                2 => vec![
                    format!("{{ r{} = ##{} }}", reg, target),
                    format!("{{ callr r{} }}", reg),
                ],
                _ => vec![
                    format!("{{ r{} = ##{} }}", reg, target),
                    format!("{{ if ({}p{}) callr r{} }}", neg, pn, reg),
                ],
            };
            deco.post[i].extend(lines);
        }
    }

    // HVX gather/scatter blocks (one self-contained line each).
    if opts.hvx_gather_scatter {
        for i in LEADING_PACKETS..n.saturating_sub(1) {
            if !in_loop[i] && rng.gen_range(0..10) == 0 {
                deco.post[i].push(gather_scatter_block(rng));
            }
        }
    }

    // Render.
    let mut body = Vec::with_capacity(n * 2);
    for i in 0..=n {
        for l in &deco.labels[i] {
            body.push(format!("{}:", l));
        }
        if i == n {
            break;
        }
        body.append(&mut deco.pre[i]);
        body.push(format!("{}{}", packets[i], deco.suffix[i]));
        body.append(&mut deco.post[i]);
        if let Some(b) = deco.standalone[i].take() {
            body.push(b);
        }
    }
    CfOutput { body, functions }
}

/// Base address of VTCM on the Hexagon machines under test (`V81QA_1`).
const VTCM_BASE: u32 = 0xd900_0000;

/// One self-contained HVX gather or scatter block on a single line.
///
/// Gather/scatter fault (exception 0x26) unless the base is in VTCM, so the
/// block seeds eight vectors of VTCM, masks the offset vector(s) into that
/// 1 KiB window, sets `m0`/`m1` to the window's last byte offset, runs the
/// operation and reads the affected memory back into vector registers.
/// Scratch registers: r6, r7, v28--v31, m0/m1.
fn gather_scatter_block(rng: &mut StdRng) -> String {
    // (name, offset-vector kind, mask for the offset lanes)
    // kinds: 0 = halfword offsets, 1 = word offsets, 2 = word offsets in a pair
    let scatter = rng.gen_bool(0.5);
    let kind = rng.gen_range(0..3usize);
    let (mask, off_reg, elem) = match kind {
        0 => (0x03fe_03fe_u32, "v30.h", "h"),
        1 => (0x0000_03fc, "v30.w", "w"),
        _ => (0x0000_03fe, "v29:28.w", "h"),
    };
    let m = if rng.gen_bool(0.5) { "m0" } else { "m1" };
    let pred = if rng.gen_bool(0.3) {
        // `if (qN)` forms exist for the non-accumulating operations only.
        Some(rng.gen_range(0..4))
    } else {
        None
    };
    let src = rng.gen_range(0..28);
    let data = rng.gen_range(0..28);

    let mut pk: Vec<String> = vec![
        format!("r6 = ##{:#x}", VTCM_BASE),
        format!("r7 = ##{:#x}", mask),
        "v31 = vsplat(r7)".to_string(),
    ];
    if kind == 2 {
        pk.push(format!("v28 = vand(v{},v31)", src));
        pk.push(format!("v29 = vand(v{},v31)", (src + 1) % 28));
    } else {
        pk.push(format!("v30 = vand(v{},v31)", src));
    }
    pk.push("r7 = #1023".to_string());
    pk.push(format!("{} = r7", m));
    for slot in 0..8 {
        pk.push(format!("vmem(r6+#{}) = v{}", slot, rng.gen_range(0..28)));
    }

    let mut tail: Vec<String> = Vec::new();
    if scatter {
        let acc = pred.is_none() && rng.gen_bool(0.4);
        let op = if acc { "+=" } else { "=" };
        let q = pred.map(|q| format!("if (q{}) ", q)).unwrap_or_default();
        let sc = match kind {
            2 => format!("{}vscatter(r6,{},{}).h {} v{}", q, m, off_reg, op, data),
            _ => format!(
                "{}vscatter(r6,{},{}).{} {} v{}",
                q, m, off_reg, elem, op, data
            ),
        };
        pk.push(sc);
        pk.push("vmem(r6+#0):scatter_release".to_string());
        for _ in 0..2 {
            tail.push(format!(
                "v{} = vmem(r6+#{})",
                rng.gen_range(0..28),
                rng.gen_range(0..8)
            ));
        }
    } else {
        let q = pred.map(|q| format!("if (q{}) ", q)).unwrap_or_default();
        let slot = rng.gen_range(0..8);
        // The gather and its `vtmp.new` consumer share a packet.
        let ty = match kind {
            0 => "h",
            1 => "w",
            _ => "h",
        };
        pk.push(format!(
            "{}vtmp.{} = vgather(r6,{},{}).{} ; vmem(r6+#{}) = vtmp.new",
            q, ty, m, off_reg, elem, slot
        ));
        tail.push(format!("v{} = vmem(r6+#{})", rng.gen_range(0..28), slot));
    }
    pk.extend(tail);
    pk.iter()
        .map(|p| format!("{{ {} }}", p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A single forward branch (jump, predicated jump or compound cmp-jump).
fn forward_branch(label: &str, rng: &mut StdRng) -> String {
    let hint = if rng.gen_bool(0.5) { "t" } else { "nt" };
    let neg = if rng.gen_bool(0.5) { "!" } else { "" };
    match rng.gen_range(0..6) {
        0 => format!("jump {}", label),
        1 | 2 => format!(
            "if ({}p{}) jump:{} {}",
            neg,
            rng.gen_range(0..4),
            hint,
            label
        ),
        _ => {
            let (p, a) = (rng.gen_range(0..2), rng.gen_range(0..SW_LOOP_COUNTER));
            let op = ["eq", "gt", "gtu"][rng.gen_range(0..3usize)];
            let rhs = if rng.gen_bool(0.5) {
                format!("r{}", rng.gen_range(0..SW_LOOP_COUNTER))
            } else {
                format!("#{}", rng.gen_range(0..32))
            };
            format!("p{p} = cmp.{op}(r{a},{rhs}) ; if ({neg}p{p}.new) jump:{hint} {label}")
        }
    }
}

/// Counted backward-branch loop around packets `a..=b`.
fn plan_sw_loop(deco: &mut Deco, a: usize, b: usize, id: usize, rng: &mut StdRng) {
    let label = format!(".Lsl_{}", id);
    let count = rng.gen_range(1..=4);
    deco.pre[a].push(format!("{{ r{} = #{} }}", SW_LOOP_COUNTER, count));
    deco.pre[a].push(format!("{}:", label));
    deco.post[b].push(format!("{{ r{c} = add(r{c},#-1) }}", c = SW_LOOP_COUNTER));
    deco.post[b].push(format!(
        "{{ p0 = cmp.gt(r{},#0) ; if (p0.new) jump:t {} }}",
        SW_LOOP_COUNTER, label
    ));
}

/// Hardware loop around packets `a..=b`: `loop0`, `loop1`, or a nest.
fn plan_hw_loop(deco: &mut Deco, a: usize, b: usize, id: usize, rng: &mut StdRng) {
    let count = |rng: &mut StdRng| rng.gen_range(1..=4);
    let kind = if b > a {
        rng.gen_range(0..3)
    } else {
        rng.gen_range(0..2)
    };
    match kind {
        0 | 1 => {
            let (setup, end) = if kind == 0 {
                ("loop0", "endloop0")
            } else {
                ("loop1", "endloop1")
            };
            let label = format!(".Lhl_{}", id);
            deco.pre[a].push(format!("{{ {}({}, #{}) }}", setup, label, count(rng)));
            deco.pre[a].push(format!("{}:", label));
            deco.end_loop(b, end, rng);
        }
        _ => {
            // Outer loop1 over a..=b, inner loop0 over c..=d.
            let c = rng.gen_range(a..=b);
            let d = rng.gen_range(c..=b);
            let (outer, inner) = (format!(".Lho_{}", id), format!(".Lhi_{}", id));
            deco.pre[a].push(format!("{{ loop1({}, #{}) }}", outer, count(rng)));
            deco.pre[a].push(format!("{}:", outer));
            deco.pre[c].push(format!("{{ loop0({}, #{}) }}", inner, count(rng)));
            deco.pre[c].push(format!("{}:", inner));
            if d == b {
                deco.end_loop(b, "endloop01", rng);
            } else {
                deco.end_loop(d, "endloop0", rng);
                deco.end_loop(b, "endloop1", rng);
            }
        }
    }
}

/// Generate one callable function (label `.Lfn_<k>`).
fn gen_function(
    k: usize,
    rng: &mut StdRng,
    gen_packet: &mut dyn FnMut(&mut StdRng) -> String,
) -> Vec<String> {
    let mut lines = vec![format!(".Lfn_{}:", k)];
    let framed = rng.gen_bool(0.5);
    let ret = if framed {
        "dealloc_return"
    } else {
        "jumpr r31"
    };
    if framed {
        lines.push("    { allocframe(#8) }".to_string());
    }
    let len = rng.gen_range(1..=4usize);
    let early = rng.gen_range(0..len);
    for j in 0..len {
        lines.push(gen_packet(rng));
        if j == early && rng.gen_bool(0.5) {
            let neg = if rng.gen_bool(0.5) { "!" } else { "" };
            let hint = if rng.gen_bool(0.5) { ":t" } else { ":nt" };
            let insn = if framed {
                "dealloc_return".to_string()
            } else {
                format!("jumpr{} r31", hint)
            };
            lines.push(format!(
                "    {{ if ({}p{}) {} }}",
                neg,
                rng.gen_range(0..4),
                insn
            ));
        }
    }
    lines.push(format!("    {{ {} }}", ret));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn run(seed: u64, opts: CfOptions) -> CfOutput {
        let mut rng = StdRng::seed_from_u64(seed);
        let packets: Vec<String> = (0..60)
            .map(|i| format!("{{ r{} = #{} }}", i % 20, i))
            .collect();
        apply(packets, &opts, &mut rng, &mut |_| "{ r1 = #1 }".to_string())
    }

    fn all() -> CfOptions {
        CfOptions {
            forward: true,
            hw_loops: true,
            sw_loops: true,
            calls: true,
            hvx_gather_scatter: false,
        }
    }

    #[test]
    fn hw_loops_are_balanced() {
        let opts = CfOptions {
            hw_loops: true,
            ..CfOptions::default()
        };
        for seed in 0..50 {
            let out = run(seed, opts).body.join("\n");
            // loop0 is closed by :endloop0 or :endloop01, loop1 by :endloop1
            // or :endloop01. Count `:endloop0` before `1` so 01 is not double
            // counted.
            let e01 = out.matches(":endloop01").count();
            let e0 = out.matches(":endloop0").count() - e01;
            let e1 = out.matches(":endloop1").count();
            assert_eq!(
                out.matches("loop0(").count(),
                e0 + e01,
                "loop0, seed {seed}"
            );
            assert_eq!(
                out.matches("loop1(").count(),
                e1 + e01,
                "loop1, seed {seed}"
            );
        }
    }

    #[test]
    fn branch_targets_have_labels_outside_loops() {
        for seed in 0..50 {
            let out = run(seed, all()).body;
            let text = out.join("\n");
            for line in &out {
                if line.trim_end().ends_with(':') {
                    continue;
                }
                if let Some(pos) = line.find(".Lcf_") {
                    let label: String = line[pos..]
                        .chars()
                        .take_while(|c| !c.is_whitespace() && *c != '}')
                        .collect();
                    assert!(text.contains(&format!("{}:", label)), "missing {label}");
                }
            }
        }
    }

    #[test]
    fn every_call_target_has_a_function() {
        for seed in 0..50 {
            let out = run(seed, all());
            let text = out.body.join("\n");
            for k in 0..3 {
                if text.contains(&format!(".Lfn_{}", k)) {
                    assert!(
                        out.functions.contains(&format!(".Lfn_{}:", k)),
                        "seed {seed}"
                    );
                }
            }
        }
    }

    #[test]
    fn structural_lines_are_recognised() {
        assert!(is_structural("{ loop0(.Lhl_0, #3) }"));
        assert!(is_structural("{ nop }:endloop1"));
        assert!(is_structural("{ allocframe(#8) }"));
        assert!(is_structural("{ r25 = add(r25,#-1) }"));
        assert!(!is_structural("{ r1 = add(r2,r3) }"));
    }
}
