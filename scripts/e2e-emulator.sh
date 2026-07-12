#!/usr/bin/env bash
# scripts/e2e-emulator.sh
#
# Emulator end-to-end verification for Recall (Task 7).
#
# Boots (or reuses) an x86_64 Android emulator, installs the debug APK, pushes
# the on-device models, drives a full ingest through the DebugReceiver contract
# (see .superpowers/sdd/task-5-report.md), asserts the transcript, and
# screenshots the Feed + Detail screens into docs/img/.
#
# Idempotent: safe to re-run. Bails clearly (message + exit 1) on any failed
# step. Pass --keep to leave the emulator running afterwards (default: killed
# on success, AVD definition is always left in place for reuse).
#
# Usage: scripts/e2e-emulator.sh [--keep]

set -uo pipefail

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
AVDMANAGER="$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager"
EMULATOR="$ANDROID_HOME/emulator/emulator"
ADB="$ANDROID_HOME/platform-tools/adb"

AVD_NAME=recall_e2e
SYSTEM_IMAGE="system-images;android-36;google_apis;x86_64"
PACKAGE=com.recall.app

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APK_PATH="$REPO_ROOT/android/app/build/outputs/apk/debug/app-debug.apk"
MODELS_DIR="$REPO_ROOT/models"
PARAKEET_DIR="$(readlink -f "$MODELS_DIR/parakeet")"
TEST_WAV="$PARAKEET_DIR/test_wavs/0.wav"
EXPECTED_WORD="portrait"
IMG_DIR="$REPO_ROOT/docs/img"

DEVICE_MODELS_DIR="/sdcard/Android/data/$PACKAGE/files/models"
# NOTE: NOT /sdcard/Download — on API 30+ scoped storage the app has no
# permission to open arbitrary shared-storage paths via a raw File/RandomAccessFile
# (confirmed empirically: EACCES). The app's own external-files-dir is always
# readable by the app without extra permissions, so the test wav is pushed there.
DEVICE_WAV="/sdcard/Android/data/$PACKAGE/files/test.wav"

SCRATCH="${TMPDIR:-/tmp}/recall-e2e"
mkdir -p "$SCRATCH"
LOGCAT_FILE="$SCRATCH/logcat-full.txt"
BOOT_LOG="$SCRATCH/emulator-boot.log"
EMU_PID_FILE="$SCRATCH/emulator.pid"
LOGCAT_PID_FILE="$SCRATCH/logcat.pid"

KEEP=false
for arg in "$@"; do
  case "$arg" in
    --keep) KEEP=true ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done

# Ingest can be very slow (int8 Parakeet on emulated x86_64) — allow up to 15 min.
INGEST_TIMEOUT_S=900
BOOT_TIMEOUT_S=300
POLL_INTERVAL_S=10

step() { echo; echo "== $* =="; }
fail() { echo "FAIL: $*" >&2; cleanup_on_fail; exit 1; }

# Bring MainActivity to the foreground, retrying through any transient system
# dialog (observed once: a headless-swiftshader "System UI isn't responding"
# ANR unrelated to our app — dismissed with BACK and a relaunch).
ensure_foreground() {
  local attempt
  for attempt in 1 2 3 4 5; do
    "$ADB" shell am start -n "$PACKAGE/.MainActivity" > /dev/null
    sleep 3
    if "$ADB" shell dumpsys window 2>/dev/null | grep -q "mCurrentFocus.*$PACKAGE/com.recall.app.MainActivity"; then
      return 0
    fi
    echo "  ...$PACKAGE not focused yet (attempt $attempt/5), sending BACK and retrying"
    "$ADB" shell input keyevent KEYCODE_BACK > /dev/null 2>&1 || true
    sleep 1
  done
  return 1
}

WE_STARTED_EMULATOR=false

cleanup_on_fail() {
  if [ -f "$LOGCAT_PID_FILE" ]; then
    kill "$(cat "$LOGCAT_PID_FILE")" 2>/dev/null || true
  fi
}

