package app.peekaround

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import com.getcapacitor.JSArray
import com.getcapacitor.JSObject
import okhttp3.Call
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Request
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

/**
 * Native streaming transfers between the phone and a PeekAround server (client mode and the
 * link UI): downloads into a phone folder chosen by the user and uploads of phone files.
 * Progress → 'transfer' events: {id, kind, file, index, count, bytes, total, done, error?, paths?, results?}
 */
class PeekTransfers(private val ctx: Context, private val emit: (JSObject) -> Unit) {
    private val exec = Executors.newFixedThreadPool(2) { r -> Thread(r, "peek-transfer").apply { isDaemon = true } }
    private class T {
        @Volatile var cancelled = false
        @Volatile var call: Call? = null
    }
    private val running = ConcurrentHashMap<String, T>()

    fun cancel(id: String) {
        running[id]?.let { it.cancelled = true; it.call?.cancel() }
    }

    private fun event(id: String, kind: String, file: String, index: Int, count: Int, bytes: Long, total: Long, done: Boolean, error: String? = null) =
        JSObject().put("id", id).put("kind", kind).put("file", file).put("index", index).put("count", count)
            .put("bytes", bytes).put("total", total).put("done", done).also { if (error != null) it.put("error", error) }

    private fun headersOf(h: JSONObject?, rb: Request.Builder): Request.Builder {
        h?.keys()?.forEach { k -> rb.header(k, h.optString(k)) }
        return rb
    }

    // ───────────────────────── downloads ─────────────────────────

    /**
     * items: [{url, name, mtime?, dir?}] (absolute http(s) URLs), dest: absolute phone folder,
     * headers: sent with every request. Returns the transfer id at once; work runs in the background.
     */
    fun download(id0: String?, items: JSONArray, dest: String, headers: JSONObject?): String {
        val id = id0 ?: UUID.randomUUID().toString()
        val t = T()
        running[id] = t
        exec.execute {
            val paths = JSArray()
            var err: String? = null
            val root = File(dest)
            try {
                if (!root.isDirectory && !root.mkdirs()) throw IOException("Cannot create $dest")
                val count = items.length()
                for (i in 0 until count) {
                    if (t.cancelled) throw IOException("Canceled")
                    val it = items.getJSONObject(i)
                    val name = PathGuard.cleanName(it.getString("name"))
                    val sub = PathGuard.cleanRel(it.optString("dir", ""))
                    val url = it.getString("url").toHttpUrlOrNull() ?: throw IOException("Bad URL: ${it.getString("url")}")
                    val dir = if (sub.isEmpty()) root else File(root, sub)
                    if (!dir.isDirectory && !dir.mkdirs()) throw IOException("Cannot create ${dir.absolutePath}")
                    val part = PeekFs.uniqueFile(dir, "$name.part")
                    var ok = false
                    try {
                        val call = PeekHttp.data.newCall(headersOf(headers, Request.Builder().url(url).get()).build())
                        t.call = call
                        if (t.cancelled) throw IOException("Canceled")
                        var mtime = if (it.has("mtime") && !it.isNull("mtime")) it.optLong("mtime") else 0L
                        call.execute().use { res ->
                            if (!res.isSuccessful) throw IOException("HTTP ${res.code}: $name")
                            val body = res.body ?: throw IOException("Empty response: $name")
                            val total = body.contentLength()
                            if (mtime <= 0) mtime = res.header("X-Peek-Mtime")?.toLongOrNull() ?: (res.headers.getDate("Last-Modified")?.time ?: 0L)
                            var bytes = 0L
                            var last = 0L
                            FileOutputStream(part).use { out ->
                                body.byteStream().use { input ->
                                    val buf = ByteArray(128 * 1024)
                                    while (true) {
                                        if (t.cancelled) throw IOException("Canceled")
                                        val n = input.read(buf)
                                        if (n < 0) break
                                        out.write(buf, 0, n)
                                        bytes += n
                                        val now = System.currentTimeMillis()
                                        if (now - last > 250) {
                                            last = now
                                            emit(event(id, "download", name, i, count, bytes, total, false))
                                        }
                                    }
                                }
                            }
                            if (total >= 0 && bytes != total) throw IOException("Incomplete download: $name")
                            emit(event(id, "download", name, i, count, bytes, maxOf(total, bytes), false))
                        }
                        val target = PeekFs.uniqueFile(dir, name)
                        if (!part.renameTo(target)) throw IOException("Cannot write ${target.absolutePath}")
                        if (mtime > 0) target.setLastModified(mtime)
                        PeekFs.scan(ctx, target.absolutePath)
                        paths.put(target.absolutePath)
                        ok = true
                    } finally {
                        if (!ok) part.delete()
                    }
                }
            } catch (e: Exception) {
                err = if (t.cancelled) "canceled" else (e.message ?: e.javaClass.simpleName)
            } finally {
                running.remove(id)
            }
            emit(event(id, "download", "", paths.length(), items.length(), 0, 0, true, err).put("paths", paths))
        }
        return id
    }

