# hex-verif: SDK QEMU (reference) vs bcain/integ QEMU (93f5afbe76)

Reference: /opt/Hexagon_SDK/6.4.0.2/tools/Tools/QEMUHexagon/bin/qemu-system-hexagon
Under test: /home/brian/src/qemu-integ/build_integ/qemu-system-hexagon (bcain/integ @ 93f5afbe76)
Instruction DB: generated from /home/brian/src/toolchain_for_hexagon/llvm-project (instructions.json, 2384 insns)
Toolchain: SDK hexagon-clang/hexagon-lldb 19.0.04, -mv73, both QEMUs `-machine V81QA_1`.
Recipes: recipes/01..08 (scalar ALU/mul/shift/sat/double/predicated/wide + HVX ALU).

## Result: no QEMU divergence found
Run 2 (final recipes): 8 recipes x 150 iterations x 30 packets (seed 5000): 1194/1200 pass first time; the 6 flagged
iterations all pass on 3 serial reruns each (0 real diffs).

| recipe | pass | flagged |
|---|---|---|
| 01_scalar_alu | 149 | 1 (iter 110) |
| 02_multiply | 150 | 0 |
| 03_shift_bit | 150 | 0 |
| 04_saturating | 149 | 1 (iter 110) |
| 05_double_regs | 150 | 0 |
| 06_predicated | 150 | 0 |
| 07_hvx_alu | 149 | 1 (iter 71) |
| 08_wide_scalar | 147 | 3 (109,110,132) |

Triage of the 6 flagged: 0 register dumps on one side, opposite sides on the same seed, and pass
3/3 when rerun with -t 1 => startup race in the harness under load (fixed 500ms sleep before
lldb connects), not an emulator bug. Raw evidence kept in 0*/iter_*.
TODO: replace the sleep with a port-ready poll and retry on empty dumps.

## Run 1 (run1_generator_issues/): generator bugs, not QEMU bugs
- 6 flagged programs (seed 1000): every one contains a `:deprecated` instruction (packhl,
  add/sub:sat) => both QEMUs raise exception 0x15 "Invalid packet" identically. Reduced to a
  single packet each with reduce_invalid.py. Fixed: ":deprecated" added to recipe skip_terms.
- 25 HVX build failures: assembler rejects V6_vrmpy{bub,ub}_rtt[_acc], V6_vmpahhsat,
  V6_vmpauhuhsat, V6_vmpsuhuhsat as emitted. Excluded in recipes/07_hvx_alu.toml.

## Tool changes (hex-verif crate)
--ref-qemu (QEMU as reference), --machine (same -machine for both; the builds' defaults differ:
SDK `sim`, integ `V66G_1024`), and a guard so empty/unequal lldb dumps count as mismatches.

## Caveats
Coverage is limited: no memory ops, control flow, or HVX loads/stores/permutes/gathers (recipes
exclude them); only ~50% of synthesizable scalar/HVX insns were hit per recipe. Cores are
V81QA_1 running v73 code. SDK QEMU warns "using v81 cpu but binary is for v73".

---
# Pass 2: memory ops + control flow enabled

New generator support (hex-prog): `allow_control_flow` synthesis option inserts forward-only
`jump`, `if ([!]pN) jump:[t|nt]` and compound `pN = cmp.xx(..); if (pN.new) jump` (labels 1-4
packets ahead, so programs always terminate; kept valid by the minimizer). Memory ops use the
existing `allow_mem_ops` (base+offset loads/stores via r27 -> mem_region, incl. FP/64-bit forms).
New recipes: 09_memory, 10_control_flow, 11_memory_control_flow, 12_hvx_control_flow.
Also: `--keep-passing`, and retry (x3) of an iteration when a side yields no register dump
(fixes the pass-1 startup-race flakiness).

| run | recipes | seed | result |
|---|---|---|---|
| run3_findings/ | 01-12, 150 iters each | 7000 | 1 real mismatch (10/iter_53), 1 build failure (11: `l2unlocka` not skipped) |
| run4/ | 09-12, 150 iters each | 8000 | 600/600 pass |
(01-08 in run3 also 8x150 pass, 0 flaky after the retry fix.)

## Finding: `tlbmatch` differs, SDK QEMU is the odd one out  (finding_tlbmatch/)
Minimal repro (2 packets): `{ p3 = tlbmatch(r15:14,r22) }` with r15:14=0xe4830000_00000400,
r22=0x6483dc90. p3 = **0xff on SDK QEMU**, **0x00 on integ QEMU**, **0x00 on hexagon-sim**
(authoritative). So integ agrees with hexagon-sim; the presumed-good SDK QEMU is wrong here.
Not a regression in bcain/integ. (Originally flagged in a control-flow recipe because the
diverging p3 steered later branches: 12 regs differed in the unreduced program.)

## Generator fix
Memory recipes: cache-maintenance ops (`l2unlocka` etc.) leaked because a recipe's skip_terms
replaces the defaults; added l2*/dc*/icinv/syncht to 09 and 11.

---
# Pass 3 (run5): 12 recipes x 400 iterations x 50 packets, seed 20000
4800 iterations: 4799 pass, 1 mismatch (10_control_flow/iter_362), no build failures.
The mismatch is `p2 = tlbmatch(r15:14,r2)` again: p2 = 0xff on SDK QEMU, 0x00 on integ QEMU and
0x00 on hexagon-sim (finding_tlbmatch/run5_iter362/). Same root cause as the earlier finding: the
SDK QEMU's tlbmatch disagrees with hexagon-sim; integ matches. No integ-only failures so far.
Cumulative (runs 3-5): ~8,400 scalar/HVX/memory/control-flow iterations, 2 hits, both tlbmatch.

---
# Pass 4: generator extended (commits abcbed2, 10cbadb)
New: HVX vmem/vmemu (allow_hvx_mem_ops), hardware loops (loop0/loop1/nested/:endloop01),
software backward loops (r25 counter), calls/returns (call/callr, predicated, jumpr r31 and
dealloc_return functions), HVX gather/scatter blocks on VTCM (0xd9000000). Recipes 13-18.
Data regions are now seeded with a pseudo-random pattern.

run6 (13-17, 300 iters, seed 30000): 1500/1500 real passes (1 generator build failure: cache
tag instruction `dctagr`, now skipped along with the rest of the cache/system family).
run7 (all 18 recipes, 400 iters, 60 packets, seed 40000): 7197/7200 pass. The 3 mismatches are
all `tlbmatch` (finding_tlbmatch/run7/): the known SDK-QEMU-vs-sim/integ difference.
`tlbmatch` is now in every recipe's skip_terms so it stops masking other failures.

## Open candidate: gather/scatter outside VTCM (finding_vgather_non_vtcm/)
With a non-VTCM base and in-range offsets, hexagon-sim and the SDK QEMU raise exception 0x26
(Coprocessor VMEM address error); the integ QEMU raises nothing (6 forms tried; try.sh).
Not yet reduced to a single minimal instruction, and not covered by the recipes (they use VTCM).
