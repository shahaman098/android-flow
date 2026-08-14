package com.efi.androidflow.audio

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.coroutines.coroutineContext

/**
 * Records mono PCM16 at 16 kHz and wraps it as a WAV byte array for flow-api STT.
 */
class MicRecorder {
    @Volatile
    private var recording = false

    private var audioRecord: AudioRecord? = null
    private val pcm = ByteArrayOutputStream()

    fun isRecording(): Boolean = recording

    fun start() {
        if (recording) return
        val sampleRate = 16_000
        val channel = AudioFormat.CHANNEL_IN_MONO
        val encoding = AudioFormat.ENCODING_PCM_16BIT
        val minBuf = AudioRecord.getMinBufferSize(sampleRate, channel, encoding)
        val bufferSize = minBuf.coerceAtLeast(sampleRate * 2)

        val recorder = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            sampleRate,
            channel,
            encoding,
            bufferSize,
        )
        if (recorder.state != AudioRecord.STATE_INITIALIZED) {
            recorder.release()
            throw IllegalStateException("Microphone unavailable")
        }

        pcm.reset()
        audioRecord = recorder
        recording = true
        recorder.startRecording()
    }

    suspend fun pump() = withContext(Dispatchers.IO) {
        val buf = ByteArray(4096)
        val recorder = audioRecord ?: return@withContext
        while (coroutineContext.isActive && recording) {
            val read = recorder.read(buf, 0, buf.size)
            if (read > 0) {
                synchronized(pcm) { pcm.write(buf, 0, read) }
            }
        }
    }

    fun stopToWav(): ByteArray {
        recording = false
        val recorder = audioRecord
        audioRecord = null
        try {
            recorder?.stop()
        } catch (_: IllegalStateException) {
            // already stopped
        }
        recorder?.release()
        val pcmBytes = synchronized(pcm) { pcm.toByteArray() }
        return pcmToWav(pcmBytes, sampleRate = 16_000, channels = 1, bitsPerSample = 16)
    }

    fun cancel() {
        recording = false
        val recorder = audioRecord
        audioRecord = null
        try {
            recorder?.stop()
        } catch (_: IllegalStateException) {
        }
        recorder?.release()
        synchronized(pcm) { pcm.reset() }
    }

    private fun pcmToWav(
        pcm: ByteArray,
        sampleRate: Int,
        channels: Int,
        bitsPerSample: Int,
    ): ByteArray {
        val byteRate = sampleRate * channels * bitsPerSample / 8
        val totalDataLen = pcm.size + 36
        val header = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN)
        header.put("RIFF".toByteArray(Charsets.US_ASCII))
        header.putInt(totalDataLen)
        header.put("WAVE".toByteArray(Charsets.US_ASCII))
        header.put("fmt ".toByteArray(Charsets.US_ASCII))
        header.putInt(16)
        header.putShort(1)
        header.putShort(channels.toShort())
        header.putInt(sampleRate)
        header.putInt(byteRate)
        header.putShort((channels * bitsPerSample / 8).toShort())
        header.putShort(bitsPerSample.toShort())
        header.put("data".toByteArray(Charsets.US_ASCII))
        header.putInt(pcm.size)
        return header.array() + pcm
    }
}
