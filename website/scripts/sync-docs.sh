#!/usr/bin/env bash
# Copy the repository reference documents into the static site.
# The website reads these Markdown files at runtime; run this after editing docs/.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
dest="$root/website/docs"
mkdir -p "$dest"

files=(
  "$root/PRODUCT.md"
  "$root/docs/WORKSPACE.md"
  "$root/docs/RUNTIME.md"
  "$root/docs/SURFACE.md"
  "$root/docs/FRONTEND.md"
  "$root/docs/CATALOG.md"
  "$root/docs/CONFIG.md"
  "$root/docs/PROJECT-STRUCTURE.md"
  "$root/docs/USER-GUIDE.md"
)

for file in "${files[@]}"; do
  cp "$file" "$dest/$(basename "$file")"
done

printf 'Synced %d reference documents into %s\n' "${#files[@]}" "$dest"
