package app.peekaround

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File

/** Codec against the golden transcripts shared with the Node fake-phone tests (packages/shared/fixtures/link). */
class LinkProtocolTest {
    private val dir: File by lazy {
        var d: File? = File(System.getProperty("user.dir")!!).absoluteFile
        while (d != null) {
            val f = File(d, "packages/shared/fixtures/link")
            if (f.isDirectory) return@lazy f
            val g = File(d, "shared/fixtures/link")
            if (g.isDirectory) return@lazy g
            d = d.parentFile
        }
        error("fixtures not found from ${System.getProperty("user.dir")}")
    }

    private fun steps(name: String): List<JSONObject> {
        val a = JSONArray(File(dir, name).readText())
        return (0 until a.length()).map { a.getJSONObject(it) }
    }

    /** order-independent canonical JSON */
    private fun canon(v: Any?): String = when (v) {
        null, JSONObject.NULL -> "null"
        is JSONObject -> v.keys().asSequence().toList().sorted().joinToString(",", "{", "}") { "\"$it\":" + canon(v.get(it)) }
        is JSONArray -> (0 until v.length()).joinToString(",", "[", "]") { canon(v.get(it)) }
        is String -> Json.quote(v)
        is Number -> Json.num(v.toDouble())
        else -> v.toString()
    }

    private fun assertJson(expected: JSONObject, actual: JSONObject) = assertEquals(canon(expected), canon(actual))

    @Test
    fun allFixturesParse() {
        val files = dir.listFiles { f -> f.name.endsWith(".json") }!!
        assertTrue(files.size >= 4)
        for (f in files) for (s in steps(f.name)) if (s.getString("dir") == "s2p") {
            val p = LinkCodec.parse(s.getJSONObject("frame").toString())
            assertTrue("${f.name}: $s", p !is Incoming.Unknown)
        }
    }

    @Test
    fun pairing() {
        val st = steps("pairing.json")
        val hello = st[0].getJSONObject("frame")
        val folders = hello.getJSONArray("folders").let { a -> (0 until a.length()).map { a.getJSONObject(it) }.map { LinkFolder(it.getString("key"), it.getString("name"), it.getString("path")) } }
        val mine = LinkCodec.hello(hello.getString("id"), hello.getString("name"), hello.getString("model"), hello.getString("app"), folders)
        assertJson(hello, mine)
        assertEquals(Incoming.Pending("4821"), LinkCodec.parse(st[1].getJSONObject("frame").toString()))
        val paired = LinkCodec.parse(st[2].getJSONObject("frame").toString())
        assertEquals(Incoming.Paired(st[2].getJSONObject("frame").getString("token")), paired)
        assertEquals(Incoming.Welcome("pc", true), LinkCodec.parse("""{"t":"welcome","server":"pc","paired":true}"""))
        assertEquals(Incoming.Rejected, LinkCodec.parse("""{"t":"rejected"}"""))
        assertEquals(Incoming.Pong, LinkCodec.parse("""{"t":"pong"}"""))
        assertTrue(LinkCodec.parse("not json") is Incoming.Unknown)
        assertTrue(LinkCodec.parse("""{"t":"req","rid":"../x","op":"list"}""") is Incoming.Unknown)
        assertJson(JSONObject("""{"t":"ping"}"""), LinkCodec.ping())
    }

