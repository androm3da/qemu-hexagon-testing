#!/usr/bin/env bash
# Differential run: SDK QEMU (reference) vs bcain/integ QEMU (under test).
cd "$(dirname "$0")/.."
D=test_run_sept20_93f5afbe76
mkdir -p $D/${OUT:-.}
for r in recipes/${RECIPES:-*}.toml; do
  n=$(basename $r .toml)
  ${BIN:-./target/release/hex-verif} --instset $D/instructions.json --recipe $r \
    --toolchain /opt/Hexagon_SDK/6.4.0.2/tools/HEXAGON_Tools/19.0.04 \
    --ref-qemu /opt/Hexagon_SDK/6.4.0.2/tools/Tools/QEMUHexagon/bin/qemu-system-hexagon \
    --qemu /home/brian/src/qemu-integ/build_integ/qemu-system-hexagon \
    --machine V81QA_1 --isa-version v73 -n ${N:-60} -p ${P:-30} -t 8 --seed ${SEED:-1000} \
    --results-dir $D/${OUT:-.}/$n > $D/${OUT:-.}/$n.log 2>&1
  echo "$n: $(grep -E 'Passed|Mismatches|Build fail' $D/${OUT:-.}/$n/summary.txt | tr -s ' ' | tr '\n' ' ')"
done