cleanup_on_exit() {
  if [ -f "$LOGCAT_PID_FILE" ]; then
    kill "$(cat "$LOGCAT_PID_FILE")" 2>/dev/null || true
    rm -f "$LOGCAT_PID_FILE"
  fi
  if [ "$KEEP" = false ] && [ "$WE_STARTED_EMULATOR" = true ]; then
    if [ -f "$EMU_PID_FILE" ]; then
      PID="$(cat "$EMU_PID_FILE")"
      echo "Killing emulator (pid $PID); AVD '$AVD_NAME' definition is left in place for reuse."
      kill "$PID" 2>/dev/null || true
      rm -f "$EMU_PID_FILE"
    fi
  elif [ "$KEEP" = true ]; then
    echo "--keep passed: leaving emulator running."
  fi
}
trap cleanup_on_exit EXIT

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------
step "Preflight"

[ -x "$AVDMANAGER" ] || fail "avdmanager not found at $AVDMANAGER"
[ -x "$EMULATOR" ] || fail "emulator not found at $EMULATOR"
[ -x "$ADB" ] || fail "adb not found at $ADB"

if [ ! -f "$APK_PATH" ]; then
  step "APK missing, building debug APK"
  (cd "$REPO_ROOT/android" && ./gradlew :app:assembleDebug --no-daemon) \
    || fail "gradlew assembleDebug failed"
fi
[ -f "$APK_PATH" ] || fail "APK still missing after build: $APK_PATH"

[ -f "$MODELS_DIR/silero_vad.onnx" ] || fail "missing $MODELS_DIR/silero_vad.onnx (run scripts/fetch-models.sh)"
[ -d "$PARAKEET_DIR" ] || fail "missing parakeet model dir (resolved: $PARAKEET_DIR)"
for f in encoder.int8.onnx decoder.int8.onnx joiner.int8.onnx tokens.txt; do
  [ -f "$PARAKEET_DIR/$f" ] || fail "missing $PARAKEET_DIR/$f"
done
[ -f "$TEST_WAV" ] || fail "missing test wav: $TEST_WAV"

mkdir -p "$IMG_DIR"

echo "APK:      $APK_PATH"
echo "Models:   $MODELS_DIR (parakeet -> $PARAKEET_DIR)"
echo "Test wav: $TEST_WAV (expect transcript to contain '$EXPECTED_WORD')"

# ---------------------------------------------------------------------------
# AVD (idempotent: only create+configure if it doesn't already exist)
# ---------------------------------------------------------------------------
step "AVD"

if "$AVDMANAGER" list avd 2>/dev/null | grep -q "Name: $AVD_NAME$"; then
  echo "AVD '$AVD_NAME' already exists, reusing."
else
  echo "Creating AVD '$AVD_NAME'..."
  echo "no" | "$AVDMANAGER" create avd -n "$AVD_NAME" -k "$SYSTEM_IMAGE" -d pixel_7 --force \
    || fail "avdmanager create avd failed"

  CFG="$HOME/.android/avd/$AVD_NAME.avd/config.ini"
  [ -f "$CFG" ] || fail "expected config.ini at $CFG after AVD creation"
  # Encoder is ~622MB; give the AVD real RAM + disk headroom.
  sed -i 's/^hw.ramSize=.*/hw.ramSize=4096/' "$CFG"
  sed -i 's/^disk.dataPartition.size=.*/disk.dataPartition.size=8G/' "$CFG"
  grep -q '^hw.ramSize=4096$' "$CFG" || fail "failed to set hw.ramSize in $CFG"
  grep -q '^disk.dataPartition.size=8G$' "$CFG" || fail "failed to set disk.dataPartition.size in $CFG"
  echo "Configured hw.ramSize=4096, disk.dataPartition.size=8G"
fi

# ---------------------------------------------------------------------------
# Boot (reuse if already running)
# ---------------------------------------------------------------------------
step "Boot emulator"

