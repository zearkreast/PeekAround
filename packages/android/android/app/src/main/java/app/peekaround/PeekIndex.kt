package app.peekaround

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import android.os.Process
import com.getcapacitor.JSArray
import com.getcapacitor.JSObject
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Local-mode grouping metadata (GPS, camera, capture offset, panorama/animation hints) in SQLite
 * (filesDir/peekindex.db). Background, low priority, incremental by (size, mtime).
 * JS (local.ts) calls indexSync({paths: library folder roots}), polls indexStatus() / listens to
 * 'index' events, and reads indexQuery({paths}) as compact columnar arrays.
 */
class PeekIndex private constructor(ctx: Context) : SQLiteOpenHelper(ctx.applicationContext, File(ctx.filesDir, "peekindex.db").absolutePath, null, 1) {
    private val exec = Executors.newSingleThreadExecutor { r -> Thread({ Process.setThreadPriority(Process.THREAD_PRIORITY_LOWEST); r.run() }, "peek-index").apply { isDaemon = true } }
    private val running = AtomicBoolean(false)
    @Volatile private var restart: List<String>? = null
    @Volatile var done = 0
        private set
    @Volatile var total = 0
        private set
    var onProgress: ((JSObject) -> Unit)? = null

    companion object {
        @Volatile private var inst: PeekIndex? = null
        fun get(ctx: Context): PeekIndex = inst ?: synchronized(this) { inst ?: PeekIndex(ctx).also { inst = it } }
    }

    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL(
            """CREATE TABLE idx (
                path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime INTEGER NOT NULL,
                lat REAL, lon REAL, make TEXT, model TEXT, lens TEXT, offset_min INTEGER, flags INTEGER NOT NULL DEFAULT 0,
                indexed_at INTEGER NOT NULL)""",
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) {
        db.execSQL("DROP TABLE IF EXISTS idx")
        onCreate(db)
    }

    fun status(): JSObject = JSObject().put("done", done).put("total", total).put("running", running.get())

    /** Start (or restart with new roots) the background indexing pass. */
    fun sync(roots: List<String>) {
        restart = roots
        if (!running.compareAndSet(false, true)) return
        exec.execute {
            try {
                while (true) {
                    val r = restart ?: break
                    restart = null
                    pass(r)
                }
            } finally {
                running.set(false)
                emit(true)
                restart?.let { sync(it) } // a sync() that raced with the end of this pass
            }
        }
    }

    private var lastEmit = 0L

    private fun emit(force: Boolean = false) {
        val now = System.currentTimeMillis()
        if (!force && now - lastEmit < 1000) return
        lastEmit = now
        onProgress?.invoke(status())
    }

    private fun pass(roots: List<String>) {
        val db = writableDatabase
        // 1. what is on disk
        val files = ArrayList<File>()
        val seenRoots = roots.map { File(it) }.filter { it.isDirectory }
        fun walk(d: File) {
            if (restart != null) return
            d.listFiles()?.forEach { c ->
                if (PeekFs.hidden(c.name)) return@forEach
                if (c.isDirectory) walk(c)
                else {
                    val k = PeekFs.kind(c.name)
                    if (k == "image" || k == "video") files.add(c)
                }
            }
        }
        seenRoots.forEach { walk(it) }
        if (restart != null) return
        // 2. what is indexed already
        val known = HashMap<String, Pair<Long, Long>>()
        for (r in seenRoots) {
            db.rawQuery("SELECT path, size, mtime FROM idx WHERE path = ? OR substr(path, 1, ?) = ?", arrayOf(r.absolutePath, (r.absolutePath.length + 1).toString(), r.absolutePath + "/")).use { c ->
                while (c.moveToNext()) known[c.getString(0)] = c.getLong(1) to c.getLong(2)
            }
        }
        val onDisk = HashSet<String>(files.size * 2)
        val todo = ArrayList<File>()
        for (f in files) {
            onDisk.add(f.absolutePath)
            val k = known[f.absolutePath]
            if (k == null || k.first != f.length() || k.second != f.lastModified()) todo.add(f)
        }
        // 3. forget vanished files
        val gone = known.keys.filter { it !in onDisk }
        if (gone.isNotEmpty()) {
            db.beginTransaction()
            try {
                gone.forEach { db.delete("idx", "path = ?", arrayOf(it)) }
                db.setTransactionSuccessful()
            } finally {
                db.endTransaction()
            }
        }
        // 4. read metadata of new / changed files (newest first: the timeline top groups first)
        todo.sortByDescending { it.lastModified() }
        total = files.size
        done = files.size - todo.size
        emit(true)
        val batch = ArrayList<ContentValues>()
        fun flush() {
            if (batch.isEmpty()) return
            db.beginTransaction()
            try {
                batch.forEach { db.insertWithOnConflict("idx", null, it, SQLiteDatabase.CONFLICT_REPLACE) }
                db.setTransactionSuccessful()
            } finally {
                db.endTransaction()
            }
            batch.clear()
        }
        for (f in todo) {
            if (restart != null) break
            val k = PeekFs.kind(f.name) ?: continue
            val m = PeekMeta.row(f, k)
            batch.add(ContentValues().apply {
                put("path", f.absolutePath); put("size", f.length()); put("mtime", f.lastModified())
                put("lat", m.lat); put("lon", m.lon); put("make", m.make); put("model", m.model); put("lens", m.lens)
                put("offset_min", m.offset); put("flags", m.flags); put("indexed_at", System.currentTimeMillis())
            })
            done++
            if (batch.size >= 100) flush()
            emit()
        }
        flush()
    }

    /** Rows under the given folder roots (or exact file paths) as columnar arrays. */
    fun query(paths: List<String>): JSObject {
        val cols = Array(8) { JSArray() }
        val db = readableDatabase
        for (p0 in paths) {
            val p = p0.trimEnd('/')
            db.rawQuery(
                "SELECT path, lat, lon, make, model, lens, offset_min, flags FROM idx WHERE path = ? OR substr(path, 1, ?) = ?",
                arrayOf(p, (p.length + 1).toString(), "$p/"),
            ).use { c ->
                while (c.moveToNext()) {
                    cols[0].put(c.getString(0))
                    cols[1].put(if (c.isNull(1)) JSONObject.NULL else c.getDouble(1))
                    cols[2].put(if (c.isNull(2)) JSONObject.NULL else c.getDouble(2))
                    cols[3].put(c.getString(3) ?: JSONObject.NULL)
                    cols[4].put(c.getString(4) ?: JSONObject.NULL)
                    cols[5].put(c.getString(5) ?: JSONObject.NULL)
                    cols[6].put(if (c.isNull(6)) JSONObject.NULL else c.getInt(6))
                    cols[7].put(c.getInt(7))
                }
            }
        }
        return JSObject()
            .put("path", cols[0]).put("lat", cols[1]).put("lon", cols[2]).put("make", cols[3])
            .put("model", cols[4]).put("lens", cols[5]).put("offset", cols[6]).put("flags", cols[7])
    }
}
