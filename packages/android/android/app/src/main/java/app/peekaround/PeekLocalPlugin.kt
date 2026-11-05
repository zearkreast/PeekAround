package app.peekaround

import android.Manifest
import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.os.FileObserver
import android.os.Handler
import android.os.Looper
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
import android.util.Base64
import android.webkit.CookieManager
import android.webkit.URLUtil
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import androidx.core.content.FileProvider
import androidx.media3.common.util.UnstableApi
import com.getcapacitor.Bridge
import com.getcapacitor.BridgeWebViewClient
import com.getcapacitor.JSArray
import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin
import com.getcapacitor.PermissionState
import com.getcapacitor.annotation.Permission
import com.getcapacitor.annotation.PermissionCallback
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.util.UUID
import java.util.concurrent.Executors

/** Routes http://localhost/_peek/… to the local media server, everything else to Capacitor. */
class PeekWebViewClient(bridge: Bridge, private val media: PeekMediaServer) : BridgeWebViewClient(bridge) {
    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
        if (request.url.path?.startsWith("/_peek/") == true && (request.url.host == "localhost" || request.url.host == null)) {
            return media.handle(request)
        }
        return super.shouldInterceptRequest(view, request)
    }
}

@UnstableApi
@CapacitorPlugin(
    name = "PeekLocal",
    permissions = [
        Permission(alias = "legacy", strings = [Manifest.permission.READ_EXTERNAL_STORAGE, Manifest.permission.WRITE_EXTERNAL_STORAGE]),
        Permission(alias = "media", strings = [Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VIDEO]),
        Permission(alias = "location", strings = [Manifest.permission.ACCESS_MEDIA_LOCATION]),
        Permission(alias = "notifications", strings = [Manifest.permission.POST_NOTIFICATIONS]),
    ],
)
class PeekLocalPlugin : Plugin() {
    private val io = Executors.newFixedThreadPool(3)
    private lateinit var media: PeekMediaServer
    private lateinit var jobs: PeekJobs
    private lateinit var transfers: PeekTransfers
    private var observer: FileObserver? = null
    private val main = Handler(Looper.getMainLooper())

    companion object {
        /** content:// URIs handed to the app through "Share → PeekAround" */
        val pendingShare = ArrayList<Uri>()
        /** text shared to the app (ACTION_SEND text/plain, e.g. a mega.nz link) */
        @Volatile var pendingText: String? = null
        var instance: PeekLocalPlugin? = null
    }

