package app.peekaround

import android.content.ContentUris
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.ImageDecoder
import android.graphics.Matrix
import android.media.MediaMetadataRetriever
import android.media.ThumbnailUtils
import android.os.Build
import android.os.Process
import android.provider.MediaStore
import android.util.Size
import android.webkit.MimeTypeMap
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import androidx.annotation.RequiresApi
import androidx.exifinterface.media.ExifInterface
import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.security.MessageDigest
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.FutureTask
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.Semaphore
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * A tiny in-WebView "media server" for local mode, reached at http://localhost/_peek/…
 *   /_peek/thumb?p=<abs path>&s=320|640|1600[&id=<MediaStore _ID>]   cached JPEG thumbnails
 *   /_peek/file?p=<abs path>                   original file with real HTTP Range support (video seeking)
 *   /_peek/display?p=<abs path>[&max=4096]     JPEG ≤ max px for formats the WebView can't decode (HEIC…) / zoom
 *   /_peek/strip?p=<abs path>&n=20&h=90        horizontal filmstrip sprite for videos
 *
 * Only paths inside the allowed roots are served (403 otherwise): the roots JS registered with
 * PeekLocal.setAllowedRoots (library folders; until then the storage roots), the folders shared
 * with a PC (PeekLink), and the app's own cache/files dirs. Paths are canonicalised (symlinks).
 *
 * One process-wide instance (get()) is shared by the WebView and PeekLinkService so the JPEG
 * cache, per-key coalescing and decode limits are common.
 */
class PeekMediaServer private constructor(private val ctx: Context) {
    private val thumbDir = File(ctx.cacheDir, "thumbs").apply { mkdirs() }
    private val locks = ConcurrentHashMap<String, Any>()

    /** at most 4 image decodes at once, video frame extraction strictly one at a time */
    private val decodeSlots = Semaphore(4)
    private val videoSlot = Semaphore(1)

    /** WebView cache misses are generated here (LIFO: the newest request = what is on screen now) */
    private val gen = ThreadPoolExecutor(3, 3, 30, TimeUnit.SECONDS, object : LinkedBlockingDeque<Runnable>() {
        override fun offer(e: Runnable): Boolean = offerFirst(e)
    }) { r -> Thread({ Process.setThreadPriority(Process.THREAD_PRIORITY_BACKGROUND + Process.THREAD_PRIORITY_MORE_FAVORABLE); r.run() }, "peek-media").apply { isDaemon = true } }
    private val pending = ConcurrentHashMap<String, Pending>()

