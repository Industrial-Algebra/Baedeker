#!/usr/bin/env bash
# Seed the fuzz corpora from the repo's existing .wasm fixtures.
# Usage: fuzz/seed.sh
set -euo pipefail

cd "$(dirname "$0")"

for target in decode validate_lower; do
    mkdir -p "corpus/$target"
    find ../crates/baedeker-testdata -name '*.wasm' -exec cp {} "corpus/$target/" \;
done

# smith_module generates modules from unstructured bytes — any short inputs
# seed it fine; reuse the same fixtures for variety.
mkdir -p corpus/smith_module
find ../crates/baedeker-testdata -name '*.wasm' -exec cp {} corpus/smith_module/ \;

# trap_edges wants short structured inputs (op selector + operands).
mkdir -p corpus/trap_edges
for f in ../crates/baedeker-testdata/spec/valid/*.wasm; do
    head -c 17 "$f" > "corpus/trap_edges/$(basename "$f").head"
done

echo "Seeded: $(find corpus -type f | wc -l) files across 4 targets"
