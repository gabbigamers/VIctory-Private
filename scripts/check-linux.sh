#!/bin/sh
# What can be validated without Windows, a GPU or game data (docs/progress.md,
# "Validation tiers"). Run from anywhere inside the repository:
#
#   scripts/check-linux.sh          # tier 1 and tier 2
#   scripts/check-linux.sh tests    # tier 1 only
#   scripts/check-linux.sh windows  # tier 2 only
#
# Tier 1: the tests of every crate but the command-line front end, on this
#         host. Tests that need Windows are compiled out.
# Tier 2: CI's clippy for the Windows target, type-checked only (nothing is
#         linked: that needs the MSVC CRT and the Windows SDK). LLVM's
#         clang-cl, llvm-lib and llvm-ml stand in for the MSVC tools the
#         blake3 build script asks for.
set -eu

cd "$(dirname "$0")/.."
what="${1:-all}"

find_tool() {
    for candidate in "$1" "$1-18" "$1-19" "$1-20" "/usr/lib/llvm-18/bin/$1" "/usr/lib/llvm-19/bin/$1" "/usr/lib/llvm-20/bin/$1"; do
        if command -v "$candidate" >/dev/null 2>&1; then
            command -v "$candidate"
            return 0
        fi
    done
    echo "check-linux: $1 not found (install LLVM: clang, lld, llvm)" >&2
    return 1
}

if [ "$what" = all ] || [ "$what" = tests ]; then
    echo "== tier 1: host tests (all crates but apps/victory)"
    cargo test --workspace --exclude victory --locked
fi

if [ "$what" = all ] || [ "$what" = windows ]; then
    echo "== tier 2: clippy for x86_64-pc-windows-msvc (no link)"
    clang_cl=$(find_tool clang-cl)
    llvm_lib=$(find_tool llvm-lib)
    llvm_ml=$(find_tool llvm-ml)
    tools=$(mktemp -d)
    trap 'rm -rf "$tools"' EXIT
    # cc-rs runs the assembler as ml64; llvm-ml takes the same arguments.
    printf '#!/bin/sh\nexec "%s" -m64 "$@"\n' "$llvm_ml" >"$tools/ml64"
    chmod +x "$tools/ml64"
    export CC_x86_64_pc_windows_msvc="$clang_cl"
    export AR_x86_64_pc_windows_msvc="$llvm_lib"
    export CC_MASM_ASM_x86_64_pc_windows_msvc="$tools/ml64"
    # Its own folder: host and Windows artifacts do not evict each other.
    export CARGO_TARGET_DIR="${VICTORY_WINDOWS_TARGET_DIR:-target/windows-check}"
    cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-msvc -- -D warnings
    cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-msvc -- -D warnings
fi

echo "check-linux: ok"
