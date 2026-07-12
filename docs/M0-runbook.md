# M0 — De-risk runbook (device-dependent, run by you)

> **Why this exists:** the research's adversarial verifiers concluded the two
> biggest risks in this whole project are **not** the AI/ML — they are
> (1) **all-day reliability on Samsung** (OEM battery-killers + a mic foreground
> service that can't restart itself after a kill) and (2) **unproven all-day
> battery/thermal cost** of continuous mic capture. M0 measures both on your real
> phone **before** we build features. The third thing M0 de-risks is the
> highest-friction build step: compiling ONNX Runtime for Android and running
> Parakeet on-device.
>
> **This cannot run in the dev sandbox** (no device).
> You run it on a machine with `adb` + your Samsung Galaxy plugged in.
>
> **Update (2026-07-12): the shipped app is the harness.** The shell, engine,
> ONNX models, and `.so` packaging are built and verified end-to-end on an
> emulator (`docs/EMULATOR-E2E.md`), so Part A now uses the app's built-in
> **Soak screen** (no bespoke harness), and Part B's build/integration friction
> is already burned down — only the on-phone RTF number remains to measure.

## Prerequisites (one-time)

- Android Studio (or the command-line SDK) + **NDK r28+** installed; set `ANDROID_HOME` and `ANDROID_NDK_HOME`.
- `adb` on your PATH; phone in **Developer options → USB debugging**.
- Note your exact model + One UI / Android version (drives the Samsung checklist below).

## Samsung "don't kill my app" checklist — APPLY BEFORE ANY MEASUREMENT

If you skip this, you'll measure a service Samsung silently killed, not the real
steady state. On the phone:

1. Settings → Apps → **Recall** → Battery → **Unrestricted**.
2. Settings → Battery → Background usage limits → **Never sleeping apps** → add the app.
3. Settings → Battery → Background usage limits → **remove** the app from "Sleeping apps" / "Deep sleeping apps" if present.
4. Settings → Device care → Battery → turn **off** "Put unused apps to sleep" / "Auto-disable unused apps" (or exempt the app).
5. In Recents, open the app's card menu → **Lock** this app (prevents swipe-kill).
6. Grant `RECORD_AUDIO`; accept the persistent notification.

> ⚠️ Re-check items 1–4 **after any system/OTA update** — Samsung silently resets
> the battery exemption. This is risk #1 and the harness must detect it later.

---

## Part A — All-day battery + thermal soak (mic + Silero VAD, ASR OFF)

**Goal:** measure %/hour battery drain and whether the SoC thermally throttles
while capturing 16 kHz mono audio + running Silero VAD continuously, **with ASR
disabled** (ASR is gated behind VAD and runs only in bursts; we isolate the
always-on baseline first).

**Harness: the shipped Recall app's Soak mode.** It is exactly the M1 capture
path with ASR/persistence disabled in the Rust engine (`soak_mode` counts VAD
frames but writes nothing — unit-test-guarded):
- Install the debug APK (`android/app/build/outputs/apk/debug/app-debug.apk`;
  build per the README) and download/push the models (the Feed banner's
  "Download models" button does it in-app, or `adb push` per
  `scripts/e2e-emulator.sh`).
- Open the **Soak** tab → turn the **Soak mode** switch ON. The same screen has
  buttons deep-linking to the battery-optimization settings, plus this
  checklist as in-app text.
- Go to **Feed** → tap **Start**. That starts the foreground capture service
  (`foregroundServiceType="microphone"`, `AudioRecord` 16 kHz/mono/16-bit on a
  dedicated thread, `PARTIAL_WAKE_LOCK`) with soak mode active.
- Every 60 s the service logs `RecallSoak: heartbeat total=<n> speech=<n>` and
  mirrors it in the notification; the Soak screen shows live frame stats.
  Gaps in the heartbeat timestamps = Samsung killed the service (risk #1).

**Procedure:**

```sh
# 0. Install the app, apply the Samsung checklist above, enable Soak mode, start capture.
#    Watch heartbeats: adb logcat -v time -s RecallSoak

# 1. Reset battery stats, then UNPLUG (batterystats only accrues on battery).
adb shell dumpsys batterystats --reset
adb shell settings put global window_animation_scale 1   # (no-op; just ensure shell works)

# 2. Note start: battery level + time.
adb shell dumpsys battery | grep level
# ... run the soak: screen OFF, phone idle in a normal room, 3–4 hours minimum.

# 3. Sample thermal periodically during the soak (run a few times):
adb shell dumpsys thermalservice | sed -n '1,40p'   # current temps + throttling status
#   Look for: "Thermal Status" (0 = NONE … 6 = SHUTDOWN) and any "throttling" lines.

# 4. At the end (still on battery): note battery level + time, then capture stats.
adb shell dumpsys battery | grep level
adb shell dumpsys batterystats > batterystats.txt
adb bugreport bugreport.zip       # feed into Battery Historian for per-UID power
```

**Battery Historian (per-app power attribution):**
```sh
docker run -p 9999:9999 gcr.io/android-battery-historian/stable:3.0 --port 9999
# open http://localhost:9999 , upload bugreport.zip , find the app's UID,
# read "Estimated power use (mAh)" and the mobile/CPU/audio breakdown.
```

**Compute:** `drain_pct_per_hour = (level_start − level_end) / soak_hours`,
cross-checked against Battery Historian's per-UID estimate (so you separate the
app's draw from the rest of the system).

**Go / no-go (heuristic — adjust to your tolerance):**

| Result | Verdict | Action |
|---|---|---|
| ≲ 5 %/hr **and** thermal status stays 0–1 (NONE/LIGHT) | 🟢 **all-day viable** | proceed to M1 as planned |
| ~5–9 %/hr or occasional LIGHT/MODERATE throttling | 🟡 **viable with mitigations** | defer ASR to charging/idle; consider two-stage VAD (RMS pre-gate); honest battery disclosure + easy pause |
| ≳ 9 %/hr or sustained MODERATE+ throttling | 🔴 **rethink "all day"** | capture-only + batch transcription when charging; or revisit the wearable question |

---

## Part B — On-device Parakeet RTF spike

> **Status: integration already proven.** The full Rust path (sherpa-onnx →
> Parakeet int8 → engine → store) runs green end-to-end on an x86_64 emulator —
> a 7 s reference wav ingests in ~23 s *including cold model load* (5–7 s
> warm), with a word-for-word-correct transcript (`docs/EMULATOR-E2E.md`). All the build
> friction the verifiers flagged (ONNX Runtime packaging, 16 KB pages,
> `SHERPA_ONNX_LIB_DIR`, libc++) is resolved in `scripts/build-android-libs.sh`.
> **What remains is only the RTF number on your real phone** — arm64 silicon
> should be far faster than the emulated x86_64 VM (research saw RTF ≈ 0.09 on
> a 2019 Galaxy S10).

**To measure on the phone:** re-run the `scripts/e2e-emulator.sh` flow against
the device (skip the AVD/boot steps — start at `adb install`):

```sh
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n com.recall.app/.MainActivity   # creates external files dir
# Push models (or use the in-app "Download models" button on the Feed banner):
adb shell mkdir -p /sdcard/Android/data/com.recall.app/files/models/parakeet
adb push models/silero_vad.onnx /sdcard/Android/data/com.recall.app/files/models/
for f in encoder.int8.onnx decoder.int8.onnx joiner.int8.onnx tokens.txt; do
  adb push "models/parakeet/$f" /sdcard/Android/data/com.recall.app/files/models/parakeet/
done
adb shell pm grant com.recall.app android.permission.RECORD_AUDIO
adb shell pm grant com.recall.app android.permission.POST_NOTIFICATIONS
adb shell am force-stop com.recall.app && adb shell am start -n com.recall.app/.MainActivity
# Push the 7s reference wav into the app's own files dir (NOT /sdcard/Download —
# scoped storage blocks it) and time the ingest:
adb push models/parakeet/test_wavs/0.wav /sdcard/Android/data/com.recall.app/files/test.wav
adb logcat -c
adb shell am broadcast -n com.recall.app/.DebugReceiver -a com.recall.DEBUG_INGEST \
  --es path /sdcard/Android/data/com.recall.app/files/test.wav
adb logcat -v time -s RecallDebug   # "received ..." → "ingest done" delta = wall time
```

RTF ≈ (ingest wall time) / 7 s. Run it twice — the second run excludes cold
model load and is the number that matters for live capture.

Original two-step spike below, kept for reference (B1 is still a useful
independent cross-check; B2 is done):

### B1 — Speed proof (prebuilt, low effort)

Use sherpa-onnx's **prebuilt** Android binaries + the Parakeet int8 model; no
from-source build yet. (Confirm the current release/model names on the
sherpa-onnx releases page and the k2-fsa model index — versions move; research
saw sherpa-onnx ~1.13.x and the model `sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8`.)

```sh
# Download the Parakeet int8 model bundle (name per the k2-fsa model index):
#   sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2   → extract locally.

# Push the arm64 sherpa-onnx-offline binary + model + a ~30s 16kHz mono test wav:
adb push sherpa-onnx-offline /data/local/tmp/
adb push sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8 /data/local/tmp/model
adb push test-30s-16k-mono.wav /data/local/tmp/

adb shell chmod +x /data/local/tmp/sherpa-onnx-offline
adb shell "cd /data/local/tmp && ./sherpa-onnx-offline \
  --encoder=model/encoder.int8.onnx \
  --decoder=model/decoder.int8.onnx \
  --joiner=model/joiner.int8.onnx \
  --tokens=model/tokens.txt \
  --num-threads=4 \
  test-30s-16k-mono.wav"
#   ↑ flag names follow the model's README; sherpa prints "Real time factor (RTF)".
```
Alternatively, build & install the official **sherpa-onnx Android ASR example
APK** (k2-fsa repo) configured for the Parakeet model and read the on-screen RTF.

**Pass:** RTF well below 1.0 (research saw ~0.09 on a 2019 Galaxy S10, so a modern
Galaxy should be far under realtime). Record the number + your exact chip.

### B2 — Integration proof (the real friction)

Prove the **`sherpa-rs` / `ort` Rust path** that the core will actually use, built
for `aarch64-linux-android`. This is the step the verifiers flagged as
"most-likely-to-surprise."

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk

# In a throwaway example (e.g. core/examples/asr_spike.rs behind a feature),
# depend on sherpa-rs and transcribe the same wav.
# sherpa-rs can fetch matching prebuilt libs; if you instead build ONNX Runtime
# from source, expect to set ORT_LIB_PATH and build with a recent NDK.

cargo ndk -t arm64-v8a --link-libcxx-shared build --release --features asr-spike
# push the resulting test binary/APK and run on device; capture RTF.
```
**Watch for (known friction):** no prebuilt ORT mobile binaries → from-source
build; **16 KB page-size** requirement on some `libonnxruntime.so` (use NDK r28+);
`libc++_shared.so` must be packaged (`--link-libcxx-shared`); pin exact
`ort`/`sherpa-rs` versions. **Log every snag** — it informs the M2 build.

**Pass:** one Parakeet inference runs through Rust on-device with RTF < 1.0.

---

## Part C — Record results → `docs/M0-results.md`

```md
## M0 results — <date>, <Samsung model>, One UI <ver> / Android <ver>, <chipset>

### A. Battery + thermal (mic + Silero VAD, ASR off)
- Soak duration: __ h, screen off, ambient: <quiet room / office / ...>
- Battery: __% → __%  ⇒  __ %/hr  (Battery Historian per-UID: __ mAh)
- Thermal status peak: __ (0–6); throttling observed: yes/no
- Samsung killed the service during the soak? yes/no  (gaps in heartbeat log?)
- Verdict: 🟢 / 🟡 / 🔴  → <chosen action>

### B. Parakeet on-device
- B1 prebuilt RTF: __ (num-threads __)
- B2 Rust/sherpa-rs RTF: __ ; build friction notes: <...>
- Verdict: pass/fail

### GO / NO-GO for M1
- <one line: proceed as planned / proceed with mitigations / rethink all-day>
```

When you've filled this in, paste it back and we set the M1 plan (capture-now vs.
charging/idle-deferred transcription) accordingly.
