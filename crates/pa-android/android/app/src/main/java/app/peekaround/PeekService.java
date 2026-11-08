package app.peekaround;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.ContentUris;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.database.Cursor;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.media.MediaMetadataRetriever;
import android.media.MediaScannerConnection;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.Uri;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.provider.MediaStore;
import android.util.Log;
import android.util.Size;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Locale;
import java.util.TimeZone;

/**
 * Foreground service (type connectedDevice) that keeps the phone ↔ PC link running while the user
 * shares folders (1.x PeekLinkService). It runs in the ":link" process; the link itself is Rust
 * (pa-phone through crates/pa-android/src/service.rs) and reads its settings from files/link.json.
 * Started only from the visible UI; when the PC is unreachable it stays in a "waiting for the PC"
 * state and reconnects with backoff / when the network comes back. A Wi-Fi lock and a partial
 * wake lock are held only while requests are served.
 */
public class PeekService extends Service {
    static final String ACTION_START = "app.peekaround.link.START";
    static final String ACTION_STOP = "app.peekaround.link.STOP";
    static final String ACTION_FOLDERS = "app.peekaround.link.FOLDERS";
    private static final String CHANNEL = "peek_link";
    private static final String CHANNEL_ALERT = "peek_link_alert";
    private static final int NOTIF_ID = 4711;
    private static final int NOTIF_ALERT_ID = 4712;
    private static final String TAG = "PeekService";

    static {
        System.loadLibrary("peekaround_android");
    }

    private static native void nativeStart(String filesDir, PeekService service, String appVersion, String model);
    private static native void nativeStop();
    private static native void nativeFoldersChanged();
    private static native void nativeReconnect();

    private final Handler main = new Handler(Looper.getMainLooper());
    private boolean running;
    private String state = "connecting";
    private String code;
    private String lastText;
    private WifiManager.WifiLock wifiLock;
    private PowerManager.WakeLock wakeLock;
    private ConnectivityManager.NetworkCallback netCb;

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public void onCreate() {
        super.onCreate();
        createChannels();
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent != null ? intent.getAction() : null;
        if (ACTION_STOP.equals(action)) {
            setEnabled(false);
            shutdown();
            return START_NOT_STICKY;
        }
        if (ACTION_FOLDERS.equals(action)) {
            if (running) {
                nativeFoldersChanged();
                updateNotification();
                return START_STICKY;
            }
            stopSelf();
            return START_NOT_STICKY;
        }
        // ACTION_START, or a sticky restart (intent == null) after the process was killed
        if (!goForeground()) {
            // e.g. a sticky restart in the background on Android 12+: the UI starts us again later
            shutdown();
            return START_NOT_STICKY;
        }
        if (!running) {
            running = true;
            registerNetwork();
            nativeStart(getFilesDir().getAbsolutePath(), this, appVersion(), model());
        }
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        if (running) shutdown();
        super.onDestroy();
    }

    private String appVersion() {
        try {
            String v = getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
            return v != null ? v : "2.0.0";
        } catch (Exception e) {
            return "2.0.0";
        }
    }

    private String model() {
        String maker = Build.MANUFACTURER == null ? "" : Build.MANUFACTURER;
        if (!maker.isEmpty()) maker = maker.substring(0, 1).toUpperCase(Locale.ROOT) + maker.substring(1);
        return (maker + " " + Build.MODEL).trim();
    }

    private boolean goForeground() {
        try {
            Notification n = buildNotification();
            if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIF_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
            else startForeground(NOTIF_ID, n);
            return true;
        } catch (Exception e) {
            Log.w(TAG, "startForeground refused", e);
            return false;
        }
    }

    private void shutdown() {
        if (running) nativeStop();
        running = false;
        unregisterNetwork();
        onLinkActive(0);
        try {
            if (Build.VERSION.SDK_INT >= 24) stopForeground(STOP_FOREGROUND_REMOVE);
            else stopForeground(true);
        } catch (Exception ignored) {
        }
        stopSelf();
    }

    // ───────────────────────── link.json ─────────────────────────

    private JSONObject prefs() {
        File f = new File(getFilesDir(), "link.json");
        try (FileInputStream in = new FileInputStream(f)) {
            byte[] b = new byte[(int) f.length()];
            int n = 0;
            while (n < b.length) {
                int k = in.read(b, n, b.length - n);
                if (k < 0) break;
                n += k;
            }
            return new JSONObject(new String(b, 0, n, StandardCharsets.UTF_8));
        } catch (Exception e) {
            return new JSONObject();
        }
    }

