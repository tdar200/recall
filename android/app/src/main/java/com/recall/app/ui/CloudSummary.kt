package com.recall.app.ui

import org.json.JSONException
import org.json.JSONObject

/**
 * Parsed form of the JSON document persisted in `FfiConversation.summaryCloud`
 * (see `core/src/cloud.rs`'s `output_schema`: `one_line_summary` / `key_points` /
 * `decisions` / `action_items` / `open_questions`). Screens render these fields
 * instead of the raw JSON blob.
 */
data class CloudSummary(
    val oneLineSummary: String,
    val keyPoints: List<String>,
)

/**
 * Parses [raw] as the cloud-summary JSON document. Returns `null` if [raw] isn't
 * valid JSON or doesn't carry a non-blank `one_line_summary`, so callers can fall
 * back to displaying the raw string verbatim.
 */
fun parseCloudSummary(raw: String): CloudSummary? {
    val obj = try {
        JSONObject(raw)
    } catch (e: JSONException) {
        return null
    }
    val oneLine = obj.optString("one_line_summary", "")
    if (oneLine.isBlank()) return null
    val pointsArray = obj.optJSONArray("key_points")
    val keyPoints = if (pointsArray != null) {
        (0 until pointsArray.length()).map { pointsArray.optString(it) }.filter { it.isNotBlank() }
    } else {
        emptyList()
    }
    return CloudSummary(oneLineSummary = oneLine, keyPoints = keyPoints)
}