if "$ADB" devices | grep -q "^emulator-.*device$"; then
  echo "An emulator is already running and online, reusing it."
else
  echo "Booting $AVD_NAME (headless, no-snapshot)..."
  nohup "$EMULATOR" -avd "$AVD_NAME" -no-window -no-audio -gpu swiftshader_indirect \
    -no-snapshot -no-boot-anim > "$BOOT_LOG" 2>&1 &
  EMU_PID=$!
  echo "$EMU_PID" > "$EMU_PID_FILE"
  WE_STARTED_EMULATOR=true
  disown || true
  echo "emulator pid=$EMU_PID, log=$BOOT_LOG"

  echo "Waiting for device node..."
  "$ADB" wait-for-device || fail "adb wait-for-device failed"

  echo "Polling sys.boot_completed (timeout ${BOOT_TIMEOUT_S}s)..."
  elapsed=0
  while true; do
    booted="$("$ADB" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r\n')"
    if [ "$booted" = "1" ]; then
      echo "Boot completed after ~${elapsed}s."
      break
    fi
    if ! ps -p "$EMU_PID" > /dev/null 2>&1; then
      echo "--- emulator boot log tail ---"
      tail -n 60 "$BOOT_LOG"
      fail "emulator process died during boot (pid $EMU_PID gone)"
    fi
    if [ "$elapsed" -ge "$BOOT_TIMEOUT_S" ]; then
      echo "--- emulator boot log tail ---"
      tail -n 60 "$BOOT_LOG"
      fail "boot did not complete within ${BOOT_TIMEOUT_S}s"
    fi
    sleep "$POLL_INTERVAL_S"
    elapsed=$((elapsed + POLL_INTERVAL_S))
  done
  # Give the shell/package manager a few more seconds to settle post-boot.
  sleep 5
fi

"$ADB" wait-for-device || fail "adb wait-for-device failed (post-boot)"

# ---------------------------------------------------------------------------
# Install + first launch (creates external files dir)
# ---------------------------------------------------------------------------
step "Install APK"
"$ADB" install -r "$APK_PATH" || fail "adb install failed"

# Grant runtime perms IMMEDIATELY after install, before any launch — otherwise
# MainActivity's runtime-permission dialog holds window focus on first launch
# and ensure_foreground can never observe MainActivity focused.
"$ADB" shell pm grant "$PACKAGE" android.permission.RECORD_AUDIO || fail "pm grant RECORD_AUDIO failed"
"$ADB" shell pm grant "$PACKAGE" android.permission.POST_NOTIFICATIONS || fail "pm grant POST_NOTIFICATIONS failed"

step "First launch (creates external files dir)"
"$ADB" shell am start -n "$PACKAGE/.MainActivity" || fail "am start failed"
# The repository constructs lazily in onCreate and probes getExternalFilesDir("models"),
# which creates the dir — poll for the dir itself, the thing we actually need.
elapsed=0
until "$ADB" shell "[ -d /sdcard/Android/data/$PACKAGE/files ]" 2>/dev/null; do
  [ "$elapsed" -ge 60 ] && fail "external files dir did not appear within 60s of first launch"
  sleep 3
  elapsed=$((elapsed + 3))
done
"$ADB" shell am force-stop "$PACKAGE" || fail "am force-stop failed"

# ---------------------------------------------------------------------------
# Push models
# ---------------------------------------------------------------------------
step "Push models (~630MB, this takes a few minutes)"

"$ADB" shell mkdir -p "$DEVICE_MODELS_DIR/parakeet" || fail "mkdir device models dir failed"

"$ADB" push "$MODELS_DIR/silero_vad.onnx" "$DEVICE_MODELS_DIR/silero_vad.onnx" \
  || fail "push silero_vad.onnx failed"

for f in encoder.int8.onnx decoder.int8.onnx joiner.int8.onnx tokens.txt; do
  "$ADB" push "$PARAKEET_DIR/$f" "$DEVICE_MODELS_DIR/parakeet/$f" \
    || fail "push parakeet/$f failed"
