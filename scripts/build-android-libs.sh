#!/usr/bin/env bash
# Cross-compiles the recall-core cdylib for arm64-v8a + x86_64 and stages every
# native .so (recall_core + the prebuilt sherpa-onnx shared libs) into
# android/app/src/main/jniLibs/<abi>/ so the Gradle build packages them into the APK.
#
# Requires:
#   - ANDROID_NDK_HOME pointing at an installed NDK (r27+ recommended for 16KB pages)
#   - cargo-ndk on PATH (cargo install cargo-ndk)
#   - rustup targets aarch64-linux-android + x86_64-linux-android installed
#
# Usage:
#   scripts/build-android-libs.sh              # full cross-compile + stage + verify
#   scripts/build-android-libs.sh --verify-only # re-run only the preflight/alignment/
#                                                # NEEDED checks against the jniLibs already
#                                                # staged on disk (useful in CI after the
#                                                # build artifacts are restored/cached).
#
# See .superpowers/sdd/task-6-brief.md and task-6-report.md for the full rationale.
set -euo pipefail

VERIFY_ONLY=false
if [ "${1:-}" = "--verify-only" ]; then
  VERIFY_ONLY=true
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME must be set (path to the NDK, e.g. \$ANDROID_HOME/ndk/27.1.12297006)}"
if [ ! -d "$ANDROID_NDK_HOME" ]; then
  echo "error: ANDROID_NDK_HOME=$ANDROID_NDK_HOME does not exist" >&2
  exit 1
fi

if ! command -v cargo-ndk >/dev/null 2>&1; then
  echo "error: cargo-ndk not found on PATH (cargo install cargo-ndk)" >&2
  exit 1
fi

SHERPA_VERSION="1.13.4"
SHERPA_TARBALL="sherpa-onnx-v${SHERPA_VERSION}-android.tar.bz2"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_VERSION}/${SHERPA_TARBALL}"
THIRD_PARTY_DIR="$REPO_ROOT/third_party/sherpa-onnx-android"
JNI_LIBS_OUT="$REPO_ROOT/android/app/src/main/jniLibs"

ABIS=(arm64-v8a x86_64)

abi_triple() {
  case "$1" in
    arm64-v8a) echo "aarch64-linux-android" ;;
    x86_64) echo "x86_64-linux-android" ;;
    *) echo "error: no known Android triple for abi '$1'" >&2; exit 1 ;;
  esac
}

# Step 4: gate on 16KB page alignment of librecall_core.so (Play Store requires
# 16KB page-size support on arm64; a misaligned LOAD segment must fail the build,
# not just be printed).
check_alignment() {
  echo "== Step 4: verify 16KB page alignment of librecall_core.so =="
  local fail=0
  for abi in "${ABIS[@]}"; do
    local so="$JNI_LIBS_OUT/$abi/librecall_core.so"
    if [ ! -f "$so" ]; then
      echo "error: $so not found (run the build first, without --verify-only)" >&2
      exit 1
    fi
    echo "--- $abi: readelf -lW | grep LOAD ---"
    readelf -lW "$so" | grep LOAD
    while read -r align; do
      if [ "$align" != "0x4000" ]; then
        echo "error: $so has a LOAD segment aligned to $align, expected 0x4000 (16KB pages)" >&2
        fail=1
      fi
    done < <(readelf -lW "$so" | awk '/LOAD/ {print $NF}')
  done
  if [ "$fail" -ne 0 ]; then
    exit 1
  fi
}

# Step 5: check the NEEDED closure of every staged lib is satisfiable, and
# auto-copy libc++_shared.so from the NDK sysroot into the ABI dir if anything
# needs it (the sherpa-onnx android prebuilts normally statically link libc++,
# but this keeps the script correct if that ever changes upstream).
check_needed() {
  echo "== Step 5: check NEEDED libs are satisfiable =="
  for abi in "${ABIS[@]}"; do
    local dest="$JNI_LIBS_OUT/$abi"
    local libs=(librecall_core.so libsherpa-onnx-c-api.so libonnxruntime.so)
    local needs_libcxx=0
    for lib in "${libs[@]}"; do
      local f="$dest/$lib"
      if [ ! -f "$f" ]; then
        echo "error: $f not found (run the build first, without --verify-only)" >&2
        exit 1
      fi
      echo "--- $abi: NEEDED for $lib ---"
      readelf -d "$f" | grep NEEDED || true
      if readelf -d "$f" | grep NEEDED | grep -q 'libc++_shared\.so'; then
        needs_libcxx=1
      fi
    done
    if [ "$needs_libcxx" -eq 1 ]; then
      local triple
      triple="$(abi_triple "$abi")"
      local src="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$triple/libc++_shared.so"
      if [ ! -f "$src" ]; then
        echo "error: $abi NEEDED closure requires libc++_shared.so but it's missing from the NDK sysroot at $src" >&2
        exit 1
      fi
      cp -f "$src" "$dest/"
      echo "copied libc++_shared.so into $dest (required by NEEDED closure)"
    fi
  done
}

if [ "$VERIFY_ONLY" = true ]; then
  check_alignment
  check_needed
  echo "--verify-only: all checks passed."
  exit 0
fi

echo "== Step 1: fetch prebuilt sherpa-onnx android shared libs =="
mkdir -p "$THIRD_PARTY_DIR"
if [ ! -d "$THIRD_PARTY_DIR/jniLibs" ]; then
  TARBALL_PATH="$THIRD_PARTY_DIR/$SHERPA_TARBALL"
  if [ ! -f "$TARBALL_PATH" ]; then
    echo "downloading $SHERPA_URL"
    curl -L --fail -o "$TARBALL_PATH" "$SHERPA_URL"
  fi
  echo "extracting $TARBALL_PATH"
  tar xjf "$TARBALL_PATH" -C "$THIRD_PARTY_DIR"
else
  echo "already extracted: $THIRD_PARTY_DIR/jniLibs"
fi

for abi in "${ABIS[@]}"; do
  if [ ! -d "$THIRD_PARTY_DIR/jniLibs/$abi" ]; then
    echo "error: expected $THIRD_PARTY_DIR/jniLibs/$abi after extraction, not found" >&2
    exit 1
  fi
done

echo "== Step 2: cargo-ndk build recall-core per ABI =="
mkdir -p "$JNI_LIBS_OUT"
for abi in "${ABIS[@]}"; do
  echo "--- building for $abi ---"
  export SHERPA_ONNX_LIB_DIR="$THIRD_PARTY_DIR/jniLibs/$abi"
  # NDK r27 defaults to 16KB-page-aligned linking already, but pin it explicitly
  # (harmless on older/newer NDKs) since Play Store now requires 16KB page-size
  # support on arm64.
  export RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"
  cargo ndk -t "$abi" -o "$JNI_LIBS_OUT" build -p recall-core --release --features onnx
  unset SHERPA_ONNX_LIB_DIR
  unset RUSTFLAGS
done

echo "== Step 3: copy prebuilt sherpa-onnx/onnxruntime shared libs alongside recall_core =="
for abi in "${ABIS[@]}"; do
  dest="$JNI_LIBS_OUT/$abi"
  mkdir -p "$dest"
  cp -f "$THIRD_PARTY_DIR/jniLibs/$abi/libsherpa-onnx-c-api.so" "$dest/"
  cp -f "$THIRD_PARTY_DIR/jniLibs/$abi/libonnxruntime.so" "$dest/"
  echo "staged: $(ls "$dest")"
done

check_alignment

check_needed

echo "Done. jniLibs staged under $JNI_LIBS_OUT/{${ABIS[*]}}"
