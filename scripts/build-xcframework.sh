#!/usr/bin/env bash
# Build the shellac static library as an XCFramework for iOS + iOS Simulator
# (arm64 + x86_64), so a Swift application can link the engine directly.
#
# Requirements:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
#
# Notes:
# - Absolute paths on purpose. Per project convention this script avoids
#   command substitution (`$(...)` / backticks) and `cd && ...` chains,
#   because command substitution triggers an interactive confirmation in the
#   agent tooling used on this repository and breaks unattended runs. So the
#   script self-locates with `${BASH_SOURCE[0]%/*}` instead of `$(dirname)`.
# - Xcode's `x86_64` iOS Simulator slice is provided by the Rust target
#   `x86_64-apple-ios` (not `x86_64-apple-ios-sim`, which does not exist —
#   iOS never shipped on Intel devices, so there is no ambiguity for x86_64).
# - Simulator slices are lipo'd into a single fat static library because
#   xcodebuild's `-create-xcframework` accepts at most one static library
#   per platform_variant (device or simulator).
# - Only `--lib` is built. The crate also has a `shellac-cli` bin target,
#   which is a host-side harness tool and has no business being
#   cross-compiled for iOS.
#
set -euo pipefail

# Self-locate without command substitution. `${BASH_SOURCE[0]%/*}` gives this
# script's directory; if BASH_SOURCE has no `/` the strip is a no-op (returns
# the whole path), which means the script was run from its own directory, so
# treat that as `.`. Relative locations are then absolutized against $PWD.
SCRIPT_DIR="${BASH_SOURCE[0]%/*}"
if [[ "$SCRIPT_DIR" == "${BASH_SOURCE[0]}" ]]; then
    SCRIPT_DIR="."
fi
if [[ "$SCRIPT_DIR" != /* ]]; then
    SCRIPT_DIR="$PWD/$SCRIPT_DIR"
fi

# Then search upward for the workspace root rather than counting directory
# levels. `bash build-xcframework.sh` from inside scripts/ absolutizes to
# "$PWD/." -- a component that is not a directory level -- so any fixed count
# lands one level off depending only on how the script was invoked.
# `rust-toolchain.toml` + `Cargo.toml` together identify the root: the crate
# directories below it have a Cargo.toml but no toolchain file.
REPO_ROOT=""
probe_dir="$SCRIPT_DIR"
while [[ -n "$probe_dir" ]]; do
    if [[ -f "$probe_dir/rust-toolchain.toml" && -f "$probe_dir/Cargo.toml" ]]; then
        REPO_ROOT="$probe_dir"
        break
    fi
    probe_parent="${probe_dir%/*}"
    if [[ "$probe_parent" == "$probe_dir" ]]; then
        break
    fi
    probe_dir="$probe_parent"
done
if [[ -z "$REPO_ROOT" ]]; then
    echo "cannot locate the workspace root above $SCRIPT_DIR" >&2
    exit 2
fi
CRATE="$REPO_ROOT/crates/shellac"
# shellac is a workspace member, so cargo puts build artifacts under the
# workspace root's target dir, not under the crate's own.
BUILD="$REPO_ROOT/target"
# Default output lives under target/, which is gitignored: the XCFramework is
# a build product and must never be committed. Override with $OUT to place it
# somewhere an Xcode project can reference.
OUT="${OUT:-$BUILD/xcframework}"

# Keep build-host absolute paths out of the shipped binary. Without this the
# .a carries the cargo registry's absolute path -- home directory and all --
# from every dependency compiled into it, which leaks the build machine's
# directory layout to anyone running `strings` on an app bundle.
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$REPO_ROOT=/shellac"
export RUSTFLAGS

TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
for t in "${TARGETS[@]}"; do
    echo "==> cargo build --release --lib --target $t"
    cargo build --release --lib --manifest-path "$CRATE/Cargo.toml" --target "$t"
done

mkdir -p "$OUT"
rm -rf "$OUT/Shellac.xcframework"

# Combine simulator slices (arm64 + x86_64) into one fat static library.
mkdir -p "$BUILD/sim-universal"
lipo -create \
    "$BUILD/aarch64-apple-ios-sim/release/libshellac.a" \
    "$BUILD/x86_64-apple-ios/release/libshellac.a" \
    -output "$BUILD/sim-universal/libshellac.a"

# Prepare a headers directory with the cbindgen-generated header + a module
# map so Swift can `import Shellac`.
HDR="$BUILD/headers"
rm -rf "$HDR"
mkdir -p "$HDR"
cp "$CRATE/include/shellac.h" "$HDR/"
printf 'module Shellac {\n    header "shellac.h"\n    export *\n}\n' > "$HDR/module.modulemap"

echo "==> xcodebuild -create-xcframework"
xcodebuild -create-xcframework \
    -library "$BUILD/aarch64-apple-ios/release/libshellac.a" -headers "$HDR" \
    -library "$BUILD/sim-universal/libshellac.a" -headers "$HDR" \
    -output "$OUT/Shellac.xcframework"

echo "OK: $OUT/Shellac.xcframework"
