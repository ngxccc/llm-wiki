#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mkdir -p "$REPO_ROOT/data/raw"

case "$(uname -s)" in
    Linux|Darwin)
        chmod -R a-w "$REPO_ROOT/data/raw"
        ;;
    *)
        echo "warning: unsupported OS for chmod hardening; configure read-only policy manually"
        ;;
esac

if [ -d "$REPO_ROOT/.git/hooks" ]; then
    cp "$REPO_ROOT/scripts/pre-commit.sh" "$REPO_ROOT/.git/hooks/pre-commit"
    chmod +x "$REPO_ROOT/.git/hooks/pre-commit"
fi

chmod +x "$REPO_ROOT/scripts/pre-commit.sh"

echo "Security bootstrap completed: data/raw hardened and pre-commit installed."
