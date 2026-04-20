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
  mac-arm64|mac-x64|linux-x64|linux-arm64|win-x64|win-arm64)
    # bblanchon/pdfium-binaries ships every platform as .tgz
    # (including Windows — despite what older docs might say).
    ARCHIVE="pdfium-${PLATFORM}.tgz"
    EXTRACT_CMD="tar -xzf"
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
    SRC="$WORKDIR/lib/libpdfium.dylib"
    EXPECTED="$DEST/libpdfium.dylib"
    ;;
  linux-x64|linux-arm64)
    SRC="$WORKDIR/lib/libpdfium.so"
    EXPECTED="$DEST/libpdfium.so"
    ;;
  win-x64|win-arm64)
    SRC="$WORKDIR/bin/pdfium.dll"
    EXPECTED="$DEST/pdfium.dll"
    ;;
esac

if [[ ! -f "$SRC" ]]; then
  echo "[fetch-pdfium] ERROR: expected source $SRC missing after extract" >&2
  echo "[fetch-pdfium] WORKDIR contents:" >&2
  find "$WORKDIR" -type f >&2
  exit 1
fi

cp "$SRC" "$EXPECTED"

if [[ ! -f "$EXPECTED" ]]; then
  echo "[fetch-pdfium] ERROR: copy failed — $EXPECTED does not exist" >&2
  exit 1
fi

echo "[fetch-pdfium] installed to $DEST:"
ls -lh "$DEST"
