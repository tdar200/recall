package com.recall.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
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
import com.recall.core.FfiActionItem
import kotlinx.coroutines.launch

@Composable
fun ActionItemsScreen(repository: RecallRepository) {
    val scope = rememberCoroutineScope()
    var items by remember { mutableStateOf(listOf<FfiActionItem>()) }
    val refresh by repository.refreshCounter.collectAsState()

    LaunchedEffect(refresh) {
        items = repository.listActionItems()
    }

    if (items.isEmpty()) {
        Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text("No open action items.")
        }
        return
    }

    LazyColumn(modifier = Modifier.fillMaxSize()) {
        items(items, key = { it.id }) { item ->
            ActionItemRow(
                item = item,
                onComplete = {
                    scope.launch {
                        repository.completeActionItem(item.id)
                        items = repository.listActionItems()
                    }
                },
            )
        }
    }
}

@Composable
private fun ActionItemRow(item: FfiActionItem, onComplete: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = false, onCheckedChange = { onComplete() })
        Column(modifier = Modifier.padding(start = 8.dp)) {
            Text(item.task)
            val meta = listOfNotNull(item.owner, item.due).joinToString(" · ")
            if (meta.isNotBlank()) {
                Text(meta, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}