    @Test
    fun listAndThumb() {
        val st = steps("list-and-thumb.json")
        val req = LinkCodec.parse(st[0].getJSONObject("frame").toString()) as Incoming.Req
        assertEquals("list", req.op)
        assertEquals("AAAAAAAAAAAAAAAAAAAAAA", req.rid)
        assertEquals(true, req.args.getBoolean("recursive"))
        // NDJSON body: re-emitting every golden line gives the exact same bytes
        val body = st[1].getString("body")
        val sb = StringBuilder()
        for (line in body.split('\n').filter { it.isNotEmpty() }) {
            val o = JSONObject(line)
            if (o.has("dir")) sb.append(LinkCodec.dirLine(o.getString("dir")))
            else sb.append(
                LinkCodec.listLine(
                    o.getString("p"), o.getString("k"), o.getLong("s"), o.getLong("m"),
                    if (o.isNull("w")) null else o.getInt("w"), if (o.isNull("h")) null else o.getInt("h"),
                    if (o.isNull("d")) null else o.getDouble("d"), if (o.isNull("t")) null else o.getLong("t"),
                    if (o.isNull("id")) null else o.getLong("id"),
                ),
            )
            sb.append('\n')
        }
        assertEquals(body, sb.toString())
        assertEquals("application/x-ndjson", st[1].getJSONObject("headers").getString("Content-Type"))

        val thumb = LinkCodec.parse(st[2].getJSONObject("frame").toString()) as Incoming.Req
        assertEquals("thumb", thumb.op)
        assertEquals(320, thumb.args.getInt("s"))
        // path guard → the golden err frame
        val bad = LinkCodec.parse(st[4].getJSONObject("frame").toString()) as Incoming.Req
        val root = kotlin.io.path.createTempDirectory("peek").toFile()
        try {
            PathGuard.resolve(root, bad.args.getString("p"))
            fail("must throw")
        } catch (e: LinkError) {
            assertJson(st[5].getJSONObject("frame"), LinkCodec.err(bad.rid, e.status, e.message!!))
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun fileRangeHeaders() {
        val st = steps("file-range.json")
        var i = 0
        var checked = 0
        while (i < st.size) {
            val s = st[i]
            if (s.getString("dir") == "s2p" && s.getJSONObject("frame").getString("t") == "req") {
                val req = LinkCodec.parse(s.getJSONObject("frame").toString()) as Incoming.Req
                val put = st[i + 1]
                assertEquals(req.rid, put.getString("rid"))
                val h = put.getJSONObject("headers")
                val total = h.getString("X-Peek-Total").toLong()
                val reply = FileReplies.build(req.args.optString("range").ifEmpty { null }, total, 1700000500000, Kinds.mime(req.args.getString("p")))
                val expected = h.keys().asSequence().toList().associateWith { h.getString(it) }
                assertEquals(expected, reply.headers.toMap())
                assertEquals(put.getLong("bodyLength"), reply.length)
                checked++
                i += 2
            } else {
                if (s.getString("dir") == "s2p") assertEquals(Incoming.Cancel("DDDDDDDDDDDDDDDDDDDDDD"), LinkCodec.parse(s.getJSONObject("frame").toString()))
                i++
            }
        }
        assertEquals(2, checked)
        // full file
        val full = FileReplies.build(null, 4096, 5, "video/mp4")
        assertEquals(mapOf("X-Peek-Status" to "200", "Content-Type" to "video/mp4", "Content-Length" to "4096", "X-Peek-Total" to "4096", "X-Peek-Mtime" to "5"), full.headers.toMap())
    }

    @Test
    fun receive() {
        val st = steps("receive.json")
        val req = LinkCodec.parse(st[0].getJSONObject("frame").toString()) as Incoming.Req
        assertEquals("receive", req.op)
        assertEquals(st[1].getString("url"), req.args.getString("url"))
        assertTrue(req.args.getString("url").startsWith("/api/devices/pull/"))
        // the phone writes into dir/name with a unique name and answers res + changed
        val root = kotlin.io.path.createTempDirectory("peek").toFile()
        try {
            val dir = File(root, PathGuard.cleanRel(req.args.getString("dir"))).apply { mkdirs() }
            val name = PathGuard.uniqueName(PathGuard.cleanName(req.args.getString("name"))) { File(dir, it).exists() }
            val dest = File(dir, name).apply { writeBytes(java.util.Base64.getDecoder().decode(st[1].getJSONObject("response").getString("bodyBase64"))) }
            assertEquals(req.args.getLong("size"), dest.length())
            assertJson(st[2].getJSONObject("frame"), LinkCodec.res(req.rid, JSONObject().put("p", PathGuard.relOf(root.canonicalFile, dest.canonicalFile))))
            assertJson(st[3].getJSONObject("frame"), LinkCodec.changed("k1", listOf(PathGuard.relOf(root.canonicalFile, dir.canonicalFile))))
            // a second push of the same name never overwrites
            assertEquals("IMG_9 (1).jpg", PathGuard.uniqueName("IMG_9.jpg") { File(dir, it).exists() })
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun folderFrame() {
        val f = LinkCodec.folders(listOf(LinkFolder("k1", "Kamera", "/x")))
        assertJson(JSONObject("""{"t":"folders","folders":[{"key":"k1","name":"Kamera","path":"/x"}]}"""), f)
    }
}
