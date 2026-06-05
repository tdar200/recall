//! Tier-0 extractive local summary (TextRank + keywords + first/last utterance).
//!
//! Deterministic, zero-hallucination, instant on any phone — this is the default
//! "basic" summary shown the moment a conversation closes. The higher-quality
//! cloud summary (Claude Haiku 4.5, opt-in) lives in [`crate::cloud`].
//!
//! Implemented in task #3 (test-first).

use std::collections::{BTreeMap, HashSet};

/// A deterministic, zero-hallucination extractive summary of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractiveSummary {
    /// The most central sentences, in original document order, joined by spaces.
    pub gist: String,
    /// Top content keywords, most frequent first (ties broken alphabetically).
    pub keywords: Vec<String>,
}

/// Tuning for [`summarize`].
#[derive(Debug, Clone, Copy)]
pub struct SummaryOptions {
    pub max_sentences: usize,
    pub max_keywords: usize,
}

impl Default for SummaryOptions {
    fn default() -> Self {
        Self { max_sentences: 3, max_keywords: 6 }
    }
}

/// Produce an extractive summary of `transcript`.
///
/// Sentences are ranked by TextRank (PageRank over a sentence-similarity graph)
/// with a fixed iteration count, so the output is fully deterministic. The gist
/// is the top `max_sentences` re-ordered to their original positions for
/// readability. Keywords are the most frequent content words.
pub fn summarize(transcript: &str, opts: SummaryOptions) -> ExtractiveSummary {
    let sentences = split_sentences(transcript);
    if sentences.is_empty() {
        return ExtractiveSummary { gist: String::new(), keywords: Vec::new() };
    }

    let keywords = top_keywords(&sentences, opts.max_keywords);

    let token_sets: Vec<HashSet<String>> = sentences
        .iter()
        .map(|s| content_tokens(s).into_iter().collect())
        .collect();
    let scores = textrank(&token_sets);

    // Rank by score desc, breaking ties by original index for determinism.
    let mut order: Vec<usize> = (0..sentences.len()).collect();
    order.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let mut chosen: Vec<usize> = order.into_iter().take(opts.max_sentences.min(sentences.len())).collect();
    chosen.sort_unstable(); // restore document order

    let gist = chosen
        .iter()
        .map(|&i| sentences[i].as_str())
        .collect::<Vec<_>>()
        .join(" ");

    ExtractiveSummary { gist, keywords }
}

/// Split on sentence terminators and newlines, keeping the terminator and
/// trimming whitespace. Empty fragments are dropped.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
        cur.clear();
    };
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            flush(&mut cur, &mut out);
            continue;
        }
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Lowercased alphanumeric content words (length ≥ 2, stopwords removed).
fn content_tokens(sentence: &str) -> Vec<String> {
    sentence
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .filter(|w| w.chars().count() >= 2 && !is_stopword(w))
        .collect()
}

fn top_keywords(sentences: &[String], max_keywords: usize) -> Vec<String> {
    let mut freq: BTreeMap<String, usize> = BTreeMap::new();
    for s in sentences {
        for tok in content_tokens(s) {
            *freq.entry(tok).or_default() += 1;
        }
    }
    let mut kv: Vec<(String, usize)> = freq.into_iter().collect();
    // BTreeMap already alpha-ordered; stable sort by frequency desc keeps ties alpha.
    kv.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    kv.into_iter().take(max_keywords).map(|(w, _)| w).collect()
}

/// PageRank over a sentence-similarity graph (fixed 40 iterations, damping 0.85).
fn textrank(token_sets: &[HashSet<String>]) -> Vec<f64> {
    let n = token_sets.len();
    if n == 1 {
        return vec![1.0];
    }
    // Symmetric similarity = shared content words / (ln(1+|a|) + ln(1+|b|)).
    let mut weights = vec![vec![0f64; n]; n];
    let mut rowsum = vec![0f64; n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let (a, b) = (&token_sets[i], &token_sets[j]);
            if a.is_empty() || b.is_empty() {
                continue;
            }
            let common = a.intersection(b).count() as f64;
            if common == 0.0 {
                continue;
            }
            let denom = (a.len() as f64).ln_1p() + (b.len() as f64).ln_1p();
            let w = common / denom;
            weights[i][j] = w;
            rowsum[i] += w;
        }
    }

    let d = 0.85;
    let mut score = vec![1f64; n];
    for _ in 0..40 {
        let mut next = vec![0f64; n];
        for i in 0..n {
            let mut acc = 0.0;
            for j in 0..n {
                if i != j && rowsum[j] > 0.0 {
                    acc += weights[j][i] / rowsum[j] * score[j];
                }
            }
            next[i] = (1.0 - d) + d * acc;
        }
        score = next;
    }
    score
}

