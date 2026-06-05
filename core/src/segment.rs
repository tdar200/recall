//! Conversation segmentation FSM.
//!
//! Turns a stream of per-frame [`VadFrame`] values into discrete
//! [`ConversationSpan`]s using hysteresis + a silence gap, a pre-roll buffer so
//! starts aren't clipped, min/max length bounds, and a secondary
//! energy/min-cumulative-speech gate to reject TV/music false triggers.
//!
//! The state machine itself is implemented in task #2 (test-first).

use crate::model::{ConversationSpan, Millis};
use crate::vad::VadFrame;

/// Tuning for the segmentation FSM. Defaults are tuned for splitting
/// *conversations* (not phrases): the silence gap is far longer than Silero's
/// 100 ms phrase default, because short pauses within a conversation must not
/// split it. All values are configurable and tuned on the user's own audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmenterConfig {
    /// Duration of one VAD frame in ms (Silero @ 16 kHz, 512 samples ≈ 32 ms).
    pub frame_ms: Millis,
    /// Speech-enter probability threshold (hysteresis high).
    pub enter_threshold: f32,
    /// Speech-exit probability threshold (hysteresis low).
    pub exit_threshold: f32,
    /// Pre-roll kept before the first speech frame so starts aren't clipped.
    pub pre_roll_ms: Millis,
    /// Continuous silence required to CLOSE a conversation.
    pub silence_gap_ms: Millis,
    /// Conversations shorter than this (after gating) are discarded as junk.
    pub min_conversation_ms: Millis,
    /// Hard cap; a conversation exceeding this is split at the longest silence.
    pub max_conversation_ms: Millis,
    /// Minimum cumulative speech within a candidate before it counts as a real
    /// conversation (secondary gate vs. isolated coughs / door slams).
    pub min_cumulative_speech_ms: Millis,
    /// Minimum frame energy to count a frame as speech (secondary gate vs.
    /// loud-but-non-speech). `0.0` disables the energy gate.
    pub energy_gate: f32,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            frame_ms: 32,
            enter_threshold: 0.5,
            exit_threshold: 0.35,
            pre_roll_ms: 400,
            silence_gap_ms: 900,
            min_conversation_ms: 1_500,
            max_conversation_ms: 600_000,
            min_cumulative_speech_ms: 1_000,
            energy_gate: 0.0,
        }
    }
}

/// One in-progress conversation candidate.
struct Candidate {
    /// Conversation start, already adjusted for pre-roll.
    start_ms: Millis,
    /// End-exclusive time of the most recent speech frame.
    last_speech_end_ms: Millis,
    /// Total speech time accumulated in this candidate.
    cumulative_speech_ms: Millis,
    /// Consecutive non-speech time since the last speech frame.
    trailing_silence_ms: Millis,
}

/// Streaming conversation segmenter.
///
/// Feed one [`VadFrame`] per fixed-size audio frame with [`Segmenter::push`]; it
/// returns a [`ConversationSpan`] each time a conversation closes — either via a
/// silence gap or a max-length force-split. Call [`Segmenter::finish`] when
/// capture ends to flush any trailing in-progress conversation.
pub struct Segmenter {
    config: SegmenterConfig,
    /// End-exclusive time of the next frame to arrive (start of session = 0).
    now_ms: Millis,
    /// Hysteresis state: are we currently inside speech?
    speech_active: bool,
    candidate: Option<Candidate>,
}

impl Segmenter {
    pub fn new(config: SegmenterConfig) -> Self {
        Self { config, now_ms: 0, speech_active: false, candidate: None }
    }

