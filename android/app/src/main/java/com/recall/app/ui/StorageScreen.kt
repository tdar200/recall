package com.recall.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.recall.app.RecallRepository
import com.recall.core.FfiStorageEntry
import kotlinx.coroutines.launch

@Composable
fun StorageScreen(repository: RecallRepository) {
    val scope = rememberCoroutineScope()
    var total by remember { mutableStateOf(0L) }
    var entries by remember { mutableStateOf(listOf<FfiStorageEntry>()) }
    val refresh by repository.refreshCounter.collectAsState()

    LaunchedEffect(refresh) {
        total = repository.totalAudioBytes()
        entries = repository.storageEntries()
    }

    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        Text("Total audio: ${formatBytes(total)}", style = MaterialTheme.typography.titleMedium)
        LazyColumn(modifier = Modifier.padding(top = 12.dp)) {
            items(entries, key = { it.conversationId }) { entry ->
                StorageRow(
                    entry = entry,
                    onDeleteAudio = {
                        scope.launch {
                            repository.deleteAudio(entry.conversationId)
                            total = repository.totalAudioBytes()
                            entries = repository.storageEntries()
                        }
                    },
                )
            }
        }
    }
}

@Composable
private fun StorageRow(entry: FfiStorageEntry, onDeleteAudio: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column {
            Text("Conversation #${entry.conversationId}")
            Text(
                if (entry.isAudioPruned) "audio pruned" else formatBytes(entry.audioBytes),
                style = MaterialTheme.typography.bodySmall,
            )
        }
        if (!entry.isAudioPruned) {
            OutlinedButton(onClick = onDeleteAudio) { Text("Delete audio") }
        }
    }
}

private fun formatBytes(bytes: Long): String {
    if (bytes < 1024) return "$bytes B"
    val kb = bytes / 1024.0
    if (kb < 1024) return "%.1f KB".format(kb)
    val mb = kb / 1024.0
    if (mb < 1024) return "%.1f MB".format(mb)
    return "%.1f GB".format(mb / 1024.0)
}
