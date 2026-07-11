//! The capture→recall pipeline, wired together.
//!
//! The native shell's microphone foreground service reads 16 kHz / mono / 16-bit
//! PCM and pushes it down in large chunks via [`EngineCore::push_audio`]. The
//! engine frames it for the VAD, drives the [`crate::segment::Segmenter`], and
//! when a conversation closes it transcribes that conversation's audio (batch),
//! stores it, generates an extractive summary, and emits coarse events to the
//! shell via [`RecallListener`].
//!
//! VAD and ASR are injected as trait objects, so this whole pipeline is
//! device-independent and unit-tested with mocks; the ONNX-backed Silero/Parakeet
//! implementations slot in on the Android build.

use crate::asr::Asr;
use crate::audio::AudioEncoder;
use crate::cloud::{build_cloud_summary_request, CloudModel};
use crate::error::{CoreError, Result};
use crate::events::RecallListener;
use crate::segment::{Segmenter, SegmenterConfig};
use crate::store::{ActionItem, Conversation, NewConversation, SearchHit, StorageEntry, Store};
use crate::summary::{summarize, SummaryOptions};
use crate::vad::{Vad, VadFrame};
use std::path::PathBuf;
use std::sync::Arc;

const SAMPLE_RATE: usize = 16_000;
const SAMPLES_PER_MS: usize = SAMPLE_RATE / 1_000; // 16
/// Silero's fixed window at 16 kHz.
const FRAME_SAMPLES: usize = 512;

/// Engine configuration.
pub struct EngineConfig {
    pub segmenter: SegmenterConfig,
    /// Unix seconds at the start of the capture session (the core has no clock;
    /// the shell supplies this so conversation timestamps are absolute).
    pub session_start_unix: i64,
    /// When true, the pipeline still frames audio and drives the VAD/segmenter
    /// (so `frame_stats` is meaningful for tuning), but `finalize` is a no-op:
    /// no ASR, no audio file, no store row, no listener events. Used for M0's
    /// on-device soak testing, where we want frame-rate/battery telemetry
    /// without persisting anything the user recorded.
    pub soak_mode: bool,
}

/// Per-frame VAD counters, exposed for the shell's soak-mode telemetry (frame
/// throughput and speech ratio don't require a full conversation to exist).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameStats {
    pub total_frames: u64,
    pub speech_frames: u64,
}

/// Orchestrates capture → VAD → segmentation → ASR → store → summary → events.
///
/// Not internally synchronized: the shell drives it from a single capture thread
/// (the Android foreground service). The UniFFI wrapper will add a `Mutex` so the
/// foreign side can hold it in an `Arc`.
pub struct EngineCore {
    vad: Box<dyn Vad>,
    asr: Box<dyn Asr>,
    encoder: Box<dyn AudioEncoder>,
    audio_dir: PathBuf,
    segmenter: Segmenter,
    store: Store,
    listener: Arc<dyn RecallListener>,
    session_start_unix: i64,
    /// Mirrors `EngineConfig::soak_mode`; see its doc comment.
    soak_mode: bool,
    /// Copied from `cfg.segmenter.enter_threshold` at construction so
    /// `push_audio`'s frame-counting doesn't need to reach into the segmenter.
    enter_threshold: f32,

    /// Retained PCM, from which closed conversations are sliced.
    pcm: Vec<i16>,
    /// Absolute ms timestamp of `pcm[0]`.
    pcm_start_ms: u64,
    /// Index of the next unframed sample (relative to `pcm[0]`).
    frame_cursor: usize,
    /// Upper bound on retained, already-framed PCM (samples).
    retain_samples: usize,
    /// Running per-frame VAD counters (see `frame_stats`).
    frame_stats: FrameStats,
}

