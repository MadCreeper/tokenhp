#!/usr/bin/env bash
# install-latest.sh -- fetch and install the latest tokenhp release.
#
# Pulls the most recent release (including prereleases) from GitHub,
# replaces /Applications/HPBar.app, removes the Gatekeeper quarantine, and
# launches. No deps beyond what ships with macOS + Xcode CLT.

set -euo pipefail

REPO="MadCreeper/tokenhp"
INSTALL_DIR="/Applications"
APP_NAME="HPBar.app"

echo "==> Latest release tag from ${REPO}..."
TAG=$(curl -sSfL "https://api.github.com/repos/${REPO}/releases?per_page=1" \
        | grep -m1 '"tag_name":' \
        | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/' || true)
if [ -z "$TAG" ]; then
    echo "FAIL: could not determine latest tag. Does ${REPO} have any releases?" >&2
    exit 1
fi
URL="https://github.com/${REPO}/releases/download/${TAG}/HPBar.zip"
echo "    ${TAG}"
echo "    ${URL}"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "==> Stopping any running HPBar..."
# killall (not pkill) -- macOS pkill regex can choke with "illegal byte
# sequence" depending on LC_CTYPE. killall does literal name match.
killall HPBar 2>/dev/null && sleep 1 || true

echo "==> Downloading..."
curl -fSL -o "${TMP}/HPBar.zip" "${URL}"

echo "==> Installing to ${INSTALL_DIR}/${APP_NAME}..."
rm -rf "${INSTALL_DIR}/${APP_NAME}"
unzip -q "${TMP}/HPBar.zip" -d "${INSTALL_DIR}/"

echo "==> Lifting Gatekeeper quarantine..."
xattr -dr com.apple.quarantine "${INSTALL_DIR}/${APP_NAME}"

echo "==> Launching..."
open "${INSTALL_DIR}/${APP_NAME}"

echo "OK: installed ${TAG}"
