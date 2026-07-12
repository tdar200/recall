package com.recall.app

import java.io.BufferedInputStream
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream

/**
 * Downloads the on-device ONNX models the core engine expects, mirroring
 * `scripts/fetch-models.sh`'s sources: `<modelsDir>/silero_vad.onnx` (direct file)
 * and `<modelsDir>/parakeet/{encoder,decoder,joiner}.int8.onnx,tokens.txt` (extracted
 * from a `.tar.bz2` bundle).
 */
object ModelManager {

    private const val SILERO_URL =
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
    private const val PARAKEET_URL =
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/" +
            "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2"

    private val PARAKEET_FILES = listOf(
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    )

    sealed class Progress {
        data class Downloading(val label: String, val bytesRead: Long, val totalBytes: Long) : Progress()
        data class Extracting(val label: String) : Progress()
        data object Done : Progress()
        data class Failed(val message: String) : Progress()
    }

    fun modelsPresent(dir: File): Boolean {
        val silero = File(dir, "silero_vad.onnx")
        val parakeetDir = File(dir, "parakeet")
        return silero.isFile && PARAKEET_FILES.all { File(parakeetDir, it).isFile }
    }

    suspend fun download(dir: File, onProgress: (Progress) -> Unit) = withContext(Dispatchers.IO) {
        try {
            dir.mkdirs()

            val silero = File(dir, "silero_vad.onnx")
            if (!silero.isFile) {
                downloadFile(SILERO_URL, silero) { read, total ->
                    onProgress(Progress.Downloading("silero_vad.onnx", read, total))
                }
            }

            val parakeetDir = File(dir, "parakeet")
            if (!PARAKEET_FILES.all { File(parakeetDir, it).isFile }) {
                val archive = File(dir, "parakeet.tar.bz2")
                downloadFile(PARAKEET_URL, archive) { read, total ->
                    onProgress(Progress.Downloading("parakeet bundle", read, total))
                }
                onProgress(Progress.Extracting("parakeet"))
                extractParakeet(archive, dir, parakeetDir)
                archive.delete()
            }

            onProgress(Progress.Done)
        } catch (t: Throwable) {
            onProgress(Progress.Failed(t.message ?: t.toString()))
        }
    }

    private fun downloadFile(url: String, dest: File, onProgress: (Long, Long) -> Unit) {
        val tmp = File(dest.parentFile, dest.name + ".tmp")
        val connection = URL(url).openConnection() as HttpURLConnection
        connection.instanceFollowRedirects = true
        try {
            connection.connect()
            val total = connection.contentLengthLong
            connection.inputStream.use { input ->
                FileOutputStream(tmp).use { output ->
                    val buf = ByteArray(64 * 1024)
                    var readTotal = 0L
                    while (true) {
                        val n = input.read(buf)
                        if (n <= 0) break
                        output.write(buf, 0, n)
                        readTotal += n
                        onProgress(readTotal, total)
                    }
                }
            }
        } finally {
            connection.disconnect()
        }
        if (!tmp.renameTo(dest)) {
            throw java.io.IOException("failed to move ${tmp.name} into place")
        }
    }

    /**
     * The archive extracts to a single top-level directory (e.g.
     * `sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/`); move its contents to the
     * stable `parakeet/` path the engine and Task 5/7 code depend on.
     */
    private fun extractParakeet(archive: File, dir: File, parakeetDir: File) {
        val extractRoot = File(dir, "parakeet_extract_tmp")
        extractRoot.deleteRecursively()
        extractRoot.mkdirs()

        BZip2CompressorInputStream(BufferedInputStream(FileInputStream(archive))).use { bz ->
            TarArchiveInputStream(bz).use { tar ->
                var entry = tar.nextEntry
                while (entry != null) {
                    val outFile = File(extractRoot, entry.name)
                    if (entry.isDirectory) {
                        outFile.mkdirs()
                    } else {
                        outFile.parentFile?.mkdirs()
                        FileOutputStream(outFile).use { out -> tar.copyTo(out) }
                    }
                    entry = tar.nextEntry
                }
            }
        }

        val topLevelDirs = extractRoot.listFiles { f -> f.isDirectory } ?: emptyArray()
        val source = topLevelDirs.firstOrNull() ?: extractRoot
        parakeetDir.deleteRecursively()
        if (!source.renameTo(parakeetDir)) {
            source.copyRecursively(parakeetDir, overwrite = true)
        }
        extractRoot.deleteRecursively()
    }
}