done

echo "Pushed models. Device listing:"
"$ADB" shell "ls -la $DEVICE_MODELS_DIR $DEVICE_MODELS_DIR/parakeet"

# ---------------------------------------------------------------------------
# Relaunch so the repository re-probes models (perms were granted pre-launch)
# ---------------------------------------------------------------------------
step "Relaunch with models in place"

"$ADB" shell am force-stop "$PACKAGE" || true
ensure_foreground || fail "app did not come to foreground on relaunch"

# ---------------------------------------------------------------------------
# Push test wav
# ---------------------------------------------------------------------------
step "Push test wav"
"$ADB" push "$TEST_WAV" "$DEVICE_WAV" || fail "push test wav failed"

# ---------------------------------------------------------------------------
# Start continuous logcat capture
# ---------------------------------------------------------------------------
step "Start logcat capture"
"$ADB" logcat -c || true
nohup "$ADB" logcat -v time > "$LOGCAT_FILE" 2>&1 &
LOGCAT_PID=$!
echo "$LOGCAT_PID" > "$LOGCAT_PID_FILE"
disown || true
echo "logcat pid=$LOGCAT_PID -> $LOGCAT_FILE"

# ---------------------------------------------------------------------------
# DEBUG_INGEST
# ---------------------------------------------------------------------------
step "Broadcast DEBUG_INGEST"
INGEST_START=$(date +%s)
"$ADB" shell am broadcast -n "$PACKAGE/.DebugReceiver" -a com.recall.DEBUG_INGEST --es path "$DEVICE_WAV" \
  || fail "DEBUG_INGEST broadcast failed"

echo "Polling for 'ingest done' (timeout ${INGEST_TIMEOUT_S}s)..."
elapsed=0
INGEST_DONE=false
while [ "$elapsed" -lt "$INGEST_TIMEOUT_S" ]; do
  # NB: `logcat -v time` renders the tag as "I/RecallDebug( <pid>): <msg>", so
  # the greppable invariant is tag + message substring, NOT "RecallDebug: <msg>".
  if grep -Eq "RecallDebug.*ingest done, conversations=" "$LOGCAT_FILE" 2>/dev/null; then
    INGEST_DONE=true
    break
  fi
  # Watch for a crash — bail immediately if the app died.
  if ! "$ADB" shell pidof "$PACKAGE" > /dev/null 2>&1; then
    echo "--- process $PACKAGE is not running; checking crash buffer ---"
    "$ADB" logcat -d -b crash || true
    echo "--- tail of full logcat ---"
    tail -n 80 "$LOGCAT_FILE"
    fail "app process disappeared during ingest (crash?)"
  fi
  sleep "$POLL_INTERVAL_S"
  elapsed=$((elapsed + POLL_INTERVAL_S))
  echo "  ...waited ${elapsed}s"
done
INGEST_END=$(date +%s)
INGEST_ELAPSED=$((INGEST_END - INGEST_START))

if [ "$INGEST_DONE" != true ]; then
  echo "--- crash buffer ---"
  "$ADB" logcat -d -b crash || true
  echo "--- tail of full logcat ---"
  tail -n 100 "$LOGCAT_FILE"
  fail "ingest did not complete within ${INGEST_TIMEOUT_S}s"
fi

INGEST_LINE="$(grep -E "RecallDebug.*ingest done, conversations=" "$LOGCAT_FILE" | tail -1)"
echo "Ingest completed in ${INGEST_ELAPSED}s: $INGEST_LINE"

# ---------------------------------------------------------------------------
# DEBUG_DUMP
# ---------------------------------------------------------------------------
step "Broadcast DEBUG_DUMP"
"$ADB" shell am broadcast -n "$PACKAGE/.DebugReceiver" -a com.recall.DEBUG_DUMP \
  || fail "DEBUG_DUMP broadcast failed"