impl EngineCore {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Store,
        vad: Box<dyn Vad>,
        asr: Box<dyn Asr>,
        encoder: Box<dyn AudioEncoder>,
        audio_dir: PathBuf,
        listener: Arc<dyn RecallListener>,
        cfg: EngineConfig,
    ) -> Self {
        let s = &cfg.segmenter;
        // Retain at least a full max-length conversation plus its pre-roll, so a
        // closing span is always still sliceable from `pcm`.
        let retain_ms = s.max_conversation_ms + s.pre_roll_ms + 2 * s.frame_ms;
        let retain_samples = retain_ms as usize * SAMPLES_PER_MS;
        let enter_threshold = cfg.segmenter.enter_threshold;
        Self {
            vad,
            asr,
            encoder,
            audio_dir,
            segmenter: Segmenter::new(cfg.segmenter),
            store,
            listener,
            session_start_unix: cfg.session_start_unix,
            soak_mode: cfg.soak_mode,
            enter_threshold,
            pcm: Vec::new(),
            pcm_start_ms: 0,
            frame_cursor: 0,
            retain_samples,
            frame_stats: FrameStats::default(),
        }
    }

    /// Feed a chunk of PCM (i16 mono @ 16 kHz). Any whole frames it completes are
    /// processed; a partial frame is buffered for the next call.
    pub fn push_audio(&mut self, pcm: &[i16]) -> Result<()> {
        self.pcm.extend_from_slice(pcm);
        while self.pcm.len() - self.frame_cursor >= FRAME_SAMPLES {
            // Copy the frame out so the VAD call doesn't alias `self.pcm`.
            let frame: Vec<i16> =
                self.pcm[self.frame_cursor..self.frame_cursor + FRAME_SAMPLES].to_vec();
            let speech_prob = self.vad.process_chunk(&frame);
            let energy = rms(&frame);
            self.frame_cursor += FRAME_SAMPLES;

            self.frame_stats.total_frames += 1;
            if speech_prob >= self.enter_threshold {
                self.frame_stats.speech_frames += 1;
            }

            if let Some(span) = self.segmenter.push(VadFrame { speech_prob, energy }) {
                self.finalize(span)?;
            }
        }
        self.trim();
        Ok(())
    }

    /// Flush at end of capture: close any in-progress conversation.
    pub fn finish_capture(&mut self) -> Result<()> {
        if let Some(span) = self.segmenter.finish() {
            self.finalize(span)?;
        }
        Ok(())
    }

    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        self.store.list_conversations()
    }

    pub fn get_conversation(&self, id: i64) -> Result<Option<Conversation>> {
        self.store.get_conversation(id)
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        self.store.search(query)
    }

    /// Concatenated transcript text for a conversation (see `Store::transcript_text`).
    pub fn transcript_text(&self, conversation_id: i64) -> Result<String> {
        self.store.transcript_text(conversation_id)
    }

    /// Cumulative per-frame VAD counters since construction — for the shell's
    /// soak-mode telemetry (frame throughput and speech ratio).
    pub fn frame_stats(&self) -> FrameStats {
        self.frame_stats
    }

    /// Build the one-tap cloud-summary request for a conversation (transcript
    /// text only). Returns `None` when there's no transcript to summarize. The
    /// shell performs the authenticated HTTPS POST.
    pub fn request_cloud_summary(&self, conversation_id: i64, deeper: bool) -> Result<Option<String>> {
        let transcript = self.store.transcript_text(conversation_id)?;
        if transcript.trim().is_empty() {
            return Ok(None);
        }
        let model = if deeper { CloudModel::Deeper } else { CloudModel::Standard };
        Ok(Some(build_cloud_summary_request(&transcript, model).to_string()))
    }

    /// Persist a cloud summary the shell received back from Claude, and extract
    /// its `action_items[]` into the action-items table. Malformed JSON still
    /// stores the raw summary (extraction is best-effort).
    pub fn store_cloud_summary(&self, conversation_id: i64, summary_json: &str) -> Result<()> {
        self.store.set_cloud_summary(conversation_id, summary_json)?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(summary_json) {
            if let Some(items) = v.get("action_items").and_then(|x| x.as_array()) {
                for item in items {
                    if let Some(task) = item.get("task").and_then(|t| t.as_str()) {
                        let owner = item.get("owner").and_then(|o| o.as_str());
                        let due = item.get("due").and_then(|d| d.as_str());
                        self.store.insert_action_item(conversation_id, task, owner, due)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Open action items across all conversations (for the Action Items screen).
    pub fn list_action_items(&self) -> Result<Vec<ActionItem>> {
        self.store.list_open_action_items()
    }

    pub fn complete_action_item(&self, id: i64) -> Result<()> {
        self.store.complete_action_item(id)
    }

    /// Total retained (un-pruned) audio bytes — for the storage dashboard.
    pub fn total_audio_bytes(&self) -> Result<i64> {
        self.store.total_audio_bytes()
    }

    /// Per-conversation audio sizes, largest first.
    pub fn storage_entries(&self) -> Result<Vec<StorageEntry>> {
        self.store.storage_entries()
    }

    /// Delete a conversation's audio file but KEEP its transcript and summary.
    pub fn delete_audio(&self, conversation_id: i64) -> Result<()> {
        if let Some(c) = self.store.get_conversation(conversation_id)? {
            let _ = std::fs::remove_file(&c.audio_path);
        }
        self.store.mark_audio_pruned(conversation_id)
    }

    /// Delete a conversation entirely — audio file, transcript, summaries, events.
    pub fn delete_conversation(&self, conversation_id: i64) -> Result<()> {
        if let Some(c) = self.store.get_conversation(conversation_id)? {
            let _ = std::fs::remove_file(&c.audio_path);
        }
        self.store.delete_conversation(conversation_id)
    }

    /// Transcribe, store, summarize, and announce a closed conversation.
    fn finalize(&mut self, span: crate::model::ConversationSpan) -> Result<()> {
        if self.soak_mode {
            // Soak mode only cares about frame-level VAD telemetry (already
            // recorded in `push_audio`); skip ASR, encoding, storage, and events
            // entirely so nothing the user recorded is persisted.
            return Ok(());
        }

        let start_sample = (span.start_ms.saturating_sub(self.pcm_start_ms)) as usize * SAMPLES_PER_MS;
        let end_sample = ((span.end_ms.saturating_sub(self.pcm_start_ms)) as usize * SAMPLES_PER_MS)
            .min(self.pcm.len());
        let slice: Vec<i16> = if start_sample < end_sample {
            self.pcm[start_sample..end_sample].to_vec()
        } else {
            Vec::new()
        };

        let utterances = self.asr.transcribe(&slice).unwrap_or_default();

        // Encode and persist this conversation's audio.
        let bytes = self.encoder.encode(&slice).unwrap_or_default();
        let path = self
            .audio_dir
            .join(format!("conv-{}.{}", span.start_ms, self.encoder.extension()));
        std::fs::create_dir_all(&self.audio_dir)
            .and_then(|_| std::fs::write(&path, &bytes))
            .map_err(|e| CoreError::Io(e.to_string()))?;

        let conv_id = self.store.insert_conversation(NewConversation {
            started_at: self.session_start_unix + (span.start_ms / 1_000) as i64,
            ended_at: self.session_start_unix + (span.end_ms / 1_000) as i64,
            audio_path: path.to_string_lossy().into_owned(),
        })?;
        self.store
            .set_audio_meta(conv_id, bytes.len() as i64, self.encoder.extension())?;
        self.listener.on_conversation_finalized(conv_id);

        for u in &utterances {
            self.store
                .insert_segment(conv_id, u.start_ms as i64, u.end_ms as i64, &u.text)?;
        }
        self.listener.on_transcript_ready(conv_id);

        let transcript = utterances
            .iter()
            .map(|u| u.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let summary = summarize(&transcript, SummaryOptions::default());
        self.store.set_basic_summary(conv_id, &summary.gist)?;
        self.listener.on_summary_ready(conv_id);

        Ok(())
    }

    /// Drop already-framed PCM beyond the retention window (ms-aligned so
    /// `pcm_start_ms` stays integral). Never drops unframed samples.
    fn trim(&mut self) {
        if self.frame_cursor > self.retain_samples {
            let mut drop = self.frame_cursor - self.retain_samples;
            drop -= drop % SAMPLES_PER_MS; // keep ms alignment
            if drop > 0 {
                self.pcm.drain(0..drop);
                self.frame_cursor -= drop;
                self.pcm_start_ms += (drop / SAMPLES_PER_MS) as u64;
            }
        }
    }
}

/// Normalized RMS energy of a frame, in `[0, 1]`.
fn rms(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = frame.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let mean = sum_sq / frame.len() as f64;
    (mean.sqrt() / i16::MAX as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::{EngineConfig, EngineCore};
    use crate::asr::{Asr, Utterance};
    use crate::error::Result;
    use crate::events::RecallListener;
    use crate::segment::SegmenterConfig;
    use crate::store::Store;
    use crate::vad::Vad;
    use std::sync::{Arc, Mutex};

    /// A VAD that replays a fixed probability per frame (ignores the audio).
    struct ScriptedVad {
        probs: Vec<f32>,
        i: usize,
    }
    impl Vad for ScriptedVad {
        fn process_chunk(&mut self, _pcm: &[i16]) -> f32 {
            let p = self.probs.get(self.i).copied().unwrap_or(0.0);
            self.i += 1;
            p
        }
        fn reset(&mut self) {}
    }

    /// An ASR that returns one canned utterance covering the whole buffer.
    struct MockAsr;
    impl Asr for MockAsr {
        fn transcribe(&mut self, pcm: &[i16]) -> Result<Vec<Utterance>> {
            Ok(vec![Utterance {
                start_ms: 0,
                end_ms: (pcm.len() / 16) as u64,
                text: "we discussed the budget and agreed to meet friday".to_string(),
            }])
        }
    }

    #[derive(Default)]
    struct RecordingListener {
        finalized: Mutex<Vec<i64>>,
        transcript: Mutex<Vec<i64>>,
        summary: Mutex<Vec<i64>>,
    }
    impl RecallListener for RecordingListener {
        fn on_conversation_finalized(&self, id: i64) {
            self.finalized.lock().unwrap().push(id);
        }
        fn on_transcript_ready(&self, id: i64) {
            self.transcript.lock().unwrap().push(id);
        }
        fn on_summary_ready(&self, id: i64) {
            self.summary.lock().unwrap().push(id);
        }
    }

    fn cfg() -> EngineConfig {
        EngineConfig {
            segmenter: SegmenterConfig {
                frame_ms: 32, // Silero @16k = 512 samples = 32 ms (fixed)
                enter_threshold: 0.5,
                exit_threshold: 0.35,
                pre_roll_ms: 64,
                silence_gap_ms: 96,
                min_conversation_ms: 64,
                max_conversation_ms: 60_000,
                min_cumulative_speech_ms: 64,
                energy_gate: 0.0,
            },
            session_start_unix: 1_000_000,
            soak_mode: false,
        }
    }

    /// Unique temp dir per call (no rand/clock needed).
    fn temp_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "recall-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn engine_in_with_cfg(
        dir: std::path::PathBuf,
        probs: Vec<f32>,
        listener: Arc<RecordingListener>,
        cfg: EngineConfig,
    ) -> EngineCore {
        EngineCore::new(
            Store::open_in_memory().unwrap(),
            Box::new(ScriptedVad { probs, i: 0 }),
            Box::new(MockAsr),
            Box::new(crate::audio::WavEncoder::new()),
            dir,
            listener,
            cfg,
        )
    }

    fn engine_in(
        dir: std::path::PathBuf,
        probs: Vec<f32>,
        listener: Arc<RecordingListener>,
    ) -> EngineCore {
        engine_in_with_cfg(dir, probs, listener, cfg())
    }

    fn engine(probs: Vec<f32>, listener: Arc<RecordingListener>) -> EngineCore {
        engine_in(temp_dir(), probs, listener)
    }

    /// Same tuning as `cfg()` but with `soak_mode` on.
    fn cfg_soak() -> EngineConfig {
        EngineConfig { soak_mode: true, ..cfg() }
    }

    /// Build a per-frame probability stream from (prob, frame_count) runs.
    fn frames(spec: &[(f32, usize)]) -> Vec<f32> {
        let mut v = Vec::new();
        for &(p, n) in spec {
            v.extend(std::iter::repeat_n(p, n));
        }
        v
    }

    fn pcm_for(frame_count: usize) -> Vec<i16> {
        vec![1000i16; frame_count * 512]
    }

    #[test]
    fn pipeline_stores_transcribed_summarized_conversation_and_emits_events() {
        let listener = Arc::new(RecordingListener::default());
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), listener.clone());
        e.push_audio(&pcm_for(9)).unwrap();
        e.finish_capture().unwrap();

        let convs = e.list_conversations().unwrap();
        assert_eq!(convs.len(), 1);
        let c = &convs[0];
        assert!(
            c.summary_basic.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
            "expected a non-empty basic summary"
        );

        // Each coarse event fired exactly once, for this conversation.
        assert_eq!(*listener.finalized.lock().unwrap(), vec![c.id]);
        assert_eq!(*listener.transcript.lock().unwrap(), vec![c.id]);
        assert_eq!(*listener.summary.lock().unwrap(), vec![c.id]);

        // The transcript is searchable.
        assert_eq!(e.search("budget").unwrap().len(), 1);
    }

    #[test]
    fn silence_only_produces_no_conversation_or_events() {
        let listener = Arc::new(RecordingListener::default());
        let mut e = engine(frames(&[(0.0, 12)]), listener.clone());
        e.push_audio(&pcm_for(12)).unwrap();
        e.finish_capture().unwrap();
        assert!(e.list_conversations().unwrap().is_empty());
        assert!(listener.finalized.lock().unwrap().is_empty());
    }

    #[test]
    fn two_conversations_separated_by_silence_are_both_stored() {
        let listener = Arc::new(RecordingListener::default());
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4), (0.9, 5), (0.0, 4)]), listener.clone());
        e.push_audio(&pcm_for(18)).unwrap();
        e.finish_capture().unwrap();
        assert_eq!(e.list_conversations().unwrap().len(), 2);
        assert_eq!(listener.finalized.lock().unwrap().len(), 2);
    }

    #[test]
    fn audio_split_across_pushes_still_yields_one_conversation() {
        let listener = Arc::new(RecordingListener::default());
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), listener.clone());
        let pcm = pcm_for(9);
        e.push_audio(&pcm[..700]).unwrap(); // split mid-frame
        e.push_audio(&pcm[700..]).unwrap();
        e.finish_capture().unwrap();
        assert_eq!(e.list_conversations().unwrap().len(), 1);
    }

    fn one_conversation(e: &mut EngineCore) -> i64 {
        e.push_audio(&pcm_for(9)).unwrap();
        e.finish_capture().unwrap();
        e.list_conversations().unwrap()[0].id
    }

    #[test]
    fn request_cloud_summary_builds_haiku_request_carrying_the_transcript() {
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        let id = one_conversation(&mut e);
        let req = e.request_cloud_summary(id, false).unwrap().expect("has a transcript");
        assert!(req.contains("claude-haiku-4-5"));
        assert!(req.contains("budget")); // the transcript text is included
    }

    #[test]
    fn finalized_conversation_writes_a_real_playable_audio_file() {
        let dir = temp_dir();
        let mut e = engine_in(dir, frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        e.push_audio(&pcm_for(9)).unwrap();
        e.finish_capture().unwrap();

        let c = &e.list_conversations().unwrap()[0];
        let path = std::path::Path::new(&c.audio_path);
        assert!(path.exists(), "audio file should exist at {}", c.audio_path);
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert!(bytes.len() > 44, "should contain audio samples, not just a header");
    }

    #[test]
    fn store_cloud_summary_persists_to_the_conversation() {
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        let id = one_conversation(&mut e);
        e.store_cloud_summary(id, "{\"one_line_summary\":\"agreed budget\"}").unwrap();
        let c = e.list_conversations().unwrap().into_iter().find(|c| c.id == id).unwrap();
        assert_eq!(c.summary_cloud.as_deref(), Some("{\"one_line_summary\":\"agreed budget\"}"));
    }

    #[test]
    fn storing_a_cloud_summary_extracts_its_action_items() {
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        let id = one_conversation(&mut e);
        let json = r#"{"one_line_summary":"plans","key_points":[],"decisions":[],
            "action_items":[{"task":"email landlord","owner":"me","due":null},
                            {"task":"book dentist","owner":null,"due":"friday"}],
            "open_questions":[]}"#;
        e.store_cloud_summary(id, json).unwrap();

        let items = e.list_action_items().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task, "email landlord");
        assert_eq!(items[1].due.as_deref(), Some("friday"));

        e.complete_action_item(items[0].id).unwrap();
        assert_eq!(e.list_action_items().unwrap().len(), 1);
    }

    #[test]
    fn finalize_records_audio_byte_size() {
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        let id = one_conversation(&mut e);
        assert!(e.total_audio_bytes().unwrap() > 0);
        let entries = e.storage_entries().unwrap();
        assert_eq!(entries[0].conversation_id, id);
        assert!(entries[0].audio_bytes > 0);
    }

    #[test]
    fn delete_audio_removes_file_but_keeps_transcript() {
        let mut e = engine_in(temp_dir(), frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        let id = one_conversation(&mut e);
        let path = e.list_conversations().unwrap()[0].audio_path.clone();
        assert!(std::path::Path::new(&path).exists());

        e.delete_audio(id).unwrap();
        assert!(!std::path::Path::new(&path).exists(), "audio file should be deleted");
        assert_eq!(e.total_audio_bytes().unwrap(), 0);
        assert_eq!(e.search("budget").unwrap().len(), 1, "transcript must be kept");
    }

    #[test]
    fn delete_conversation_removes_everything_including_the_file() {
        let mut e = engine_in(temp_dir(), frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        let id = one_conversation(&mut e);
        let path = e.list_conversations().unwrap()[0].audio_path.clone();

        e.delete_conversation(id).unwrap();
        assert!(!std::path::Path::new(&path).exists());
        assert!(e.list_conversations().unwrap().is_empty());
        assert!(e.search("budget").unwrap().is_empty());
    }

    #[test]
    fn frame_stats_counts_total_and_speech_frames() {
        // cfg()'s enter_threshold is 0.5: 5 frames at 0.9 are speech, 4 at 0.0 aren't.
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        e.push_audio(&pcm_for(9)).unwrap();

        let stats = e.frame_stats();
        assert_eq!(stats.total_frames, 9);
        assert_eq!(stats.speech_frames, 5);
    }

    #[test]
    fn soak_mode_counts_frames_but_persists_nothing() {
        let dir = temp_dir();
        let mut e = engine_in_with_cfg(
            dir.clone(),
            frames(&[(0.9, 5), (0.0, 4)]),
            Arc::new(RecordingListener::default()),
            cfg_soak(),
        );
        e.push_audio(&pcm_for(9)).unwrap();
        e.finish_capture().unwrap();

        assert!(e.frame_stats().total_frames > 0);
        assert!(e.list_conversations().unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "soak mode must not write any audio files"
        );
    }

    #[test]
    fn engine_exposes_conversation_and_transcript() {
        let mut e = engine(frames(&[(0.9, 5), (0.0, 4)]), Arc::new(RecordingListener::default()));
        let id = one_conversation(&mut e);

        let c = e.get_conversation(id).unwrap().expect("conversation should exist");
        assert_eq!(c.id, id);

        let text = e.transcript_text(id).unwrap();
        assert_eq!(text, "we discussed the budget and agreed to meet friday");
    }
}
