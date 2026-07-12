//! UniFFI bridge: the single object (`RecallEngine`) and foreign-callback
//! trait (`RecallListener`, exported in `events.rs`) that Kotlin (now) and
//! Swift (later) see.
//!
//! Kept intentionally thin: this module only adapts types (byte buffers ↔
//! `Vec<i16>`, store/engine structs ↔ `Ffi*` records, `&self` + `Mutex` ↔
//! `EngineCore`'s `&mut self`) and picks VAD/ASR backends at construction
//! time. All actual behavior lives in `engine::EngineCore`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::asr::Asr;
#[cfg(feature = "onnx")]
use crate::asr::NoopAsr;
use crate::audio::{AudioEncoder, OpusEncoder, WavEncoder};
use crate::engine::{EngineConfig, EngineCore, FrameStats};
use crate::error::{CoreError, Result};
use crate::events::RecallListener;
use crate::segment::SegmenterConfig;
use crate::store::{ActionItem, Conversation, SearchHit, StorageEntry, Store};
use crate::vad::Vad;

// ---------------------------------------------------------------------------
// Records — plain data mirrors of the store/engine types, `From`-converted at
// the boundary so `core::store`/`core::engine` stay UniFFI-agnostic.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiConversation {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_s: i64,
    pub audio_path: String,
    pub summary_basic: Option<String>,
    pub summary_cloud: Option<String>,
}