    /** absolute path → MediaStore _ID (-1 = known to be absent) */
    private val ids = object : LinkedHashMap<String, Long>(1024, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Long>?): Boolean = size > 60_000
    }

    @Volatile private var jsRoots: List<File>? = null
    @Volatile private var linkRoots: List<File> = emptyList()
    private val appRoots: List<File> by lazy {
        listOfNotNull(ctx.cacheDir, ctx.filesDir, ctx.externalCacheDir, ctx.getExternalFilesDir(null)).mapNotNull { canon(it) }
    }
    private val storageRoots: List<File> by lazy {
        val a = PeekFs.storageRoots(ctx)
        (0 until a.length()).mapNotNull { canon(File(a.getJSONObject(it).getString("path"))) }
    }

    companion object {
        @Volatile private var inst: PeekMediaServer? = null
        fun get(ctx: Context): PeekMediaServer = inst ?: synchronized(this) {
            inst ?: PeekMediaServer(ctx.applicationContext).also {
                inst = it
                it.setLinkRoots(LinkPrefs(ctx).folders.map { f -> f.path })
            }
        }

        private fun canon(f: File): File? = try { f.canonicalFile } catch (_: IOException) { null }
    }

    // ───────────────────────── access control ─────────────────────────

    /** JS library folders (null = not configured yet → storage roots). */
    fun setAllowedRoots(paths: List<String>?) {
        jsRoots = paths?.mapNotNull { canon(File(it)) }
    }

    fun setLinkRoots(paths: List<String>) {
        linkRoots = paths.mapNotNull { canon(File(it)) }
    }

    fun allowedRoots(): List<File> = (jsRoots ?: storageRoots) + linkRoots + appRoots

    fun isAllowed(f: File): Boolean = PathGuard.allowed(f, allowedRoots())

    // ───────────────────────── MediaStore ids ─────────────────────────

    fun rememberId(path: String, id: Long) {
        synchronized(ids) { ids[path] = id }
    }

    fun mediaId(f: File): Long? {
        synchronized(ids) { ids[f.absolutePath] }?.let { return it.takeIf { v -> v >= 0 } }
        var id = -1L
        try {
            ctx.contentResolver.query(
                MediaStore.Files.getContentUri("external"), arrayOf(MediaStore.MediaColumns._ID),
                "${MediaStore.MediaColumns.DATA} = ?", arrayOf(f.absolutePath), null,
            )?.use { c -> if (c.moveToFirst()) id = c.getLong(0) }
        } catch (_: Exception) {
        }
        rememberId(f.absolutePath, id)
        return id.takeIf { it >= 0 }
    }

    // ───────────────────────── WebView entry ─────────────────────────

    fun handle(req: WebResourceRequest): WebResourceResponse? {
        val url = req.url
        val path = url.path ?: return null
        if (!path.startsWith("/_peek/")) return null
        val p = url.getQueryParameter("p") ?: return error(400)
        val f = File(p)
        if (!isAllowed(f)) return error(403)
        if (!f.exists() || !f.isFile) return error(404)
        return try {
            when (path) {
                "/_peek/file" -> serveFile(f, req.requestHeaders)
                "/_peek/thumb" -> {
                    val s = bucket((url.getQueryParameter("s") ?: "320").toIntOrNull() ?: 320)
                    val id = url.getQueryParameter("id")?.toLongOrNull()
                    cached(key(f, s.toString())) ?: lazy(key(f, s.toString())) { thumb(f, s, id) }
                }
                "/_peek/display" -> {
                    val max = (url.getQueryParameter("max")?.toIntOrNull() ?: 4096).coerceIn(256, 8192)
                    cached(displayKey(f, max)) ?: lazy(displayKey(f, max)) { display(f, max) }
                }
                "/_peek/strip" -> {
                    if (PeekFs.kind(f.name) != "video") return error(404)
                    val n = (url.getQueryParameter("n")?.toIntOrNull() ?: 20).coerceIn(2, 60)
                    val h = (url.getQueryParameter("h")?.toIntOrNull() ?: 90).coerceIn(32, 240)
                    cached(key(f, "strip-$n-$h")) ?: lazy(key(f, "strip-$n-$h")) { strip(f, n, h) }
                }
                else -> error(404)
            }
        } catch (e: Exception) {
            error(500)
        }
    }

    private fun cached(out: File): WebResourceResponse? = if (out.exists()) serveFile(out, emptyMap(), "image/jpeg", true) else null

    /**
     * Cache miss: answer immediately with a stream that blocks until the JPEG exists. Generation
     * runs on our own pool (not the WebView's intercept thread); if the <img> goes away before
     * the work started (fast scrolling), closing the stream drops the task.
     */
    private fun lazy(out: File, work: () -> File?): WebResourceResponse {
        val k = out.path
        var pd: Pending
        while (true) {
            val cur = pending[k]
            if (cur != null) {
                if (cur.refs.incrementAndGet() > 1 && !cur.future.isCancelled) { pd = cur; break }
                cur.refs.decrementAndGet()
                pending.remove(k, cur)
                continue
            }
            val fresh = Pending()
            fresh.future = FutureTask<File?> {
                try { work() } finally { pending.remove(k, fresh) }
            }
            if (pending.putIfAbsent(k, fresh) == null) {
                gen.execute(fresh.future)
                pd = fresh
                break
            }
        }
        val owner = pd
        return WebResourceResponse("image/jpeg", null, 200, "OK", headers(mapOf("Cache-Control" to "public, max-age=31536000, immutable")), LazyStream(owner) {
            if (owner.refs.decrementAndGet() <= 0 && !owner.future.isDone) {
                owner.future.cancel(false)
                pending.remove(k, owner)
            }
        })
    }

    private class Pending {
        val refs = AtomicInteger(1)
        lateinit var future: FutureTask<File?>
    }

    private class LazyStream(private val pd: Pending, private val onClose: () -> Unit) : InputStream() {
        private var inner: InputStream? = null
        private var failed = false
        private var closed = false

        private fun open(): InputStream? {
            if (inner == null && !failed) {
                val f = try { pd.future.get() } catch (_: Exception) { null }
                if (f == null || !f.exists()) failed = true else inner = FileInputStream(f)
            }
            return inner
        }

        override fun read(): Int = open()?.read() ?: -1
        override fun read(b: ByteArray, off: Int, len: Int): Int = open()?.read(b, off, len) ?: -1

        override fun close() {
            if (closed) return
            closed = true
            inner?.close()
            onClose()
        }
    }

    private fun headers(extra: Map<String, String> = emptyMap()): MutableMap<String, String> {
        val h = mutableMapOf("Access-Control-Allow-Origin" to "*", "Accept-Ranges" to "bytes")
        h.putAll(extra)
        return h
    }

    private fun error(code: Int): WebResourceResponse =
        WebResourceResponse("text/plain", "utf-8", code, when (code) { 404 -> "Not Found"; 403 -> "Forbidden"; 400 -> "Bad Request"; else -> "Error" }, headers(), ByteArrayInputStream(ByteArray(0)))

    fun mime(f: File): String {
        val e = f.extension.lowercase(Locale.ROOT)
        return when (e) {
            "mkv" -> "video/x-matroska"
            "mov" -> "video/quicktime"
            "heic", "heif" -> "image/heif"
            "woff2" -> "font/woff2"
            "woff" -> "font/woff"
            "ttf" -> "font/ttf"
            "otf" -> "font/otf"
            else -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(e) ?: Kinds.mime(f.name)
        }
    }

    private fun serveFile(f: File, reqHeaders: Map<String, String>, type: String = mime(f), immutable: Boolean = false): WebResourceResponse {
        val len = f.length()
        val range = reqHeaders.entries.firstOrNull { it.key.equals("Range", true) }?.value
        val cache = if (immutable) "public, max-age=31536000, immutable" else "private, max-age=600"
        return when (val r = Ranges.parse(range, len)) {
            is ByteRange.Part -> {
                val input: InputStream = FileInputStream(f).also { it.skipFully(r.start) }
                WebResourceResponse(type, null, 206, "Partial Content", headers(mapOf(
                    "Content-Range" to "bytes ${r.start}-${r.end}/$len",
                    "Content-Length" to r.length.toString(),
                    "Cache-Control" to cache,
                )), BoundedInputStream(input, r.length))
            }
            is ByteRange.Unsatisfiable -> WebResourceResponse(type, null, 416, "Range Not Satisfiable", headers(mapOf("Content-Range" to "bytes */$len")), ByteArrayInputStream(ByteArray(0)))
            is ByteRange.Full -> WebResourceResponse(type, null, 200, "OK", headers(mapOf("Content-Length" to len.toString(), "Cache-Control" to cache)), FileInputStream(f))
        }
    }

    // ───────────────────────── cache ─────────────────────────

    private fun key(f: File, variant: String): File {
        val md = MessageDigest.getInstance("SHA-1").digest("${f.absolutePath}|${f.lastModified()}|${f.length()}".toByteArray())
        val hex = md.joinToString("") { "%02x".format(it) }
        return File(thumbDir, "${hex.substring(0, 2)}/$hex-$variant.jpg")
    }

    private fun displayKey(f: File, max: Int) = key(f, if (max >= 4096) "full" else "full-$max")

    private inline fun <T> once(k: String, fn: () -> T): T {
        val lock = locks.getOrPut(k) { Any() }
        try {
            return synchronized(lock) { fn() }
        } finally {
            locks.remove(k, lock)
        }
    }

    fun bucket(size: Int) = when {
        size <= 320 -> 320
        size <= 640 -> 640
        else -> 1600
    }

    /** Cached JPEG thumbnail (longest side ≈ s). `id` = MediaStore _ID when the caller knows it. */
    fun thumb(f: File, size: Int, id: Long? = null): File? {
        val s = bucket(size)
        val out = key(f, s.toString())
        if (out.exists()) return out
        return once(out.path) {
            if (out.exists()) return@once out
            val sys = if (Build.VERSION.SDK_INT >= 29 && s <= 640) systemThumb(f, s, id) else null
            val bmp = sys ?: decode(f, s) ?: return@once null
            write(bmp, out, if (s >= 1600) 88 else 80)
            bmp.recycle()
            out
        }
    }

    /** ContentResolver.loadThumbnail: the system's (usually already cached) thumbnail — much faster for videos. */
    @RequiresApi(29)
    private fun systemThumb(f: File, s: Int, id0: Long?): Bitmap? {
        val kind = PeekFs.kind(f.name)
        if (kind != "image" && kind != "video") return null
        val ext = PeekFs.ext(f.name)
        if (ext == "gif" || ext == "svg") return null
        val id = id0 ?: mediaId(f) ?: return null
        return try {
            val base = if (kind == "video") MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI
            val bmp = ctx.contentResolver.loadThumbnail(ContentUris.withAppendedId(base, id), Size(s, s), null)
            val longest = maxOf(bmp.width, bmp.height)
            when {
                longest < s / 2 -> { bmp.recycle(); null } // too small for this bucket → decode ourselves
                longest > s * 5 / 4 -> {
                    val sc = s.toFloat() / longest
                    Bitmap.createScaledBitmap(bmp, maxOf(1, (bmp.width * sc).toInt()), maxOf(1, (bmp.height * sc).toInt()), true).also { if (it != bmp) bmp.recycle() }
                }
                else -> bmp
            }
        } catch (_: Exception) {
            null
        }
    }

    fun display(f: File, max: Int = 4096): File? {
        val out = displayKey(f, max)
        if (out.exists()) return out
        return once(out.path) {
            if (out.exists()) return@once out
            val bmp = decode(f, max) ?: return@once null
            write(bmp, out, 92)
            bmp.recycle()
            out
        }
    }

    private fun write(bmp: Bitmap, out: File, q: Int) {
        out.parentFile?.mkdirs()
        val tmp = File(out.path + ".tmp")
        FileOutputStream(tmp).use { bmp.compress(Bitmap.CompressFormat.JPEG, q, it) }
        tmp.renameTo(out)
    }

    private inline fun <T> slot(sem: Semaphore, fn: () -> T): T {
        sem.acquire()
        try {
            return fn()
        } finally {
            sem.release()
        }
    }

    /** Decode an image or a video frame so that its longest side is <= max, EXIF-rotated. */
    fun decode(f: File, max: Int): Bitmap? {
        val kind = PeekFs.kind(f.name)
        if (kind == "video") {
            return slot(videoSlot) {
                if (Build.VERSION.SDK_INT >= 29 && max <= 640) {
                    try { ThumbnailUtils.createVideoThumbnail(f, Size(max, max), null) } catch (_: Exception) { videoFrame(f, max, -1) }
                } else videoFrame(f, max, -1)
            }
        }
        if (kind != "image") return null
        return slot(decodeSlots) { decodeImage(f, max) }
    }

    private fun decodeImage(f: File, max: Int): Bitmap? {
        if (Build.VERSION.SDK_INT >= 28) {
            try {
                val src = ImageDecoder.createSource(f)
                return ImageDecoder.decodeBitmap(src) { dec, info, _ ->
                    val w = info.size.width
                    val h = info.size.height
                    val scale = minOf(1.0, max.toDouble() / maxOf(w, h))
                    dec.setTargetSize(maxOf(1, (w * scale).toInt()), maxOf(1, (h * scale).toInt()))
                    dec.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
                }
            } catch (_: Exception) {
                // fall through to BitmapFactory
            }
        }
        val o = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(f.absolutePath, o)
        var sample = 1
        while (maxOf(o.outWidth, o.outHeight) / (sample * 2) >= max) sample *= 2
        val bmp = BitmapFactory.decodeFile(f.absolutePath, BitmapFactory.Options().apply { inSampleSize = sample }) ?: return null
        val rot = try { ExifInterface(f.absolutePath).rotationDegrees } catch (_: Exception) { 0 }
        val scale = minOf(1f, max.toFloat() / maxOf(bmp.width, bmp.height))
        if (rot == 0 && scale >= 1f) return bmp
        val m = Matrix().apply { postScale(scale, scale); postRotate(rot.toFloat()) }
        return Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, m, true).also { if (it != bmp) bmp.recycle() }
    }

    private fun videoFrame(f: File, max: Int, atUs: Long): Bitmap? {
        val r = MediaMetadataRetriever()
        return try {
            r.setDataSource(f.absolutePath)
            val dur = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 0
            val t = if (atUs >= 0) atUs else minOf(dur * 100, 3_000_000L)
            val bmp = if (Build.VERSION.SDK_INT >= 27) r.getScaledFrameAtTime(t, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, max, max) else r.getFrameAtTime(t)
            bmp
        } catch (_: Exception) {
            null
        } finally {
            r.release()
        }
    }

    fun strip(f: File, n0: Int, h0: Int): File? {
        if (PeekFs.kind(f.name) != "video") return null
        val n = n0.coerceIn(2, 60)
        val h = h0.coerceIn(32, 240)
        val out = key(f, "strip-$n-$h")
        if (out.exists()) return out
        return once(out.path) {
            if (out.exists()) return@once out
            slot(videoSlot) {
                val r = MediaMetadataRetriever()
                try {
                    r.setDataSource(f.absolutePath)
                    val durUs = (r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 1) * 1000
                    var sprite: Bitmap? = null
                    var canvas: Canvas? = null
                    var w = h
                    for (i in 0 until n) {
                        if (Thread.currentThread().isInterrupted) break
                        val t = durUs * (2 * i + 1) / (2 * n)
                        val fr = (if (Build.VERSION.SDK_INT >= 27) r.getScaledFrameAtTime(t, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, h * 4, h) else r.getFrameAtTime(t)) ?: continue
                        if (sprite == null) {
                            w = maxOf(1, fr.width * h / maxOf(1, fr.height))
                            sprite = Bitmap.createBitmap(w * n, h, Bitmap.Config.RGB_565)
                            canvas = Canvas(sprite)
                        }
                        val scaled = Bitmap.createScaledBitmap(fr, w, h, true)
                        canvas!!.drawBitmap(scaled, (i * w).toFloat(), 0f, null)
                        if (scaled != fr) scaled.recycle()
                        fr.recycle()
                    }
                    sprite?.let { write(it, out, 70); it.recycle(); out }
                } catch (_: Exception) {
                    null
                } finally {
                    r.release()
                }
            }
        }
    }

    fun clear() {
        thumbDir.deleteRecursively()
        thumbDir.mkdirs()
    }

    fun size(): Long = PeekFs.dirSize(thumbDir)
}

fun InputStream.skipFully(n: Long) {
    var left = n
    while (left > 0) {
        val k = skip(left)
        if (k <= 0) {
            if (read() < 0) break
            left--
        } else left -= k
    }
}

class BoundedInputStream(private val inner: InputStream, private var left: Long) : InputStream() {
    override fun read(): Int {
        if (left <= 0) return -1
        val b = inner.read()
        if (b >= 0) left--
        return b
    }

    override fun read(b: ByteArray, off: Int, len: Int): Int {
        if (left <= 0) return -1
        val n = inner.read(b, off, minOf(len.toLong(), left).toInt())
        if (n > 0) left -= n
        return n
    }

    override fun close() = inner.close()
}