    /** the Stop action: sharing off in link.json (the link would otherwise come back with the UI) */
    private void setEnabled(boolean on) {
        try {
            JSONObject o = prefs();
            o.put("enabled", on);
            File tmp = new File(getFilesDir(), "link.json.tmp");
            try (java.io.FileOutputStream out = new java.io.FileOutputStream(tmp)) {
                out.write(o.toString().getBytes(StandardCharsets.UTF_8));
            }
            //noinspection ResultOfMethodCallIgnored
            tmp.renameTo(new File(getFilesDir(), "link.json"));
        } catch (Exception e) {
            Log.w(TAG, "link.json", e);
        }
    }

    // ───────────────────────── called from Rust ─────────────────────────

    /** LinkStatus JSON: {"state","code","error","active"} */
    public void onLinkState(String json) {
        main.post(() -> {
            String error = null;
            try {
                JSONObject o = new JSONObject(json);
                state = o.optString("state", "connecting");
                code = o.isNull("code") ? null : o.optString("code", null);
                error = o.isNull("error") ? null : o.optString("error", null);
            } catch (Exception ignored) {
            }
            if (!running) return;
            // "error" is terminal (not configured / forgotten / unsupported server); retries stay "connecting"
            boolean fatal = "rejected".equals(state) || "off".equals(state) || "error".equals(state);
            if (fatal) {
                shutdown();
                if ("rejected".equals(state)) notifyAlert(getString(R.string.link_rejected));
                else if ("forgotten".equals(error)) notifyAlert(getString(R.string.link_forgotten));
                else if (!"off".equals(state)) notifyAlert(getString(R.string.link_failed));
                return;
            }
            updateNotification();
        });
    }

    public void onLinkActive(int n) {
        main.post(() -> setLocks(n > 0));
    }

    public void scanFile(String path) {
        MediaScannerConnection.scanFile(getApplicationContext(), new String[]{path}, null, null);
    }

    /** MediaStore's (usually cached) thumbnail: much faster than decoding, especially for videos */
    public byte[] systemThumb(String path, int video, int size) {
        if (Build.VERSION.SDK_INT < 29) return null;
        try {
            Uri base = video != 0 ? MediaStore.Video.Media.EXTERNAL_CONTENT_URI : MediaStore.Images.Media.EXTERNAL_CONTENT_URI;
            long id = -1;
            try (Cursor c = getContentResolver().query(base, new String[]{MediaStore.MediaColumns._ID},
                    MediaStore.MediaColumns.DATA + "=?", new String[]{path}, null)) {
                if (c != null && c.moveToFirst()) id = c.getLong(0);
            }
            if (id < 0) return null;
            Bitmap bmp = getContentResolver().loadThumbnail(ContentUris.withAppendedId(base, id), new Size(size, size), null);
            int longest = Math.max(bmp.getWidth(), bmp.getHeight());
            if (longest < size / 2) {
                bmp.recycle();
                return null;
            }
            return jpeg(scaleTo(bmp, size), 80);
        } catch (Exception e) {
            return null;
        }
    }

