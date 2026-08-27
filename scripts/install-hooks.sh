#!/usr/bin/env bash
# Install tracked git hooks from .githooks/ into the current repo.
# Idempotent: safe to run multiple times.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

chmod +x .githooks/pre-commit
git config core.hooksPath .githooks

echo "Installed git hooks from .githooks/"
echo "  core.hooksPath = $(git config core.hooksPath)"
echo "  pre-commit     = $(git config core.hooksPath)/pre-commit"