    // ───────────────────────── uploads ─────────────────────────

    private data class Src(val path: String, val rel: String, val size: Long, val mtime: Long, val uri: Uri?)

    /** files (and folders, recursively) / content:// URIs → their upload entries */
    private fun collect(paths: JSONArray): List<Src> {
        val out = ArrayList<Src>()
        for (i in 0 until paths.length()) {
            val p = paths.getString(i)
            if (p.startsWith("content://")) {
                val uri = Uri.parse(p)
                var name = uri.lastPathSegment ?: "file"
                var size = -1L
                try {
                    ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                        if (c.moveToFirst()) {
                            name = c.getString(0) ?: name
                            if (!c.isNull(1)) size = c.getLong(1)
                        }
                    }
                } catch (_: Exception) {
                }
                out.add(Src(p, name.substringAfterLast('/'), size, 0, uri))
                continue
            }
            val f = File(p)
            if (f.isDirectory) {
                f.walkTopDown().onEnter { it == f || !PeekFs.hidden(it.name) }.filter { it.isFile && !PeekFs.hidden(it.name) }.forEach { c ->
                    out.add(Src(c.absolutePath, f.name + "/" + c.relativeTo(f).path.replace(File.separatorChar, '/'), c.length(), c.lastModified(), null))
                }
            } else if (f.isFile) out.add(Src(f.absolutePath, f.name, f.length(), f.lastModified(), null))
            else throw IOException("Not found: $p")
        }
        return out
    }

    /** PUT `${url}/api/upload?folderId&dir&path&mtime[&zip]` per file, streamed from disk. */
    fun upload(id0: String?, base: String, folderId: String, dir: String, paths: JSONArray, zip: String?, headers: JSONObject?): String {
        val id = id0 ?: UUID.randomUUID().toString()
        val t = T()
        running[id] = t
        exec.execute {
            val results = JSArray()
            var err: String? = null
            var count = 0
            try {
                val files = collect(paths)
                count = files.size
                files.forEachIndexed { i, s ->
                    if (t.cancelled) throw IOException("Canceled")
                    val ub = (base.trimEnd('/') + "/api/upload").toHttpUrlOrNull()?.newBuilder() ?: throw IOException("Bad server URL")
                    ub.addQueryParameter("folderId", folderId).addQueryParameter("dir", dir).addQueryParameter("path", s.rel)
                    if (s.mtime > 0) ub.addQueryParameter("mtime", s.mtime.toString())
                    if (zip != null) ub.addQueryParameter("zip", zip)
                    var last = 0L
                    val onBytes = { n: Long ->
                        val now = System.currentTimeMillis()
                        if (now - last > 250) {
                            last = now
                            emit(event(id, "upload", s.rel, i, count, n, s.size, false))
                        }
                    }
                    val body = if (s.uri != null) {
                        // provider sizes are not always exact → chunked
                        InputBody({ PeekFs.openOriginal(ctx, s.uri) }, -1, "application/octet-stream", onBytes) { t.cancelled }
                    } else FileBody(File(s.path), 0, File(s.path).length(), "application/octet-stream", onBytes) { t.cancelled }
                    val rb = headersOf(headers, Request.Builder().url(ub.build()).put(body)).header("X-Peek", "1")
                    val call = PeekHttp.data.newCall(rb.build())
                    t.call = call
                    if (t.cancelled) throw IOException("Canceled")
                    call.execute().use { res ->
                        val text = res.body?.string() ?: ""
                        if (!res.isSuccessful) {
                            val msg = try { JSONObject(text).optString("error", "") } catch (_: Exception) { "" }
                            throw IOException("Upload failed (${res.code})${if (msg.isNotEmpty()) ": $msg" else ""}: ${s.rel}")
                        }
                        results.put(try { JSONObject(text) } catch (_: Exception) { JSONObject() })
                    }
                    emit(event(id, "upload", s.rel, i, count, s.size, s.size, false))
                }
            } catch (e: Exception) {
                err = if (t.cancelled) "canceled" else (e.message ?: e.javaClass.simpleName)
            } finally {
                running.remove(id)
            }
            emit(event(id, "upload", "", results.length(), count, 0, 0, true, err).put("results", results))
        }
        return id
    }
}
