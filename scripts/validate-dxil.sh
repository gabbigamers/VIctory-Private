#!/usr/bin/env bash
# Checks the DXIL the host tests emit with Microsoft's validator, outside
# Windows. The validator is a tool the user supplies, never part of this
# repository: the `dxv` program of a DirectXShaderCompiler release for
# Linux, with the libraries beside it (`../lib`).
#
#   VICTORY_DXV=/path/to/bin/dxv scripts/validate-dxil.sh [cargo test args]
#
# Runs the host tests of victory-shader with VICTORY_DXIL_DUMP set, then
# runs dxv over every distinct container they finished. Prints the count
# of containers, of failures, and each distinct message with its count.
# Exit status 0 only if every container validates; 2 if no validator.
#
# This says the containers are well-formed DXIL by that validator's rules.
# It does not run them, and it is not a driver's compiler.
set -euo pipefail
cd "$(dirname "$0")/.."

dxv="${VICTORY_DXV:-}"
if [ -z "$dxv" ] || [ ! -x "$dxv" ]; then
    echo "validate-dxil: set VICTORY_DXV to the dxv program of a DXC release; nothing checked" >&2
    exit 2
fi
lib="$(cd "$(dirname "$dxv")/../lib" 2>/dev/null && pwd || true)"
out="${VICTORY_DXIL_DUMP:-$(mktemp -d)}"
mkdir -p "$out"

VICTORY_DXIL_DUMP="$out" cargo test -p victory-shader --locked "$@" >"$out/tests.log" 2>&1 || {
    tail -30 "$out/tests.log" >&2
    echo "validate-dxil: the tests failed; see $out/tests.log" >&2
    exit 1
}

total=0
bad=0
: >"$out/failures.log"
for f in "$out"/*.dxil; do
    [ -e "$f" ] || break
    total=$((total + 1))
    if ! msg="$(LD_LIBRARY_PATH="$lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$dxv" "$f" 2>&1)" \
        || ! grep -q 'Validation succeeded' <<<"$msg"; then
        bad=$((bad + 1))
        printf '%s  (from %s)\n%s\n\n' "$(basename "$f")" "$(cat "${f%.dxil}.from" 2>/dev/null)" "$msg" >>"$out/failures.log"
    fi
done

echo "validate-dxil: $total containers, $bad rejected"
if [ "$bad" -ne 0 ]; then
    # Each distinct message once, most frequent first, without the file names.
    grep -v '\.dxil  (from ' "$out/failures.log" | grep -v '^$' \
        | sed -E 's/%[0-9A-Za-z._]+/%N/g; s/[0-9a-f]{32}/H/g' | sort | uniq -c | sort -rn | head -40
    grep '\.dxil  (from ' "$out/failures.log" | sed 's/^/  /'
    echo "validate-dxil: details in $out/failures.log"
    exit 1
fi
if [ "$total" -eq 0 ]; then
    echo "validate-dxil: the tests finished no container; nothing was checked" >&2
    exit 1
fi
