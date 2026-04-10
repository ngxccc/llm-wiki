#!/bin/bash
set -euo pipefail

echo "🛡️ Running Local Immune System (Fmt, Clippy, Tests)..."

if command -v cargo >/dev/null 2>&1; then
    CARGO_BIN="cargo"
elif [ -x "$HOME/.cargo/bin/cargo" ]; then
    CARGO_BIN="$HOME/.cargo/bin/cargo"
else
    echo "❌ cargo not found in PATH or ~/.cargo/bin"
    exit 1
fi

# 1. Check formatting
if ! "$CARGO_BIN" fmt -- --check; then
    echo "❌ Code formatting failed! Run 'cargo fmt' to fix it."
    exit 1
fi

# 2. Strict Clippy (Deny warnings, catch concurrency bugs)
if ! "$CARGO_BIN" clippy -- -D warnings -W clippy::pedantic -W clippy::await_holding_lock -W clippy::unwrap_used; then
    echo "❌ Clippy found bad practices or concurrency risks! Fix them."
    exit 1
fi

# 3. Run unit tests
if ! "$CARGO_BIN" test --release; then
    echo "❌ Unit tests failed! Code is broken."
    exit 1
fi

echo "✅ All checks passed! Committing..."
