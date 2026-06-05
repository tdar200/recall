//! Core domain types shared across modules.

/// Milliseconds, measured from the start of a capture session.
pub type Millis = u64;

/// The time span of one finalized conversation within a capture session,
/// as produced by the segmentation FSM ([`crate::segment::Segmenter`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationSpan {
    pub start_ms: Millis,
    pub end_ms: Millis,
}

impl ConversationSpan {
    pub fn duration_ms(&self) -> Millis {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_is_end_minus_start_and_never_underflows() {
        assert_eq!(ConversationSpan { start_ms: 100, end_ms: 900 }.duration_ms(), 800);
        // saturating: a malformed span never panics
        assert_eq!(ConversationSpan { start_ms: 900, end_ms: 100 }.duration_ms(), 0);
    }
}
