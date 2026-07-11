#!/usr/bin/env bash
# Idempotent download of the on-device ONNX models used by the `onnx` cargo
# feature (SileroVad + ParakeetAsr) and by Task 5's ModelManager on-device.
#
# Downloads into `models/` at the repo root (git-ignored):
#   models/silero_vad.onnx
#   models/parakeet/{encoder,decoder,joiner}.int8.onnx, tokens.txt, test_wavs/...
#
# Safe to re-run: skips anything already present.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS_DIR="${REPO_ROOT}/models"
SILERO_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
PARAKEET_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2"
PARAKEET_ARCHIVE_DIRNAME="sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"

mkdir -p "${MODELS_DIR}"
cd "${MODELS_DIR}"

if [[ -f silero_vad.onnx ]]; then
  echo "silero_vad.onnx already present, skipping"
else
  echo "downloading silero_vad.onnx ..."
  curl -fL -o silero_vad.onnx.tmp "${SILERO_URL}"
  mv silero_vad.onnx.tmp silero_vad.onnx
fi

# `models/parakeet` is the stable path Task 5/7 code depends on. The archive
# extracts to `${PARAKEET_ARCHIVE_DIRNAME}/`; we point `parakeet` at it (symlink
# if the extracted dir already exists under its original name, real dir otherwise).
if [[ -e parakeet/tokens.txt ]]; then
  echo "models/parakeet already present, skipping"
elif [[ -f "${PARAKEET_ARCHIVE_DIRNAME}/tokens.txt" ]]; then
  echo "found extracted ${PARAKEET_ARCHIVE_DIRNAME}/, linking as parakeet"
  ln -sfn "${PARAKEET_ARCHIVE_DIRNAME}" parakeet
else
  if [[ -f "${PARAKEET_ARCHIVE_DIRNAME}.tar.bz2" ]]; then
    echo "found partial/complete archive, verifying by extraction ..."
  else
    echo "downloading parakeet bundle (~460 MB) ..."
    curl -fL -o "${PARAKEET_ARCHIVE_DIRNAME}.tar.bz2.tmp" "${PARAKEET_URL}"
    mv "${PARAKEET_ARCHIVE_DIRNAME}.tar.bz2.tmp" "${PARAKEET_ARCHIVE_DIRNAME}.tar.bz2"
  fi
  echo "extracting ..."
  tar xjf "${PARAKEET_ARCHIVE_DIRNAME}.tar.bz2"
  rm -f "${PARAKEET_ARCHIVE_DIRNAME}.tar.bz2"
  ln -sfn "${PARAKEET_ARCHIVE_DIRNAME}" parakeet
fi

echo "models ready:"
ls -la "${MODELS_DIR}"
ls -la "${MODELS_DIR}/parakeet"
