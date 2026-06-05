//! Cloud summary request builder — Claude Haiku 4.5 (`claude-haiku-4-5`).
//!
//! Builds the one-tap, opt-in "deeper summary" request from a conversation's
//! TRANSCRIPT TEXT ONLY (raw audio never leaves the device), using a frozen
//! system prompt and a strict JSON schema (one_line_summary / key_points /
//! decisions / action_items / open_questions; empty arrays allowed so the model
//! never invents items). The HTTPS call itself is made by the native shell.
//!
//! Implemented in task #5 (test-first), including a test asserting that no audio
//! bytes/path can ever be serialized into the request.

use serde_json::{json, Value};

/// Frozen system prompt for converting a messy spoken-conversation transcript
/// into structured notes. Wording is load-bearing: it must forbid fabrication
/// so the model never invents action items (false memories for an ADHD user).
const SYSTEM_PROMPT: &str = "You convert a raw, possibly messy spoken-conversation transcript \
into concise structured notes. Speakers may be unlabeled and punctuation may be poor. \
Extract only what is actually present in the transcript — never invent action items, decisions, \
or facts that were not said. Return empty arrays for any category that has nothing.";

/// Which cloud model to use for a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudModel {
    /// Default one-tap summary — Claude Haiku 4.5 (cheapest capable tier).
    Standard,
    /// "Deeper analysis" upgrade for long/hard transcripts — Claude Sonnet 4.6.
    Deeper,
}

impl CloudModel {
    pub fn id(self) -> &'static str {
        match self {
            CloudModel::Standard => "claude-haiku-4-5",
            CloudModel::Deeper => "claude-sonnet-4-6",
        }
    }
}

/// The strict JSON schema the model must fill. `additionalProperties: false`
/// and every field in `required`; `owner`/`due` are nullable so they may be
/// absent without violating strict mode. Empty arrays are valid values.
fn output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "one_line_summary": { "type": "string" },
            "key_points": { "type": "array", "items": { "type": "string" } },
            "decisions": { "type": "array", "items": { "type": "string" } },
            "action_items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "task": { "type": "string" },
                        "owner": { "type": ["string", "null"] },
                        "due": { "type": ["string", "null"] }
                    },
                    "required": ["task", "owner", "due"]
                }
            },
            "open_questions": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["one_line_summary", "key_points", "decisions", "action_items", "open_questions"]
    })
}

/// Build the Anthropic Messages API request body for a one-tap cloud summary.
///
/// Sends ONLY the transcript text — the raw audio never leaves the device. The
/// native shell adds auth headers (`x-api-key`, `anthropic-version: 2023-06-01`)
/// and performs the HTTPS POST; the core never holds the API key.
pub fn build_cloud_summary_request(transcript: &str, model: CloudModel) -> Value {
    json!({
        "model": model.id(),
        "max_tokens": 2048,
        "system": SYSTEM_PROMPT,
        "messages": [
            { "role": "user", "content": transcript }
        ],
        "output_config": {
            "format": {
                "type": "json_schema",
                "schema": output_schema()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_request_targets_haiku_and_uses_json_schema_format() {
        let req = build_cloud_summary_request("we agreed to ship friday", CloudModel::Standard);
        assert_eq!(req["model"], "claude-haiku-4-5");
        // Structured outputs go through output_config.format, NOT the deprecated output_format.
        assert_eq!(req["output_config"]["format"]["type"], "json_schema");
        assert!(req.get("output_format").is_none());
        // No thinking/effort on Haiku.
        assert!(req.get("thinking").is_none());
        assert!(req["output_config"].get("effort").is_none());
    }

    #[test]
    fn deeper_request_uses_sonnet() {
        let req = build_cloud_summary_request("transcript", CloudModel::Deeper);
        assert_eq!(req["model"], "claude-sonnet-4-6");
    }

    #[test]
    fn request_carries_only_transcript_text_and_no_audio() {
        let transcript = "remind me to email the landlord about the boiler";
        let req = build_cloud_summary_request(transcript, CloudModel::Standard);

        // The transcript is the single user message.
        assert_eq!(req["messages"][0]["role"], "user");
        assert_eq!(req["messages"][0]["content"], transcript);

        // Nothing audio-related can appear in the serialized request.
        let serialized = serde_json::to_string(&req).unwrap();
        for forbidden in ["audio", ".opus", "pcm", "base64", "file_id"] {
            assert!(
                !serialized.to_lowercase().contains(forbidden),
                "request must never contain `{forbidden}`: {serialized}"
            );
        }
    }

    #[test]
    fn schema_requires_all_fields_and_forbids_extra_properties() {
        let req = build_cloud_summary_request("t", CloudModel::Standard);
        let schema = &req["output_config"]["format"]["schema"];
        assert_eq!(schema["additionalProperties"], false);
        let required = schema["required"].as_array().unwrap();
        for key in ["one_line_summary", "key_points", "decisions", "action_items", "open_questions"] {
            assert!(required.iter().any(|v| v == key), "missing required field: {key}");
        }
        // action_items.owner/due are nullable so they can be omitted without breaking strict mode.
        let owner = &schema["properties"]["action_items"]["items"]["properties"]["owner"]["type"];
        assert!(owner.as_array().unwrap().iter().any(|v| v == "null"));
    }

    #[test]
    fn system_prompt_forbids_inventing_items() {
        let req = build_cloud_summary_request("t", CloudModel::Standard);
        let system = req["system"].as_str().unwrap().to_lowercase();
        assert!(system.contains("never invent") || system.contains("only what is actually present"));
    }
}
