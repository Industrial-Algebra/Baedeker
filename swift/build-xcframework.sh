#!/usr/bin/env bash
# Builds baedeker-ffi for every Apple platform and packages it as
# swift/Vendor/CBaedeker.xcframework (gitignored). Requires: Xcode,
# and the Rust Apple targets (the script adds them via rustup).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$REPO_ROOT/swift/Vendor"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

MACOS_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
IOS_TARGETS=(aarch64-apple-ios)
SIM_TARGETS=(aarch64-apple-ios-sim x86_64-apple-ios)

for target in "${MACOS_TARGETS[@]}" "${IOS_TARGETS[@]}" "${SIM_TARGETS[@]}"; do
    rustup target add "$target"
    cargo build -p baedeker-ffi --release --target "$target" --manifest-path "$REPO_ROOT/Cargo.toml"
done

lib_for() { echo "$REPO_ROOT/target/$1/release/libbaedeker_ffi.a"; }

mkdir -p "$WORK_DIR/macos" "$WORK_DIR/sim"
lipo -create "$(lib_for aarch64-apple-darwin)" "$(lib_for x86_64-apple-darwin)" \
    -output "$WORK_DIR/macos/libbaedeker_ffi.a"
lipo -create "$(lib_for aarch64-apple-ios-sim)" "$(lib_for x86_64-apple-ios)" \
    -output "$WORK_DIR/sim/libbaedeker_ffi.a"

# Stage headers with a module map so SPM/Xcode can import the binary
# target as module CBaedeker (xcodebuild copies the -headers dir verbatim).
HEADERS="$WORK_DIR/headers"
mkdir -p "$HEADERS"
cp "$REPO_ROOT/crates/baedeker-ffi/include/baedeker.h" "$HEADERS/"
cat > "$HEADERS/module.modulemap" << 'EOF'
module CBaedeker {
    header "baedeker.h"
    export *
}
EOF

rm -rf "$OUT_DIR/CBaedeker.xcframework"
mkdir -p "$OUT_DIR"
xcodebuild -create-xcframework \
    -library "$WORK_DIR/macos/libbaedeker_ffi.a" -headers "$HEADERS" \
    -library "$(lib_for aarch64-apple-ios)" -headers "$HEADERS" \
    -library "$WORK_DIR/sim/libbaedeker_ffi.a" -headers "$HEADERS" \
    -output "$OUT_DIR/CBaedeker.xcframework"

echo "Wrote $OUT_DIR/CBaedeker.xcframework"
