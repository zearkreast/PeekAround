package app.peekaround

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.util.Locale
import kotlin.math.min
import kotlin.random.Random

/*
 * Pure (android-free) pieces of the phone ↔ PC link (docs/link-protocol.md) and of the local
 * media server: frame codec, NDJSON list lines, HTTP Range parsing, reply headers, path guard,
 * unique names, ISO 6709 parsing, reconnect backoff. Covered by JVM unit tests (app/src/test).
 */

/** An error answered to the server as `{"t":"err","rid","status","message"}`. */
class LinkError(val status: Int, message: String) : IOException(message)

// ───────────────────────── frames ─────────────────────────

data class LinkFolder(val key: String, val name: String, val path: String) {
    fun toJson(): JSONObject = JSONObject().put("key", key).put("name", name).put("path", path)
}

sealed class Incoming {
    data class Pending(val code: String) : Incoming()
    data class Paired(val token: String) : Incoming()
    data class Welcome(val server: String, val paired: Boolean) : Incoming()
    object Rejected : Incoming()
    object Ping : Incoming()
    object Pong : Incoming()
    data class Req(val rid: String, val op: String, val args: JSONObject) : Incoming()
    data class Cancel(val rid: String) : Incoming()
    data class Unknown(val t: String?) : Incoming()
}

object LinkCodec {
    const val VERSION = 1
    private val RID = Regex("^[A-Za-z0-9_-]{8,64}$")
    val KEY = Regex("^[A-Za-z0-9_-]{1,64}$")

    /** Parse one text frame from the server; malformed frames → Unknown(null). */
    fun parse(text: String): Incoming {
        val o = try {
            JSONObject(text)
        } catch (_: Exception) {
            return Incoming.Unknown(null)
        }
        return when (val t = o.optString("t", "")) {
            "pending" -> Incoming.Pending(o.optString("code", ""))
            "paired" -> o.optString("token", "").let { if (it.isEmpty()) Incoming.Unknown(t) else Incoming.Paired(it) }
            "welcome" -> Incoming.Welcome(o.optString("server", ""), o.optBoolean("paired", true))
            "rejected" -> Incoming.Rejected
            "ping" -> Incoming.Ping
            "pong" -> Incoming.Pong
            "req" -> {
                val rid = o.optString("rid", "")
                val op = o.optString("op", "")
                if (!RID.matches(rid) || op.isEmpty()) Incoming.Unknown(t)
                else Incoming.Req(rid, op, o.optJSONObject("args") ?: JSONObject())
            }
            "cancel" -> o.optString("rid", "").let { if (RID.matches(it)) Incoming.Cancel(it) else Incoming.Unknown(t) }
            else -> Incoming.Unknown(t)
        }
    }

    fun hello(id: String, name: String, model: String, app: String, folders: List<LinkFolder>, read: Boolean = true, receive: Boolean = true): JSONObject =
        JSONObject()
            .put("t", "hello").put("v", VERSION).put("id", id).put("name", name).put("model", model).put("app", app)
            .put("folders", foldersJson(folders))
            .put("caps", JSONObject().put("read", read).put("receive", receive))

    fun folders(folders: List<LinkFolder>): JSONObject = JSONObject().put("t", "folders").put("folders", foldersJson(folders))

    fun foldersJson(folders: List<LinkFolder>): JSONArray = JSONArray().also { a -> folders.forEach { a.put(it.toJson()) } }

    fun err(rid: String, status: Int, message: String): JSONObject = JSONObject().put("t", "err").put("rid", rid).put("status", status).put("message", message)

    fun res(rid: String, data: JSONObject): JSONObject = JSONObject().put("t", "res").put("rid", rid).put("ok", true).put("data", data)

    fun changed(key: String, dirs: Collection<String>): JSONObject = JSONObject().put("t", "changed").put("key", key).put("dirs", JSONArray(dirs.toList()))

    fun ping(): JSONObject = JSONObject().put("t", "ping")
    fun pong(): JSONObject = JSONObject().put("t", "pong")

