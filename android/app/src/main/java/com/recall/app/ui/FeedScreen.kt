package com.recall.app.ui

import android.content.Intent
import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.recall.app.CaptureService
import com.recall.app.ModelManager
import com.recall.app.RecallRepository
import com.recall.core.FfiConversation
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.launch

@Composable
fun FeedScreen(
    repository: RecallRepository,
    capturePermissionsGranted: Boolean,
    onOpenConversation: (Long) -> Unit,
) {
    val context = LocalContext.current
    var conversations by remember { mutableStateOf(listOf<FfiConversation>()) }
    var isCapturing by remember { mutableStateOf(false) }
    var engineReady by remember { mutableStateOf(repository.engineReady) }
    val refresh by repository.refreshCounter.collectAsState()

    LaunchedEffect(refresh) {
        conversations = repository.listConversations()
    }

    Scaffold(
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = {
                    if (!capturePermissionsGranted) {
                        Toast.makeText(context, "Grant microphone permission first", Toast.LENGTH_SHORT).show()
                        return@ExtendedFloatingActionButton
                    }
                    val next = !isCapturing
                    val intent = Intent(context, CaptureService::class.java).apply {
                        action = if (next) CaptureService.ACTION_START else CaptureService.ACTION_STOP
                    }
                    if (next) {
                        context.startForegroundService(intent)
                    } else {
                        context.startService(intent)
                    }
                    isCapturing = next
                },
                icon = { Icon(if (isCapturing) Icons.Filled.Stop else Icons.Filled.Mic, contentDescription = null) },
                text = { Text(if (isCapturing) "Stop" else "Start") },
            )
        },
    ) { padding ->
        Column(modifier = Modifier.fillMaxSize().padding(padding)) {
            if (!engineReady) {
                EngineNotReadyBanner(repository) { engineReady = repository.engineReady }
            }
            if (conversations.isEmpty()) {
                Text("No conversations yet.", modifier = Modifier.padding(16.dp))
            } else {
                LazyColumn {
                    items(conversations, key = { it.id }) { conv ->
                        ConversationRow(conv, onClick = { onOpenConversation(conv.id) })
                    }
                }
            }
        }
    }
}

@Composable
private fun EngineNotReadyBanner(repository: RecallRepository, onDownloaded: () -> Unit) {
    val scope = rememberCoroutineScope()
    var downloading by remember { mutableStateOf(false) }
    var statusText by remember { mutableStateOf<String?>(null) }

    Surface(color = MaterialTheme.colorScheme.errorContainer, modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text("Models not downloaded — capture and transcription are unavailable.")
            statusText?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            Button(
                enabled = !downloading,
                onClick = {
                    downloading = true
                    scope.launch {
                        ModelManager.download(repository.modelsDir) { progress ->
                            statusText = describeProgress(progress)
                        }
                        downloading = false
                        onDownloaded()
                    }
                },
                modifier = Modifier.padding(top = 8.dp),
            ) {
                Text(if (downloading) "Downloading…" else "Download models")
            }
        }
    }
}

private fun describeProgress(progress: ModelManager.Progress): String = when (progress) {
    is ModelManager.Progress.Downloading -> {
        val pct = if (progress.totalBytes > 0) (progress.bytesRead * 100 / progress.totalBytes) else -1
        "Downloading ${progress.label}" + if (pct >= 0) " ($pct%)" else ""
    }
    is ModelManager.Progress.Extracting -> "Extracting ${progress.label}…"
    ModelManager.Progress.Done -> "Models ready."
    is ModelManager.Progress.Failed -> "Failed: ${progress.message}"
}

@Composable
private fun ConversationRow(conv: FfiConversation, onClick: () -> Unit) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .clickable(onClick = onClick),
    ) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(formatStart(conv.startedAt), style = MaterialTheme.typography.titleMedium)
            Text("${conv.durationS}s", style = MaterialTheme.typography.bodySmall)
            val summary = conv.summaryCloud ?: conv.summaryBasic
            if (!summary.isNullOrBlank()) {
                Text(
                    summary.lineSequence().firstOrNull().orEmpty(),
                    maxLines = 2,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }
    }
}

private fun formatStart(epochSeconds: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(epochSeconds * 1000))
