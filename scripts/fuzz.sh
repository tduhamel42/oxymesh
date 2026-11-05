#!/usr/bin/env bash
# Lance chaque cible de fuzz pendant N secondes (défaut 60). Usage : scripts/fuzz.sh [N] [cible…]
set -euo pipefail
cd "$(dirname "$0")/.."
if [ ! -f fuzz/Cargo.toml ]; then echo "pas de cibles de fuzz"; exit 0; fi
secs="${1:-60}"; shift || true
targets=("$@")
if [ "${#targets[@]}" -eq 0 ]; then
  while IFS= read -r t; do targets+=("$t"); done < <(cargo +nightly fuzz list)
fi
for t in "${targets[@]}"; do
  echo "-- $t (${secs} s)"
  seeds="fuzz/seeds/$t"
  mkdir -p "fuzz/corpus/$t"
  if [ -d "$seeds" ]; then
    cargo +nightly fuzz run "$t" "fuzz/corpus/$t" "$seeds" -- -max_total_time="$secs" -print_final_stats=1
  else
    cargo +nightly fuzz run "$t" "fuzz/corpus/$t" -- -max_total_time="$secs" -print_final_stats=1
  fi
done
