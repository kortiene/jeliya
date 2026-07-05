#!/usr/bin/env bash
# Package this repo (no remote) as a single-file git bundle for machine B.
# See docs/realnet-runbook.md.
#
# Usage: scripts/make-bundle.sh [output-path]   (default: ./jeliya.bundle)
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${1:-$repo_root/jeliya.bundle}"

cd "$repo_root"
branch="$(git rev-parse --abbrev-ref HEAD)"
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "warning: uncommitted changes are NOT included in the bundle" >&2
fi

git bundle create "$out" "$branch"
git bundle verify "$out" >/dev/null

echo "bundle written: $out (branch: $branch, head: $(git rev-parse --short HEAD))"
echo ""
echo "copy it to machine B (scp/AirDrop/USB), then on B:"
echo "  git clone -b $branch $(basename "$out") jeliya && cd jeliya"
echo "  cargo build --workspace"