    /** One NDJSON line of a `list` reply (exact field order of the golden transcripts). */
    fun listLine(p: String, k: String, s: Long, m: Long, w: Int?, h: Int?, d: Double?, t: Long?, id: Long?): String {
        val sb = StringBuilder(160)
        sb.append("{\"p\":").append(Json.quote(p))
        sb.append(",\"k\":").append(Json.quote(k))
        sb.append(",\"s\":").append(s)
        sb.append(",\"m\":").append(m)
        sb.append(",\"w\":").append(w?.toString() ?: "null")
        sb.append(",\"h\":").append(h?.toString() ?: "null")
        sb.append(",\"d\":").append(d?.let { Json.num(it) } ?: "null")
        sb.append(",\"t\":").append(t?.toString() ?: "null")
        sb.append(",\"id\":").append(id?.toString() ?: "null")
        return sb.append('}').toString()
    }

    fun dirLine(dir: String): String = "{\"dir\":${Json.quote(dir)}}"
}

object Json {
    fun quote(s: String): String {
        val sb = StringBuilder(s.length + 2).append('"')
        for (c in s) {
            when (c) {
                '"' -> sb.append("\\\"")
                '\\' -> sb.append("\\\\")
                '\n' -> sb.append("\\n")
                '\r' -> sb.append("\\r")
                '\t' -> sb.append("\\t")
                '\b' -> sb.append("\\b")
                '\u000C' -> sb.append("\\f")
                else -> if (c < ' ' || c == ' ' || c == ' ') sb.append(String.format(Locale.ROOT, "\\u%04x", c.code)) else sb.append(c)
            }
        }
        return sb.append('"').toString()
    }

    /** JS-like number formatting: 12.0 → "12", 12.5 → "12.5" */
    fun num(d: Double): String = if (d.isNaN() || d.isInfinite()) "null" else if (d == Math.floor(d) && Math.abs(d) < 1e15) d.toLong().toString() else d.toString()
}

// ───────────────────────── HTTP Range ─────────────────────────

sealed class ByteRange {
    object Full : ByteRange()
    data class Part(val start: Long, val end: Long) : ByteRange() {
        val length get() = end - start + 1
    }
    object Unsatisfiable : ByteRange()
}

object Ranges {
    /**
     * Parse a single `bytes=` range against a resource of `total` bytes (RFC 9110):
     *   bytes=0-99 · bytes=100- · bytes=-500 (suffix). Multiple ranges → the first one.
     * Missing / malformed / non-bytes header → Full (ignore it, like most servers).
     */
    fun parse(header: String?, total: Long): ByteRange {
        if (header == null) return ByteRange.Full
        val h = header.trim()
        if (!h.startsWith("bytes=", ignoreCase = true)) return ByteRange.Full
        val spec = h.substring(6).split(',')[0].trim()
        val dash = spec.indexOf('-')
        if (dash < 0) return ByteRange.Full
        val a = spec.substring(0, dash).trim()
        val b = spec.substring(dash + 1).trim()
        if (a.isEmpty()) {
            val n = b.toLongOrNull() ?: return ByteRange.Full
            if (n <= 0 || total <= 0) return ByteRange.Unsatisfiable
            return ByteRange.Part(maxOf(0, total - n), total - 1)
        }
        val start = a.toLongOrNull() ?: return ByteRange.Full
        if (start < 0) return ByteRange.Full
        val end = if (b.isEmpty()) total - 1 else (b.toLongOrNull() ?: return ByteRange.Full)
        if (b.isNotEmpty() && end < start) return ByteRange.Full
        if (start >= total) return ByteRange.Unsatisfiable
        return ByteRange.Part(start, min(end, total - 1))
    }
}

/** Status + headers of a `file` reply PUT (and the bytes to stream). */
data class FileReply(val status: Int, val offset: Long, val length: Long, val headers: LinkedHashMap<String, String>)