fn is_stopword(w: &str) -> bool {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "if", "then", "so", "to", "of", "in", "on",
        "at", "by", "for", "with", "about", "as", "is", "are", "was", "were", "be", "been",
        "being", "am", "do", "does", "did", "have", "has", "had", "will", "would", "shall",
        "should", "can", "could", "may", "might", "must", "i", "you", "he", "she", "it",
        "we", "they", "me", "him", "her", "us", "them", "my", "your", "his", "its", "our",
        "their", "this", "that", "these", "those", "there", "here", "what", "which", "who",
        "whom", "not", "no", "yes", "up", "out", "from", "into", "over", "again", "just",
        "now", "too", "very", "let", "lets",
    ];
    STOPWORDS.contains(&w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(max_sentences: usize, max_keywords: usize) -> SummaryOptions {
        SummaryOptions { max_sentences, max_keywords }
    }

    /// Split a produced gist back into trimmed, terminator-free sentence pieces.
    fn pieces(gist: &str) -> Vec<String> {
        gist.split_terminator(['.', '!', '?'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn empty_transcript_yields_empty_summary() {
        let r = summarize("   \n  ", opts(3, 6));
        assert_eq!(r.gist, "");
        assert!(r.keywords.is_empty());
    }

    #[test]
    fn single_sentence_is_returned_as_gist() {
        let r = summarize("Remember to call the dentist.", opts(3, 6));
        assert_eq!(r.gist, "Remember to call the dentist.");
    }

    #[test]
    fn keywords_are_frequent_content_words_excluding_stopwords() {
        // "the" is a stopword and excluded; ranked by frequency desc then alpha.
        let r = summarize("Budget budget budget. The the the meeting meeting. Coffee.", opts(3, 2));
        assert_eq!(r.keywords, vec!["budget".to_string(), "meeting".to_string()]);
    }

    #[test]
    fn outlier_sentence_is_not_selected() {
        let t = "Let's meet about the budget. The budget review is on Friday. \
                 Friday works for the budget meeting. I will bring coffee.";
        let r = summarize(t, opts(2, 6));
        assert!(
            !r.gist.to_lowercase().contains("coffee"),
            "low-content outlier should be dropped, got: {}",
            r.gist
        );
        assert!(r.gist.to_lowercase().contains("budget"));
    }

    #[test]
    fn selected_sentences_preserve_document_order() {
        let t = "Alpha beta gamma topic here. Beta gamma topic again now. \
                 Totally separate unrelated remark. Topic alpha beta gamma core point.";
        let r = summarize(t, opts(2, 6));
        let ps = pieces(&r.gist);
        assert_eq!(ps.len(), 2, "expected two sentences, got: {}", r.gist);
        let p0 = t.find(&ps[0]).expect("piece 0 present in source");
        let p1 = t.find(&ps[1]).expect("piece 1 present in source");
        assert!(p0 < p1, "sentences must stay in document order: {:?}", ps);
    }

    #[test]
    fn gist_caps_to_max_sentences() {
        let t = "Aa bb cc dd. Bb cc dd ee. Cc dd ee ff. Dd ee ff gg. Ee ff gg hh.";
        let r = summarize(t, opts(2, 6));
        assert!(pieces(&r.gist).len() <= 2, "got: {}", r.gist);
    }

    #[test]
    fn summarize_is_deterministic() {
        let t = "One two three alpha. Two three four beta. Three four five gamma. Four five six delta.";
        assert_eq!(summarize(t, opts(2, 4)), summarize(t, opts(2, 4)));
    }
}
