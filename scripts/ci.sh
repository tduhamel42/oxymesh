#!/usr/bin/env bash
# Local checks, identical to CI. Usage: scripts/ci.sh
# CI_SKIP_FUZZ=1 skips the fuzz smoke test; FUZZ_SECS sets its duration per target (60 s).
# CI_SKIP_FIRMWARE=1 skips the firmware build (requires the Xtensa `esp` toolchain).
set -euo pipefail
cd "$(dirname "$0")/.."

# Core crates: no_std, no allocator, no floats.
NOSTD=(mc-core mc-air mc-proto mc-node)
step() { printf '\n== %s\n' "$*"; }

step "fmt"
cargo fmt --all -- --check

step "clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "test"
cargo test --workspace --quiet

step "no_std (thumbv7em-none-eabihf, no allocator)"
pkgs=()
for p in "${NOSTD[@]}"; do pkgs+=(-p "$p"); done
cargo build --quiet --target thumbv7em-none-eabihf "${pkgs[@]}"

step "no_std with alloc (mc-ui)"
cargo build --quiet --target thumbv7em-none-eabihf -p mc-ui

step "no floats in the no_std crates"
found=0
for p in "${NOSTD[@]}"; do
  # Ignore comment lines.
  if grep -rnwE 'f32|f64' --include='*.rs' "crates/$p/src" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//'; then
    found=1
  fi
done
if [ "$found" -ne 0 ]; then echo "float found in a no_std crate" >&2; exit 1; fi

step "firmware (esp32s3)"
if [ "${CI_SKIP_FIRMWARE:-0}" != "1" ] && [ -f firmware/esp32s3/Cargo.toml ] && rustup toolchain list | grep -q '^esp'; then
  # The Xtensa linker comes from the espup environment.
  [ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
  (cd firmware/esp32s3 && cargo build --quiet --release)
else
  echo "skipped: no firmware, CI_SKIP_FIRMWARE=1 or esp toolchain missing (espup install --targets esp32s3)"
fi

if [ "${CI_SKIP_FUZZ:-0}" != "1" ] && [ -f fuzz/Cargo.toml ]; then
  step "fuzz smoke test (${FUZZ_SECS:-60} s per target)"
  scripts/fuzz.sh "${FUZZ_SECS:-60}"
fi

printf '\nOK\n'
