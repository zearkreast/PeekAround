package app.peekaround

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.media3.common.util.UnstableApi
import com.getcapacitor.BridgeActivity

@UnstableApi
class MainActivity : BridgeActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        registerPlugin(PeekLocalPlugin::class.java)
        // system back / predictive back → web back stack (see PeekBackPlugin)
        registerPlugin(PeekBackPlugin::class.java)
        // phone ↔ PC folder sharing (PeekLinkService)
        registerPlugin(PeekLinkPlugin::class.java)
        super.onCreate(savedInstanceState)
        handleShare(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleShare(intent)
    }

    /** "Share → PeekAround" from the system share sheet. */
    private fun handleShare(intent: Intent?) {
        if (intent == null) return
        val uris = ArrayList<Uri>()
        when (intent.action) {
            Intent.ACTION_SEND -> {
                if (intent.type?.startsWith("text/") == true && !intent.hasExtra(Intent.EXTRA_STREAM)) {
                    // shared text, e.g. a mega.nz link from the MEGA app → getShared().text
                    val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.trim()?.take(8192)
                    if (text.isNullOrEmpty()) return
                    PeekLocalPlugin.pendingShare.clear()
                    PeekLocalPlugin.pendingText = text
                    PeekLocalPlugin.instance?.onShared()
                    return
                }
                val u: Uri? = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                else @Suppress("DEPRECATION") intent.getParcelableExtra(Intent.EXTRA_STREAM)
                u?.let { uris.add(it) }
            }
            Intent.ACTION_SEND_MULTIPLE -> {
                val list: ArrayList<Uri>? = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
                else @Suppress("DEPRECATION") intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
                list?.let { uris.addAll(it) }
            }
            else -> return
        }
        if (uris.isEmpty()) return
        PeekLocalPlugin.pendingShare.clear()
        PeekLocalPlugin.pendingText = null
        PeekLocalPlugin.pendingShare.addAll(uris)
        PeekLocalPlugin.instance?.onShared()
    }
}
