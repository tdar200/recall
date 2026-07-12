# Recall

A privacy-first, always-on conversation recorder for ADHD / poor-memory users.
Captures ambient speech all day, transcribes it **on-device** (audio never leaves
the phone), auto-splits it into conversations, and delivers glanceable summaries
so you can recall what was said — without ever becoming a guilt-inducing backlog.

> Personal MVP first (Android, sideloaded). Architected to ship later.
> Full plan: `~/.claude/plans/research-into-this-completeley-zany-meadow.md`.

## Architecture

A shared **Rust core** (`core/`) does all the hard, device-independent work —
VAD-gated segmentation, on-device ASR, storage, summaries — and is exposed to a
native shell via **UniFFI** (Kotlin/Compose now, Swift/SwiftUI later). The shell
owns only the microphone foreground service, OS lifecycle, and UI.

Hard rule: nothing high-frequency crosses the FFI. The shell pushes large PCM
chunks down; the core emits only coarse events up.

```
core/   Rust shared core (host-buildable + unit-tested)
  vad.rs / asr.rs        traits — ONNX backends in core/src/onnx/ (Silero, Parakeet/sherpa-onnx)
  segment.rs             conversation segmentation FSM
  summary.rs             Tier-0 extractive local summary
  store.rs               SQLite (bundled, FTS5; SQLCipher on device)
  cloud.rs               Claude Haiku 4.5 request builder (transcript text only)
  events.rs              foreign-trait listener (events up to the shell)
  ffi.rs                 UniFFI surface (RecallEngine object + RecallListener foreign trait)
android/  Kotlin/Jetpack-Compose shell (capture service, 5 screens, model manager)
scripts/  fetch-models.sh · build-android-libs.sh · e2e-emulator.sh
docs/     M0 runbook + emulator e2e results
```

## Status

The app is **shipped end-to-end and emulator-verified**: Rust core, ONNX
VAD/ASR backends, UniFFI bridge, Kotlin/Compose shell, and Android `.so`
packaging are all built, and the full pipeline (wav → VAD → Parakeet → store →
Feed/Detail UI) passes a scripted end-to-end run on an Android emulator
(`scripts/e2e-emulator.sh`; results + screenshots in `docs/EMULATOR-E2E.md`).

- ✅ Repo + Cargo workspace + `core` crate scaffold (bundled SQLite/FTS5 builds from source).
- ✅ `Vad`/`Asr` traits + `RecallListener` event shape.
- ✅ **Segmentation FSM** (`segment.rs`) — hysteresis, pre-roll, silence-gap splitting, energy gate, max-length force-split, end-of-capture flush.
- ✅ **Extractive summary** (`summary.rs`) — deterministic TextRank + keywords, zero hallucination.
- ✅ **Store** (`store.rs`) — SQLCipher-ready schema + FTS5 keyword search, sync triggers, transcript assembly, **storage tooling** (sizes/total/prune, FTS-safe delete), **action items** CRUD.
- ✅ **Cloud request builder** (`cloud.rs`) — Claude Haiku 4.5, strict JSON schema, transcript-text-only (test-guarded against any audio egress).
- ✅ **Engine** (`engine.rs`) — the full pipeline: `push_audio` → 512-sample VAD framing + RMS energy → segmentation → batch ASR on close → **encode+persist audio** → store + extractive summary → coarse events. Plus: search, one-tap cloud-summary request/persist (with **action-item extraction**), storage dashboard, and **delete-audio (keep transcript) / delete-conversation**. Bounded PCM retention.
- ✅ **Audio encoding** (`audio.rs`) — WAV + **Opus** (pure-Rust `unsafe-libopus` + Ogg, ~7 MB/hr; encode→decode round-trip verified). The shell uses Opus on-device.
- ✅ **ONNX backends** (`core/src/onnx/`) — Silero VAD + Parakeet-TDT-0.6B v2 int8 via the official `sherpa-onnx` crate; host integration tests run against the real models (transcript reference word pinned).
- ✅ **UniFFI bridge** (`ffi.rs`) — `RecallEngine` object + `RecallListener` foreign trait; Kotlin bindings generated into the Android app. Capture-thread ownership resolved: **Kotlin owns the mic**, Rust owns everything after `pushAudio`.
- ✅ **Android shell** (`android/`) — foreground capture service (microphone type, wake lock), Feed / Search / Actions / Storage / Soak screens, in-app model downloader, debug broadcast receiver (debug builds only), soak mode with 60 s heartbeat.
- ✅ **Native packaging** — `scripts/build-android-libs.sh` cross-compiles `librecall_core.so` (arm64-v8a + x86_64, 16 KB-page-aligned, gated) and stages the sherpa-onnx prebuilts into `jniLibs/`.
- ✅ **Emulator e2e** — `scripts/e2e-emulator.sh`: boots a headless AVD, installs the APK, pushes models, ingests a reference wav through the real engine, asserts the transcript, screenshots Feed + Detail. Green on a clean run.

Still device-gated (needs the real Samsung, not an emulator):

- ⏳ **M0 battery/thermal soak numbers** — use the shipped app's **Soak screen**
  (heartbeat + frame stats are built in); procedure in `docs/M0-runbook.md` Part A.
- ⏳ **On-device Parakeet RTF** — the pipeline is proven on the emulator; re-run
  the `scripts/e2e-emulator.sh` flow against the phone for a real RTF number
  (`docs/M0-runbook.md` Part B).
- ⏳ **Opus playback spot-check** — confirm a recorded `.opus` plays in a stock
  player on the Samsung.
- ⏳ **Real-mic sanity** — start capture from the Feed FAB and speak; the
  emulator e2e drives the same code path but with file audio.

## Build

Host (core only — no device needed):

```sh
cargo test                    # 50 unit tests
cargo test --features onnx    # + integration tests against real models (run scripts/fetch-models.sh first)
```

Android APK:

```sh
scripts/fetch-models.sh              # downloads Silero VAD + Parakeet int8 into models/
scripts/build-android-libs.sh        # cross-compiles librecall_core.so + stages sherpa-onnx prebuilts
cd android && ./gradlew :app:assembleDebug
```

> **Fresh clone gotcha:** `third_party/` and `android/app/src/main/jniLibs/` are
> git-ignored build artifacts. You **must** run `scripts/build-android-libs.sh`
> before the Gradle build, or the APK will silently build *without* native libs
> (Gradle does not fail on an empty jniLibs dir) and crash at first FFI call.
> Requires `ANDROID_NDK_HOME` (NDK r27+), `cargo-ndk`, and the
> `aarch64-linux-android` / `x86_64-linux-android` Rust targets.

Emulator end-to-end (headless, needs `ANDROID_HOME` + KVM):

```sh
scripts/e2e-emulator.sh              # full pipeline proof; --keep leaves the emulator running
```
