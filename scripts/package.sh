#!/usr/bin/env bash
# Release packaging script for HoloFan (Linux tar.gz and Windows portable zip).
# Produces production-ready standalone distribution archives with checksums.
#
# Usage:
#   WIN_VM_NAME=<ssh-alias> ./scripts/package.sh  # build both packages
#   ./scripts/package.sh --linux     # build only Linux package
#   ./scripts/package.sh --windows   # build only Windows package
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DIST="${DIST:-$ROOT/dist}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BUILD_LINUX=0
BUILD_WINDOWS=0
WIN_SHARED_STAGE=""

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --linux)
      BUILD_LINUX=1
      shift
      ;;
    --windows)
      BUILD_WINDOWS=1
      shift
      ;;
    --all)
      BUILD_LINUX=1
      BUILD_WINDOWS=1
      shift
      ;;
    --dist)
      DIST="$2"
      shift 2
      ;;
    -h|--help)
      sed -n '2,11p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

# Default to building both if neither explicitly selected
if [[ "$BUILD_LINUX" -eq 0 && "$BUILD_WINDOWS" -eq 0 ]]; then
  BUILD_LINUX=1
  BUILD_WINDOWS=1
fi

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
if [[ -z "$VERSION" ]]; then
  echo "Error: could not determine version from Cargo.toml" >&2
  exit 1
fi

ARCH="${ARCH:-x86_64}"
mkdir -p "$DIST"
TMP_STAGE="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP_STAGE"
  if [[ -n "$WIN_SHARED_STAGE" ]]; then
    rm -rf "$WIN_SHARED_STAGE"
  fi
}
trap cleanup EXIT

echo "==> Packaging HoloFan v$VERSION ($ARCH)"

# -----------------------------------------------------------------------------
# Linux Release Package (.tar.gz)
# -----------------------------------------------------------------------------
if [[ "$BUILD_LINUX" -eq 1 ]]; then
  echo "--> Building Linux release binary ($ARCH)..."
  cargo build --release --locked --bins

  LINUX_BIN="$TARGET_DIR/release/holofan"
  if [[ ! -f "$LINUX_BIN" ]]; then
    echo "Error: Linux binary not found at $LINUX_BIN" >&2
    exit 1
  fi

  LINUX_STAGE="$TMP_STAGE/holofan-$VERSION-linux-$ARCH"
  rm -rf "$LINUX_STAGE"
  mkdir -p "$LINUX_STAGE/bin"
  mkdir -p "$LINUX_STAGE/share/applications"
  mkdir -p "$LINUX_STAGE/share/icons/hicolor/scalable/apps"
  mkdir -p "$LINUX_STAGE/share/icons/hicolor/256x256/apps"

  install -m 755 "$LINUX_BIN" "$LINUX_STAGE/bin/holofan"
  strip "$LINUX_STAGE/bin/holofan" 2>/dev/null || true

  if [[ -f "$ROOT/assets/holofan.desktop" ]]; then
    install -m 644 "$ROOT/assets/holofan.desktop" "$LINUX_STAGE/share/applications/holofan.desktop"
  fi
  if [[ -f "$ROOT/assets/icon.svg" ]]; then
    install -m 644 "$ROOT/assets/icon.svg" "$LINUX_STAGE/share/icons/hicolor/scalable/apps/holofan.svg"
  fi
  if [[ -f "$ROOT/assets/icon.png" ]]; then
    install -m 644 "$ROOT/assets/icon.png" "$LINUX_STAGE/share/icons/hicolor/256x256/apps/holofan.png"
  fi
  if [[ -f "$ROOT/README.md" ]]; then
    install -m 644 "$ROOT/README.md" "$LINUX_STAGE/README.md"
  fi
  if [[ -f "$ROOT/LICENSE" ]]; then
    install -m 644 "$ROOT/LICENSE" "$LINUX_STAGE/LICENSE"
  fi

  LINUX_ARCHIVE="$DIST/holofan-$VERSION-linux-$ARCH.tar.gz"
  # Keep package files at the archive root. File managers already create a folder
  # named after the archive, so embedding another same-named directory nests twice.
  tar -czf "$LINUX_ARCHIVE" -C "$LINUX_STAGE" .
  echo "Wrote $LINUX_ARCHIVE"

  # Smoke test on host
  "$LINUX_STAGE/bin/holofan" --version
