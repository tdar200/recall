package com.recall.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.concurrent.thread
import kotlinx.coroutines.runBlocking

/**
 * Debug-only broadcast receiver driving headless engine tests via `adb shell am
 * broadcast`. Registered in `src/debug/AndroidManifest.xml` (exported, debug builds
 * only). `onReceive` has a ~10 s system budget so real work happens on a background
 * thread started from `goAsync()`; only a quick ack log line runs inline.
 *
 * Contract (verbatim — Task 7's e2e greps these):
 *  - `-a com.recall.DEBUG_INGEST --es path <wav>` → reads the wav, pushes it in
 *    8000-sample chunks, calls `finishCapture()`, then logs
 *    `RecallDebug: ingest done, conversations=<n>`.
 *  - `-a com.recall.DEBUG_DUMP` → logs each conversation as
 *    `RecallDebug: conv id=<id> dur=<s>s transcript=<full text>` then
 *    `RecallDebug: dump done`.
 */
class DebugReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val repository = (context.applicationContext as RecallApp).repository
        Log.i(TAG, "received ${intent.action}")
        val pendingResult = goAsync()
        when (intent.action) {
            ACTION_INGEST -> {
                val path = intent.getStringExtra(EXTRA_PATH)
                thread(name = "RecallDebugIngest") {
                    try {
                        runIngest(repository, path)
                    } catch (t: Throwable) {
                        Log.e(TAG, "ingest failed: ${t.message}", t)
                    } finally {
                        pendingResult.finish()
                    }
                }
            }
            ACTION_DUMP -> {
                thread(name = "RecallDebugDump") {
                    try {
                        runDump(repository)
                    } catch (t: Throwable) {
                        Log.e(TAG, "dump failed: ${t.message}", t)
                    } finally {
                        pendingResult.finish()
                    }
                }
            }
            else -> pendingResult.finish()
        }
    }

    private fun runIngest(repository: RecallRepository, path: String?) = runBlocking {
        if (path.isNullOrBlank()) {
            Log.e(TAG, "ingest: no path extra provided")
            return@runBlocking
        }
        val file = File(path)
        if (!file.isFile) {
            Log.e(TAG, "ingest: file not found: $path")
            return@runBlocking
        }
        val wav = try {
            parseWav(file)
        } catch (t: Throwable) {
            Log.e(TAG, "ingest: failed to parse wav $path: ${t.message}")
            return@runBlocking
        }

        val samplesPerChunk = 8000
        val bytesPerChunk = samplesPerChunk * 2
        var offset = 0
        while (offset < wav.pcm.size) {
            val end = minOf(offset + bytesPerChunk, wav.pcm.size)
            repository.pushAudioBytes(wav.pcm.copyOfRange(offset, end))
            offset = end
        }
        repository.finishCapture()

        val n = repository.listConversations().size
        Log.i(TAG, "ingest done, conversations=$n")
    }

    private fun runDump(repository: RecallRepository) = runBlocking {
        val conversations = repository.listConversations()
        for (conv in conversations) {
            val transcript = repository.transcriptText(conv.id)
            Log.i(TAG, "conv id=${conv.id} dur=${conv.durationS}s transcript=$transcript")
        }
        Log.i(TAG, "dump done")
    }

    private data class WavData(val pcm: ByteArray, val sampleRate: Int, val channels: Int, val bitsPerSample: Int)

    /** Minimal RIFF/WAVE parser: finds `fmt ` + `data` chunks, requires 16 kHz mono 16-bit PCM. */
    private fun parseWav(file: File): WavData {
        RandomAccessFile(file, "r").use { raf ->
            val header = ByteArray(12)
            raf.readFully(header)
            require(riffTag(header, 0) == "RIFF") { "not a RIFF file" }
            require(riffTag(header, 8) == "WAVE") { "not a WAVE file" }

            var sampleRate = 0
            var channels = 0
            var bitsPerSample = 0
            var pcm: ByteArray? = null

            val chunkHeader = ByteArray(8)
            while (raf.filePointer + 8 <= raf.length()) {
                val readLen = raf.read(chunkHeader)
                if (readLen < 8) break
                val chunkId = String(chunkHeader, 0, 4, Charsets.US_ASCII)
                val chunkSize = ByteBuffer.wrap(chunkHeader, 4, 4).order(ByteOrder.LITTLE_ENDIAN).int
                when (chunkId) {
                    "fmt " -> {
                        val fmt = ByteArray(chunkSize)
                        raf.readFully(fmt)
                        val buf = ByteBuffer.wrap(fmt).order(ByteOrder.LITTLE_ENDIAN)
                        buf.short // audio format tag
                        channels = buf.short.toInt()
                        sampleRate = buf.int
                        buf.int // byte rate
                        buf.short // block align
                        bitsPerSample = buf.short.toInt()
                    }
                    "data" -> {
                        val data = ByteArray(chunkSize)
                        raf.readFully(data)
                        pcm = data
                    }
                    else -> {
                        raf.seek(raf.filePointer + chunkSize)
                    }
                }
                if (chunkSize % 2 == 1 && raf.filePointer < raf.length()) {
                    raf.seek(raf.filePointer + 1) // chunks are word-aligned
                }
            }

            val samples = pcm ?: throw IllegalStateException("no data chunk found")
            if (sampleRate != 16000 || channels != 1 || bitsPerSample != 16) {
                throw IllegalStateException(
                    "unsupported wav format: sampleRate=$sampleRate channels=$channels " +
                        "bitsPerSample=$bitsPerSample (require 16000 Hz / mono / 16-bit)",
                )
            }
            return WavData(samples, sampleRate, channels, bitsPerSample)
        }
    }

    private fun riffTag(bytes: ByteArray, offset: Int): String = String(bytes, offset, 4, Charsets.US_ASCII)

    companion object {
        private const val TAG = "RecallDebug"
        const val ACTION_INGEST = "com.recall.DEBUG_INGEST"
        const val ACTION_DUMP = "com.recall.DEBUG_DUMP"
        const val EXTRA_PATH = "path"
    }
}
