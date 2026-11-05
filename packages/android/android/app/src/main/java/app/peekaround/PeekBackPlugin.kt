package app.peekaround

import androidx.activity.OnBackPressedCallback
import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin

/**
 * System back (button / gesture, predictive back on targetSdk 36) → JS back stack.
 *
 * The web UI keeps a back stack (viewer, sheets, selection mode, in-app history) and calls
 * setBackEnabled(depth > 0) whenever it changes. While enabled, this androidx
 * OnBackPressedCallback consumes back and emits a "back" event to JS; while disabled the
 * system default runs (predictive "back to home" animation, task moves to the back).
 * fallback() lets JS hand a back press it could not use to the system.
 */
@CapacitorPlugin(name = "PeekBack")
class PeekBackPlugin : Plugin() {
    private val callback = object : OnBackPressedCallback(false) {
        override fun handleOnBackPressed() {
            notifyListeners("back", JSObject())
        }
    }

    override fun load() {
        activity.runOnUiThread {
            activity.onBackPressedDispatcher.addCallback(activity, callback)
        }
    }

    @PluginMethod
    fun setBackEnabled(call: PluginCall) {
        val enabled = call.getBoolean("enabled", false) ?: false
        activity.runOnUiThread {
            callback.isEnabled = enabled
            call.resolve()
        }
    }

    @PluginMethod
    fun fallback(call: PluginCall) {
        activity.runOnUiThread {
            callback.isEnabled = false
            activity.onBackPressedDispatcher.onBackPressed()
            call.resolve()
        }
    }

    override fun handleOnDestroy() {
        callback.remove()
    }
}
