#!/usr/bin/env bash
# Reports gzip sizes of the built bundle and fails if a budget is exceeded.
# Budgets (TECHNICAL_ARCHITECTURE §5.4): game wasm ≤ 2.5 MB gzip, total ≤ 3.5 MB gzip.
set -euo pipefail
DIST="${1:-dist}"
GAME_BUDGET=$((2500 * 1024))
TOTAL_BUDGET=$((3500 * 1024))
total=0
game=0
printf "%-48s %12s %12s\n" "file" "raw" "gzip"
for f in "$DIST"/*.wasm "$DIST"/*.js "$DIST"/*.html; do
  [ -f "$f" ] || continue
  raw=$(wc -c < "$f")
  gz=$(gzip -9 -c "$f" | wc -c)
  total=$((total + gz))
  case "$f" in *_bg.wasm) game=$gz ;; esac
  printf "%-48s %12d %12d\n" "$(basename "$f")" "$raw" "$gz"
done
printf "%-48s %12s %12d\n" "TOTAL (gzip)" "" "$total"
status=0
if [ "$game" -gt "$GAME_BUDGET" ]; then echo "::error::game wasm $game B gzip exceeds budget $GAME_BUDGET B"; status=1; fi
if [ "$total" -gt "$TOTAL_BUDGET" ]; then echo "::error::total $total B gzip exceeds budget $TOTAL_BUDGET B"; status=1; fi
exit $status
