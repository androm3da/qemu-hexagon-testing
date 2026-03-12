
This is a tool to verify emulator(s) for the Hexagon ISA and
associated coprocessors/ISA extensions.

# Toolchains

Use /pkg/qct/software/hexagon/releases/tools/21.0.01 - this contains
hexagon-clang, hexagon-sim and hexagon-lldb.

/opt/Hexagon_SDK/6.4.0.2/tools/Tools/QEMUHexagon/bin/qemu-system-hexagon is
a recent QEMU release that can be used during hex-verif development/testing.

# Reference

* Tablegen files for Hexagon are in /local/mnt/workspace/upstream/llvm-project/llvm/lib/Target/Hexagon/
* The logic guiding the packet rules is written into the assembler in: /local/mnt/workspace/upstream/llvm-project/llvm/lib/Target/Hexagon/MCTargetDesc/HexagonMCChecker.cpp and /local/mnt/workspace/upstream/llvm-project/llvm/lib/Target/Hexagon/MCTargetDesc/HexagonMCChecker.cpp
* The scalar programmer's reference manual is in ./80-N2040-59_AA.pdf
* The HVX programmer's reference manual is in ./80-N2040-630_AA_Qualcomm_Hexagon_v81_HVX_PRM.pdf

# Testing

Code never gets committed until it's tested:

```
export RUSTFLAGS="-D warnings"
# Check code formatting
cargo fmt --all -- --check

# Run Clippy with strict checks
cargo clippy --all-targets -- -D warnings

# Run the actual tests
cargo test --all-targets
cargo test --doc
```
