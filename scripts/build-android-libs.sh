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
# See .superpowers/sdd/task-6-brief.md and task-6-report.md for the full rationale.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME must be set (path to the NDK, e.g. \$ANDROID_HOME/ndk/27.1.12297006)}"
if [ ! -d "$ANDROID_NDK_HOME" ]; then
  echo "error: ANDROID_NDK_HOME=$ANDROID_NDK_HOME does not exist" >&2
  exit 1
fi

SHERPA_VERSION="1.13.4"
SHERPA_TARBALL="sherpa-onnx-v${SHERPA_VERSION}-android.tar.bz2"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_VERSION}/${SHERPA_TARBALL}"
THIRD_PARTY_DIR="$REPO_ROOT/third_party/sherpa-onnx-android"
JNI_LIBS_OUT="$REPO_ROOT/android/app/src/main/jniLibs"

ABIS=(arm64-v8a x86_64)

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

echo "== Step 4: verify 16KB page alignment of librecall_core.so =="
for abi in "${ABIS[@]}"; do
  so="$JNI_LIBS_OUT/$abi/librecall_core.so"
  echo "--- $abi: readelf -lW | grep LOAD ---"
  readelf -lW "$so" | grep LOAD
done

echo "== Step 5: check NEEDED libs are satisfiable =="
for abi in "${ABIS[@]}"; do
  dest="$JNI_LIBS_OUT/$abi"
  echo "--- $abi: NEEDED for librecall_core.so ---"
  readelf -d "$dest/librecall_core.so" | grep NEEDED || true
  echo "--- $abi: NEEDED for libsherpa-onnx-c-api.so ---"
  readelf -d "$dest/libsherpa-onnx-c-api.so" | grep NEEDED || true
done

echo "Done. jniLibs staged under $JNI_LIBS_OUT/{${ABIS[*]}}"
echo "If any NEEDED lib is libc++_shared.so, copy it from:"
echo "  \$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/*/sysroot/usr/lib/<triple>/libc++_shared.so"