    /// Process one VAD frame. Returns `Some(span)` iff a conversation closed on
    /// this frame.
    pub fn push(&mut self, frame: VadFrame) -> Option<ConversationSpan> {
        let frame_start = self.now_ms;
        let frame_end = self.now_ms + self.config.frame_ms;
        self.now_ms = frame_end;

        // Secondary energy gate: a high probability with low energy (e.g. a
        // distant TV) is forced to non-speech before hysteresis.
        let energetic =
            self.config.energy_gate == 0.0 || frame.energy >= self.config.energy_gate;
        let prob = if energetic { frame.speech_prob } else { 0.0 };

        // Hysteresis on the (gated) probability.
        if self.speech_active {
            if prob < self.config.exit_threshold {
                self.speech_active = false;
            }
        } else if prob >= self.config.enter_threshold {
            self.speech_active = true;
        }

        if self.speech_active {
            match &mut self.candidate {
                None => {
                    self.candidate = Some(Candidate {
                        start_ms: frame_start.saturating_sub(self.config.pre_roll_ms),
                        last_speech_end_ms: frame_end,
                        cumulative_speech_ms: self.config.frame_ms,
                        trailing_silence_ms: 0,
                    });
                }
                Some(c) => {
                    c.last_speech_end_ms = frame_end;
                    c.cumulative_speech_ms += self.config.frame_ms;
                    c.trailing_silence_ms = 0;
                }
            }
            // Force-split an over-long conversation; the next speech frame opens
            // a fresh candidate.
            let c = self.candidate.as_ref().expect("candidate just set");
            if c.last_speech_end_ms - c.start_ms >= self.config.max_conversation_ms {
                return self.close_candidate();
            }
            None
        } else {
            if let Some(c) = &mut self.candidate {
                c.trailing_silence_ms += self.config.frame_ms;
                if c.trailing_silence_ms >= self.config.silence_gap_ms {
                    return self.close_candidate();
                }
            }
            None
        }
    }

    /// Flush at end of capture, returning the trailing conversation if it
    /// qualifies. Idempotent.
    pub fn finish(&mut self) -> Option<ConversationSpan> {
        self.close_candidate()
    }

