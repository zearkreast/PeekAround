package app.peekaround

import android.Manifest
import android.os.Build
import com.getcapacitor.JSArray
import com.getcapacitor.JSObject
import com.getcapacitor.PermissionState
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin
import com.getcapacitor.annotation.Permission
import com.getcapacitor.annotation.PermissionCallback
import java.io.File
import java.net.URI

/**
 * "Share folders with a PC" (docs/link-protocol.md). TS: packages/web/src/backend/android/link-plugin.ts
 *   status() · start({url, folders, name?}) · stop() · setFolders({folders}) · setName({name}) · forget()
 *   event 'link' → same shape as status()
 */
@CapacitorPlugin(
    name = "PeekLink",
    permissions = [Permission(alias = "notifications", strings = [Manifest.permission.POST_NOTIFICATIONS])],
)
class PeekLinkPlugin : Plugin() {
    private lateinit var prefs: LinkPrefs
    private val listener: () -> Unit = { notifyListeners("link", status()) }

    override fun load() {
        prefs = LinkPrefs(context)
        LinkHub.addListener(listener)
    }

    override fun handleOnDestroy() {
        LinkHub.removeListener(listener)
    }

    /** The UI is visible again: bring the service back if sharing is on but it was stopped (process death, FGS refusal). */
    override fun handleOnResume() {
        if (prefs.enabled && !PeekLinkService.running && prefs.url != null && prefs.folders.isNotEmpty()) PeekLinkService.start(context)
    }

    private fun status(): JSObject {
        val o = JSObject()
        val enabled = prefs.enabled
        val st = LinkHub.state.let { if (!enabled && (it == "connecting" || it == "online" || it == "pending")) "off" else it }
        o.put("enabled", enabled)
        o.put("state", st)
        if (st == "pending") o.put("code", LinkHub.code)
        o.put("server", prefs.url)
        o.put("serverName", prefs.serverName)
        o.put("deviceId", prefs.deviceId)
        o.put("deviceName", prefs.deviceName)
        o.put("model", prefs.model)
        // paired → token for the web's own API calls to that server (Authorization / query param)
        prefs.token?.let { o.put("token", it) }
        o.put("paired", prefs.token != null)
        o.put("active", LinkHub.active)
        val a = JSArray()
        prefs.folders.forEach { a.put(JSObject().put("key", it.key).put("name", it.name).put("path", it.path)) }
        o.put("folders", a)
        LinkHub.error?.let { o.put("error", it) }
        return o
    }

    @PluginMethod
    fun status(call: PluginCall) = call.resolve(status())

    private fun parseFolders(call: PluginCall): List<LinkFolder>? {
        val arr = call.getArray("folders") ?: return null
        val input = ArrayList<Pair<String, String>>()
        for (i in 0 until arr.length()) {
            val f = arr.optJSONObject(i) ?: continue
            val path = f.optString("path", "").trimEnd('/')
            if (path.isEmpty()) continue
            val dir = File(path)
            if (!dir.isDirectory) {
                call.reject("Not a folder: $path")
                return null
            }
            input.add(f.optString("name", "").trim().take(120) to dir.absolutePath)
        }
        if (input.size > 64) {
            call.reject("At most 64 folders")
            return null
        }
        return prefs.assignKeys(input)
    }

    private fun normalizeUrl(s: String?): String? {
        val u = s?.trim()?.trimEnd('/') ?: return null
        return try {
            val uri = URI(u)
            if (uri.scheme != "http" && uri.scheme != "https") return null
            if (uri.host.isNullOrEmpty() || uri.userInfo != null) return null
            "${uri.scheme}://${uri.rawAuthority}${uri.rawPath ?: ""}".trimEnd('/')
        } catch (_: Exception) {
            null
        }
    }

    /** start({url, folders:[{name,path}], name?}) — asks for the notification permission first (Android 13+). */
    @PluginMethod
    fun start(call: PluginCall) {
        val url = normalizeUrl(call.getString("url")) ?: return call.reject("Invalid server URL (http/https only)")
        val folders = parseFolders(call) ?: if (call.getArray("folders") == null) return call.reject("folders required") else return
        if (folders.isEmpty()) return call.reject("Choose at least one folder")
        call.getString("name")?.let { prefs.deviceName = it }
        if (prefs.url != url) {
            // a token belongs to one server
            prefs.token = null
            prefs.serverName = null
        }
        prefs.url = url
        prefs.folders = folders
        prefs.enabled = true
        PeekMediaServer.get(context).setLinkRoots(folders.map { it.path })
        if (Build.VERSION.SDK_INT >= 33 && getPermissionState("notifications") != PermissionState.GRANTED) {
            requestPermissionForAlias("notifications", call, "startAfterPermission")
        } else launch(call)
    }

    @PermissionCallback
    private fun startAfterPermission(call: PluginCall) = launch(call) // without the permission the service still runs, the notification is just hidden

    private fun launch(call: PluginCall) {
        LinkHub.set("connecting")
        if (PeekLinkService.running) {
            PeekLinkService.command(context, PeekLinkService.ACTION_FOLDERS)
            PeekLinkService.command(context, PeekLinkService.ACTION_RECONNECT)
        } else if (!PeekLinkService.start(context)) {
            LinkHub.set("error", error = "cannot start service")
            return call.reject("Cannot start the sharing service")
        }
        call.resolve(status())
    }

    @PluginMethod
    fun stop(call: PluginCall) {
        prefs.enabled = false
        PeekLinkService.stop(context)
        LinkHub.set("off")
        call.resolve(status())
    }

    @PluginMethod
    fun setFolders(call: PluginCall) {
        val folders = parseFolders(call) ?: if (call.getArray("folders") == null) return call.reject("folders required") else return
        prefs.folders = folders
        PeekMediaServer.get(context).setLinkRoots(folders.map { it.path })
        if (folders.isEmpty() && prefs.enabled) {
            prefs.enabled = false
            PeekLinkService.stop(context)
        } else PeekLinkService.command(context, PeekLinkService.ACTION_FOLDERS)
        call.resolve(status())
    }

    @PluginMethod
    fun setName(call: PluginCall) {
        prefs.deviceName = call.getString("name") ?: ""
        PeekLinkService.command(context, PeekLinkService.ACTION_RECONNECT)
        call.resolve(status())
    }

    /** Drop the pairing token (next connection asks the PC user again). */
    @PluginMethod
    fun forget(call: PluginCall) {
        prefs.token = null
        prefs.serverName = null
        PeekLinkService.command(context, PeekLinkService.ACTION_RECONNECT)
        call.resolve(status())
    }
}