object FileReplies {
    fun build(range: String?, total: Long, mtime: Long, mime: String): FileReply {
        val h = LinkedHashMap<String, String>()
        return when (val r = Ranges.parse(range, total)) {
            is ByteRange.Unsatisfiable -> {
                h["X-Peek-Status"] = "416"
                h["X-Peek-Content-Range"] = "bytes */$total"
                h["X-Peek-Total"] = total.toString()
                h["Content-Length"] = "0"
                FileReply(416, 0, 0, h)
            }
            is ByteRange.Part -> {
                h["X-Peek-Status"] = "206"
                h["Content-Type"] = mime
                h["Content-Length"] = r.length.toString()
                h["X-Peek-Content-Range"] = "bytes ${r.start}-${r.end}/$total"
                h["X-Peek-Total"] = total.toString()
                h["X-Peek-Mtime"] = mtime.toString()
                FileReply(206, r.start, r.length, h)
            }
            is ByteRange.Full -> {
                h["X-Peek-Status"] = "200"
                h["Content-Type"] = mime
                h["Content-Length"] = total.toString()
                h["X-Peek-Total"] = total.toString()
                h["X-Peek-Mtime"] = mtime.toString()
                FileReply(200, 0, total, h)
            }
        }
    }
}

// ───────────────────────── paths ─────────────────────────

object PathGuard {
    /** Validate a protocol-relative path: '/'-separated, no '..', '.', empty segments, NUL, '\' or leading '/'. "" = root. */
    fun cleanRel(rel: String): String {
        if (rel.isEmpty()) return ""
        if (rel.startsWith("/") || rel.contains('\u0000') || rel.contains('\\') || Regex("^[A-Za-z]:").containsMatchIn(rel)) throw LinkError(403, "outside shared folder")
        val segs = rel.trimEnd('/').split('/')
        for (s in segs) if (s.isEmpty() || s == "." || s == "..") throw LinkError(403, "outside shared folder")
        return segs.joinToString("/")
    }

    /**
     * Resolve `rel` inside `root`, following symlinks: the canonical result must be the root's
     * canonical path or lie below it, otherwise LinkError(403).
     */
    fun resolve(root: File, rel: String): File {
        val clean = cleanRel(rel)
        val base = root.canonicalFile
        if (clean.isEmpty()) return base
        val f = File(base, clean).canonicalFile
        if (!isInside(base, f)) throw LinkError(403, "outside shared folder")
        return f
    }

    fun isInside(canonicalRoot: File, canonicalFile: File): Boolean {
        val r = canonicalRoot.path
        val p = canonicalFile.path
        return p == r || p.startsWith(if (r.endsWith(File.separator)) r else r + File.separator)
    }

    /** True when `path` (canonicalised) is inside one of the canonical `roots`. */
    fun allowed(path: File, roots: Collection<File>): Boolean {
        val c = try { path.canonicalFile } catch (_: IOException) { return false }
        return roots.any { isInside(it, c) }
    }

    /** A plain file name received from the PC: no separators, not '.'/'..', no control chars. */
    fun cleanName(name: String): String {
        val n = name.trim()
        if (n.isEmpty() || n == "." || n == ".." || n.contains('/') || n.contains('\\') || n.any { it < ' ' } || n.length > 255) throw LinkError(400, "bad file name")
        return n
    }

    /** "IMG_1.jpg" → "IMG_1 (1).jpg", "IMG_1 (2).jpg"…; dot-files and names without extension keep the whole name as base. */
    fun uniqueName(name: String, exists: (String) -> Boolean): String {
        if (!exists(name)) return name
        val dot = name.lastIndexOf('.')
        val base = if (dot > 0) name.substring(0, dot) else name
        val ext = if (dot > 0) name.substring(dot) else ""
        var i = 1
        while (true) {
            val c = "$base ($i)$ext"
            if (!exists(c)) return c
            i++
        }
    }

    fun relOf(root: File, f: File): String {
        val r = root.path.trimEnd(File.separatorChar)
        return if (f.path == r) "" else f.path.removePrefix(r + File.separator).replace(File.separatorChar, '/')
    }
}

// ───────────────────────── ISO 6709 ─────────────────────────

object Iso6709 {
    private val RE = Regex("^([+-]\\d+(?:\\.\\d+)?)([+-]\\d+(?:\\.\\d+)?)([+-]\\d+(?:\\.\\d+)?)?(?:CRS[^/]*)?/?$")

