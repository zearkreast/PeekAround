package app.peekaround

import android.os.Build
import android.os.FileObserver
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/**
 * Watches the shared folders and reports changed directories (relative to each folder root),
 * debounced by 1 s → `{"t":"changed","key","dirs":[…]}`.
 * One FileObserver (inotify) per directory, up to MAX_WATCHES per folder; bigger trees fall back
 * to polling directory mtimes every 10 s.
 */
class LinkWatch(private val exec: ScheduledExecutorService, private val onChanged: (key: String, dirs: Set<String>) -> Unit) {
    private class Folder(val f: LinkFolder, val root: File) {
        val observers = ConcurrentHashMap<String, FileObserver>()
        var polling = false
        var snapshot: Map<String, Long> = emptyMap()
        val dirty = HashSet<String>()
        var flush: ScheduledFuture<*>? = null
    }

    private val folders = HashMap<String, Folder>()
    private var poll: ScheduledFuture<*>? = null

    companion object {
        const val MAX_WATCHES = 1500
        private const val MASK = FileObserver.CREATE or FileObserver.DELETE or FileObserver.MOVED_FROM or FileObserver.MOVED_TO or
            FileObserver.CLOSE_WRITE or FileObserver.DELETE_SELF or FileObserver.MOVE_SELF
        private const val IS_DIR = 0x40000000
    }

    /** (Re)configure; called on the control executor. */
    fun set(list: List<LinkFolder>) {
        exec.execute {
            val keep = list.associateBy { it.key }
            for ((k, fo) in folders.toMap()) {
                if (keep[k]?.path != fo.f.path) {
                    fo.observers.values.forEach { it.stopWatching() }
                    fo.flush?.cancel(false)
                    folders.remove(k)
                }
            }
            for (f in list) {
                if (folders.containsKey(f.key)) continue
                val root = try { File(f.path).canonicalFile } catch (_: Exception) { continue }
                val fo = Folder(f, root)
                folders[f.key] = fo
                val dirs = ArrayList<File>()
                collectDirs(root, dirs, MAX_WATCHES + 1)
                if (dirs.size > MAX_WATCHES) {
                    fo.polling = true
                    fo.snapshot = snapshot(root)
                } else dirs.forEach { watch(fo, it) }
            }
            poll?.cancel(false)
            poll = if (folders.values.any { it.polling }) exec.scheduleWithFixedDelay({ pollAll() }, 10, 10, TimeUnit.SECONDS) else null
        }
    }

    fun stop() {
        exec.execute {
            folders.values.forEach { fo -> fo.observers.values.forEach { it.stopWatching() }; fo.flush?.cancel(false) }
            folders.clear()
            poll?.cancel(false)
            poll = null
        }
    }

    /** a receive op wrote into this dir: report it right away (debounced like the others) */
    fun touched(key: String, rel: String) = exec.execute { folders[key]?.let { mark(it, rel) } }

    private fun collectDirs(d: File, out: MutableList<File>, cap: Int) {
        if (out.size >= cap) return
        out.add(d)
        d.listFiles()?.forEach { c ->
            if (out.size >= cap) return
            if (c.isDirectory && !PeekFs.hidden(c.name)) collectDirs(c, out, cap)
        }
    }

    private fun watch(fo: Folder, dir: File) {
        val rel = PathGuard.relOf(fo.root, dir)
        if (fo.observers.containsKey(rel)) return
        if (fo.observers.size >= MAX_WATCHES) {
            if (!fo.polling) {
                fo.polling = true
                fo.snapshot = snapshot(fo.root)
                if (poll == null) poll = exec.scheduleWithFixedDelay({ pollAll() }, 10, 10, TimeUnit.SECONDS)
            }
            return
        }
        val cb = { event: Int, name: String? ->
            exec.execute {
                val ev = event and FileObserver.ALL_EVENTS
                if (ev and (FileObserver.DELETE_SELF or FileObserver.MOVE_SELF) != 0) {
                    fo.observers.remove(rel)?.stopWatching()
                    mark(fo, rel.substringBeforeLast('/', ""))
                } else {
                    if (name != null && !PeekFs.hidden(name) && (event and IS_DIR) != 0 && (ev and (FileObserver.CREATE or FileObserver.MOVED_TO)) != 0) {
                        val sub = File(dir, name)
                        val all = ArrayList<File>()
                        collectDirs(sub, all, MAX_WATCHES)
                        all.forEach { watch(fo, it) }
                    }
                    if (name == null || !name.endsWith(".part")) mark(fo, rel)
                }
            }
        }
        val o = if (Build.VERSION.SDK_INT >= 29) object : FileObserver(dir, MASK) {
            override fun onEvent(event: Int, path: String?) = cb(event, path)
        } else @Suppress("DEPRECATION") object : FileObserver(dir.absolutePath, MASK) {
            override fun onEvent(event: Int, path: String?) = cb(event, path)
        }
        o.startWatching()
        fo.observers[rel] = o
    }

    private fun mark(fo: Folder, rel: String) {
        fo.dirty.add(rel)
        if (fo.flush == null) {
            fo.flush = exec.schedule({
                fo.flush = null
                val dirs = HashSet(fo.dirty)
                fo.dirty.clear()
                if (dirs.isNotEmpty() && folders[fo.f.key] === fo) onChanged(fo.f.key, dirs)
            }, 1, TimeUnit.SECONDS)
        }
    }

    /** dir rel path → mtime, for every directory (polling fallback) */
    private fun snapshot(root: File): Map<String, Long> {
        val out = HashMap<String, Long>()
        fun walk(d: File) {
            out[PathGuard.relOf(root, d)] = d.lastModified()
            d.listFiles()?.forEach { if (it.isDirectory && !PeekFs.hidden(it.name)) walk(it) }
        }
        walk(root)
        return out
    }

    private fun pollAll() {
        for (fo in folders.values) {
            if (!fo.polling) continue
            val now = snapshot(fo.root)
            val old = fo.snapshot
            for ((d, m) in now) if (old[d] != m) mark(fo, d)
            for (d in old.keys) if (d !in now) mark(fo, d.substringBeforeLast('/', ""))
            fo.snapshot = now
        }
    }
}
