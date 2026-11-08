package app.peekaround;

import android.Manifest;
import android.app.NativeActivity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.media.MediaScannerConnection;
import android.net.Uri;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.provider.Settings;
import android.util.Log;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.Locale;

/**
 * Hosts the Slint UI (android-activity calls android_main in libpeekaround_android.so). The public
 * methods are called from Rust through JNI (crates/pa-android/src/platform.rs) on the native thread.
 */
public class MainActivity extends NativeActivity {
    private static final String TAG = "PeekAround";
    private WifiManager.MulticastLock multicast;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        migrateLinkPrefs();
        super.onCreate(savedInstanceState);
        // mDNS discovery of PCs on the first-run screen
        try {
            WifiManager wm = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            multicast = wm.createMulticastLock("PeekAround:mdns");
            multicast.setReferenceCounted(false);
            multicast.acquire();
        } catch (Exception e) {
            multicast = null;
        }
    }

    @Override
    protected void onDestroy() {
        if (multicast != null) {
            try {
                multicast.release();
            } catch (Exception ignored) {
            }
        }
        super.onDestroy();
        // Slint's platform is set once per process: a new activity needs a new UI process.
        // Folder sharing runs in the ":link" process and is not affected.
        new Handler(Looper.getMainLooper()).postDelayed(() -> Process.killProcess(Process.myPid()), 400);
    }

    /**
     * 1.3.x kept the link settings in SharedPreferences "peek_link": carry the device id and token
     * over so a phone paired with a PC stays paired after the update.
     */
    private void migrateLinkPrefs() {
        try {
            File out = new File(getFilesDir(), "link.json");
            if (out.exists()) return;
            SharedPreferences sp = getSharedPreferences("peek_link", MODE_PRIVATE);
            String id = sp.getString("deviceId", null);
            if (id == null) return;
            JSONObject o = new JSONObject();
            o.put("enabled", false);
            o.put("deviceId", id);
            String url = sp.getString("url", null);
            if (url != null) o.put("url", url);
            String token = sp.getString("token", null);
            if (token != null) o.put("token", token);
            String server = sp.getString("serverName", null);
            if (server != null) o.put("serverName", server);
            String name = sp.getString("deviceName", null);
            if (name != null) o.put("deviceName", name);
            String folders = sp.getString("folders", null);
            o.put("folders", folders != null ? new JSONArray(folders) : new JSONArray());
            try (FileOutputStream f = new FileOutputStream(out)) {
                f.write(o.toString().getBytes(StandardCharsets.UTF_8));
            }
        } catch (Exception e) {
            Log.w(TAG, "link prefs migration", e);
        }
    }

    // ───────────────────────── called from Rust ─────────────────────────

    public String storageRoot() {
        return Environment.getExternalStorageDirectory().getAbsolutePath();
    }

    public String systemLanguage() {
        return Locale.getDefault().getLanguage();
    }

    public boolean hasAllFilesAccess() {
        if (Build.VERSION.SDK_INT >= 30) return Environment.isExternalStorageManager();
        return checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                && checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED;
    }

    public void requestAllFilesAccess() {
        runOnUiThread(() -> {
            if (Build.VERSION.SDK_INT >= 30) {
                try {
                    startActivity(new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:" + getPackageName())));
                } catch (Exception e) {
                    try {
                        startActivity(new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION));
                    } catch (Exception ignored) {
                    }
                }
            } else {
                requestPermissions(new String[]{Manifest.permission.READ_EXTERNAL_STORAGE, Manifest.permission.WRITE_EXTERNAL_STORAGE}, 1);
            }
        });
    }

    public void requestNotifications() {
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            runOnUiThread(() -> requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, 2));
        }
    }

    public void toast(String text) {
        runOnUiThread(() -> Toast.makeText(this, text, Toast.LENGTH_LONG).show());
    }

    public void openUrl(String url) {
        runOnUiThread(() -> {
            try {
                startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            } catch (Exception e) {
                Log.w(TAG, "open " + url, e);
            }
        });
    }

    public void copyText(String text) {
        runOnUiThread(() -> {
            ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
            if (cm != null) cm.setPrimaryClip(ClipData.newPlainText("PeekAround", text));
        });
    }

    public void scanFile(String path) {
        MediaScannerConnection.scanFile(getApplicationContext(), new String[]{path}, null, null);
    }

    public boolean startLinkService() {
        try {
            Intent i = new Intent(this, PeekService.class).setAction(PeekService.ACTION_START);
            if (Build.VERSION.SDK_INT >= 26) startForegroundService(i);
            else startService(i);
            return true;
        } catch (Exception e) {
            Log.w(TAG, "cannot start the sharing service", e);
            return false;
        }
    }

    public void stopLinkService() {
        command(PeekService.ACTION_STOP);
    }

    public void linkFoldersChanged() {
        command(PeekService.ACTION_FOLDERS);
    }

    private void command(String action) {
        try {
            startService(new Intent(this, PeekService.class).setAction(action));
        } catch (Exception e) {
            Log.w(TAG, "cannot reach the sharing service", e);
        }
    }

    public void restartApp() {
        RestartActivity.restart(this);
    }

    public void finishApp() {
        runOnUiThread(this::finish);
    }

    public String deviceName() {
        String n = null;
        try {
            if (Build.VERSION.SDK_INT >= 25) n = Settings.Global.getString(getContentResolver(), Settings.Global.DEVICE_NAME);
        } catch (Exception ignored) {
        }
        return n != null && !n.trim().isEmpty() ? n : deviceModel();
    }

    public String deviceModel() {
        String maker = Build.MANUFACTURER == null ? "" : Build.MANUFACTURER;
        if (!maker.isEmpty()) maker = maker.substring(0, 1).toUpperCase(Locale.ROOT) + maker.substring(1);
        return (maker + " " + Build.MODEL).trim();
    }
}
