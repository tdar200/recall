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
core/   Rust shared core (this crate; host-buildable + unit-tested)
  vad.rs / asr.rs        traits — ONNX backends (Silero, Parakeet/sherpa-onnx) slot in on the NDK build
  segment.rs             conversation segmentation FSM
  summary.rs             Tier-0 extractive local summary
  store.rs               SQLite (bundled, FTS5; SQLCipher on device)
  cloud.rs               Claude Haiku 4.5 request builder (transcript text only)
  events.rs              foreign-trait listener (events up to the shell)
android/  Kotlin/Jetpack-Compose shell (added after the M0 gate)
docs/     M0 runbook + notes
```

## Status

The device-independent core is **built and verified end-to-end** (47 unit tests,
clippy-clean — no device needed). It backs all four planned UI screens (Feed,
summary-first Detail, Action Items, Search):

- ✅ Repo + Cargo workspace + `core` crate scaffold (bundled SQLite/FTS5 builds from source).
- ✅ `Vad`/`Asr` traits + `RecallListener` event shape (ONNX backends slot in later).
- ✅ **Segmentation FSM** (`segment.rs`) — hysteresis, pre-roll, silence-gap splitting, energy gate, max-length force-split, end-of-capture flush.
- ✅ **Extractive summary** (`summary.rs`) — deterministic TextRank + keywords, zero hallucination.
- ✅ **Store** (`store.rs`) — SQLCipher-ready schema + FTS5 keyword search, sync triggers, transcript assembly, **storage tooling** (sizes/total/prune, FTS-safe delete), **action items** CRUD.
- ✅ **Cloud request builder** (`cloud.rs`) — Claude Haiku 4.5, strict JSON schema, transcript-text-only (test-guarded against any audio egress).
- ✅ **Engine** (`engine.rs`) — the full pipeline: `push_audio` → 512-sample VAD framing + RMS energy → segmentation → batch ASR on close → **encode+persist audio** → store + extractive summary → coarse events. Plus: search, one-tap cloud-summary request/persist (with **action-item extraction**), storage dashboard, and **delete-audio (keep transcript) / delete-conversation**. Bounded PCM retention. Tested with mock VAD/ASR end-to-end.
- ✅ **Audio encoding** (`audio.rs`) — `AudioEncoder` trait with two impls: **WAV** (verified, files read back) and **Opus** (pure-Rust `unsafe-libopus` + Ogg, ~7 MB/hr; verified by encode→decode **round-trip** preserving signal energy). Engine accepts either; the shell picks Opus on-device.

Pending (device / NDK gated):

- ⏳ **M0 gate (device-dependent):** measure all-day battery/thermal and on-device
  Parakeet RTF on the target Samsung Galaxy **before** building features
  (see `docs/M0-runbook.md`). The two biggest risks are OEM battery-killers and
  unproven all-day battery cost — not the ML.
- ⏳ **Opus on-device playback check** — the encoder is round-trip-verified here,
  but confirm a `.opus` file actually plays in a standard player on the Samsung
  before relying on it for the permanent archive (cheap to verify on device).
- ⏳ **UniFFI bridge** — deferred until the M0 decision on audio-thread ownership
  (Rust-owned vs Kotlin-owned capture) is made, since that shapes the FFI surface.
- ⏳ **Android shell + ONNX VAD/ASR wiring** (M1–M3).

## Build (host)

```sh
cargo test            # builds the core and runs unit tests (no device needed)
```

The ONNX-backed VAD/ASR and the Android `.so` packaging (`cargo-ndk`) require the
Android NDK and are wired during M0/M2.
