package app.peekaround

import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import android.provider.Settings
import org.json.JSONArray
import java.util.UUID

/**
 * Persistent state of the phone ↔ PC link (works without the WebView: the foreground service
 * reads everything from here). SharedPreferences "peek_link".
 */
class LinkPrefs(ctx: Context) {
    private val sp: SharedPreferences = ctx.applicationContext.getSharedPreferences("peek_link", Context.MODE_PRIVATE)
    private val appCtx = ctx.applicationContext

    var enabled: Boolean
        get() = sp.getBoolean("enabled", false)
        set(v) = sp.edit().putBoolean("enabled", v).apply()

    /** server base URL, e.g. http://192.168.1.20:47213 (no trailing slash) */
    var url: String?
        get() = sp.getString("url", null)
        set(v) = sp.edit().putString("url", v).apply()

    var token: String?
        get() = sp.getString("token", null)
        set(v) = sp.edit().putString("token", v).apply()

    var serverName: String?
        get() = sp.getString("serverName", null)
        set(v) = sp.edit().putString("serverName", v).apply()

    /** UUID generated once per install */
    val deviceId: String
        get() {
            sp.getString("deviceId", null)?.let { return it }
            val id = UUID.randomUUID().toString()
            sp.edit().putString("deviceId", id).apply()
            return id
        }

    var deviceName: String
        get() = sp.getString("deviceName", null) ?: defaultName()
        set(v) = sp.edit().putString("deviceName", v.trim().take(80).ifEmpty { defaultName() }).apply()

    val model: String get() = "${Build.MANUFACTURER.replaceFirstChar { it.uppercase() }} ${Build.MODEL}".trim()

    var folders: List<LinkFolder>
        get() {
            val s = sp.getString("folders", null) ?: return emptyList()
            return try {
                val a = JSONArray(s)
                (0 until a.length()).mapNotNull { i ->
                    val o = a.optJSONObject(i) ?: return@mapNotNull null
                    LinkFolder(o.optString("key"), o.optString("name"), o.optString("path"))
                }.filter { LinkCodec.KEY.matches(it.key) && it.path.isNotEmpty() }
            } catch (_: Exception) {
                emptyList()
            }
        }
        set(v) = sp.edit().putString("folders", LinkCodec.foldersJson(v).toString()).apply()

    /** New folder list from the UI: keeps the key of a path that was already shared, new keys otherwise. */
    fun assignKeys(input: List<Pair<String, String>>): List<LinkFolder> {
        val old = folders.associateBy { it.path }
        val used = HashSet<String>()
        val out = ArrayList<LinkFolder>()
        for ((name, path) in input) {
            if (out.any { it.path == path }) continue
            var key = old[path]?.key
            if (key == null || key in used) {
                do key = "k" + UUID.randomUUID().toString().replace("-", "").take(8) while (key in used)
            }
            used.add(key!!)
            out.add(LinkFolder(key, name.ifEmpty { path.substringAfterLast('/') }, path))
        }
        return out
    }

    private fun defaultName(): String {
        val n = try {
            if (Build.VERSION.SDK_INT >= 25) Settings.Global.getString(appCtx.contentResolver, Settings.Global.DEVICE_NAME) else null
        } catch (_: Exception) {
            null
        }
        return n?.takeIf { it.isNotBlank() } ?: model
    }
}
