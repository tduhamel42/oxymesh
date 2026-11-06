#!/usr/bin/env bash
# Vérifications locales, identiques à la CI. Usage : scripts/ci.sh
# CI_SKIP_FUZZ=1 saute la fumée de fuzz ; FUZZ_SECS règle sa durée par cible (60 s).
# CI_SKIP_FIRMWARE=1 saute le build du firmware (chaîne Xtensa `esp` requise).
set -euo pipefail
cd "$(dirname "$0")/.."

# Crates du cœur : no_std, sans allocateur, sans flottants.
NOSTD=(mc-core mc-air mc-proto mc-node)
step() { printf '\n== %s\n' "$*"; }

step "fmt"
cargo fmt --all -- --check

step "clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "test"
cargo test --workspace --quiet

step "no_std (thumbv7em-none-eabihf, sans allocateur)"
pkgs=()
for p in "${NOSTD[@]}"; do pkgs+=(-p "$p"); done
cargo build --quiet --target thumbv7em-none-eabihf "${pkgs[@]}"

step "pas de flottants dans les crates no_std"
found=0
for p in "${NOSTD[@]}"; do
  # Ignore les lignes de commentaire.
  if grep -rnwE 'f32|f64' --include='*.rs' "crates/$p/src" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//'; then
    found=1
  fi
done
if [ "$found" -ne 0 ]; then echo "flottant trouvé dans une crate no_std" >&2; exit 1; fi

step "firmware (esp32s3)"
if [ "${CI_SKIP_FIRMWARE:-0}" != "1" ] && [ -f firmware/esp32s3/Cargo.toml ] && rustup toolchain list | grep -q '^esp'; then
  # L'éditeur de liens Xtensa vient de l'environnement d'espup.
  [ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
  (cd firmware/esp32s3 && cargo build --quiet --release)
else
  echo "sauté : pas de firmware, CI_SKIP_FIRMWARE=1 ou chaîne esp absente (espup install --targets esp32s3)"
fi

if [ "${CI_SKIP_FUZZ:-0}" != "1" ] && [ -f fuzz/Cargo.toml ]; then
  step "fumée de fuzz (${FUZZ_SECS:-60} s par cible)"
  scripts/fuzz.sh "${FUZZ_SECS:-60}"
fi

printf '\nOK\n'
