#!/usr/bin/env bash
# Regenerate the ZIM layout-regression corpus used by `cargo run --example corpus`.
#
# Each title is extracted (HTML + its stylesheets) into its own subdir so the
# corpus example can render the lot headlessly to PNGs for visual review.
#
#   ./tools/extract-corpus.sh [CORPUS_DIR] [ZIM_PATH]
#
# Defaults: CORPUS_DIR=/tmp/corpus, ZIM=~/test-vault/wikipedia_en_all_nopic_2026-03.zim
# Then:  CORPUS_DIR=/tmp/corpus cargo run --example corpus
set -euo pipefail

CORPUS_DIR="${1:-/tmp/corpus}"
ZIM="${2:-$HOME/test-vault/wikipedia_en_all_nopic_2026-03.zim}"

# Resolve zxr: walk up from this script looking for a built `target/debug/zxr`,
# then fall back to PATH. (htmlview is a nested submodule, so don't assume depth.)
ZXR=""
d="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
while [ "$d" != "/" ]; do
  if [ -x "$d/target/debug/zxr" ]; then ZXR="$d/target/debug/zxr"; break; fi
  d="$(dirname "$d")"
done
[ -n "$ZXR" ] || ZXR="$(command -v zxr || true)"
[ -n "$ZXR" ] || { echo "zxr not found — build it first:  cargo build -p zxr --bin zxr"; exit 1; }

# Articles chosen to stress distinct layout paths: float infoboxes (country/bio),
# chembox, taxobox, math, road junction boxes, sidebars/navboxes, wide tables.
TITLES=(
  "Water" "Commonwealth Games" "Mechanosynthesis" "New Jersey Route 24"
  "Albert Einstein" "Australia" "France" "Lion" "Mount Everest"
  "Aspirin" "Ethanol" "Sodium chloride" "Pythagorean theorem"
  "The Beatles" "Python (programming language)" "Periodic table"
  "Chess" "World War II" "DNA"
)

mkdir -p "$CORPUS_DIR"
for t in "${TITLES[@]}"; do
  slug="$(echo "$t" | tr ' ()' '___' | tr -s '_')"
  if "$ZXR" --zim "$ZIM" --extract "$t" --out "$CORPUS_DIR/$slug" >/dev/null 2>&1; then
    echo "OK   $slug"
  else
    echo "MISS $t (not in this ZIM)"
  fi
done
echo
echo "corpus -> $CORPUS_DIR"
echo "render:  CORPUS_DIR=$CORPUS_DIR cargo run --example corpus"
