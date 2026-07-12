package com.recall.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.recall.app.RecallRepository
import com.recall.core.FfiSearchHit

@Composable
fun SearchScreen(repository: RecallRepository, onOpenConversation: (Long) -> Unit) {
    var query by remember { mutableStateOf("") }
    var hits by remember { mutableStateOf(listOf<FfiSearchHit>()) }

    LaunchedEffect(query) {
        hits = if (query.isBlank()) emptyList() else repository.search(query)
    }

    Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            label = { Text("Search transcripts") },
            modifier = Modifier.fillMaxWidth(),
        )
        LazyColumn(modifier = Modifier.padding(top = 12.dp)) {
            items(hits, key = { "${it.conversationId}-${it.snippet.hashCode()}" }) { hit ->
                Card(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .clickable { onOpenConversation(hit.conversationId) },
                ) {
                    Text(hit.snippet, modifier = Modifier.padding(12.dp), style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
    }
}
