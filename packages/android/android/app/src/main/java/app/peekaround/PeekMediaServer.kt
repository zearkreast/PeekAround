package app.peekaround

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.ImageDecoder
import android.graphics.Matrix
import android.media.MediaMetadataRetriever
import android.media.ThumbnailUtils
import android.os.Build
import android.util.Size
import android.webkit.MimeTypeMap
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import androidx.exifinterface.media.ExifInterface
import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream
import java.security.MessageDigest
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap

/**
 * A tiny in-WebView "media server" for local mode, reached at http://localhost/_peek/…
 *   /_peek/thumb?p=<abs path>&s=320|640|1600   cached JPEG thumbnails (images + video frames)
 *   /_peek/file?p=<abs path>                   original file with real HTTP Range support (video seeking)
 *   /_peek/display?p=<abs path>                full-size JPEG for formats the WebView can't decode (HEIC…)
 *   /_peek/strip?p=<abs path>&n=20&h=90        horizontal filmstrip sprite for videos
 */
class PeekMediaServer(private val ctx: Context) {
    private val thumbDir = File(ctx.cacheDir, "thumbs").apply { mkdirs() }
    private val locks = ConcurrentHashMap<String, Any>()

    fun handle(req: WebResourceRequest): WebResourceResponse? {
        val url = req.url
        val path = url.path ?: return null
        if (!path.startsWith("/_peek/")) return null
        val p = url.getQueryParameter("p") ?: return error(400)
        val f = File(p)
        if (!f.exists() || !f.isFile) return error(404)
        return try {
            when (path) {
                "/_peek/file" -> serveFile(f, req.requestHeaders)
                "/_peek/thumb" -> thumb(f, (url.getQueryParameter("s") ?: "320").toIntOrNull() ?: 320)?.let { serveFile(it, emptyMap(), "image/jpeg", true) } ?: error(404)
                "/_peek/display" -> display(f)?.let { serveFile(it, emptyMap(), "image/jpeg", true) } ?: error(415)
                "/_peek/strip" -> strip(f, url.getQueryParameter("n")?.toIntOrNull() ?: 20, url.getQueryParameter("h")?.toIntOrNull() ?: 90)?.let { serveFile(it, emptyMap(), "image/jpeg", true) } ?: error(404)
                else -> error(404)
            }
        } catch (e: Exception) {
            error(500)
        }
    }

    private fun headers(extra: Map<String, String> = emptyMap()): MutableMap<String, String> {
        val h = mutableMapOf("Access-Control-Allow-Origin" to "*", "Accept-Ranges" to "bytes")
        h.putAll(extra)
        return h
    }

    private fun error(code: Int): WebResourceResponse =
        WebResourceResponse("text/plain", "utf-8", code, if (code == 404) "Not Found" else "Error", headers(), ByteArrayInputStream(ByteArray(0)))

    private fun mime(f: File): String {
        val e = f.extension.lowercase(Locale.ROOT)
        return when (e) {
            "mkv" -> "video/x-matroska"
            "mov" -> "video/quicktime"
            "heic", "heif" -> "image/heif"
            "woff2" -> "font/woff2"
            "woff" -> "font/woff"
            "ttf" -> "font/ttf"
            "otf" -> "font/otf"
            else -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(e) ?: "application/octet-stream"
        }
    }

