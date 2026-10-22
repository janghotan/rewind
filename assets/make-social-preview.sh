#!/usr/bin/env bash
# Regenerate assets/social-preview.png (1280x640, GitHub's recommended size).
#
# Requires ImageMagick and the JetBrains Mono / DejaVu fonts. GitHub has no API
# for social previews: upload the PNG under Settings -> Social preview.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out="$root/assets/social-preview.png"
mkdir -p "$root/assets"

mono=/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-ExtraBold.ttf
mono_regular=/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf
mono_medium=/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Medium.ttf
sans=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf

# Soft teal glow in the upper right, over near-black. A blurred ellipse keeps
# the falloff entirely on-canvas, unlike a composited gradient rectangle.
convert -size 1280x640 xc:'#080b0f' \
    \( -size 320x160 xc:black -fill '#15544f' \
       -draw "ellipse 265,-10 118,80 0,360" -blur 0x14 -resize 1280x640\! \) \
    -compose screen -composite \
    \
    -stroke '#1b2733' -strokewidth 1 -fill none \
    -draw "line 0,560 1280,560" \
    \
    -fill '#0d141c' -stroke '#22303f' -strokewidth 2 \
    -draw "roundrectangle 690,112 1206,548 18,18" \
    -stroke none \
    -fill '#ff5f56' -draw "circle 722,146 729,146" \
    -fill '#ffbd2e' -draw "circle 746,146 753,146" \
    -fill '#27c93f' -draw "circle 770,146 777,146" \
    \
    -font "$mono_regular" -pointsize 21 -fill '#5eead4' -annotate +722+238 '$' \
    -fill '#e6edf3' -annotate +746+238 'rm -rf src/' \
    -fill '#5eead4' -annotate +722+314 '$' \
    -fill '#e6edf3' -annotate +746+314 'rewind undo' \
    -fill '#4ade80' -annotate +722+368 'rewind: undone #1 rm -rf src/' \
    -fill '#5eead4' -annotate +722+444 '$' \
    -fill '#e6edf3' -annotate +746+444 'ls' \
    -fill '#9fb0c0' -annotate +722+492 'src/' \
    \
    -font "$sans" -pointsize 112 -fill '#34d399' -annotate +72+218 '↶' \
    -font "$mono" -pointsize 124 -fill '#f2f7fb' -annotate +178+210 'rewind' \
    \
    -font "$sans" -pointsize 34 -fill '#c7d5e3' -annotate +76+306 'Undo for destructive' \
    -annotate +76+352 'shell commands.' \
    \
    -font "$mono_medium" -pointsize 17 -fill '#7dd3c7' -annotate +78+420 'SINGLE STATIC BINARY' \
    -fill '#526172' -annotate +348+420 'BASH  ZSH  FISH' \
    \
    -fill '#0f1a20' -stroke '#24413f' -strokewidth 1 \
    -draw "roundrectangle 76,458 158,502 10,10" \
    -draw "roundrectangle 168,458 250,502 10,10" \
    -draw "roundrectangle 260,458 342,502 10,10" \
    -draw "roundrectangle 352,458 458,502 10,10" \
    -draw "roundrectangle 468,458 574,502 10,10" \
    -draw "roundrectangle 584,458 666,502 10,10" \
    -stroke none \
    -font "$mono_regular" -pointsize 19 -fill '#c9d6e2' \
    -annotate +92+487 'rm' \
    -annotate +184+487 'mv' \
    -annotate +276+487 'cp' \
    -annotate +368+487 'chmod' \
    -annotate +484+487 'chown' \
    -annotate +600+487 'git' \
    \
    -font "$mono_regular" -pointsize 17 -fill '#5d6f80' -annotate +76+604 'github.com/janghotan/rewind' \
    -annotate +1128+604 'MIT' \
    "$out"

identify "$out"
echo "wrote $out"
