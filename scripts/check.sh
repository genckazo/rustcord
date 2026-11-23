#!/usr/bin/env bash
# Runs every repository check locally: fmt, clippy, tests, cargo-deny,
# cargo-audit and a release build that reports the binary size.
set -euo pipefail
cd "$(dirname "$0")/.."

for tool in cargo-deny cargo-audit; do
  if ! command -v "$tool" >/dev/null; then
    echo "$tool is not installed; run: cargo install --locked cargo-deny cargo-audit" >&2
    exit 1
  fi
done

step() { printf '\n==> %s\n' "$*"; "$@"; }

step cargo fmt --check
step cargo clippy --locked --all-targets -- -D warnings
step cargo test --locked
step cargo deny --locked check
step cargo audit
step cargo build --locked --release

bin=target/release/rustcord
bytes=$(wc -c < "$bin" | tr -d ' ')
printf '\nRelease binary %s: %s bytes (%s MiB)\n' "$bin" "$bytes" \
  "$(awk -v b="$bytes" 'BEGIN { printf "%.2f", b / 1048576 }')"
echo "All checks passed."
