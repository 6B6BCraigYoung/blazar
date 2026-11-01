#!/usr/bin/env bash
cd "$(dirname "$0")/../.."
PY="python3 -I scripts/faults/faults.py"
OUT=${OUT:-target/faults/repeat.tsv}
for kind in "hub edit" "sleep chat"; do
  for i in $(seq 1 ${N:-22}); do
    before=$($PY pid)
    [ -z "$before" ] && { $PY start >/dev/null 2>&1; before=$($PY pid); }
    res=$($PY run $kind 2>&1 | tr '\n' ' ')
    sleep 2
    after=$($PY pid)
    printf '%s\t%s\t%s\t%s\t%s\n' "$kind" "$i" "$before" "${after:-gone}" "$res" >> $OUT
    [ -z "$after" ] && $PY start >/dev/null 2>&1
  done
done
echo finished
