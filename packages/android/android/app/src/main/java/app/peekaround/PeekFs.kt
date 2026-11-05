package app.peekaround

import android.content.Context
import android.media.MediaMetadataRetriever
import android.media.MediaScannerConnection
import android.os.Environment
import android.provider.MediaStore
import androidx.exifinterface.media.ExifInterface
import com.getcapacitor.JSArray
import com.getcapacitor.JSObject
import java.io.File
import java.io.FileOutputStream
import java.nio.charset.Charset
import java.text.SimpleDateFormat
import java.util.Locale
import java.util.TimeZone
import java.util.zip.ZipFile

/** File-system + metadata helpers for the local (on-device) gallery. */
object PeekFs {
    val IMAGE = setOf("jpg", "jpeg", "png", "gif", "webp", "avif", "bmp", "heic", "heif", "jfif")
    val VIDEO = setOf("mp4", "m4v", "mov", "webm", "mkv", "3gp", "ts", "avi")
    val AUDIO = setOf("mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac")
    const val TRASH = ".peekaround-trash"

    fun ext(name: String): String = name.substringAfterLast('.', "").lowercase(Locale.ROOT)

    fun kind(name: String): String? {
        val e = ext(name)
        return when {
            e in IMAGE -> "image"
            e in VIDEO -> "video"
            e in AUDIO -> "audio"
            e == "zip" -> "archive"
            else -> null
        }
    }

    fun hidden(name: String) = name.startsWith(".") || name == TRASH

    data class Meta(val width: Int?, val height: Int?, val duration: Double?, val takenAt: Long?, val id: Long? = null)

    /** MediaStore metadata for every indexed file directly in (or below) a directory. */
    fun mediaStoreMeta(ctx: Context, dir: String, recursive: Boolean): Map<String, Meta> {
        val out = HashMap<String, Meta>()
        val uri = MediaStore.Files.getContentUri("external")
        val proj = arrayOf(
            MediaStore.MediaColumns._ID,
            MediaStore.MediaColumns.DATA,
            MediaStore.MediaColumns.WIDTH,
            MediaStore.MediaColumns.HEIGHT,
            MediaStore.MediaColumns.ORIENTATION,
            MediaStore.MediaColumns.DURATION,
            MediaStore.MediaColumns.DATE_TAKEN,
        )
        val prefix = dir.trimEnd('/') + "/"
        try {
            ctx.contentResolver.query(uri, proj, "${MediaStore.MediaColumns.DATA} LIKE ?", arrayOf("$prefix%"), null)?.use { c ->
                val iId = c.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
                val iData = c.getColumnIndexOrThrow(MediaStore.MediaColumns.DATA)
                val media = PeekMediaServer.get(ctx)
                val iW = c.getColumnIndex(MediaStore.MediaColumns.WIDTH)
                val iH = c.getColumnIndex(MediaStore.MediaColumns.HEIGHT)
                val iO = c.getColumnIndex(MediaStore.MediaColumns.ORIENTATION)
                val iD = c.getColumnIndex(MediaStore.MediaColumns.DURATION)
                val iT = c.getColumnIndex(MediaStore.MediaColumns.DATE_TAKEN)
                while (c.moveToNext()) {
                    val path = c.getString(iData) ?: continue
                    if (!recursive && path.substring(prefix.length).contains('/')) continue
                    var w = if (iW >= 0 && !c.isNull(iW)) c.getInt(iW) else 0
                    var h = if (iH >= 0 && !c.isNull(iH)) c.getInt(iH) else 0
                    val o = if (iO >= 0 && !c.isNull(iO)) c.getInt(iO) else 0
                    if (o == 90 || o == 270) { val t = w; w = h; h = t }
                    val d = if (iD >= 0 && !c.isNull(iD)) c.getLong(iD) / 1000.0 else null
                    val taken = if (iT >= 0 && !c.isNull(iT)) c.getLong(iT) else null
                    val id = c.getLong(iId)
                    media.rememberId(path, id)
                    out[path] = Meta(w.takeIf { it > 0 }, h.takeIf { it > 0 }, d?.takeIf { it > 0 }, taken?.takeIf { it > 0 }, id)
                }
            }
        } catch (_: Exception) {
        }
        return out
    }

