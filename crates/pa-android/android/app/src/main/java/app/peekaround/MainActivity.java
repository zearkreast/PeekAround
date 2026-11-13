package app.peekaround;

import android.Manifest;
import android.app.NativeActivity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.database.Cursor;
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
import android.provider.OpenableColumns;
import android.provider.Settings;
import android.util.Log;
import android.webkit.MimeTypeMap;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Locale;

/**
 * Hosts the Slint UI (android-activity calls android_main in libpeekaround_android.so). The public
 * methods are called from Rust through JNI (crates/pa-android/src/platform.rs) on the native thread.
 */
public class MainActivity extends NativeActivity {
    private static final String TAG = "PeekAround";
    private WifiManager.MulticastLock multicast;
    private final Object sharedLock = new Object();
    /** what other apps shared and Rust has not taken yet: {"files":[path…],"text":…} */
    private JSONObject shared;

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
        new Thread(() -> dropOld(new File(getCacheDir(), "incoming"))).start();
        handleShare(getIntent());
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleShare(intent);
    }

    /** "Share → PeekAround": copy the content into the cache (the grant ends with this activity). */
    private void handleShare(Intent intent) {
        if (intent == null) return;
        String action = intent.getAction();
        boolean many = Intent.ACTION_SEND_MULTIPLE.equals(action);
        if (!many && !Intent.ACTION_SEND.equals(action)) return;
        // reopened from Recents: that share was handled already
        if ((intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) != 0) return;
        final ArrayList<Uri> uris = new ArrayList<>();
        try {
            if (many) {
                ArrayList<Uri> l = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
                if (l != null) uris.addAll(l);
            } else {
                Uri u = intent.getParcelableExtra(Intent.EXTRA_STREAM);
                if (u != null) uris.add(u);
            }
        } catch (Exception e) {
            Log.w(TAG, "share intent", e);
        }
        final CharSequence text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT);
        if (uris.isEmpty() && text == null) return;
        new Thread(() -> {
            try {
                File dir = new File(getCacheDir(), "incoming/" + System.currentTimeMillis());
                JSONArray files = new JSONArray();
                for (Uri u : uris) {
                    File f = copyIn(u, dir);
                    if (f != null) files.put(f.getAbsolutePath());
                }
                if (files.length() == 0 && text == null) return;
                synchronized (sharedLock) {
                    if (shared == null) shared = new JSONObject().put("files", new JSONArray());
                    JSONArray all = shared.getJSONArray("files");
                    for (int i = 0; i < files.length(); i++) all.put(files.get(i));
                    if (text != null) shared.put("text", text.toString());
                }
            } catch (Exception e) {
                Log.w(TAG, "share", e);
            }
        }).start();
    }

    private File copyIn(Uri uri, File dir) {
        try {
            String name = null;
            try (Cursor c = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                if (c != null && c.moveToFirst()) name = c.getString(0);
            } catch (Exception ignored) {
            }
            if (name == null || name.trim().isEmpty()) name = uri.getLastPathSegment();
            if (name == null || name.trim().isEmpty()) name = "shared";
            name = name.replaceAll("[\\\\/:*?\"<>|\\x00-\\x1f]", "_");
            if (name.lastIndexOf('.') <= 0) {
                String ext = MimeTypeMap.getSingleton().getExtensionFromMimeType(getContentResolver().getType(uri));
                if (ext != null) name = name + "." + ext;
            }
            if (!dir.isDirectory() && !dir.mkdirs()) return null;
            File out = new File(dir, name);
            for (int n = 1; out.exists(); n++) out = new File(dir, n + "-" + name);
            try (InputStream in = getContentResolver().openInputStream(uri); FileOutputStream o = new FileOutputStream(out)) {
                if (in == null) return null;
                byte[] buf = new byte[1 << 16];
                for (int r; (r = in.read(buf)) > 0; ) o.write(buf, 0, r);
            }
            return out;
        } catch (Exception e) {
            Log.w(TAG, "shared file " + uri, e);
            return null;
        }
    }

    /** shared files are uploaded right after they arrive: a day later they are leftovers */
    private static void dropOld(File dir) {
        File[] list = dir.listFiles();
        if (list == null) return;
        long limit = System.currentTimeMillis() - 24L * 3600 * 1000;
        for (File d : list) {
            if (d.lastModified() > limit) continue;
            File[] inner = d.listFiles();
            if (inner != null) for (File f : inner) f.delete();
            d.delete();
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

    /** JSON of what was shared into the app since the last call, "" when nothing */
    public String takeShared() {
        synchronized (sharedLock) {
            if (shared == null) return "";
            String s = shared.toString();
            shared = null;
            return s;
        }
    }

    /** the share sheet for files in cache/share (a JSON array of paths) */
    public void shareFiles(String json) {
        try {
            JSONArray a = new JSONArray(json);
            ArrayList<Uri> uris = new ArrayList<>();
            String kind = null;
            for (int i = 0; i < a.length(); i++) {
                File f = new File(a.getString(i));
                uris.add(ShareProvider.uriOf(f));
                String mime = ShareProvider.mimeOf(f.getName());
                String k = mime.substring(0, mime.indexOf('/') + 1) + "*";
                kind = kind == null ? (a.length() == 1 ? mime : k) : (kind.equals(k) ? k : "*/*");
            }
            if (uris.isEmpty()) return;
            Intent i;
            if (uris.size() == 1) {
                i = new Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris.get(0));
            } else {
                i = new Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
            }
            i.setType(kind).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            ClipData clip = ClipData.newRawUri("", uris.get(0));
            for (int n = 1; n < uris.size(); n++) clip.addItem(new ClipData.Item(uris.get(n)));
            i.setClipData(clip);
            final Intent chooser = Intent.createChooser(i, null);
            runOnUiThread(() -> {
                try {
                    startActivity(chooser);
                } catch (Exception e) {
                    Log.w(TAG, "share", e);
                }
            });
        } catch (Exception e) {
            Log.w(TAG, "share", e);
        }
    }

    /** where the APK's native libraries were extracted (ffmpeg lives there as libffmpeg.so) */
    public String nativeLibDir() {
        return getApplicationInfo().nativeLibraryDir;
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
