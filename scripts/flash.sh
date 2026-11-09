#!/usr/bin/env bash
# Builds and flashes oxymesh with the board's partition table. Usage: scripts/flash.sh m5|heltec-v4 <port>
set -euo pipefail
cd "$(dirname "$0")/../firmware/esp32s3"
board="${1:?board: m5 or heltec-v4}"
port="${2:?serial port (ls /dev/cu.*)}"
case "$board" in
  m5) bin=mc-m5; table=partitions.csv; size=4mb ;;
  heltec-v4) bin=mc-heltec-v4; table=partitions-heltec-v4.csv; size=16mb ;;
  *) echo "unknown board: $board" >&2; exit 1 ;;
esac
[ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
cargo build --release --bin "$bin"
espflash flash --chip esp32s3 --port "$port" --flash-size "$size" --partition-table "$table" \
  "target/xtensa-esp32s3-none-elf/release/$bin"
