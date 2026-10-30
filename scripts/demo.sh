#!/usr/bin/env bash
# Headless Kyte demo (section 12): no display, no tmux needed.
# Writes docs/demo/{demo.cast,frames/*.txt,steps.tsv}; with agg also demo.gif;
# with ffmpeg (and a gif) also demo.mp4 and one PNG per storyboard step.
# DEMO_OUT=dir overrides the output directory.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${DEMO_OUT:-docs/demo}"
mkdir -p "$OUT/frames"

written=()
skipped=()

cargo build --release
cargo run --release --example demo -- "$OUT"
written+=("$OUT/demo.cast" "$OUT/steps.tsv" "$OUT/frames/*.txt ($(ls "$OUT"/frames/*.txt | wc -l) frames)")

if command -v agg >/dev/null 2>&1; then
  agg --font-size 16 "$OUT/demo.cast" "$OUT/demo.gif"
  written+=("$OUT/demo.gif")
else
  rm -f "$OUT/demo.gif"
  skipped+=("demo.gif (agg not installed: cargo install --locked --git https://github.com/asciinema/agg)")
fi

if command -v ffmpeg >/dev/null 2>&1; then
  if [ -f "$OUT/demo.gif" ]; then
    ffmpeg -y -loglevel error -i "$OUT/demo.gif" -movflags +faststart -pix_fmt yuv420p \
      -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" "$OUT/demo.mp4"
    written+=("$OUT/demo.mp4")
    rm -f "$OUT"/[0-9][0-9]-*.png
    n=0
    while IFS=$'\t' read -r step secs; do
      [ "$step" = "step" ] && continue
      # agg writes a frame only when the screen changes; fps=20 resamples to a
      # constant rate so a seek inside a long hold (or the last frame) still hits.
      ffmpeg -y -loglevel error -i "$OUT/demo.gif" -vf fps=20 -ss "$secs" -frames:v 1 "$OUT/$step.png"
      if [ -s "$OUT/$step.png" ]; then n=$((n + 1)); else echo "warning: no PNG for $step" >&2; fi
    done < "$OUT/steps.tsv"
    written+=("$OUT/NN-step.png ($n screenshots)")
  else
    skipped+=("demo.mp4 and PNG screenshots (they are rendered from demo.gif, which needs agg)")
  fi
else
  skipped+=("demo.mp4 and PNG screenshots (ffmpeg not installed)")
fi

echo
echo "Demo artifacts written:"
for a in "${written[@]}"; do echo "  $a"; done
if [ "${#skipped[@]}" -gt 0 ]; then
  echo "Skipped:"
  for s in "${skipped[@]}"; do echo "  $s"; done
fi