elapsed=0
DUMP_DONE=false
while [ "$elapsed" -lt 60 ]; do
  if grep -Eq "RecallDebug.*dump done" "$LOGCAT_FILE" 2>/dev/null; then
    DUMP_DONE=true
    break
  fi
  sleep 3
  elapsed=$((elapsed + 3))
done

[ "$DUMP_DONE" = true ] || fail "dump did not complete within 60s"

step "Assert transcript"
CONV_LINE="$(grep -E "RecallDebug.*conv id=" "$LOGCAT_FILE" | tail -1)"
[ -n "$CONV_LINE" ] || fail "no 'RecallDebug … conv id=' line found in logcat"
echo "$CONV_LINE"

if ! echo "$CONV_LINE" | grep -qi "$EXPECTED_WORD"; then
  fail "transcript does not contain expected word '$EXPECTED_WORD': $CONV_LINE"
fi
echo "PASS: transcript contains '$EXPECTED_WORD'"

# ---------------------------------------------------------------------------
# Screenshots
# ---------------------------------------------------------------------------
step "Screenshot: Feed"
ensure_foreground || fail "app did not come to foreground for Feed screenshot"
"$ADB" exec-out screencap -p > "$IMG_DIR/emulator-feed.png" \
  || fail "screencap (feed) failed"
[ -s "$IMG_DIR/emulator-feed.png" ] || fail "feed screenshot is empty"
echo "Saved $IMG_DIR/emulator-feed.png"

step "Screenshot: Detail (via UI Automator tap, no deep link exists)"
"$ADB" shell uiautomator dump /sdcard/window_dump.xml > /dev/null 2>&1 || true
if "$ADB" pull /sdcard/window_dump.xml "$SCRATCH/window_dump.xml" > /dev/null 2>&1; then
  # Find the duration text node ("<N>s") that lives inside the clickable
  # ConversationRow Card; tap its center. Falls back to a fixed coordinate
  # (first-row heuristic for the pixel_7 1080x2400 profile) if not found.
  BOUNDS="$(grep -o 'text="[0-9][0-9]*s"[^>]*bounds="\[[0-9]*,[0-9]*\]\[[0-9]*,[0-9]*\]"' "$SCRATCH/window_dump.xml" \
    | grep -o 'bounds="\[[0-9]*,[0-9]*\]\[[0-9]*,[0-9]*\]"' | head -1 | grep -o '[0-9]\+')"
  if [ -n "$BOUNDS" ]; then
    read -r X1 Y1 X2 Y2 <<< "$(echo "$BOUNDS" | tr '\n' ' ')"
    TAP_X=$(( (X1 + X2) / 2 ))
    TAP_Y=$(( (Y1 + Y2) / 2 ))
    echo "Tapping conversation row at ($TAP_X, $TAP_Y)"
  else
    TAP_X=540
    TAP_Y=280
    echo "Could not parse uiautomator dump; falling back to fixed tap ($TAP_X, $TAP_Y)"
  fi
else
  TAP_X=540
  TAP_Y=280
  echo "uiautomator dump/pull failed; falling back to fixed tap ($TAP_X, $TAP_Y)"
fi

"$ADB" shell input tap "$TAP_X" "$TAP_Y" || fail "input tap failed"
sleep 2
"$ADB" exec-out screencap -p > "$IMG_DIR/emulator-detail.png" \
  || fail "screencap (detail) failed"
[ -s "$IMG_DIR/emulator-detail.png" ] || fail "detail screenshot is empty"
echo "Saved $IMG_DIR/emulator-detail.png"

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
step "Summary"
echo "Ingest time:      ${INGEST_ELAPSED}s"
echo "Ingest line:       $INGEST_LINE"
echo "Transcript line:   $CONV_LINE"
echo "Feed screenshot:   $IMG_DIR/emulator-feed.png"
echo "Detail screenshot: $IMG_DIR/emulator-detail.png"
echo "Full logcat:        $LOGCAT_FILE"
echo
echo "E2E PASSED."
