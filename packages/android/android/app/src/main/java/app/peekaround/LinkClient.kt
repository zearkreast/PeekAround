package app.peekaround

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.Process
import android.util.Log
import okhttp3.Call
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * The phone side of PeekAround Link v1 (docs/link-protocol.md): one control WebSocket to
 * `GET /api/devices/connect`, data replies as `PUT /api/devices/reply/:rid`, PC → phone files
 * pulled from one-time URLs. Owned by PeekLinkService; all state changes go through [ctl].
 *
 * The phone only ever reads files inside the shared folders and writes new files (unique
 * names) into them: it never deletes, renames or overwrites anything.
 */
class LinkClient(private val ctx: Context, private val onActive: (Int) -> Unit, private val onFatal: () -> Unit) {
    private val prefs = LinkPrefs(ctx)
    private val media = PeekMediaServer.get(ctx)
    private val ctl = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "peek-link").apply { isDaemon = true } }
    private val workers = ThreadPoolExecutor(8, 8, 30, TimeUnit.SECONDS, LinkedBlockingQueue()) { r ->
        Thread({ Process.setThreadPriority(Process.THREAD_PRIORITY_BACKGROUND); r.run() }, "peek-link-op").apply { isDaemon = true }
    }.apply { allowCoreThreadTimeOut(true) }
    private val watch = LinkWatch(ctl) { key, dirs -> send(LinkCodec.changed(key, dirs)) }

    private var ws: WebSocket? = null
    private var gen = 0
    private var attempt = 0
    private var running = false
    private var lastSeen = 0L
    private var retry: ScheduledFuture<*>? = null
    private var beat: ScheduledFuture<*>? = null
    private var netCb: ConnectivityManager.NetworkCallback? = null
    @Volatile private var folders: List<LinkFolder> = prefs.folders
    @Volatile private var base: String = ""
    private val jobs = ConcurrentHashMap<String, Job>()
    private val active = AtomicInteger(0)

    private class Job(val rid: String, val op: String, val args: JSONObject) {
        @Volatile var cancelled = false
        @Volatile var call: Call? = null
        @Volatile var future: Future<*>? = null
    }

    companion object {
        private const val TAG = "PeekLink"
        private const val PING_MS = 30_000L
        private const val IDLE_MS = 75_000L
    }

    // ───────────────────────── lifecycle ─────────────────────────

    fun start() = ctl.execute {
        if (running) return@execute
        running = true
        folders = prefs.folders
        media.setLinkRoots(folders.map { it.path })
        watch.set(folders)
        registerNetwork()
        attempt = 0
        connect()
    }

    fun stop() = ctl.execute {
        running = false
        retry?.cancel(false)
        beat?.cancel(false)
        unregisterNetwork()
        watch.stop()
        cancelAll()
        gen++
        ws?.close(1000, "stopped")
        ws = null
    }

    fun shutdown() {
        stop()
        ctl.execute {
            ctl.shutdown()
            workers.shutdownNow()
        }
    }

    /** prefs.folders changed (setFolders) → tell the server, re-watch */
    fun foldersChanged() = ctl.execute {
        folders = prefs.folders
        media.setLinkRoots(folders.map { it.path })
        watch.set(folders)
        if (LinkHub.state == "online") send(LinkCodec.folders(folders))
    }

    /** reconnect now (url / name / token changed) */
    fun reconnect() = ctl.execute {
        if (!running) return@execute
        gen++
        ws?.close(1000, "reconnect")
        ws = null
        cancelAll()
        attempt = 0
        connect()
    }

    // ───────────────────────── connection ─────────────────────────

    private fun appVersion(): String = try {
        ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName ?: "1.1.0"
    } catch (_: Exception) {
        "1.1.0"
    }

    private fun connect() {
        retry?.cancel(false)
        retry = null
        if (!running) return
        val url = prefs.url
        if (url == null || folders.isEmpty()) {
            LinkHub.set("error", error = "not configured")
            return
        }
        base = url.trimEnd('/')
        if (!hasNetwork()) {
            LinkHub.set("connecting", error = "no network")
            return // the network callback reconnects
        }
        val my = ++gen
        LinkHub.set(if (LinkHub.state == "pending") "pending" else "connecting", LinkHub.code, LinkHub.error)
        val rb = Request.Builder().url("$base/api/devices/connect?v=1").header("X-Peek-Device", prefs.deviceId)
        prefs.token?.let { rb.header("Authorization", "Bearer $it") }
        val req = try {
            rb.build()
        } catch (e: IllegalArgumentException) {
            LinkHub.set("error", error = "bad url")
            return
        }
        ws = PeekHttp.ws.newWebSocket(req, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) = ctl.execute {
                if (my != gen) return@execute
                lastSeen = System.currentTimeMillis()
                webSocket.send(LinkCodec.hello(prefs.deviceId, prefs.deviceName, prefs.model, appVersion(), folders).toString())
                beat?.cancel(false)
                beat = ctl.scheduleWithFixedDelay({ heartbeat(my) }, PING_MS, PING_MS, TimeUnit.MILLISECONDS)
            }

            override fun onMessage(webSocket: WebSocket, text: String) = ctl.execute {
                if (my != gen) return@execute
                lastSeen = System.currentTimeMillis()
                onFrame(LinkCodec.parse(text))
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
                ctl.execute { if (my == gen) closed(code, reason) }
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = ctl.execute {
                if (my == gen) closed(code, reason)
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = ctl.execute {
                if (my != gen) return@execute
                val code = response?.code
                closed(if (code != null) 5000 + code else -1, t.message ?: t.javaClass.simpleName)
            }
        })
    }

    private fun heartbeat(my: Int) {
        if (my != gen) return
        if (System.currentTimeMillis() - lastSeen > IDLE_MS) {
            ws?.cancel() // → onFailure → reconnect
            return
        }
        send(LinkCodec.ping())
    }

    /** Socket gone. code: WS close code, 5000+HTTP status when the upgrade failed, -1 network error. */
    private fun closed(code: Int, reason: String) {
        gen++
        ws = null
        beat?.cancel(false)
        cancelAll()
        if (!running) return
        Log.i(TAG, "link closed $code $reason")
        when (code) {
            4403 -> return rejected()
            4401 -> { // the PC forgot this phone
                prefs.token = null
                prefs.enabled = false
                LinkHub.set("error", error = "forgotten")
                running = false
                onFatal()
                return
            }
            4426 -> {
                prefs.enabled = false
                LinkHub.set("error", error = "unsupported server version")
                running = false
                onFatal()
                return
            }
        }
        val err = when {
            code == 4409 -> "replaced"
            code in 5000..5999 -> "HTTP ${code - 5000}"
            code == -1 -> reason
            else -> "closed ($code)"
        }
        LinkHub.set("connecting", error = err)
        // replaced by another connection of this device: back off longer so two sockets don't fight
        val delay = if (code == 4409) 60_000L else Backoff.delayMs(attempt++)
        retry = ctl.schedule({ connect() }, delay, TimeUnit.MILLISECONDS)
    }

    private fun rejected() {
        prefs.enabled = false
        prefs.token = null
        running = false
        LinkHub.set("rejected")
        onFatal()
    }

    private fun send(frame: JSONObject) {
        ws?.send(frame.toString())
    }

    private fun onFrame(f: Incoming) {
        when (f) {
            is Incoming.Pending -> {
                // the server does not (or no longer) accept our token: forget it, wait for approval
                prefs.token = null
                LinkHub.set("pending", code = f.code)
            }
            is Incoming.Paired -> {
                prefs.token = f.token
                attempt = 0
                LinkHub.set("online")
            }
            is Incoming.Welcome -> {
                attempt = 0
                if (f.server.isNotEmpty()) prefs.serverName = f.server
                LinkHub.set("online")
            }
            is Incoming.Rejected -> {
                gen++
                ws?.close(1000, "rejected")
                ws = null
                rejected()
            }
            is Incoming.Ping -> send(LinkCodec.pong())
            is Incoming.Pong -> {}
            is Incoming.Req -> dispatch(f)
            is Incoming.Cancel -> cancel(f.rid)
            is Incoming.Unknown -> {}
        }
    }

    // ───────────────────────── network ─────────────────────────

    private fun hasNetwork(): Boolean {
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        return try { cm.activeNetwork != null } catch (_: Exception) { true }
    }

    private fun registerNetwork() {
        if (netCb != null) return
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = ctl.execute {
                // network (back): skip the remaining backoff
                if (running && ws == null) {
                    attempt = 0
                    retry?.cancel(false)
                    retry = ctl.schedule({ if (ws == null) connect() }, 500, TimeUnit.MILLISECONDS)
                }
            }

            override fun onLost(network: Network) = ctl.execute {
                if (running && ws == null) LinkHub.set("connecting", error = "no network")
            }
        }
        try {
            cm.registerDefaultNetworkCallback(cb)
            netCb = cb
        } catch (_: Exception) {
        }
    }

    private fun unregisterNetwork() {
        val cb = netCb ?: return
        netCb = null
        try {
            (ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager).unregisterNetworkCallback(cb)
        } catch (_: Exception) {
        }
    }

    // ───────────────────────── requests ─────────────────────────

    private fun dispatch(r: Incoming.Req) {
        val job = Job(r.rid, r.op, r.args)
        if (jobs.putIfAbsent(r.rid, job) != null) return
        job.future = workers.submit {
            val n = active.incrementAndGet()
            onActive(n)
            try {
                run(job)
            } finally {
                jobs.remove(job.rid)
                onActive(active.decrementAndGet())
            }
        }
    }

    private fun cancel(rid: String) {
        val j = jobs[rid] ?: return
        j.cancelled = true
        j.call?.cancel()
        // a job cancelled before a worker picked it up never runs its `finally`: drop it here
        if (j.future?.cancel(false) == true) jobs.remove(rid, j)
    }

    private fun cancelAll() {
        for (rid in jobs.keys.toList()) cancel(rid)
    }

    private fun run(job: Job) {
        try {
            val a = job.args
            val folder = folders.firstOrNull { it.key == a.optString("key") } ?: throw LinkError(404, "unknown folder")
            val root = File(folder.path)
            if (!root.isDirectory) throw LinkError(404, "folder not available")
            when (job.op) {
                "list" -> opList(job, root, a.optString("dir", ""), a.optBoolean("recursive", true))
                "thumb" -> {
                    val f = file(root, a)
                    val k = PeekFs.kind(f.name)
                    if (k != "image" && k != "video") throw LinkError(404, "no thumbnail")
                    val out = media.thumb(f, a.optInt("s", 320), null) ?: throw LinkError(500, "cannot decode")
                    if (!job.cancelled) put(job, mapOf("Content-Type" to "image/jpeg"), FileBody(out, 0, out.length(), "image/jpeg") { job.cancelled })
                }
                "file" -> {
                    val f = file(root, a)
                    val len = f.length()
                    val reply = FileReplies.build(if (a.has("range") && !a.isNull("range")) a.optString("range") else null, len, f.lastModified(), media.mime(f))
                    val type = reply.headers["Content-Type"]
                    put(job, reply.headers.filterKeys { it != "Content-Length" && it != "Content-Type" } + (if (type != null) mapOf("Content-Type" to type) else emptyMap()),
                        FileBody(f, reply.offset, reply.length, type) { job.cancelled })
                }
                "display" -> {
                    val f = file(root, a)
                    val max = a.optInt("max", 4096).coerceIn(256, 8192)
                    val out = media.display(f, max) ?: throw LinkError(500, "cannot decode")
                    if (!job.cancelled) put(job, mapOf("Content-Type" to "image/jpeg"), FileBody(out, 0, out.length(), "image/jpeg") { job.cancelled })
                }
                "strip" -> {
                    val f = file(root, a)
                    if (PeekFs.kind(f.name) != "video") throw LinkError(400, "not a video")
                    val out = media.strip(f, a.optInt("n", 10), a.optInt("h", 90)) ?: throw LinkError(500, "cannot decode")
                    if (!job.cancelled) put(job, mapOf("Content-Type" to "image/jpeg"), FileBody(out, 0, out.length(), "image/jpeg") { job.cancelled })
                }
                "details" -> {
                    val f = file(root, a)
                    val body = PeekMeta.details(f, PeekFs.kind(f.name) ?: "image").toString().toByteArray()
                    put(job, mapOf("Content-Type" to "application/json"), body.toRequestBody("application/json".toMediaType()))
                }
                "receive" -> opReceive(job, folder, root)
                else -> throw LinkError(400, "unknown op ${job.op}")
            }
        } catch (e: LinkError) {
            if (!job.cancelled) send(LinkCodec.err(job.rid, e.status, e.message ?: "error"))
        } catch (e: Exception) {
            if (!job.cancelled) {
                Log.w(TAG, "op ${job.op} failed", e)
                if (job.call == null) send(LinkCodec.err(job.rid, 500, e.message ?: e.javaClass.simpleName))
            }
        }
    }

    /** existing regular file at args.p inside the shared folder */
    private fun file(root: File, a: JSONObject): File {
        val p = a.optString("p", "")
        if (p.isEmpty()) throw LinkError(400, "p required")
        val f = PathGuard.resolve(root, p)
        if (!f.isFile) throw LinkError(404, "not found")
        if (!f.canRead()) throw LinkError(403, "not readable")
        return f
    }

    private fun authed(rb: Request.Builder): Request.Builder {
        rb.header("X-Peek-Device", prefs.deviceId)
        prefs.token?.let { rb.header("Authorization", "Bearer $it") }
        return rb
    }

    /** reply PUT; Content-Length comes from the body (OkHttp), chunked only when it is unknown */
    private fun put(job: Job, headers: Map<String, String>, body: RequestBody) {
        if (job.cancelled) return
        val rb = authed(Request.Builder().url("$base/api/devices/reply/${job.rid}").put(body))
        for ((k, v) in headers) if (!k.equals("Content-Length", true)) rb.header(k, v)
        val call = PeekHttp.data.newCall(rb.build())
        job.call = call
        if (job.cancelled) return call.cancel()
        try {
            call.execute().use { res ->
                if (res.code == 499 || res.code == 410) job.cancelled = true
                else if (res.code >= 400) Log.w(TAG, "reply ${job.op} → ${res.code}")
            }
        } catch (e: IOException) {
            if (!job.cancelled) Log.w(TAG, "reply ${job.op} failed: ${e.message}")
        }
    }

    // ───────────────────────── list ─────────────────────────

    private fun opList(job: Job, root0: File, dir: String, recursive: Boolean) {
        val root = root0.canonicalFile
        val start = PathGuard.resolve(root, dir)
        if (!start.isDirectory) throw LinkError(404, "not found")
        put(job, mapOf("Content-Type" to "application/x-ndjson"), StreamBody("application/x-ndjson") { sink ->
            sink.flush() // headers out now: the server's first-byte timer stops
            val meta = PeekFs.mediaStoreMeta(ctx, start.absolutePath, recursive)
            var lines = 0
            var lastFlush = System.currentTimeMillis()
            fun line(s: String) {
                sink.writeUtf8(s).writeByte('\n'.code)
                lines++
                val now = System.currentTimeMillis()
                if (lines % 200 == 0 || now - lastFlush > 500) {
                    sink.flush()
                    lastFlush = now
                }
            }
            fun walk(d: File) {
                if (job.cancelled) throw IOException("Canceled")
                val children = d.listFiles() ?: return
                var files = 0
                val subdirs = ArrayList<File>()
                for (c in children.sortedBy { it.name }) {
                    if (PeekFs.hidden(c.name)) continue
                    if (c.isDirectory) {
                        val cc = try { c.canonicalFile } catch (_: IOException) { continue }
                        if (!PathGuard.isInside(root, cc)) continue // symlink out of the shared folder
                        subdirs.add(c)
                        continue
                    }
                    val k = PeekFs.kind(c.name) ?: continue
                    if (Build.VERSION.SDK_INT >= 26 && java.nio.file.Files.isSymbolicLink(c.toPath())) {
                        val cc = try { c.canonicalFile } catch (_: IOException) { continue }
                        if (!PathGuard.isInside(root, cc)) continue
                    }
                    val m = meta[c.absolutePath] ?: if (k == "image" || k == "video") PeekFs.fileMeta(c, k) else null
                    line(LinkCodec.listLine(PathGuard.relOf(root, c), k, c.length(), c.lastModified(), m?.width, m?.height, m?.duration, m?.takenAt, m?.id))
                    files++
                }
                val rel = PathGuard.relOf(root, d)
                if (files == 0 && rel.isNotEmpty()) line(LinkCodec.dirLine(rel))
                for (s in subdirs) {
                    if (recursive) walk(s) else {
                        line(LinkCodec.dirLine(PathGuard.relOf(root, s)))
                    }
                }
            }
            walk(start)
            sink.flush()
        })
    }

    // ───────────────────────── receive (PC → phone) ─────────────────────────

    private fun opReceive(job: Job, folder: LinkFolder, root0: File) {
        val a = job.args
        val root = root0.canonicalFile
        val dirRel = PathGuard.cleanRel(a.optString("dir", ""))
        val name = PathGuard.cleanName(a.optString("name", ""))
        if (PeekFs.hidden(name)) throw LinkError(400, "bad file name")
        val url = a.optString("url", "")
        if (!url.startsWith("/api/devices/pull/")) throw LinkError(400, "bad url")
        val size = a.optLong("size", -1)
        val mtime = a.optLong("mtime", 0)
        // create the target dir (inside the folder; every step re-checked after canonicalisation)
        var dir = root
        if (dirRel.isNotEmpty()) for (seg in dirRel.split('/')) {
            val next = File(dir, seg)
            if (!next.exists() && !next.mkdir() && !next.isDirectory) throw LinkError(500, "cannot create folder")
            dir = next.canonicalFile
            if (!PathGuard.isInside(root, dir) || !dir.isDirectory) throw LinkError(403, "outside shared folder")
        }
        if (size > 0 && dir.usableSpace in 0 until size + 16 * 1024 * 1024) throw LinkError(413, "not enough space on the phone")
        val part = PeekFs.uniqueFile(dir, "$name.part")
        val call = PeekHttp.data.newCall(authed(Request.Builder().url(base + url).get()).build())
        job.call = call
        if (job.cancelled) return call.cancel()
        var ok = false
        var mt = mtime
        try {
            call.execute().use { res ->
                res.header("X-Peek-Mtime")?.toLongOrNull()?.takeIf { it > 0 }?.let { mt = it }
                if (!res.isSuccessful) throw LinkError(if (res.code == 404) 404 else 500, "pull failed (${res.code})")
                val body = res.body ?: throw LinkError(500, "empty pull")
                var n = 0L
                FileOutputStream(part).use { out ->
                    body.byteStream().use { input ->
                        val buf = ByteArray(128 * 1024)
                        while (true) {
                            if (job.cancelled) throw IOException("Canceled")
                            val k = input.read(buf)
                            if (k < 0) break
                            out.write(buf, 0, k)
                            n += k
                        }
                    }
                    out.fd.sync()
                }
                if (size >= 0 && n != size) throw LinkError(500, "size mismatch ($n of $size bytes)")
            }
            // unique final name: never overwrite an existing file
            var dest = PeekFs.uniqueFile(dir, name)
            var tries = 0
            while (!part.renameTo(dest)) {
                if (++tries > 3) throw LinkError(500, "cannot rename")
                dest = PeekFs.uniqueFile(dir, name)
            }
            if (mt > 0) dest.setLastModified(mt)
            PeekFs.scan(ctx, dest.absolutePath)
            ok = true
            val rel = PathGuard.relOf(root, dest)
            send(LinkCodec.res(job.rid, JSONObject().put("p", rel)))
            watch.touched(folder.key, PathGuard.relOf(root, dir))
        } finally {
            if (!ok) part.delete()
        }
    }
}
