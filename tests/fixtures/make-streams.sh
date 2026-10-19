#!/usr/bin/env bash
# Regenerates the *.hevc test streams (see README.md). Needs ffmpeg with libx265.
set -euo pipefail
FFMPEG="${FFMPEG:-ffmpeg}"
cd "$(dirname "$0")"

SRC='color=c=0xc83c28:s=32x24:d=1,format=rgb24[r];color=c=0x3cb45a:s=32x24:d=1,format=rgb24[g];color=c=0x3250c8:s=32x24:d=1,format=rgb24[b];color=c=0xdcc83c:s=32x24:d=1,format=rgb24[y];[r][g]hstack[t];[b][y]hstack[u];[t][u]vstack'

gen() { # name filters pix_fmt
  "$FFMPEG" -hide_banner -loglevel error -y -filter_complex "$SRC,$2[v]" -map "[v]" -frames:v 1 \
    -c:v libx265 -pix_fmt "$3" -x265-params "log-level=error:repeat-headers=1:aud=0:keyint=1:crf=10" \
    -f hevc "$1.hevc"
}

gen quad601_8     "scale=out_color_matrix=bt601:out_range=tv,format=yuv420p" yuv420p
gen quad709_8     "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p" yuv420p
gen quad601full_8 "scale=out_color_matrix=bt601:out_range=pc,format=yuv420p" yuv420p
gen quad2020_10   "scale=out_color_matrix=bt2020:out_range=tv,format=yuv420p10le" yuv420p10le
gen quad601_gray  "scale=out_range=pc,format=gray" gray
gen quad_50x34    "scale=50:34:flags=neighbor,scale=out_color_matrix=bt601:out_range=tv,format=yuv420p" yuv420p
