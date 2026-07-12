package com.recall.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.recall.app.RecallRepository
import com.recall.core.FfiConversation
import java.io.BufferedReader
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DetailScreen(repository: RecallRepository, conversationId: Long, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    var conversation by remember { mutableStateOf<FfiConversation?>(null) }
    var transcript by remember { mutableStateOf("") }
    var cloudBusy by remember { mutableStateOf(false) }
    var cloudError by remember { mutableStateOf<String?>(null) }
    val refresh by repository.refreshCounter.collectAsState()
    val apiKey = repository.anthropicApiKey

    LaunchedEffect(conversationId, refresh) {
        conversation = repository.getConversation(conversationId)
        transcript = repository.transcriptText(conversationId)
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Conversation") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
    ) { padding ->
        val conv = conversation
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(16.dp)
                .verticalScroll(rememberScrollState()),
        ) {
            if (conv == null) {
                Text("Loading…")
                return@Scaffold
            }

            SummaryCard(conv)

            Text(
                "Transcript",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(top = 16.dp),
            )
            Text(transcript.ifBlank { "(no transcript yet)" }, modifier = Modifier.padding(top = 8.dp))

            Row(
                modifier = Modifier.fillMaxWidth().padding(top = 24.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                OutlinedButton(onClick = { scope.launch { repository.deleteAudio(conversationId) } }) {
                    Text("Delete audio")
                }
                OutlinedButton(onClick = {
                    scope.launch {
                        repository.deleteConversation(conversationId)
                        onBack()
                    }
                }) {
                    Text("Delete conversation")
                }
            }

            Button(
                enabled = !apiKey.isNullOrBlank() && !cloudBusy,
                onClick = {
                    cloudBusy = true
                    cloudError = null
                    scope.launch {
                        try {
                            postCloudSummary(repository, conversationId, apiKey!!)
                        } catch (t: Exception) {
                            cloudError = t.message ?: t.toString()
                        }
                        cloudBusy = false
                    }
                },
                modifier = Modifier.padding(top = 16.dp),
            ) {
                Text(if (cloudBusy) "Summarizing…" else "Cloud summary")
            }
            if (apiKey.isNullOrBlank()) {
                Text(
                    "Set an Anthropic API key on the Soak screen to enable cloud summaries.",
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
            cloudError?.let {
                Text(
                    "Cloud summary failed: $it",
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }
    }
}

@Composable
private fun SummaryCard(conv: FfiConversation) {
    val cloud = conv.summaryCloud
    // summaryCloud is a JSON document (core/src/cloud.rs's output_schema); parse it
    // and show one_line_summary + key_points rather than the raw JSON, falling back
    // to the raw string if it doesn't parse.
    val parsedCloud = cloud?.let { parseCloudSummary(it) }
    Card(
        colors = if (cloud != null) {
            CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.primaryContainer)
        } else {
            CardDefaults.cardColors()
        },
    ) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(if (cloud != null) "Cloud summary" else "Summary", style = MaterialTheme.typography.labelLarge)
            when {
                parsedCloud != null -> {
                    Text(parsedCloud.oneLineSummary, modifier = Modifier.padding(top = 4.dp))
                    parsedCloud.keyPoints.forEach { point ->
                        Text("• $point", modifier = Modifier.padding(top = 2.dp))
                    }
                }
                cloud != null -> Text(cloud, modifier = Modifier.padding(top = 4.dp))
                else -> Text(conv.summaryBasic ?: "(no summary yet)", modifier = Modifier.padding(top = 4.dp))
            }
        }
    }
}

/**
 * POSTs the engine's `requestCloudSummary` JSON verbatim to the Anthropic Messages
 * API, extracts `content[0].text`, and stores it back via `storeCloudSummary`.
 */
private suspend fun postCloudSummary(repository: RecallRepository, id: Long, apiKey: String) {
    val requestJson = repository.requestCloudSummary(id, deeper = false)
        ?: throw IllegalStateException("engine not ready")

    val responseBody = withContext(Dispatchers.IO) {
        val connection = URL("https://api.anthropic.com/v1/messages").openConnection() as HttpURLConnection
        try {
            connection.requestMethod = "POST"
            connection.doOutput = true
            connection.setRequestProperty("x-api-key", apiKey)
            connection.setRequestProperty("anthropic-version", "2023-06-01")
            connection.setRequestProperty("content-type", "application/json")
            connection.outputStream.use { it.write(requestJson.toByteArray(Charsets.UTF_8)) }

            val code = connection.responseCode
            val stream = if (code in 200..299) connection.inputStream else connection.errorStream
            val body = stream?.bufferedReader()?.use(BufferedReader::readText).orEmpty()
            if (code !in 200..299) {
                throw IllegalStateException("HTTP $code: $body")
            }
            body
        } finally {
            connection.disconnect()
        }
    }

    val text = JSONObject(responseBody).getJSONArray("content").getJSONObject(0).getString("text")
    // storeCloudSummary swallows and logs repository/engine-level failures internally
    // (so a native panic can't crash the UI) — check its return so the caller's
    // catch block surfaces the failure as cloudError instead of silently no-oping.
    val stored = repository.storeCloudSummary(id, text)
    if (!stored) {
        throw IllegalStateException("failed to save cloud summary")
    }
}