impl From<Conversation> for FfiConversation {
    fn from(c: Conversation) -> Self {
        Self {
            id: c.id,
            started_at: c.started_at,
            ended_at: c.ended_at,
            duration_s: c.duration_s,
            audio_path: c.audio_path,
            summary_basic: c.summary_basic,
            summary_cloud: c.summary_cloud,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiSearchHit {
    pub conversation_id: i64,
    pub snippet: String,
}

impl From<SearchHit> for FfiSearchHit {
    fn from(h: SearchHit) -> Self {
        Self { conversation_id: h.conversation_id, snippet: h.snippet }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiActionItem {
    pub id: i64,
    pub conversation_id: i64,
    pub task: String,
    pub owner: Option<String>,
    pub due: Option<String>,
}

impl From<ActionItem> for FfiActionItem {
    fn from(a: ActionItem) -> Self {
        Self { id: a.id, conversation_id: a.conversation_id, task: a.task, owner: a.owner, due: a.due }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiStorageEntry {
    pub conversation_id: i64,
    pub audio_bytes: i64,
    pub is_audio_pruned: bool,
}

impl From<StorageEntry> for FfiStorageEntry {
    fn from(e: StorageEntry) -> Self {
        Self {
            conversation_id: e.conversation_id,
            audio_bytes: e.audio_bytes,
            is_audio_pruned: e.is_audio_pruned,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Record)]
pub struct FfiFrameStats {
    pub total_frames: u64,
    pub speech_frames: u64,
}

impl From<FrameStats> for FfiFrameStats {
    fn from(s: FrameStats) -> Self {
        Self { total_frames: s.total_frames, speech_frames: s.speech_frames }
    }
}

// ---------------------------------------------------------------------------
// PCM byte <-> i16 conversion (LE) — `push_audio`'s wire format.
// ---------------------------------------------------------------------------

/// Convert little-endian i16 PCM bytes (as Kotlin's `ByteArray` carries them)
/// into samples. An odd trailing byte means the caller split a sample across
/// two `pushAudio` calls, which the shell must never do (frames are pushed in
/// whole-sample chunks) — treated as a caller error, not silently dropped.
pub(crate) fn bytes_le_to_i16(bytes: &[u8]) -> Result<Vec<i16>> {
    if bytes.len() % 2 != 0 {
        return Err(CoreError::InvalidInput(format!(
            "pcm byte length {} is not a multiple of 2 (odd trailing byte)",
            bytes.len()
        )));
    }
    Ok(bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
}

// ---------------------------------------------------------------------------
// VAD/ASR backend selection — real ONNX backends behind `onnx`, otherwise a
// clear constructor error (Silero VAD is required even for soak mode).
// ---------------------------------------------------------------------------

#[cfg(feature = "onnx")]
fn build_vad_asr(models_dir: &Path, soak_mode: bool) -> Result<(Box<dyn Vad>, Box<dyn Asr>)> {
    let vad = crate::onnx::SileroVad::new(&models_dir.join("silero_vad.onnx"))?;
    let asr: Box<dyn Asr> = if soak_mode {
        Box::new(NoopAsr)
    } else {
        Box::new(crate::onnx::ParakeetAsr::new(&models_dir.join("parakeet"))?)
    };
    Ok((Box::new(vad), asr))
}

#[cfg(not(feature = "onnx"))]
fn build_vad_asr(_models_dir: &Path, _soak_mode: bool) -> Result<(Box<dyn Vad>, Box<dyn Asr>)> {
    Err(CoreError::Vad(
        "this build of recall-core was compiled without the `onnx` feature, so no VAD backend \
         is available (Silero VAD is required even in soak mode); rebuild with `--features onnx`"
            .to_string(),
    ))
}

// ---------------------------------------------------------------------------
// RecallEngine — the single object Kotlin/Swift hold.
// ---------------------------------------------------------------------------

/// The UniFFI-exported engine handle. Not `Sync`-free-for-all under the hood:
/// `EngineCore` isn't internally synchronized, so this wraps it in a `Mutex`
/// (foreign callers hold it behind a shared `Arc<RecallEngine>`, and
/// `push_audio`/`finish_capture` need `&mut EngineCore`).
#[derive(uniffi::Object)]
pub struct RecallEngine {
    inner: Mutex<EngineCore>,
}

#[uniffi::export]
impl RecallEngine {
    /// Construct the engine. `db_path` is a SQLite file path (or `:memory:`);
    /// `audio_dir` is where finalized-conversation audio files are written;
    /// `models_dir` is joined with `"silero_vad.onnx"` and `"parakeet"` to
    /// locate the ONNX models (only used by `onnx`-feature builds — see
    /// `build_vad_asr`). `session_start_unix` anchors conversation timestamps
    /// (the core has no clock). `soak_mode` skips ASR/storage/events entirely
    /// (frame-level VAD telemetry only). `use_opus` picks the Opus encoder
    /// over WAV for finalized-conversation audio.
    #[allow(clippy::too_many_arguments)]
    #[uniffi::constructor]
    pub fn new(
        db_path: String,
        audio_dir: String,
        models_dir: String,
        session_start_unix: i64,
        soak_mode: bool,
        use_opus: bool,
        listener: Arc<dyn RecallListener>,
    ) -> Result<Arc<Self>> {
        let store = Store::open(&db_path)?;
        let (vad, asr) = build_vad_asr(Path::new(&models_dir), soak_mode)?;
        let encoder: Box<dyn AudioEncoder> =
            if use_opus { Box::new(OpusEncoder::new()) } else { Box::new(WavEncoder::new()) };
        let cfg = EngineConfig { segmenter: SegmenterConfig::default(), session_start_unix, soak_mode };
        let core = EngineCore::new(store, vad, asr, encoder, PathBuf::from(audio_dir), listener, cfg);
        Ok(Arc::new(Self { inner: Mutex::new(core) }))
    }

    pub fn push_audio(&self, pcm_le: Vec<u8>) -> Result<()> {
        let pcm = bytes_le_to_i16(&pcm_le)?;
        self.lock()?.push_audio(&pcm)
    }

    pub fn finish_capture(&self) -> Result<()> {
        self.lock()?.finish_capture()
    }

    pub fn list_conversations(&self) -> Result<Vec<FfiConversation>> {
        Ok(self.lock()?.list_conversations()?.into_iter().map(Into::into).collect())
    }

    pub fn get_conversation(&self, id: i64) -> Result<Option<FfiConversation>> {
        Ok(self.lock()?.get_conversation(id)?.map(Into::into))
    }

    pub fn transcript_text(&self, id: i64) -> Result<String> {
        self.lock()?.transcript_text(id)
    }

    pub fn search(&self, q: String) -> Result<Vec<FfiSearchHit>> {
        Ok(self.lock()?.search(&q)?.into_iter().map(Into::into).collect())
    }

    pub fn list_action_items(&self) -> Result<Vec<FfiActionItem>> {
        Ok(self.lock()?.list_action_items()?.into_iter().map(Into::into).collect())
    }

    pub fn complete_action_item(&self, id: i64) -> Result<()> {
        self.lock()?.complete_action_item(id)
    }

    pub fn storage_entries(&self) -> Result<Vec<FfiStorageEntry>> {
        Ok(self.lock()?.storage_entries()?.into_iter().map(Into::into).collect())
    }

    pub fn total_audio_bytes(&self) -> Result<i64> {
        self.lock()?.total_audio_bytes()
    }

    pub fn delete_audio(&self, id: i64) -> Result<()> {
        self.lock()?.delete_audio(id)
    }

    pub fn delete_conversation(&self, id: i64) -> Result<()> {
        self.lock()?.delete_conversation(id)
    }

    pub fn request_cloud_summary(&self, id: i64, deeper: bool) -> Result<Option<String>> {
        self.lock()?.request_cloud_summary(id, deeper)
    }

    pub fn store_cloud_summary(&self, id: i64, json: String) -> Result<()> {
        self.lock()?.store_cloud_summary(id, &json)
    }

    pub fn frame_stats(&self) -> Result<FfiFrameStats> {
        Ok(self.lock()?.frame_stats().into())
    }
}

impl RecallEngine {
    fn lock(&self) -> Result<MutexGuard<'_, EngineCore>> {
        self.inner
            .lock()
            .map_err(|_| CoreError::Io("recall engine mutex was poisoned by a prior panic".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::bytes_le_to_i16;

    #[test]
    fn bytes_le_to_i16_round_trips_even_length_input() {
        let samples: Vec<i16> = vec![0, 1, -1, 32767, -32768, 12345];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(bytes_le_to_i16(&bytes).unwrap(), samples);
    }

    #[test]
    fn bytes_le_to_i16_empty_input_is_empty_output() {
        assert_eq!(bytes_le_to_i16(&[]).unwrap(), Vec::<i16>::new());
    }

    #[test]
    fn bytes_le_to_i16_rejects_odd_trailing_byte() {
        let err = bytes_le_to_i16(&[1, 2, 3]).unwrap_err();
        assert!(matches!(err, crate::error::CoreError::InvalidInput(_)));
    }
}