    /** Fallback metadata straight from the file (EXIF / container). */
    fun fileMeta(f: File, kind: String): Meta {
        try {
            if (kind == "image") {
                val ex = ExifInterface(f.absolutePath)
                var w = ex.getAttributeInt(ExifInterface.TAG_IMAGE_WIDTH, 0)
                var h = ex.getAttributeInt(ExifInterface.TAG_IMAGE_LENGTH, 0)
                if (w == 0 || h == 0) {
                    val o = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
                    android.graphics.BitmapFactory.decodeFile(f.absolutePath, o)
                    w = o.outWidth; h = o.outHeight
                }
                if (ex.rotationDegrees == 90 || ex.rotationDegrees == 270) { val t = w; w = h; h = t }
                val taken = exifDate(ex)
                return Meta(w.takeIf { it > 0 }, h.takeIf { it > 0 }, null, taken)
            }
            if (kind == "video" || kind == "audio") {
                val r = MediaMetadataRetriever()
                try {
                    r.setDataSource(f.absolutePath)
                    var w = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)?.toIntOrNull()
                    var h = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)?.toIntOrNull()
                    val rot = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION)?.toIntOrNull() ?: 0
                    if (rot == 90 || rot == 270) { val t = w; w = h; h = t }
                    val d = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull()?.div(1000.0)
                    val date = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DATE)?.let { parseMp4Date(it) }
                    return Meta(w, h, d, date)
                } finally {
                    r.release()
                }
            }
        } catch (_: Exception) {
        }
        return Meta(null, null, null, null)
    }

    private fun exifDate(ex: ExifInterface): Long? {
        val s = ex.getAttribute(ExifInterface.TAG_DATETIME_ORIGINAL) ?: ex.getAttribute(ExifInterface.TAG_DATETIME) ?: return null
        return try {
            SimpleDateFormat("yyyy:MM:dd HH:mm:ss", Locale.US).parse(s)?.time
        } catch (_: Exception) {
            null
        }
    }

    private fun parseMp4Date(s: String): Long? = try {
        val f = SimpleDateFormat("yyyyMMdd'T'HHmmss.SSS'Z'", Locale.US)
        f.timeZone = TimeZone.getTimeZone("UTC")
        f.parse(s)?.time?.takeIf { it > 0 }
    } catch (_: Exception) {
        null
    }

    fun fileJson(f: File, kind: String, meta: Meta?): JSObject {
        val m = meta ?: fileMeta(f, kind)
        val o = JSObject()
        o.put("path", f.absolutePath)
        o.put("name", f.name)
        o.put("kind", kind)
        o.put("size", f.length())
        o.put("mtime", f.lastModified())
        o.put("width", m.width)
        o.put("height", m.height)
        o.put("duration", m.duration)
        o.put("takenAt", m.takenAt ?: f.lastModified())
        o.put("id", meta?.id)
        return o
    }

    /** List a directory: sub folders (with counts + covers) and media files. */
    fun list(ctx: Context, path: String, recursive: Boolean, showHidden: Boolean, limit: Int): JSObject {
        val root = File(path)
        val files = JSArray()
        val dirs = JSArray()
        val meta = mediaStoreMeta(ctx, root.absolutePath, recursive)
        var count = 0
        fun walk(d: File, top: Boolean) {
            val children = d.listFiles() ?: return
            for (c in children.sortedBy { it.name.lowercase(Locale.ROOT) }) {
                if (!showHidden && hidden(c.name)) continue
                if (c.isDirectory) {
                    if (top) dirs.put(dirJson(c, showHidden))
                    if (recursive) walk(c, false)
                    continue
                }
                val k = kind(c.name) ?: continue
                if (count >= limit) return
                files.put(fileJson(c, k, meta[c.absolutePath]))
                count++
            }
        }
        walk(root, true)
        val out = JSObject()
        out.put("path", root.absolutePath)
        out.put("files", files)
        out.put("dirs", dirs)
        out.put("truncated", count >= limit)
        return out
    }

    private fun dirJson(d: File, showHidden: Boolean): JSObject {
        val o = JSObject()
        o.put("name", d.name)
        o.put("path", d.absolutePath)
        var n = 0
        val covers = JSArray()
        d.listFiles()?.sortedByDescending { it.lastModified() }?.forEach { c ->
            if (!showHidden && hidden(c.name)) return@forEach
            val k = kind(c.name)
            if (c.isFile && (k == "image" || k == "video")) {
                n++
                if (covers.length() < 3) covers.put(c.absolutePath)
            }
        }
        o.put("count", n)
        o.put("covers", covers)
        return o
    }

    /** Only directories (for the folder picker). */
    fun listDirs(path: String): JSArray {
        val a = JSArray()
        File(path).listFiles()?.filter { it.isDirectory && !hidden(it.name) }?.sortedBy { it.name.lowercase(Locale.ROOT) }?.forEach {
            val o = JSObject(); o.put("name", it.name); o.put("path", it.absolutePath); a.put(o)
        }
        return a
    }

    fun storageRoots(ctx: Context): JSArray {
        val a = JSArray()
        val primary = Environment.getExternalStorageDirectory()
        fun add(name: String, f: File) {
            if (f.exists()) { val o = JSObject(); o.put("name", name); o.put("path", f.absolutePath); a.put(o) }
        }
        add("DCIM", File(primary, Environment.DIRECTORY_DCIM))
        add("Camera", File(primary, "${Environment.DIRECTORY_DCIM}/Camera"))
        add("Pictures", File(primary, Environment.DIRECTORY_PICTURES))
        add("Movies", File(primary, Environment.DIRECTORY_MOVIES))
        add("Download", File(primary, Environment.DIRECTORY_DOWNLOADS))
        add("Screenshots", File(primary, "${Environment.DIRECTORY_PICTURES}/Screenshots"))
        add("WhatsApp", File(primary, "Android/media/com.whatsapp/WhatsApp/Media"))
        add("Telegram", File(primary, "${Environment.DIRECTORY_PICTURES}/Telegram"))
        add("Internal storage", primary)
        // removable SD cards
        ctx.getExternalFilesDirs(null).drop(1).filterNotNull().forEach { f ->
            val idx = f.absolutePath.indexOf("/Android/")
            if (idx > 0) add("SD card", File(f.absolutePath.substring(0, idx)))
        }
        return a
    }

    fun uniqueFile(dir: File, name: String): File = File(dir, PathGuard.uniqueName(name) { File(dir, it).exists() })

    fun uniqueEditFile(dir: File, base: String, suffix: String, ext: String): File {
        var f = File(dir, "$base$suffix.$ext")
        var i = 2
        while (f.exists()) { f = File(dir, "$base$suffix-$i.$ext"); i++ }
        return f
    }

    fun scan(ctx: Context, vararg paths: String) {
        try {
            MediaScannerConnection.scanFile(ctx, paths, null, null)
        } catch (_: Exception) {
        }
    }

    fun move(src: File, dest: File): Boolean {
        dest.parentFile?.mkdirs()
        if (src.renameTo(dest)) return true
        if (src.isDirectory) { src.copyRecursively(dest, overwrite = false); return src.deleteRecursively() }
        src.copyTo(dest, overwrite = false)
        return src.delete()
    }

    fun trash(f: File): File {
        val root = Environment.getExternalStorageDirectory()
        val stamp = SimpleDateFormat("yyyy-MM-dd_HH-mm-ss", Locale.US).format(System.currentTimeMillis())
        val rel = f.absolutePath.removePrefix(root.absolutePath).trimStart('/')
        val dest = uniqueFile(File(root, "$TRASH/$stamp/${rel.substringBeforeLast('/', "")}"), f.name)
        move(f, dest)
        return dest
    }

    /** Extract a zip with zip-slip protection; tries UTF-8, then the Turkish DOS code page. */
    fun extractZip(zip: File, dest: File): Int {
        val charsets = listOf(Charsets.UTF_8, runCatching { Charset.forName("IBM857") }.getOrNull(), runCatching { Charset.forName("IBM437") }.getOrNull()).filterNotNull()
        var lastErr: Exception? = null
        for (cs in charsets) {
            try {
                return extractWith(zip, dest, cs)
            } catch (e: IllegalArgumentException) {
                lastErr = e // malformed names for this charset
            }
        }
        throw lastErr ?: IllegalStateException("Cannot read archive")
    }

    private fun extractWith(zip: File, dest: File, cs: Charset): Int {
        val root = dest.canonicalFile
        root.mkdirs()
        var n = 0
        ZipFile(zip, cs).use { z ->
            val entries = z.entries()
            while (entries.hasMoreElements()) {
                val e = entries.nextElement()
                val name = e.name.replace('\\', '/')
                if (name.startsWith("__MACOSX/") || name.endsWith(".DS_Store")) continue
                val target = File(root, name).canonicalFile
                if (!target.path.startsWith(root.path + File.separator)) continue
                if (e.isDirectory) { target.mkdirs(); continue }
                target.parentFile?.mkdirs()
                z.getInputStream(e).use { input -> FileOutputStream(target).use { input.copyTo(it, 256 * 1024) } }
                n++
            }
        }
        return n
    }

    fun dirSize(f: File): Long = if (f.isDirectory) (f.listFiles()?.sumOf { dirSize(it) } ?: 0L) else f.length()

    fun exifJson(path: String): JSObject? {
        return try {
            val ex = ExifInterface(path)
            val o = JSObject()
            fun s(tag: String, key: String) { ex.getAttribute(tag)?.let { o.put(key, it) } }
            s(ExifInterface.TAG_MAKE, "Make"); s(ExifInterface.TAG_MODEL, "Model"); s(ExifInterface.TAG_LENS_MODEL, "LensModel")
            ex.getAttributeDouble(ExifInterface.TAG_F_NUMBER, 0.0).takeIf { it > 0 }?.let { o.put("FNumber", it) }
            ex.getAttributeDouble(ExifInterface.TAG_EXPOSURE_TIME, 0.0).takeIf { it > 0 }?.let { o.put("ExposureTime", it) }
            ex.getAttributeInt(ExifInterface.TAG_PHOTOGRAPHIC_SENSITIVITY, 0).takeIf { it > 0 }?.let { o.put("ISO", it) }
            ex.getAttributeDouble(ExifInterface.TAG_FOCAL_LENGTH, 0.0).takeIf { it > 0 }?.let { o.put("FocalLength", it) }
            ex.latLong?.let { o.put("latitude", it[0]); o.put("longitude", it[1]) }
            if (o.length() == 0) null else o
        } catch (_: Exception) {
            null
        }
    }

    /** Copy capture date + camera tags from the original to an edited copy. */
    fun copyExif(from: String, to: String) {
        try {
            val a = ExifInterface(from)
            val b = ExifInterface(to)
            for (t in listOf(ExifInterface.TAG_DATETIME_ORIGINAL, ExifInterface.TAG_DATETIME, ExifInterface.TAG_MAKE, ExifInterface.TAG_MODEL, ExifInterface.TAG_LENS_MODEL, ExifInterface.TAG_GPS_LATITUDE, ExifInterface.TAG_GPS_LATITUDE_REF, ExifInterface.TAG_GPS_LONGITUDE, ExifInterface.TAG_GPS_LONGITUDE_REF)) {
                a.getAttribute(t)?.let { b.setAttribute(t, it) }
            }
            b.setAttribute(ExifInterface.TAG_SOFTWARE, "PeekAround")
            b.saveAttributes()
        } catch (_: Exception) {
        }
    }

    /**
     * Open a content:// URI; MediaStore items are opened with setRequireOriginal (Android 10+,
     * needs ACCESS_MEDIA_LOCATION) so the EXIF GPS block is not redacted. Falls back to the plain URI.
     */
    fun openOriginal(ctx: android.content.Context, uri: android.net.Uri): java.io.InputStream {
        if (android.os.Build.VERSION.SDK_INT >= 29 && uri.authority == android.provider.MediaStore.AUTHORITY &&
            androidx.core.content.ContextCompat.checkSelfPermission(ctx, android.Manifest.permission.ACCESS_MEDIA_LOCATION) == android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            try {
                ctx.contentResolver.openInputStream(android.provider.MediaStore.setRequireOriginal(uri))?.let { return it }
            } catch (_: Exception) {
            }
        }
        return ctx.contentResolver.openInputStream(uri) ?: throw java.io.FileNotFoundException(uri.toString())
    }
}
