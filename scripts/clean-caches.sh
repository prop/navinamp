#!/usr/bin/env bash
# Frees most of target/ without forcing a full rebuild on the next
# `tauri dev`:
#   - incremental compilation caches (the biggest chunk after heavy
#     iteration — cargo never prunes them itself)
#   - test artifacts
# Keeps compiled dependencies + the release profile intact.
#
# Usage: ./scripts/clean-caches.sh [full]
#   full  -> also removes the debug profile (next dev build recompiles
#            everything, ~minutes) and the release profile.

set -e
cd "$(dirname "$0")/.."

if [ "${1:-}" = "full" ]; then
  echo "full clean — next build recompiles everything"
  cargo clean
  exit 0
fi

BEFORE=$(du -sh target 2>/dev/null | cut -f1 || echo "0")
rm -rf \
  target/debug/incremental \
  target/release/incremental \
  target/debug/test \
  target/release/test
AFTER=$(du -sh target 2>/dev/null | cut -f1 || echo "0")
echo "target: $BEFORE -> $AFTER (incremental caches removed)"