    /**
     * ISO 6709 location strings as written by phones (MediaMetadataRetriever METADATA_KEY_LOCATION,
     * QuickTime ©xyz): "+41.0082+028.9784/", "+41.0082+028.9784+012.300/", also the
     * degree-minute(-second) forms ±DDMM.mm±DDDMM.mm and ±DDMMSS±DDDMMSS.
     * Returns [lat, lon] or null (invalid, out of range, or the 0,0 null island).
     */
    fun parse(s: String?): DoubleArray? {
        if (s == null) return null
        val m = RE.matchEntire(s.trim()) ?: return null
        val lat = component(m.groupValues[1], 2) ?: return null
        val lon = component(m.groupValues[2], 3) ?: return null
        if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return null
        if (lat == 0.0 && lon == 0.0) return null
        return doubleArrayOf(lat, lon)
    }

    /** degDigits = 2 for latitude, 3 for longitude */
    private fun component(v: String, degDigits: Int): Double? {
        val sign = if (v[0] == '-') -1.0 else 1.0
        val body = v.substring(1)
        val intPart = body.substringBefore('.')
        val frac = if (body.contains('.')) "0." + body.substringAfter('.') else "0"
        val f = frac.toDoubleOrNull() ?: return null
        val n = intPart.length
        val deg: Double = when (n) {
            degDigits -> (intPart.toIntOrNull() ?: return null) + f
            degDigits + 2 -> {
                val d = intPart.substring(0, degDigits).toInt()
                val mm = intPart.substring(degDigits).toInt() + f
                if (mm >= 60) return null
                d + mm / 60.0
            }
            degDigits + 4 -> {
                val d = intPart.substring(0, degDigits).toInt()
                val mm = intPart.substring(degDigits, degDigits + 2).toInt()
                val ss = intPart.substring(degDigits + 2).toInt() + f
                if (mm >= 60 || ss >= 60) return null
                d + mm / 60.0 + ss / 3600.0
            }
            else -> {
                // some writers drop leading zeros of the degrees ("+8.5+-…" is not valid, but "+8.5" is common)
                if (n in 1 until degDigits) (intPart.toIntOrNull() ?: return null) + f else return null
            }
        }
        return sign * deg
    }
}

// ───────────────────────── reconnect ─────────────────────────

object Backoff {
    /** attempt 0 → ~1 s, 1 → ~2 s, … capped at 60 s, ±20 % jitter (never below 500 ms). */
    fun delayMs(attempt: Int, random: Random = Random.Default): Long {
        val base = 1000L shl attempt.coerceIn(0, 6)
        val capped = min(base, 60_000L)
        val jitter = (capped * 0.2 * (random.nextDouble() * 2 - 1)).toLong()
        return maxOf(500L, capped + jitter)
    }
}

/** Kind of a media file by extension (same lists as PeekFs, kept here for JVM tests). */
object Kinds {
    fun mime(name: String): String = when (name.substringAfterLast('.', "").lowercase(Locale.ROOT)) {
        "jpg", "jpeg", "jfif" -> "image/jpeg"
        "png" -> "image/png"
        "gif" -> "image/gif"
        "webp" -> "image/webp"
        "avif" -> "image/avif"
        "bmp" -> "image/bmp"
        "heic", "heif" -> "image/heif"
        "mp4", "m4v" -> "video/mp4"
        "mov" -> "video/quicktime"
        "webm" -> "video/webm"
        "mkv" -> "video/x-matroska"
        "3gp" -> "video/3gpp"
        "ts" -> "video/mp2t"
        "avi" -> "video/x-msvideo"
        "mp3" -> "audio/mpeg"
        "m4a" -> "audio/mp4"
        "aac" -> "audio/aac"
        "ogg", "oga" -> "audio/ogg"
        "opus" -> "audio/opus"
        "wav" -> "audio/wav"
        "flac" -> "audio/flac"
        "zip" -> "application/zip"
        "ttf" -> "font/ttf"
        "otf" -> "font/otf"
        "woff" -> "font/woff"
        "woff2" -> "font/woff2"
        else -> "application/octet-stream"
    }
}
