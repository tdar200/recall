package com.recall.app.ui

import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.recall.app.RecallRepository
import com.recall.core.FfiFrameStats
import kotlinx.coroutines.delay

/**
 * Soak-mode settings + live telemetry. Also hosts the Anthropic API key field (no
 * other screen owns settings) and the Samsung "don't kill my app" checklist from
 * `docs/M0-runbook.md` as static reference text.
 */
@Composable
fun SoakScreen(repository: RecallRepository) {
    val context = LocalContext.current
    var soakEnabled by remember { mutableStateOf(repository.soakMode) }
    var apiKey by remember { mutableStateOf(repository.anthropicApiKey.orEmpty()) }
    var stats by remember { mutableStateOf(FfiFrameStats(0uL, 0uL)) }

    LaunchedEffect(Unit) {
        while (true) {
            stats = repository.frameStats()
            delay(5_000)
        }
    }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(16.dp)
            .verticalScroll(rememberScrollState()),
    ) {
        Text("Soak mode", style = MaterialTheme.typography.titleLarge)
        Row(
            modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text(
                "Skip ASR/storage, telemetry only (takes effect on next capture start)",
                modifier = Modifier.padding(end = 8.dp),
            )
            Switch(
                checked = soakEnabled,
                onCheckedChange = {
                    soakEnabled = it
                    repository.soakMode = it
                },
            )
        }
        Text(
            "Frame stats: total=${stats.totalFrames} speech=${stats.speechFrames}",
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.padding(top = 16.dp),
        )

        HorizontalDivider(modifier = Modifier.padding(vertical = 16.dp))

        Text("Anthropic API key", style = MaterialTheme.typography.titleMedium)
        OutlinedTextField(
            value = apiKey,
            onValueChange = {
                apiKey = it
                repository.anthropicApiKey = it
            },
            label = { Text("x-api-key") },
            modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
        )

        HorizontalDivider(modifier = Modifier.padding(vertical = 16.dp))

        Text("Samsung don't-kill-my-app checklist", style = MaterialTheme.typography.titleMedium)
        Text(SAMSUNG_CHECKLIST, modifier = Modifier.padding(top = 8.dp))

        Row(
            modifier = Modifier.fillMaxWidth().padding(top = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Button(onClick = {
                context.startActivity(
                    Intent(
                        Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                        Uri.fromParts("package", context.packageName, null),
                    ),
                )
            }) { Text("App settings") }
            Button(onClick = {
                context.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
            }) { Text("Battery settings") }
        }
    }
}

private const val SAMSUNG_CHECKLIST = """1. Settings -> Apps -> Recall -> Battery -> Unrestricted.
2. Settings -> Battery -> Background usage limits -> Never sleeping apps -> add Recall.
3. Settings -> Battery -> Background usage limits -> remove Recall from Sleeping / Deep sleeping apps.
4. Settings -> Device care -> Battery -> turn off "Put unused apps to sleep" (or exempt Recall).
5. In Recents, open Recall's card menu -> Lock this app.
6. Grant RECORD_AUDIO; accept the persistent notification.

Re-check items 1-4 after any system/OTA update — Samsung silently resets the battery exemption."""
