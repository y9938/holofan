#!/usr/bin/env bash
# Local CI quality gate: format, clippy, tests, and cross-platform check.
# Usage: ./scripts/ci.sh [--skip-tests] [--skip-clippy] [--skip-fmt]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SKIP_TESTS=0
SKIP_CLIPPY=0
SKIP_FMT=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-tests) SKIP_TESTS=1; shift ;;
    --skip-clippy) SKIP_CLIPPY=1; shift ;;
    --skip-fmt) SKIP_FMT=1; shift ;;
    -h|--help)
      echo "Usage: $0 [--skip-tests] [--skip-clippy] [--skip-fmt]"
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

echo "==> Running local CI gate in $ROOT"

# 1. Format check
if [[ "$SKIP_FMT" -eq 0 ]]; then
  echo "--> cargo fmt --all -- --check"
  cargo fmt --all -- --check
fi

# 2. Clippy
if [[ "$SKIP_CLIPPY" -eq 0 ]]; then
  echo "--> cargo clippy --workspace --locked --all-targets -- -D warnings"
  cargo clippy --workspace --locked --all-targets --keep-going -- -D warnings
fi

# 3. Tests
if [[ "$SKIP_TESTS" -eq 0 ]]; then
  echo "--> cargo test --workspace --locked"
  cargo test --workspace --locked
fi

echo "==> All CI checks passed successfully!"
