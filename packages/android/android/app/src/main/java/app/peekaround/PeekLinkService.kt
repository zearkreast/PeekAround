package app.peekaround

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

/**
 * Foreground service (type connectedDevice) that keeps the phone ↔ PC link alive while the user
 * shares folders. Started ONLY from the visible UI (PeekLink.start / resume on activity resume);
 * it never restarts itself from the background: when the PC is unreachable it stays alive in a
 * "waiting for the PC" state and reconnects with backoff / on network changes.
 * A Wi-Fi lock (low latency) and a partial wake lock are held only while transfers run.
 */
class PeekLinkService : Service() {
    private var client: LinkClient? = null
    private val main = Handler(Looper.getMainLooper())
    private var wifiLock: WifiManager.WifiLock? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var lastText: String? = null
    private val listener: () -> Unit = { main.post { updateNotification() } }

    companion object {
        const val ACTION_START = "app.peekaround.link.START"
        const val ACTION_STOP = "app.peekaround.link.STOP"
        const val ACTION_FOLDERS = "app.peekaround.link.FOLDERS"
        const val ACTION_RECONNECT = "app.peekaround.link.RECONNECT"
        private const val CHANNEL = "peek_link"
        private const val CHANNEL_ALERT = "peek_link_alert"
        private const val NOTIF_ID = 4711
        private const val NOTIF_ALERT_ID = 4712
        private const val TAG = "PeekLinkService"

        @Volatile var running = false
            private set

        /** From the visible UI only (FGS start restrictions). Returns false when the system refused. */
        fun start(ctx: Context, action: String = ACTION_START): Boolean = try {
            ContextCompat.startForegroundService(ctx, Intent(ctx, PeekLinkService::class.java).setAction(action))
            true
        } catch (e: Exception) {
            Log.w(TAG, "cannot start link service", e)
            false
        }

        /** Deliver a command to a running service (never starts it). */
        fun command(ctx: Context, action: String) {
            if (!running) return
            try {
                ctx.startService(Intent(ctx, PeekLinkService::class.java).setAction(action))
            } catch (e: Exception) {
                Log.w(TAG, "cannot reach link service", e)
            }
        }

        fun stop(ctx: Context) {
            if (running) command(ctx, ACTION_STOP)
            else LinkHub.set("off")
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        createChannels()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val prefs = LinkPrefs(this)
        when (intent?.action) {
            ACTION_STOP -> {
                prefs.enabled = false
                shutdown()
                LinkHub.set("off")
                return START_NOT_STICKY
            }
            ACTION_FOLDERS -> if (client != null) {
                client?.foldersChanged()
                updateNotification()
                return START_STICKY
            }
            ACTION_RECONNECT -> if (client != null) {
                client?.reconnect()
                return START_STICKY
            }
        }
        // ACTION_START, or a sticky restart (intent == null) after the process was killed
        if (!prefs.enabled || prefs.url == null || prefs.folders.isEmpty()) {
            if (intent?.action == ACTION_START) goForeground() // startForegroundService() demands it even when we stop at once
            shutdown()
            if (!prefs.enabled) LinkHub.set("off")
            return START_NOT_STICKY
        }
        if (!goForeground()) {
            // e.g. a sticky restart while the app is in the background on Android 12+: stop cleanly,
            // the UI starts us again the next time it is opened (prefs.enabled stays true)
            shutdown()
            return START_NOT_STICKY
        }
        if (client == null) {
            LinkHub.addListener(listener)
            client = LinkClient(this, { n -> main.post { onActive(n) } }, { main.post { fatal() } }).also { it.start() }
            running = true
        }
        return START_STICKY
    }

    private fun goForeground(): Boolean = try {
        val n = buildNotification()
        if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIF_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        else startForeground(NOTIF_ID, n)
        true
    } catch (e: Exception) {
        Log.w(TAG, "startForeground refused", e)
        false
    }

    private fun shutdown() {
        running = false
        LinkHub.removeListener(listener)
        client?.shutdown()
        client = null
        onActive(0)
        try {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } catch (_: Exception) {
        }
        stopSelf()
    }

    /** rejected / forgotten / unsupported: stop and tell the user (the UI shows LinkHub state) */
    private fun fatal() {
        val st = LinkHub.state
        val err = LinkHub.error
        shutdown()
        LinkHub.set(st, error = err)
        val text = getString(if (st == "rejected") R.string.link_rejected else if (err == "forgotten") R.string.link_forgotten else R.string.link_failed)
        notifyAlert(text)
    }

    override fun onDestroy() {
        if (client != null) shutdown()
        super.onDestroy()
    }

    // ───────────────────────── wifi / wake locks during transfers ─────────────────────────

    @SuppressLint("WakelockTimeout")
    private fun onActive(n: Int) {
        LinkHub.setActive(n)
        if (n > 0) {
            if (wifiLock == null) {
                val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
                @Suppress("DEPRECATION")
                val mode = if (Build.VERSION.SDK_INT >= 29) WifiManager.WIFI_MODE_FULL_LOW_LATENCY else WifiManager.WIFI_MODE_FULL_HIGH_PERF
                wifiLock = try { wm.createWifiLock(mode, "PeekAround:link").apply { setReferenceCounted(false); acquire() } } catch (_: Exception) { null }
            }
            if (wakeLock == null) {
                val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
                wakeLock = try { pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "PeekAround:link").apply { setReferenceCounted(false); acquire(30 * 60_000L) } } catch (_: Exception) { null }
            }
        } else {
            try { wifiLock?.release() } catch (_: Exception) {}
            try { wakeLock?.release() } catch (_: Exception) {}
            wifiLock = null
            wakeLock = null
        }
    }

