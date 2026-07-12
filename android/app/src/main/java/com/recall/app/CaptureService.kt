package com.recall.app

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.runBlocking

/**
 * Foreground microphone-capture service. Started (user-initiated) from the Feed
 * screen's toggle via `startForegroundService`. Runs `AudioRecord` on a dedicated
 * thread, pushing 8000-sample (0.5 s) chunks into the repository/engine. In soak
 * mode it also logs+shows a 60 s heartbeat of VAD frame telemetry.
 */
class CaptureService : Service() {

    private val running = AtomicBoolean(false)
    private var recordThread: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null

    private val repository: RecallRepository
        get() = (application as RecallApp).repository

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            running.set(false)
            return START_NOT_STICKY
        }
        startCapture()
        return START_STICKY
    }

    override fun onDestroy() {
        running.set(false)
        super.onDestroy()
    }

    private fun startCapture() {
        if (running.getAndSet(true)) return
        startForeground(NOTIFICATION_ID, buildNotification("Recording…"), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
        acquireWakeLock()
        recordThread = Thread(::recordLoop, "RecallCapture").apply { start() }
    }

    private fun acquireWakeLock() {
        val pm = getSystemService(POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "recall:capture").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    @Suppress("MissingPermission")
    private fun recordLoop() {
        val sampleRate = 16000
        val channelConfig = AudioFormat.CHANNEL_IN_MONO
        val encoding = AudioFormat.ENCODING_PCM_16BIT
        val minBuf = AudioRecord.getMinBufferSize(sampleRate, channelConfig, encoding)
        if (minBuf <= 0) {
            Log.e(TAG, "AudioRecord.getMinBufferSize failed: $minBuf")
            running.set(false)
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return
        }

        val audioRecord = AudioRecord(MediaRecorder.AudioSource.MIC, sampleRate, channelConfig, encoding, 4 * minBuf)
        if (audioRecord.state != AudioRecord.STATE_INITIALIZED) {
            Log.e(TAG, "AudioRecord failed to initialize")
            audioRecord.release()
            running.set(false)
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return
        }

        val buffer = ShortArray(8000)
        var lastHeartbeat = System.currentTimeMillis()
        try {
            audioRecord.startRecording()
            while (running.get()) {
                val read = audioRecord.read(buffer, 0, buffer.size)
                if (read > 0) {
                    runBlocking { repository.pushAudio(buffer, read) }
                }
                if (repository.soakMode) {
                    val now = System.currentTimeMillis()
                    if (now - lastHeartbeat >= HEARTBEAT_INTERVAL_MS) {
                        lastHeartbeat = now
                        emitHeartbeat()
                    }
                }
            }
        } finally {
            audioRecord.stop()
            audioRecord.release()
            runBlocking { repository.finishCapture() }
            wakeLock?.let { if (it.isHeld) it.release() }
            wakeLock = null
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
    }

    private fun emitHeartbeat() {
        val stats = runBlocking { repository.frameStats() }
        Log.i("RecallSoak", "heartbeat total=${stats.totalFrames} speech=${stats.speechFrames}")
        updateNotification("total=${stats.totalFrames} speech=${stats.speechFrames}")
    }

    private fun buildNotification(text: String): android.app.Notification =
        NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Recall is listening")
            .setContentText(text)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setOngoing(true)
            .build()

    private fun updateNotification(text: String) {
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(NOTIFICATION_ID, buildNotification(text))
    }

    private fun createNotificationChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        val channel = NotificationChannel(CHANNEL_ID, "Recall capture", NotificationManager.IMPORTANCE_LOW)
        manager.createNotificationChannel(channel)
    }

    companion object {
        private const val TAG = "CaptureService"
        private const val CHANNEL_ID = "recall_capture"
        private const val NOTIFICATION_ID = 1
        private const val HEARTBEAT_INTERVAL_MS = 60_000L
        const val ACTION_START = "com.recall.app.action.START_CAPTURE"
        const val ACTION_STOP = "com.recall.app.action.STOP_CAPTURE"
    }
}
