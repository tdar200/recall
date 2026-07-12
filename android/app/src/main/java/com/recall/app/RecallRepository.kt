package com.recall.app

import android.content.Context
import android.content.SharedPreferences
import android.util.Log
import com.recall.core.CoreException
import com.recall.core.FfiActionItem
import com.recall.core.FfiConversation
import com.recall.core.FfiFrameStats
import com.recall.core.FfiSearchHit
import com.recall.core.FfiStorageEntry
import com.recall.core.RecallEngine
import com.recall.core.RecallListener
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext

/**
 * Single owner of the native [RecallEngine]. The engine is constructed lazily and
 * guardedly: it will throw until real ONNX models exist on device (and until the
 * native `.so` is packaged), so every access goes through [engineOrNull] and every
 * public method swallows [CoreException] rather than letting it escape to the UI.
 */
class RecallRepository(context: Context) {

    private val appContext = context.applicationContext

    private val prefs: SharedPreferences =
        appContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    /** `<external-files>/models` — falls back to internal storage if unavailable. */
    val modelsDir: File =
        appContext.getExternalFilesDir("models") ?: File(appContext.filesDir, "models")

    private val dbPath: String = File(appContext.filesDir, "recall.db").absolutePath
    private val audioDir: File = File(appContext.filesDir, "audio").apply { mkdirs() }

    private val _refreshCounter = MutableStateFlow(0L)

    /** Bumped whenever the engine reports a conversation/transcript/summary event. */
    val refreshCounter: StateFlow<Long> = _refreshCounter.asStateFlow()

    /** Whether the on-device models required to construct the engine are present. */
    val engineReady: Boolean
        get() = ModelManager.modelsPresent(modelsDir)

    var soakMode: Boolean
        get() = prefs.getBoolean(KEY_SOAK_MODE, false)
        set(value) = prefs.edit().putBoolean(KEY_SOAK_MODE, value).apply()

    var anthropicApiKey: String?
        get() = prefs.getString(KEY_API_KEY, null)?.takeIf { it.isNotBlank() }
        set(value) = prefs.edit().putString(KEY_API_KEY, value).apply()

    private val listener = object : RecallListener {
        override fun onConversationFinalized(conversationId: Long) = bump()
        override fun onTranscriptReady(conversationId: Long) = bump()
        override fun onSummaryReady(conversationId: Long) = bump()
    }

    private fun bump() {
        _refreshCounter.value = _refreshCounter.value + 1
    }

    private var engine: RecallEngine? = null

    /**
     * Lazily constructs the engine. Any construction failure (missing models, missing
     * native library, corrupt db, ...) is caught and logged; callers get `null` back
     * instead of a crash and retry on the next call (e.g. once models finish downloading).
     */
    @Synchronized
    private fun engineOrNull(): RecallEngine? {
        engine?.let { return it }
        if (!engineReady) return null
        return try {
            val e = RecallEngine(
                dbPath = dbPath,
                audioDir = audioDir.absolutePath,
                modelsDir = modelsDir.absolutePath,
                sessionStartUnix = System.currentTimeMillis() / 1000,
                soakMode = soakMode,
                useOpus = true,
                listener = listener,
            )
            engine = e
            e
        } catch (t: Throwable) {
            Log.w(TAG, "engine construction failed: ${t.message}")
            null
        }
    }

    suspend fun pushAudio(pcm: ShortArray, len: Int) {
        val bytes = ByteArray(len * 2)
        for (i in 0 until len) {
            val v = pcm[i].toInt()
            bytes[i * 2] = (v and 0xFF).toByte()
            bytes[i * 2 + 1] = ((v shr 8) and 0xFF).toByte()
        }
        pushAudioBytes(bytes)
    }

    /** Push already-LE-encoded 16-bit PCM bytes (used by [DebugReceiver] ingesting a wav). */
    suspend fun pushAudioBytes(bytes: ByteArray) = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.pushAudio(bytes)
        } catch (t: CoreException) {
            Log.w(TAG, "pushAudio failed: ${t.message}")
        }
    }

    suspend fun finishCapture() = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.finishCapture()
        } catch (t: CoreException) {
            Log.w(TAG, "finishCapture failed: ${t.message}")
        }
    }

    suspend fun listConversations(): List<FfiConversation> = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.listConversations() ?: emptyList()
        } catch (t: CoreException) {
            Log.w(TAG, "listConversations failed: ${t.message}")
            emptyList()
        }
    }

    suspend fun getConversation(id: Long): FfiConversation? = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.getConversation(id)
        } catch (t: CoreException) {
            Log.w(TAG, "getConversation failed: ${t.message}")
            null
        }
    }

    suspend fun transcriptText(id: Long): String = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.transcriptText(id) ?: ""
        } catch (t: CoreException) {
            Log.w(TAG, "transcriptText failed: ${t.message}")
            ""
        }
    }

    suspend fun search(q: String): List<FfiSearchHit> = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.search(q) ?: emptyList()
        } catch (t: CoreException) {
            Log.w(TAG, "search failed: ${t.message}")
            emptyList()
        }
    }

    suspend fun listActionItems(): List<FfiActionItem> = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.listActionItems() ?: emptyList()
        } catch (t: CoreException) {
            Log.w(TAG, "listActionItems failed: ${t.message}")
            emptyList()
        }
    }

    suspend fun completeActionItem(id: Long) = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.completeActionItem(id)
            bump()
        } catch (t: CoreException) {
            Log.w(TAG, "completeActionItem failed: ${t.message}")
        }
    }

    suspend fun storageEntries(): List<FfiStorageEntry> = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.storageEntries() ?: emptyList()
        } catch (t: CoreException) {
            Log.w(TAG, "storageEntries failed: ${t.message}")
            emptyList()
        }
    }

    suspend fun totalAudioBytes(): Long = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.totalAudioBytes() ?: 0L
        } catch (t: CoreException) {
            Log.w(TAG, "totalAudioBytes failed: ${t.message}")
            0L
        }
    }

    suspend fun deleteAudio(id: Long) = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.deleteAudio(id)
            bump()
        } catch (t: CoreException) {
            Log.w(TAG, "deleteAudio failed: ${t.message}")
        }
    }

    suspend fun deleteConversation(id: Long) = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.deleteConversation(id)
            bump()
        } catch (t: CoreException) {
            Log.w(TAG, "deleteConversation failed: ${t.message}")
        }
    }

    /** Returns the Anthropic request JSON to POST verbatim, or null if the engine isn't ready. */
    suspend fun requestCloudSummary(id: Long, deeper: Boolean): String? = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.requestCloudSummary(id, deeper)
        } catch (t: CoreException) {
            Log.w(TAG, "requestCloudSummary failed: ${t.message}")
            null
        }
    }

    suspend fun storeCloudSummary(id: Long, json: String) = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.storeCloudSummary(id, json)
            bump()
        } catch (t: CoreException) {
            Log.w(TAG, "storeCloudSummary failed: ${t.message}")
        }
    }

    suspend fun frameStats(): FfiFrameStats = withContext(Dispatchers.IO) {
        try {
            engineOrNull()?.frameStats() ?: FfiFrameStats(0uL, 0uL)
        } catch (t: CoreException) {
            Log.w(TAG, "frameStats failed: ${t.message}")
            FfiFrameStats(0uL, 0uL)
        }
    }

    companion object {
        private const val TAG = "RecallRepository"
        private const val PREFS_NAME = "recall_prefs"
        private const val KEY_SOAK_MODE = "soak_mode"
        private const val KEY_API_KEY = "anthropic_api_key"
    }
}