fi

# -----------------------------------------------------------------------------
# Windows Release Package (.zip)
# -----------------------------------------------------------------------------
if [[ "$BUILD_WINDOWS" -eq 1 ]]; then
  WIN_TARGET="${WIN_TARGET:-x86_64-pc-windows-msvc}"
  if [[ "$WIN_TARGET" != "x86_64-pc-windows-msvc" ]]; then
    echo "Error: native Windows VM builds currently support x86_64-pc-windows-msvc only" >&2
    exit 1
  fi
  WIN_VM="${WIN_VM_NAME:-}"
  if [[ -z "$WIN_VM" ]]; then
    echo "Error: set WIN_VM_NAME to the SSH host alias for the Windows VM" >&2
    exit 1
  fi
  WIN_SHARED_DIR="${WIN_SHARED_DIR:-${VM_SHARE_DIR:-$HOME/vm-share/windows}}"
  if [[ ! -d "$WIN_SHARED_DIR" ]]; then
    echo "Error: Windows VM shared directory not found: $WIN_SHARED_DIR" >&2
    exit 1
  fi
  if ! command -v vm-ssh >/dev/null 2>&1; then
    echo "Error: vm-ssh is required to build Windows artifacts in the VM" >&2
    exit 1
  fi

  echo "--> Building Windows release binary ($WIN_TARGET) in VM $WIN_VM..."
  WIN_SHARED_STAGE="$(mktemp -d "$WIN_SHARED_DIR/holofan-build.XXXXXX")"
  WIN_SHARED_NAME="$(basename "$WIN_SHARED_STAGE")"
  git archive --format=tar.gz --output="$WIN_SHARED_STAGE/holofan-src.tar.gz" HEAD
  cp "$ROOT/scripts/build-windows-vm.cmd" "$WIN_SHARED_STAGE/build-windows-vm.cmd"
  vm-ssh "$WIN_VM" "cmd /c Z:\\$WIN_SHARED_NAME\\build-windows-vm.cmd"
  WIN_BIN="$WIN_SHARED_STAGE/holofan-windows-x86_64.exe"
  if [[ ! -f "$WIN_BIN" ]]; then
    echo "Error: Windows VM did not produce $WIN_BIN" >&2
    exit 1
  fi

  WIN_STAGE="$TMP_STAGE/holofan-$VERSION-windows-$ARCH"
  rm -rf "$WIN_STAGE"
  mkdir -p "$WIN_STAGE"

  install -m 755 "$WIN_BIN" "$WIN_STAGE/holofan.exe"
  if [[ -f "$ROOT/README.md" ]]; then
    install -m 644 "$ROOT/README.md" "$WIN_STAGE/README.md"
  fi
  if [[ -f "$ROOT/LICENSE" ]]; then
    install -m 644 "$ROOT/LICENSE" "$WIN_STAGE/LICENSE"
  fi

  WIN_ARCHIVE="$DIST/holofan-$VERSION-windows-$ARCH.zip"
  # Keep package files at the archive root for a single extraction folder.
  (cd "$WIN_STAGE" && zip -rq "$WIN_ARCHIVE" *)
  echo "Wrote $WIN_ARCHIVE"

fi

# -----------------------------------------------------------------------------
# Generate SHA256 Checksums
# -----------------------------------------------------------------------------
echo "--> Generating SHA256 checksums..."
(
  cd "$DIST"
  sha256sum holofan-"$VERSION"-* > SHA256SUMS
)
echo "Wrote $DIST/SHA256SUMS"

echo "==> Done. Release assets created in $DIST:"
ls -lh "$DIST"