    // ───────────────────────── notification ─────────────────────────

    private fun createChannels() {
        if (Build.VERSION.SDK_INT < 26) return
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, getString(R.string.link_channel), NotificationManager.IMPORTANCE_LOW).apply { setShowBadge(false) })
        nm.createNotificationChannel(NotificationChannel(CHANNEL_ALERT, getString(R.string.link_channel_alert), NotificationManager.IMPORTANCE_DEFAULT))
    }

    private fun contentText(): String {
        val prefs = LinkPrefs(this)
        val pc = prefs.serverName ?: prefs.url?.substringAfter("://")?.substringBefore('/') ?: "PC"
        val n = prefs.folders.size
        return when (LinkHub.state) {
            "online" -> resources.getQuantityString(R.plurals.link_sharing, n, n, pc)
            "pending" -> getString(R.string.link_pending, LinkHub.code ?: "")
            else -> getString(R.string.link_waiting, pc)
        }
    }

    private fun buildNotification(): Notification {
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val stop = PendingIntent.getService(this, 1, Intent(this, PeekLinkService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val text = contentText()
        lastText = text
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_notify_sync_noanim)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(open)
            .addAction(0, getString(R.string.link_stop), stop)
            .build()
    }

    @SuppressLint("MissingPermission")
    private fun updateNotification() {
        if (!running) return
        if (contentText() == lastText) return
        try {
            if (NotificationManagerCompat.from(this).areNotificationsEnabled()) NotificationManagerCompat.from(this).notify(NOTIF_ID, buildNotification())
        } catch (_: Exception) {
        }
    }

    @SuppressLint("MissingPermission")
    private fun notifyAlert(text: String) {
        try {
            val nm = NotificationManagerCompat.from(this)
            if (!nm.areNotificationsEnabled()) return
            val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
                PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            }
            nm.notify(NOTIF_ALERT_ID, NotificationCompat.Builder(this, CHANNEL_ALERT)
                .setSmallIcon(android.R.drawable.stat_notify_error)
                .setContentTitle(getString(R.string.app_name))
                .setContentText(text)
                .setAutoCancel(true)
                .setContentIntent(open)
                .build())
        } catch (_: Exception) {
        }
    }
}
