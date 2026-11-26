#!/usr/bin/env bash
# Build osearch (release, with the stealth fetcher) and optionally install it.
# Needs: Rust, CMake, LLVM (libclang for bindgen), NASM on Windows (BoringSSL
# assembly); Node.js and npm for the web UI (skipped with --no-default-features).
#
# Install folder: OSEARCH_INSTALL_DIR, or set it in build.local.sh (sourced when
# present, never published). Without one, the binary stays in target/release.
set -euo pipefail
cd "$(dirname "$0")"

[ -f build.local.sh ] && source build.local.sh

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) WINDOWS=1 ;;
  *) WINDOWS=0 ;;
esac

if [ "$WINDOWS" = 1 ]; then
  LLVM_BIN="${OSEARCH_LLVM_DIR:-/c/Program Files/LLVM/bin}"
  [ -d "$LLVM_BIN" ] || { echo "LLVM not found at $LLVM_BIN (set OSEARCH_LLVM_DIR)"; exit 1; }
  export LIBCLANG_PATH="$(cygpath -w "$LLVM_BIN")"
  # NASM: PATH first, then the standard install folders, then OSEARCH_NASM_DIR.
  if ! command -v nasm >/dev/null 2>&1; then
    for d in "${OSEARCH_NASM_DIR:-}" "/c/Program Files/NASM" "$(cygpath -u "${LOCALAPPDATA:-C:/}")/bin/NASM"; do
      if [ -n "$d" ] && [ -x "$d/nasm.exe" ]; then export PATH="$d:$PATH"; break; fi
    done
  fi
  command -v nasm >/dev/null 2>&1 || { echo "NASM not found (winget install NASM.NASM)"; exit 1; }
  export CMAKE_ASM_NASM_COMPILER="$(cygpath -w "$(command -v nasm)")"
fi

# The web UI is built into the binary; build its files first unless the
# `ui` feature is switched off.
if [[ " $* " != *" --no-default-features "* ]] && [ -d ui ]; then
  command -v npm >/dev/null 2>&1 || { echo "npm not found: install Node.js, or build with --no-default-features"; exit 1; }
  (cd ui && { [ -d node_modules ] || npm ci; } && npm run build)
fi

cargo build --release "$@"

EXE=osearch
[ "$WINDOWS" = 1 ] && EXE=osearch.exe
DEST="${OSEARCH_INSTALL_DIR:-}"
if [ -n "$DEST" ] && [ -d "$DEST" ]; then
  # On Windows a running osearch (the web UI, a background fetch) keeps its
  # exe open: an open exe can be renamed but not replaced, so it moves aside.
  # Copies left by an earlier install go first, unless still running.
  if [ "$WINDOWS" = 1 ] && [ -f "$DEST/$EXE" ]; then
    for old in "$DEST"/osearch.old*.exe; do
      [ -e "$old" ] && rm -f "$old" 2>/dev/null
    done
    aside="$DEST/osearch.old.exe"
    [ -e "$aside" ] && aside="$DEST/osearch.old.$(date +%s).exe"
    mv "$DEST/$EXE" "$aside"
  fi
  cp "target/release/$EXE" "$DEST/$EXE"
  echo "installed: $DEST/$EXE"
else
  echo "built: target/release/$EXE (set OSEARCH_INSTALL_DIR to copy it somewhere on PATH)"
fi