    override fun load() {
        instance = this
        media = PeekMediaServer.get(context)
        jobs = PeekJobs(context) { notifyListeners("job", it) }
        transfers = PeekTransfers(context) { notifyListeners("transfer", it) }
        PeekIndex.get(context).onProgress = { notifyListeners("index", it) }
        bridge.setWebViewClient(PeekWebViewClient(bridge, media))
        // downloads (server mode "Download" buttons) → system DownloadManager
        bridge.webView.setDownloadListener { url, ua, cd, mime, _ ->
            try {
                val req = DownloadManager.Request(Uri.parse(url))
                val name = URLUtil.guessFileName(url, cd, mime)
                req.setMimeType(mime)
                req.addRequestHeader("User-Agent", ua)
                CookieManager.getInstance().getCookie(url)?.let { req.addRequestHeader("Cookie", it) }
                req.setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
                req.setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS, "PeekAround/$name")
                (context.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager).enqueue(req)
                notifyListeners("download", JSObject().put("name", name))
            } catch (_: Exception) {
            }
        }
    }

    fun onShared() {
        notifyListeners("shared", JSObject().put("count", pendingShare.size).put("text", pendingText))
    }

    private fun bg(call: PluginCall, fn: () -> Unit) {
        io.execute {
            try {
                fn()
            } catch (e: Exception) {
                call.reject(e.message ?: e.toString(), e)
            }
        }
    }

    // ───────────────────────── storage access ─────────────────────────

    private fun hasAccess(): Boolean =
        if (Build.VERSION.SDK_INT >= 30) Environment.isExternalStorageManager()
        else getPermissionState("legacy") == com.getcapacitor.PermissionState.GRANTED

    @PluginMethod
    fun checkAccess(call: PluginCall) {
        call.resolve(JSObject().put("granted", hasAccess()).put("sdk", Build.VERSION.SDK_INT).put("media", mediaPermissions()))
    }

    /** 'granted' | 'denied' | 'prompt' (…-with-rationale is reported as 'prompt'); not applicable on this SDK → 'granted' */
    private fun perm(alias: String, minSdk: Int): String {
        if (Build.VERSION.SDK_INT < minSdk) return "granted"
        return when (getPermissionState(alias)) {
            PermissionState.GRANTED -> "granted"
            PermissionState.DENIED -> "denied"
            else -> "prompt"
        }
    }

    private fun mediaPermissions(): JSObject = JSObject()
        .put("media", perm("media", 33))
        .put("location", perm("location", 29))
        .put("notifications", perm("notifications", 33))

    /**
     * Runtime permissions used by the gallery: READ_MEDIA_IMAGES/VIDEO (33+), ACCESS_MEDIA_LOCATION
     * (29+, un-redacted GPS) and POST_NOTIFICATIONS (33+). Resolves with their states.
     */
    @PluginMethod
    fun requestMediaPermissions(call: PluginCall) {
        val want = ArrayList<String>()
        if (Build.VERSION.SDK_INT >= 33 && getPermissionState("media") != PermissionState.GRANTED) want.add("media")
        if (Build.VERSION.SDK_INT >= 29 && getPermissionState("location") != PermissionState.GRANTED) want.add("location")
        if (Build.VERSION.SDK_INT >= 33 && getPermissionState("notifications") != PermissionState.GRANTED) want.add("notifications")
        if (want.isEmpty()) return call.resolve(mediaPermissions())
        requestPermissionForAliases(want.toTypedArray(), call, "mediaPermsCallback")
    }

    @PermissionCallback
    private fun mediaPermsCallback(call: PluginCall) {
        call.resolve(mediaPermissions())
    }

    /**
     * Roots the /_peek/ media server may serve (library folders + temp sessions). Until the first
     * call the storage roots of roots() are allowed. App cache/files dirs and folders shared with a
     * PC are always allowed. paths: null/absent = back to the default.
     */
    @PluginMethod
    fun setAllowedRoots(call: PluginCall) {
        val arr = call.getArray("paths")
        media.setAllowedRoots(arr?.toList<String>())
        call.resolve()
    }

    // ───────────────────────── metadata index (grouping) ─────────────────────────

    @PluginMethod
    fun indexSync(call: PluginCall) {
        val paths = call.getArray("paths")?.toList<String>() ?: return call.reject("paths required")
        val idx = PeekIndex.get(context)
        idx.sync(paths)
        call.resolve(idx.status())
    }

    @PluginMethod
    fun indexStatus(call: PluginCall) = call.resolve(PeekIndex.get(context).status())

    @PluginMethod
    fun indexQuery(call: PluginCall) = bg(call) {
        val paths = call.getArray("paths")?.toList<String>() ?: return@bg call.reject("paths required")
        call.resolve(PeekIndex.get(context).query(paths))
    }

    // ───────────────────────── native transfers ─────────────────────────

    @PluginMethod
    fun downloadTo(call: PluginCall) {
        val items = call.getArray("items") ?: return call.reject("items required")
        val dest = call.getString("dest") ?: return call.reject("dest required")
        val id = transfers.download(call.getString("id"), items, dest, call.getObject("headers"))
        call.resolve(JSObject().put("id", id))
    }

    @PluginMethod
    fun uploadPaths(call: PluginCall) {
        val url = call.getString("url") ?: return call.reject("url required")
        if (!url.startsWith("http://") && !url.startsWith("https://")) return call.reject("http/https only")
        val folderId = call.getString("folderId") ?: return call.reject("folderId required")
        val paths = call.getArray("paths") ?: return call.reject("paths required")
        val id = transfers.upload(call.getString("id"), url, folderId, call.getString("dir", "")!!, paths, call.getString("zip"), call.getObject("headers"))
        call.resolve(JSObject().put("id", id))
    }

    @PluginMethod
    fun cancelTransfer(call: PluginCall) {
        transfers.cancel(call.getString("id") ?: "")
        call.resolve()
    }

    @PluginMethod
    fun requestAccess(call: PluginCall) {
        if (hasAccess()) return call.resolve(JSObject().put("granted", true))
        if (Build.VERSION.SDK_INT >= 30) {
            try {
                val i = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${context.packageName}"))
                activity.startActivity(i)
            } catch (_: ActivityNotFoundException) {
                activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
            }
            call.resolve(JSObject().put("granted", false).put("pending", true))
        } else {
            requestPermissionForAlias("legacy", call, "accessCallback")
        }
    }

    @com.getcapacitor.annotation.PermissionCallback
    private fun accessCallback(call: PluginCall) {
        call.resolve(JSObject().put("granted", hasAccess()))
    }

    @PluginMethod
    fun roots(call: PluginCall) = bg(call) {
        call.resolve(JSObject().put("roots", PeekFs.storageRoots(context)).put("cacheDir", context.cacheDir.absolutePath).put("filesDir", context.filesDir.absolutePath))
    }

    // ───────────────────────── listing ─────────────────────────

    @PluginMethod
    fun list(call: PluginCall) = bg(call) {
        val path = call.getString("path") ?: return@bg call.reject("path required")
        call.resolve(PeekFs.list(context, path, call.getBoolean("recursive", false)!!, call.getBoolean("showHidden", false)!!, call.getInt("limit", 50000)!!))
    }

    @PluginMethod
    fun listDirs(call: PluginCall) = bg(call) {
        val path = call.getString("path") ?: return@bg call.reject("path required")
        call.resolve(JSObject().put("dirs", PeekFs.listDirs(path)))
    }

    @PluginMethod
    fun details(call: PluginCall) = bg(call) {
        val path = call.getString("path") ?: return@bg call.reject("path required")
        val f = File(path)
        val k = PeekFs.kind(f.name) ?: "image"
        val o = PeekFs.fileJson(f, k, null)
        if (k == "image") PeekFs.exifJson(path)?.let { o.put("exif", it) }
        call.resolve(o)
    }

    // ───────────────────────── file operations ─────────────────────────

    @PluginMethod
    fun mkdir(call: PluginCall) = bg(call) {
        val dir = File(call.getString("path") ?: return@bg call.reject("path required"))
        val name = try { PathGuard.cleanName(call.getString("name") ?: "") } catch (_: LinkError) { return@bg call.reject("Invalid folder name") }
        if (!dir.isDirectory) return@bg call.reject("Parent folder not found")
        val f = PeekFs.uniqueFile(dir, name)
        if (!f.mkdirs() && !f.isDirectory) return@bg call.reject("Cannot create folder")
        call.resolve(JSObject().put("path", f.absolutePath).put("name", f.name))
    }

    @PluginMethod
    fun rename(call: PluginCall) = bg(call) {
        val f = File(call.getString("path")!!)
        val dest = File(f.parentFile, call.getString("name")!!)
        if (dest.exists() && !dest.absolutePath.equals(f.absolutePath, true)) return@bg call.reject("A file with that name already exists")
        if (!f.renameTo(dest)) return@bg call.reject("Rename failed")
        PeekFs.scan(context, f.absolutePath, dest.absolutePath)
        call.resolve(JSObject().put("path", dest.absolutePath))
    }

    @PluginMethod
    fun move(call: PluginCall) = bg(call) {
        val paths = call.getArray("paths")!!.toList<String>()
        val dest = File(call.getString("dest")!!)
        val copy = call.getBoolean("copy", false)!!
        dest.mkdirs()
        val out = JSArray()
        for (p in paths) {
            val src = File(p)
            if (!src.exists()) continue
            val target = PeekFs.uniqueFile(dest, src.name)
            if (copy) {
                if (src.isDirectory) src.copyRecursively(target) else src.copyTo(target)
            } else PeekFs.move(src, target)
            PeekFs.scan(context, src.absolutePath, target.absolutePath)
            out.put(target.absolutePath)
        }
        call.resolve(JSObject().put("paths", out))
    }

    @PluginMethod
    fun remove(call: PluginCall) = bg(call) {
        val paths = call.getArray("paths")!!.toList<String>()
        val permanent = call.getBoolean("permanent", false)!!
        for (p in paths) {
            val f = File(p)
            if (!f.exists()) continue
            if (permanent) f.deleteRecursively() else PeekFs.trash(f)
            PeekFs.scan(context, p)
        }
        call.resolve(JSObject().put("deleted", paths.size))
    }

    /** Append base64 data to a file (chunked uploads / font imports / job assets). */
    @PluginMethod
    fun writeChunk(call: PluginCall) = bg(call) {
        val f = File(call.getString("path")!!)
        f.parentFile?.mkdirs()
        val data = Base64.decode(call.getString("data") ?: "", Base64.DEFAULT)
        FileOutputStream(f, call.getBoolean("append", false)!!).use { it.write(data) }
        call.resolve()
    }

    @PluginMethod
    fun finishFile(call: PluginCall) = bg(call) {
        val tmp = File(call.getString("tmp")!!)
        val dir = File(call.getString("dir")!!)
        val dest = PeekFs.uniqueFile(dir, call.getString("name")!!)
        dir.mkdirs()
        PeekFs.move(tmp, dest)
        PeekFs.scan(context, dest.absolutePath)
        call.resolve(JSObject().put("path", dest.absolutePath))
    }

    /** Save an edited image (PNG/JPEG base64) as a copy next to the source, or as a new file in a folder. */
    @PluginMethod
    fun saveImage(call: PluginCall) = bg(call) {
        val src = File(call.getString("tmp")!!)
        val format = call.getString("format", "jpg")!!
        val quality = call.getInt("quality", 92)!!
        val source = call.getString("source")
        val dest: File = if (source != null) {
            val s = File(source)
            PeekFs.uniqueEditFile(s.parentFile!!, s.name.substringBeforeLast('.'), call.getString("suffix", "_edit")!!, format)
        } else {
            val dir = File(call.getString("dir")!!)
            dir.mkdirs()
            PeekFs.uniqueFile(dir, "${call.getString("name", "PeekAround")}.$format")
        }
        val bmp = BitmapFactory.decodeFile(src.absolutePath) ?: return@bg call.reject("Cannot decode image")
        val cf = when (format) {
            "png" -> Bitmap.CompressFormat.PNG
            "webp" -> if (Build.VERSION.SDK_INT >= 30) Bitmap.CompressFormat.WEBP_LOSSY else @Suppress("DEPRECATION") Bitmap.CompressFormat.WEBP
            else -> Bitmap.CompressFormat.JPEG
        }
        FileOutputStream(dest).use { bmp.compress(cf, quality, it) }
        bmp.recycle()
        src.delete()
        if (source != null && format == "jpg") PeekFs.copyExif(source, dest.absolutePath)
        PeekFs.scan(context, dest.absolutePath)
        call.resolve(JSObject().put("path", dest.absolutePath))
    }

    @PluginMethod
    fun extractZip(call: PluginCall) = bg(call) {
        val zip = File(call.getString("zip")!!)
        val parent = File(call.getString("dest")!!)
        val dest = if (call.getBoolean("unique", true)!!) PeekFs.uniqueFile(parent, zip.name.substringBeforeLast('.')) else parent
        val n = PeekFs.extractZip(zip, dest)
        PeekFs.scan(context, dest.absolutePath)
        call.resolve(JSObject().put("path", dest.absolutePath).put("files", n))
    }

    @PluginMethod
    fun cacheSize(call: PluginCall) = bg(call) { call.resolve(JSObject().put("bytes", media.size())) }

    @PluginMethod
    fun clearCache(call: PluginCall) = bg(call) {
        media.clear()
        call.resolve()
    }

    @PluginMethod
    fun share(call: PluginCall) {
        val paths = call.getArray("paths")!!.toList<String>()
        val uris = ArrayList(paths.map { shareUri(File(it)) ?: return call.reject("Cannot share ${File(it).name}") })
        val i = if (uris.size == 1) Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris[0])
        else Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
        i.type = call.getString("mime", "*/*")
        i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        activity.startActivity(Intent.createChooser(i, null))
        call.resolve()
    }

    /** FileProvider (primary storage / app dirs); files elsewhere (SD card) via their MediaStore URI. */
    private fun shareUri(f: File): Uri? {
        try {
            return FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", f)
        } catch (_: IllegalArgumentException) {
        }
        val id = media.mediaId(f) ?: return null
        return android.content.ContentUris.withAppendedId(MediaStore.Files.getContentUri("external"), id)
    }

    // ───────────────────────── live folder watching ─────────────────────────

    @PluginMethod
    fun watch(call: PluginCall) {
        observer?.stopWatching()
        val path = call.getString("path")
        if (path == null) {
            observer = null
            return call.resolve()
        }
        var pending = false
        val mask = FileObserver.CREATE or FileObserver.DELETE or FileObserver.MOVED_FROM or FileObserver.MOVED_TO or FileObserver.CLOSE_WRITE
        val cb: (String?) -> Unit = {
            if (!pending) {
                pending = true
                main.postDelayed({
                    pending = false
                    notifyListeners("changed", JSObject().put("path", path))
                }, 600)
            }
        }
        observer = if (Build.VERSION.SDK_INT >= 29) object : FileObserver(File(path), mask) {
            override fun onEvent(event: Int, p: String?) = cb(p)
        } else @Suppress("DEPRECATION") object : FileObserver(path, mask) {
            override fun onEvent(event: Int, p: String?) = cb(p)
        }
        observer!!.startWatching()
        call.resolve()
    }

    // ───────────────────────── jobs (Media3) ─────────────────────────

    @PluginMethod
    fun videoEdit(call: PluginCall) = bg(call) {
        val id = UUID.randomUUID().toString()
        val spec = JSONObject(call.data.toString())
        val dest = jobs.videoEdit(id, spec, call.getString("suffix", "_edit")!!)
        call.resolve(JSObject().put("id", id).put("path", dest.absolutePath))
    }

    @PluginMethod
    fun slideshow(call: PluginCall) = bg(call) {
        val id = UUID.randomUUID().toString()
        val dest = jobs.slideshow(id, JSONObject(call.data.toString()))
        call.resolve(JSObject().put("id", id).put("path", dest.absolutePath))
    }

    @PluginMethod
    fun cancelJob(call: PluginCall) {
        jobs.cancel(call.getString("id")!!)
        call.resolve()
    }

    // ───────────────────────── server discovery (mDNS) ─────────────────────────

    @PluginMethod
    fun discover(call: PluginCall) {
        val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        val lock = wifi.createMulticastLock("peekaround").apply { setReferenceCounted(false); acquire() }
        val found = JSArray()
        val seen = HashSet<String>()
        val timeout = call.getInt("timeout", 4000)!!.toLong()
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}
            override fun onDiscoveryStopped(serviceType: String) {}
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {}
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
            override fun onServiceLost(serviceInfo: NsdServiceInfo) {}
            override fun onServiceFound(info: NsdServiceInfo) {
                @Suppress("DEPRECATION")
                nsd.resolveService(info, object : NsdManager.ResolveListener {
                    override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {}
                    override fun onServiceResolved(s: NsdServiceInfo) {
                        @Suppress("DEPRECATION")
                        val host = s.host?.hostAddress ?: return
                        if (host.contains(':')) return // skip IPv6 link-local
                        val key = "$host:${s.port}"
                        synchronized(seen) {
                            if (!seen.add(key)) return
                            found.put(JSObject().put("name", s.serviceName).put("host", host).put("port", s.port).put("url", "http://$host:${s.port}"))
                        }
                    }
                })
            }
        }
        try {
            nsd.discoverServices("_peekaround._tcp.", NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: Exception) {
            lock.release()
            return call.resolve(JSObject().put("servers", found))
        }
        main.postDelayed({
            try { nsd.stopServiceDiscovery(listener) } catch (_: Exception) {}
            lock.release()
            call.resolve(JSObject().put("servers", found))
        }, timeout)
    }

    // ───────────────────────── "Share → PeekAround" ─────────────────────────

    @PluginMethod
    fun getShared(call: PluginCall) {
        val a = JSArray()
        for (u in pendingShare) {
            var name = u.lastPathSegment ?: "file"
            var size = -1L
            try {
                context.contentResolver.query(u, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                    if (c.moveToFirst()) {
                        name = c.getString(0) ?: name
                        if (!c.isNull(1)) size = c.getLong(1)
                    }
                }
            } catch (_: Exception) {
            }
            a.put(JSObject().put("uri", u.toString()).put("name", name).put("size", size).put("mime", context.contentResolver.getType(u)))
        }
        call.resolve(JSObject().put("files", a).put("text", pendingText))
    }

    @PluginMethod
    fun clearShared(call: PluginCall) {
        pendingShare.clear()
        pendingText = null
        call.resolve()
    }

    /** Stream shared files to a PeekAround server (PUT /api/upload) with progress events. */
    @PluginMethod
    fun uploadShared(call: PluginCall) = bg(call) {
        val base = call.getString("url")!!.trimEnd('/')
        val folderId = call.getString("folderId")!!
        val dir = call.getString("dir", "")!!
        val zip = call.getString("zip", "keep")!!
        val files = call.getArray("files")!!
        var done = 0
        for (i in 0 until files.length()) {
            val f = files.getJSONObject(i)
            val uri = Uri.parse(f.getString("uri"))
            val name = f.getString("name")
            val q = "folderId=${enc(folderId)}&dir=${enc(dir)}&path=${enc(name)}&zip=${enc(zip)}"
            val conn = URL("$base/api/upload?$q").openConnection() as HttpURLConnection
            conn.requestMethod = "PUT"
            conn.doOutput = true
            conn.setRequestProperty("Content-Type", "application/octet-stream")
            conn.setRequestProperty("X-Peek", "1")
            call.getObject("headers")?.let { h -> h.keys().forEach { k -> conn.setRequestProperty(k, h.optString(k)) } }
            val size = f.optLong("size", -1)
            conn.setChunkedStreamingMode(256 * 1024) // provider SIZE values are not always exact
            PeekFs.openOriginal(context, uri).use { input ->
                conn.outputStream.use { out ->
                    val buf = ByteArray(256 * 1024)
                    var sent = 0L
                    var last = 0L
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        sent += n
                        val now = System.currentTimeMillis()
                        if (now - last > 250) {
                            last = now
                            notifyListeners("uploadProgress", JSObject().put("index", i).put("name", name).put("sent", sent).put("size", size).put("total", files.length()))
                        }
                    }
                }
            }
            val code = conn.responseCode
            conn.disconnect()
            if (code !in 200..299) return@bg call.reject("Upload failed ($code): $name")
            done++
        }
        pendingShare.clear()
        call.resolve(JSObject().put("uploaded", done))
    }

    /** Local mode: copy shared files into a folder on the phone. */
    @PluginMethod
    fun importShared(call: PluginCall) = bg(call) {
        val dir = File(call.getString("dir")!!)
        dir.mkdirs()
        val files = call.getArray("files")!!
        val out = JSArray()
        for (i in 0 until files.length()) {
            val f = files.getJSONObject(i)
            val dest = PeekFs.uniqueFile(dir, f.getString("name"))
            PeekFs.openOriginal(context, Uri.parse(f.getString("uri"))).use { input -> FileOutputStream(dest).use { input.copyTo(it, 256 * 1024) } }
            PeekFs.scan(context, dest.absolutePath)
            out.put(dest.absolutePath)
        }
        pendingShare.clear()
        call.resolve(JSObject().put("paths", out))
    }

    private fun enc(s: String) = URLEncoder.encode(s, "UTF-8")
}