    /// Close the current candidate (if any): emit its span when it clears the
    /// min-length and min-cumulative-speech gates, otherwise drop it as junk.
    fn close_candidate(&mut self) -> Option<ConversationSpan> {
        let c = self.candidate.take()?;
        let span = ConversationSpan { start_ms: c.start_ms, end_ms: c.last_speech_end_ms };
        let qualifies = span.duration_ms() >= self.config.min_conversation_ms
            && c.cumulative_speech_ms >= self.config.min_cumulative_speech_ms;
        qualifies.then_some(span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConversationSpan;
    use crate::vad::VadFrame;

    /// Round-number config for legible assertions (100 ms frames).
    fn cfg() -> SegmenterConfig {
        SegmenterConfig {
            frame_ms: 100,
            enter_threshold: 0.5,
            exit_threshold: 0.35,
            pre_roll_ms: 100,
            silence_gap_ms: 500,
            min_conversation_ms: 300,
            max_conversation_ms: 2_000,
            min_cumulative_speech_ms: 200,
            energy_gate: 0.0,
        }
    }

    fn frame(p: f32, e: f32) -> VadFrame {
        VadFrame { speech_prob: p, energy: e }
    }
    fn speech() -> VadFrame {
        frame(0.9, 1.0)
    }
    fn silence() -> VadFrame {
        frame(0.0, 0.0)
    }

    /// Push every frame, collecting any spans the FSM emits along the way.
    fn feed(seg: &mut Segmenter, frames: &[VadFrame]) -> Vec<ConversationSpan> {
        frames.iter().filter_map(|f| seg.push(*f)).collect()
    }

    fn repeat(f: VadFrame, n: usize) -> Vec<VadFrame> {
        vec![f; n]
    }

    #[test]
    fn silence_only_never_emits() {
        let mut seg = Segmenter::new(cfg());
        assert_eq!(feed(&mut seg, &repeat(silence(), 20)), vec![]);
        assert_eq!(seg.finish(), None);
    }

    #[test]
    fn single_burst_emits_one_span_with_preroll_and_excludes_trailing_silence() {
        let mut seg = Segmenter::new(cfg());
        let mut frames = repeat(silence(), 2); // t 0..200
        frames.extend(repeat(speech(), 5)); // t 200..700
        frames.extend(repeat(silence(), 5)); // gap closes at the 5th (trailing 500)
        let spans = feed(&mut seg, &frames);
        // start = first_speech(200) - pre_roll(100) = 100; end = last speech end (700)
        assert_eq!(spans, vec![ConversationSpan { start_ms: 100, end_ms: 700 }]);
        assert_eq!(seg.finish(), None);
    }

    #[test]
    fn isolated_blip_is_discarded() {
        let mut seg = Segmenter::new(cfg());
        let mut frames = repeat(speech(), 1); // 100 ms < min_cumulative(200) & < min_conversation(300)
        frames.extend(repeat(silence(), 5));
        assert_eq!(feed(&mut seg, &frames), vec![]);
        assert_eq!(seg.finish(), None);
    }

    #[test]
    fn short_pause_below_gap_does_not_split() {
        let mut seg = Segmenter::new(cfg());
        let mut frames = repeat(speech(), 3); // 0..300
        frames.extend(repeat(silence(), 3)); // 300 ms silence < 500 gap → stays open
        frames.extend(repeat(speech(), 3)); // 600..900
        frames.extend(repeat(silence(), 5)); // closes
        let spans = feed(&mut seg, &frames);
        assert_eq!(spans, vec![ConversationSpan { start_ms: 0, end_ms: 900 }]);
    }

    #[test]
    fn hysteresis_keeps_speech_between_exit_and_enter() {
        let mut seg = Segmenter::new(cfg());
        // enter at 0.9, then frames at 0.4 (below enter 0.5, above exit 0.35) must
        // still count as speech and not accrue silence.
        let mut frames = repeat(speech(), 1); // idx0
        frames.extend(repeat(frame(0.4, 1.0), 5)); // idx1..5 stay active
        frames.extend(repeat(silence(), 5)); // close
        let spans = feed(&mut seg, &frames);
        // If hysteresis were wrong, the 0.4 frames would be silence and close early
        // with too little cumulative speech → no span.
        assert_eq!(spans, vec![ConversationSpan { start_ms: 0, end_ms: 600 }]);
    }

    #[test]
    fn two_conversations_separated_by_long_gap() {
        let mut seg = Segmenter::new(cfg());
        let mut frames = repeat(speech(), 3); // 0..300
        frames.extend(repeat(silence(), 5)); // closes conv 1
        frames.extend(repeat(speech(), 3)); // 800..1100
        frames.extend(repeat(silence(), 5)); // closes conv 2
        let spans = feed(&mut seg, &frames);
        assert_eq!(
            spans,
            vec![
                ConversationSpan { start_ms: 0, end_ms: 300 },
                ConversationSpan { start_ms: 700, end_ms: 1100 },
            ]
        );
    }

    #[test]
    fn energy_gate_rejects_high_prob_low_energy() {
        let c = SegmenterConfig { energy_gate: 0.5, ..cfg() };
        let mut seg = Segmenter::new(c);
        // High speech prob but energy below the gate (e.g. distant TV) → non-speech.
        let frames = repeat(frame(0.95, 0.1), 10);
        assert_eq!(feed(&mut seg, &frames), vec![]);
        assert_eq!(seg.finish(), None);
    }

    #[test]
    fn finish_flushes_in_progress_conversation_and_clamps_preroll() {
        let mut seg = Segmenter::new(cfg());
        // speech from t=0 with no trailing silence; capture ends → finish() flushes.
        let spans = feed(&mut seg, &repeat(speech(), 5)); // 0..500
        assert_eq!(spans, vec![]); // nothing closed mid-stream
        // pre-roll clamps at session start: start = max(0, 0 - 100) = 0
        assert_eq!(seg.finish(), Some(ConversationSpan { start_ms: 0, end_ms: 500 }));
        // finishing twice is a no-op
        assert_eq!(seg.finish(), None);
    }

    #[test]
    fn over_long_speech_force_splits_at_max_length() {
        let c = SegmenterConfig {
            pre_roll_ms: 0,
            max_conversation_ms: 500,
            ..cfg()
        };
        let mut seg = Segmenter::new(c);
        // 10 continuous speech frames (0..1000) → two 500 ms chunks.
        let spans = feed(&mut seg, &repeat(speech(), 10));
        assert_eq!(
            spans,
            vec![
                ConversationSpan { start_ms: 0, end_ms: 500 },
                ConversationSpan { start_ms: 500, end_ms: 1000 },
            ]
        );
        assert_eq!(seg.finish(), None);
    }
}
