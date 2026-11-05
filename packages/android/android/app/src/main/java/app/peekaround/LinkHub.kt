package app.peekaround

import okhttp3.Dispatcher
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.OkHttpClient
import okhttp3.RequestBody
import okio.BufferedSink
import java.io.File
import java.io.InputStream
import java.io.RandomAccessFile
import java.nio.channels.Channels
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit

/**
 * Process-wide link status, shared by PeekLinkService (writer) and the PeekLink plugin / the
 * notification (readers). Survives the WebView: the service runs without any activity.
 */
object LinkHub {
    /** 'off' | 'connecting' | 'pending' | 'online' | 'rejected' | 'error' */
    @Volatile var state: String = "off"
        private set
    @Volatile var code: String? = null
        private set
    @Volatile var error: String? = null
        private set
    @Volatile var active: Int = 0
        private set

    private val listeners = CopyOnWriteArrayList<() -> Unit>()

    fun addListener(l: () -> Unit) = listeners.add(l)
    fun removeListener(l: () -> Unit) = listeners.remove(l)

    fun set(state: String, code: String? = null, error: String? = null) {
        if (this.state == state && this.code == code && this.error == error) return
        this.state = state
        this.code = code
        this.error = error
        fire()
    }

    fun setActive(n: Int) {
        active = n
    }

    fun fire() = listeners.forEach { runCatching { it() } }
}

/** Shared OkHttp clients: the control WebSocket and the data plane never share a queue. */
object PeekHttp {
    /** WebSocket: no read timeout (heartbeat is ours), app-level ping/pong every 30 s */
    val ws: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .writeTimeout(15, TimeUnit.SECONDS)
            .retryOnConnectionFailure(false)
            .build()
    }

    /** reply PUTs, pulls, uploads, downloads: up to 16 in parallel per host */
    val data: OkHttpClient by lazy {
        val d = Dispatcher().apply {
            maxRequests = 32
            maxRequestsPerHost = 16
        }
        OkHttpClient.Builder()
            .dispatcher(d)
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .writeTimeout(60, TimeUnit.SECONDS)
            .retryOnConnectionFailure(false)
            .build()
    }
}

/** Streams [length] bytes of a file from [offset] (Content-Length known → never chunked). */
class FileBody(
    private val file: File,
    private val offset: Long,
    private val length: Long,
    private val type: String?,
    private val onBytes: ((Long) -> Unit)? = null,
    private val cancelled: () -> Boolean = { false },
) : RequestBody() {
    override fun contentType(): MediaType? = type?.toMediaTypeOrNull()
    override fun contentLength(): Long = length
    override fun isOneShot(): Boolean = true

    override fun writeTo(sink: BufferedSink) {
        if (length == 0L) return
        RandomAccessFile(file, "r").use { raf ->
            raf.seek(offset)
            val input = BoundedInputStream(Channels.newInputStream(raf.channel), length)
            copy(input, sink, onBytes, cancelled)
        }
    }

    companion object {
        fun copy(input: InputStream, sink: BufferedSink, onBytes: ((Long) -> Unit)?, cancelled: () -> Boolean): Long {
            val buf = ByteArray(128 * 1024)
            var total = 0L
            while (true) {
                if (cancelled()) throw java.io.IOException("Canceled")
                val n = input.read(buf)
                if (n < 0) break
                sink.write(buf, 0, n)
                total += n
                onBytes?.invoke(total)
            }
            return total
        }
    }
}

/** Unknown length (chunked): the body is produced while it is sent (NDJSON listings). */
class StreamBody(private val type: String?, private val writer: (BufferedSink) -> Unit) : RequestBody() {
    override fun contentType(): MediaType? = type?.toMediaTypeOrNull()
    override fun contentLength(): Long = -1
    override fun isOneShot(): Boolean = true
    override fun writeTo(sink: BufferedSink) = writer(sink)
}

/** Known-length body read from a stream (content:// uploads with an exact size). */
class InputBody(private val open: () -> InputStream, private val length: Long, private val type: String?, private val onBytes: ((Long) -> Unit)?, private val cancelled: () -> Boolean) : RequestBody() {
    override fun contentType(): MediaType? = type?.toMediaTypeOrNull()
    override fun contentLength(): Long = length
    override fun isOneShot(): Boolean = true
    override fun writeTo(sink: BufferedSink) {
        open().use { input ->
            if (length >= 0) FileBody.copy(BoundedInputStream(input, length), sink, onBytes, cancelled)
            else FileBody.copy(input, sink, onBytes, cancelled)
        }
    }
}
