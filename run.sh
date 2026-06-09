#!/usr/bin/env bash
# One-click: kill any running HPBar, rebuild, and launch the fresh build.
set -euo pipefail
cd "$(dirname "$0")"

echo "▶︎ Stopping running HPBar instances…"
# Use killall (not pkill) — macOS pkill regex chokes on illegal byte sequences
# when LC_CTYPE isn't C, which surfaces as `pkill: Regular expression
# evaluation error`. killall does literal name match and avoids that.
killall HPBar 2>/dev/null || true
sleep 1

echo "▶︎ Generating project…"
xcodegen generate >/dev/null

echo "▶︎ Building…"
if ! xcodebuild -project HPBar.xcodeproj -scheme HPBar -configuration Debug build >/tmp/hpbar_build.log 2>&1; then
    echo "✗ Build failed:"
    grep -E "error:" /tmp/hpbar_build.log | head -20 || tail -20 /tmp/hpbar_build.log
    exit 1
fi

APP=$(xcodebuild -project HPBar.xcodeproj -scheme HPBar -configuration Debug -showBuildSettings 2>/dev/null \
    | awk -F' = ' '/ BUILT_PRODUCTS_DIR =/{d=$2} / FULL_PRODUCT_NAME =/{n=$2} END{print d"/"n}')

echo "▶︎ Launching $APP"
open "$APP"
echo "✓ Done."