    public byte[] videoFrame(String path, int max) {
        MediaMetadataRetriever r = new MediaMetadataRetriever();
        try {
            r.setDataSource(path);
            long durUs = parseLong(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)) * 1000;
            long t = Math.min(1_000_000L, durUs / 2);
            Bitmap fr = Build.VERSION.SDK_INT >= 27
                    ? r.getScaledFrameAtTime(t, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, max, max)
                    : r.getFrameAtTime(t);
            if (fr == null) return null;
            return jpeg(scaleTo(fr, max), 82);
        } catch (Exception e) {
            return null;
        } finally {
            try {
                r.release();
            } catch (Exception ignored) {
            }
        }
    }

    public byte[] videoStrip(String path, int n, int h) {
        MediaMetadataRetriever r = new MediaMetadataRetriever();
        Bitmap sprite = null;
        try {
            r.setDataSource(path);
            long durUs = Math.max(1, parseLong(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION))) * 1000;
            Canvas canvas = null;
            int w = h;
            for (int i = 0; i < n; i++) {
                long t = durUs * (2L * i + 1) / (2L * n);
                Bitmap fr = Build.VERSION.SDK_INT >= 27
                        ? r.getScaledFrameAtTime(t, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, h * 4, h)
                        : r.getFrameAtTime(t);
                if (fr == null) continue;
                if (sprite == null) {
                    w = Math.max(1, fr.getWidth() * h / Math.max(1, fr.getHeight()));
                    sprite = Bitmap.createBitmap(w * n, h, Bitmap.Config.RGB_565);
                    canvas = new Canvas(sprite);
                }
                Bitmap scaled = Bitmap.createScaledBitmap(fr, w, h, true);
                canvas.drawBitmap(scaled, i * w, 0f, null);
                if (scaled != fr) scaled.recycle();
                fr.recycle();
            }
            return sprite != null ? jpeg(sprite, 70) : null;
        } catch (Exception e) {
            return null;
        } finally {
            try {
                r.release();
            } catch (Exception ignored) {
            }
        }
    }

    /** "width|height|durationMs|takenMs|location" (oriented size; 0 / empty when unknown) */
    public String videoInfo(String path) {
        MediaMetadataRetriever r = new MediaMetadataRetriever();
        try {
            r.setDataSource(path);
            long w = parseLong(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH));
            long h = parseLong(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT));
            long rot = parseLong(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION));
            if (rot == 90 || rot == 270) {
                long x = w;
                w = h;
                h = x;
            }
            long dur = parseLong(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION));
            long taken = parseDate(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DATE));
            String loc = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_LOCATION);
            return w + "|" + h + "|" + dur + "|" + taken + "|" + (loc != null ? loc.replace("|", "") : "");
        } catch (Exception e) {
            return null;
        } finally {
            try {
                r.release();
            } catch (Exception ignored) {
            }
        }
    }

    private static long parseLong(String s) {
        try {
            return s == null ? 0 : Long.parseLong(s.trim());
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    /** METADATA_KEY_DATE: "20240705T143000.000Z" (1904 / 1970 epochs = unknown) */
    private static long parseDate(String s) {
        if (s == null) return 0;
        for (String f : new String[]{"yyyyMMdd'T'HHmmss.SSS'Z'", "yyyyMMdd'T'HHmmss'Z'", "yyyyMMdd'T'HHmmss"}) {
            try {
                SimpleDateFormat p = new SimpleDateFormat(f, Locale.ROOT);
                p.setTimeZone(TimeZone.getTimeZone("UTC"));
                Date d = p.parse(s.trim());
                if (d != null && d.getTime() > 86_400_000L * 366) return d.getTime();
            } catch (Exception ignored) {
            }
        }
        return 0;
    }

    private static Bitmap scaleTo(Bitmap b, int max) {
        int longest = Math.max(b.getWidth(), b.getHeight());
        if (longest <= max) return b;
        float sc = (float) max / longest;
        Bitmap s = Bitmap.createScaledBitmap(b, Math.max(1, Math.round(b.getWidth() * sc)), Math.max(1, Math.round(b.getHeight() * sc)), true);
        if (s != b) b.recycle();
        return s;
    }

    private static byte[] jpeg(Bitmap b, int q) {
        ByteArrayOutputStream out = new ByteArrayOutputStream(64 * 1024);
        b.compress(Bitmap.CompressFormat.JPEG, q, out);
        b.recycle();
        return out.toByteArray();
    }

    // ───────────────────────── locks / network ─────────────────────────

    private void setLocks(boolean on) {
        if (on) {
            if (wifiLock == null) {
                try {
                    WifiManager wm = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
                    @SuppressWarnings("deprecation")
                    int mode = Build.VERSION.SDK_INT >= 29 ? WifiManager.WIFI_MODE_FULL_LOW_LATENCY : WifiManager.WIFI_MODE_FULL_HIGH_PERF;
                    wifiLock = wm.createWifiLock(mode, "PeekAround:link");
                    wifiLock.setReferenceCounted(false);
                    wifiLock.acquire();
                } catch (Exception e) {
                    wifiLock = null;
                }
            }
            if (wakeLock == null) {
                try {
                    PowerManager pm = (PowerManager) getSystemService(Context.POWER_SERVICE);
                    wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "PeekAround:link");
                    wakeLock.setReferenceCounted(false);
                    wakeLock.acquire(30 * 60_000L);
                } catch (Exception e) {
                    wakeLock = null;
                }
            }
        } else {
            try {
                if (wifiLock != null) wifiLock.release();
            } catch (Exception ignored) {
            }
            try {
                if (wakeLock != null) wakeLock.release();
            } catch (Exception ignored) {
            }
            wifiLock = null;
            wakeLock = null;
        }
    }

    private void registerNetwork() {
        if (netCb != null || Build.VERSION.SDK_INT < 24) return;
        ConnectivityManager cm = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        ConnectivityManager.NetworkCallback cb = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                // network (back): skip the remaining backoff
                main.postDelayed(() -> {
                    if (running && !"online".equals(state)) nativeReconnect();
                }, 500);
            }
        };
        try {
            cm.registerDefaultNetworkCallback(cb);
            netCb = cb;
        } catch (Exception ignored) {
        }
    }

    private void unregisterNetwork() {
        if (netCb == null) return;
        try {
            ((ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE)).unregisterNetworkCallback(netCb);
        } catch (Exception ignored) {
        }
        netCb = null;
    }

    // ───────────────────────── notification ─────────────────────────

    private void createChannels() {
        if (Build.VERSION.SDK_INT < 26) return;
        NotificationManager nm = getSystemService(NotificationManager.class);
        NotificationChannel c = new NotificationChannel(CHANNEL, getString(R.string.link_channel), NotificationManager.IMPORTANCE_LOW);
        c.setShowBadge(false);
        nm.createNotificationChannel(c);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL_ALERT, getString(R.string.link_channel_alert), NotificationManager.IMPORTANCE_DEFAULT));
    }

    private String contentText() {
        JSONObject p = prefs();
        String pc = p.optString("serverName", "");
        if (pc.isEmpty()) {
            String url = p.optString("url", "");
            int s = url.indexOf("://");
            pc = s >= 0 ? url.substring(s + 3) : url;
            int slash = pc.indexOf('/');
            if (slash >= 0) pc = pc.substring(0, slash);
        }
        if (pc.isEmpty()) pc = "PC";
        int n = p.optJSONArray("folders") != null ? p.optJSONArray("folders").length() : 0;
        if ("online".equals(state)) return getResources().getQuantityString(R.plurals.link_sharing, n, n, pc);
        if ("pending".equals(state)) return getString(R.string.link_pending, code != null ? code : "");
        return getString(R.string.link_waiting, pc);
    }

    private Notification.Builder builder(String channel) {
        return Build.VERSION.SDK_INT >= 26 ? new Notification.Builder(this, channel) : new Notification.Builder(this);
    }

    private PendingIntent openApp() {
        Intent launch = getPackageManager().getLaunchIntentForPackage(getPackageName());
        return launch == null ? null : PendingIntent.getActivity(this, 0, launch, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    private Notification buildNotification() {
        PendingIntent stop = PendingIntent.getService(this, 1, new Intent(this, PeekService.class).setAction(ACTION_STOP),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        String text = contentText();
        lastText = text;
        Notification.Builder b = builder(CHANNEL)
                .setSmallIcon(android.R.drawable.stat_notify_sync_noanim)
                .setContentTitle(getString(R.string.app_name))
                .setContentText(text)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setCategory(Notification.CATEGORY_SERVICE)
                .setContentIntent(openApp())
                .addAction(new Notification.Action.Builder(null, getString(R.string.link_stop), stop).build());
        if (Build.VERSION.SDK_INT >= 31) b.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE);
        return b.build();
    }

    private void updateNotification() {
        if (!running) return;
        if (contentText().equals(lastText)) return;
        try {
            NotificationManager nm = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
            nm.notify(NOTIF_ID, buildNotification());
        } catch (Exception ignored) {
        }
    }

    private void notifyAlert(String text) {
        try {
            NotificationManager nm = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
            if (Build.VERSION.SDK_INT >= 24 && !nm.areNotificationsEnabled()) return;
            nm.notify(NOTIF_ALERT_ID, builder(CHANNEL_ALERT)
                    .setSmallIcon(android.R.drawable.stat_notify_error)
                    .setContentTitle(getString(R.string.app_name))
                    .setContentText(text)
                    .setAutoCancel(true)
                    .setContentIntent(openApp())
                    .build());
        } catch (Exception ignored) {
        }
    }
}
