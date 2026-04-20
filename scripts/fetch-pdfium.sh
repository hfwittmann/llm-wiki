#!/usr/bin/env bash
# Download the platform-appropriate PDFium dynamic library from
# bblanchon/pdfium-binaries into src-tauri/pdfium/. Invoked by CI
# before `tauri build` and can also be run locally.
#
# Usage:
#   scripts/fetch-pdfium.sh [platform]
#
# If platform is omitted, it is auto-detected. Supported platforms:
#   mac-arm64, mac-x64, win-x64, linux-x64
set -euo pipefail

PLATFORM="${1:-}"

if [[ -z "$PLATFORM" ]]; then
  OS="$(uname -s)"
  ARCH="$(uname -m)"
  case "$OS" in
    Darwin)
      case "$ARCH" in
        arm64)  PLATFORM="mac-arm64" ;;
        x86_64) PLATFORM="mac-x64" ;;
        *) echo "Unsupported macOS arch: $ARCH" >&2; exit 1 ;;
      esac
      ;;
    Linux)
      case "$ARCH" in
        x86_64) PLATFORM="linux-x64" ;;
        aarch64) PLATFORM="linux-arm64" ;;
        *) echo "Unsupported Linux arch: $ARCH" >&2; exit 1 ;;
      esac
      ;;
    MINGW*|MSYS*|CYGWIN*)
      PLATFORM="win-x64"
      ;;
    *)
      echo "Unsupported OS: $OS" >&2; exit 1 ;;
  esac
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$REPO_ROOT/src-tauri/pdfium"
mkdir -p "$DEST"

case "$PLATFORM" in
  mac-arm64|mac-x64|linux-x64|linux-arm64)
    ARCHIVE="pdfium-${PLATFORM}.tgz"
    EXTRACT_CMD="tar -xzf"
    ;;
  win-x64)
    ARCHIVE="pdfium-${PLATFORM}.zip"
    EXTRACT_CMD="unzip -o"
    ;;
  *)
    echo "Unsupported platform: $PLATFORM" >&2; exit 1 ;;
esac

URL="https://github.com/bblanchon/pdfium-binaries/releases/latest/download/${ARCHIVE}"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

echo "[fetch-pdfium] platform=$PLATFORM"
echo "[fetch-pdfium] downloading $URL"
curl -fsSL "$URL" -o "$WORKDIR/$ARCHIVE"

echo "[fetch-pdfium] extracting"
(cd "$WORKDIR" && $EXTRACT_CMD "$ARCHIVE")

# Copy just the dynamic library into src-tauri/pdfium/.
# The archives place it at lib/libpdfium.<dylib|so> on Unix and at
# bin/pdfium.dll on Windows.
case "$PLATFORM" in
  mac-arm64|mac-x64)
    cp "$WORKDIR/lib/libpdfium.dylib" "$DEST/libpdfium.dylib"
    ;;
  linux-x64|linux-arm64)
    cp "$WORKDIR/lib/libpdfium.so" "$DEST/libpdfium.so"
    ;;
  win-x64)
    cp "$WORKDIR/bin/pdfium.dll" "$DEST/pdfium.dll"
    ;;
esac

echo "[fetch-pdfium] installed to $DEST:"
ls -lh "$DEST"