    private fun serveFile(f: File, reqHeaders: Map<String, String>, type: String = mime(f), immutable: Boolean = false): WebResourceResponse {
        val len = f.length()
        val range = reqHeaders.entries.firstOrNull { it.key.equals("Range", true) }?.value
        val cache = if (immutable) "public, max-age=31536000, immutable" else "private, max-age=600"
        if (range != null && range.startsWith("bytes=")) {
            val spec = range.removePrefix("bytes=").split(",")[0].trim()
            val parts = spec.split("-")
            var start = parts.getOrNull(0)?.toLongOrNull()
            var end = parts.getOrNull(1)?.toLongOrNull()
            if (start == null && end != null) { start = len - end; end = len - 1 } // suffix range
            val s = (start ?: 0L).coerceIn(0, maxOf(0, len - 1))
            val e = (end ?: (len - 1)).coerceIn(s, len - 1)
            val input: InputStream = FileInputStream(f).also { it.skipFully(s) }
            val bounded = BoundedInputStream(input, e - s + 1)
            return WebResourceResponse(type, null, 206, "Partial Content", headers(mapOf(
                "Content-Range" to "bytes $s-$e/$len",
                "Content-Length" to (e - s + 1).toString(),
                "Cache-Control" to cache,
            )), bounded)
        }
        return WebResourceResponse(type, null, 200, "OK", headers(mapOf("Content-Length" to len.toString(), "Cache-Control" to cache)), FileInputStream(f))
    }

    private fun InputStream.skipFully(n: Long) {
        var left = n
        while (left > 0) {
            val k = skip(left)
            if (k <= 0) break
            left -= k
        }
    }

    private class BoundedInputStream(private val inner: InputStream, private var left: Long) : InputStream() {
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

    private fun key(f: File, variant: String): File {
        val md = MessageDigest.getInstance("SHA-1").digest("${f.absolutePath}|${f.lastModified()}|${f.length()}".toByteArray())
        val hex = md.joinToString("") { "%02x".format(it) }
        return File(thumbDir, "${hex.substring(0, 2)}/$hex-$variant.jpg")
    }

    private inline fun <T> once(k: String, fn: () -> T): T = synchronized(locks.getOrPut(k) { Any() }) { fn() }

    fun thumb(f: File, size: Int): File? {
        val s = when {
            size <= 320 -> 320
            size <= 640 -> 640
            else -> 1600
        }
        val out = key(f, s.toString())
        if (out.exists()) return out
        return once(out.path) {
            if (out.exists()) return@once out
            val bmp = decode(f, s) ?: return@once null
            write(bmp, out, if (s >= 1600) 88 else 80)
            bmp.recycle()
            out
        }
    }

    fun display(f: File): File? {
        val out = key(f, "full")
        if (out.exists()) return out
        return once(out.path) {
            if (out.exists()) return@once out
            val bmp = decode(f, 4096) ?: return@once null
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

    /** Decode an image or a video frame so that its longest side is <= max, EXIF-rotated. */
    fun decode(f: File, max: Int): Bitmap? {
        val kind = PeekFs.kind(f.name)
        if (kind == "video") {
            if (Build.VERSION.SDK_INT >= 29 && max <= 640) {
                return try { ThumbnailUtils.createVideoThumbnail(f, Size(max, max), null) } catch (_: Exception) { videoFrame(f, max, -1) }
            }
            return videoFrame(f, max, -1)
        }
        if (kind != "image") return null
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
            val r = MediaMetadataRetriever()
            try {
                r.setDataSource(f.absolutePath)
                val durUs = (r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 1) * 1000
                var sprite: Bitmap? = null
                var canvas: Canvas? = null
                var w = h
                for (i in 0 until n) {
                    val t = durUs * (2 * i + 1) / (2 * n)
                    val fr = (if (Build.VERSION.SDK_INT >= 27) r.getScaledFrameAtTime(t, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, h * 4, h) else r.getFrameAtTime(t)) ?: continue
                    if (sprite == null) {
                        w = maxOf(1, fr.width * h / maxOf(1, fr.height))
                        sprite = Bitmap.createBitmap(w * n, h, Bitmap.Config.RGB_565)
                        canvas = Canvas(sprite)
                    }
                    canvas!!.drawBitmap(Bitmap.createScaledBitmap(fr, w, h, true), (i * w).toFloat(), 0f, null)
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

    fun clear() {
        thumbDir.deleteRecursively()
        thumbDir.mkdirs()
    }

    fun size(): Long = PeekFs.dirSize(thumbDir)
}
