package app.peekaround

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File
import java.nio.file.Files
import kotlin.random.Random

class CoreTest {
    private fun tmp(): File = Files.createTempDirectory("peekcore").toFile().canonicalFile

    private fun forbidden(root: File, rel: String) {
        try {
            PathGuard.resolve(root, rel)
            fail("$rel must be rejected")
        } catch (e: LinkError) {
            assertEquals(403, e.status)
        }
    }

    @Test
    fun pathGuard() {
        val root = tmp()
        try {
            File(root, "Sub").mkdirs()
            File(root, "Sub/a.jpg").writeText("x")
            assertEquals(File(root, "Sub/a.jpg"), PathGuard.resolve(root, "Sub/a.jpg"))
            assertEquals(root, PathGuard.resolve(root, ""))
            for (bad in listOf("..", "../x", "Sub/../../x", "Sub/..", "/etc/passwd", "./a", "Sub//a.jpg", "Sub\\..\\x", "C:/x", "a\u0000b")) forbidden(root, bad)
            // a sibling with the root as name prefix is not inside
            val sib = File(root.parentFile, root.name + "-evil").apply { mkdirs() }
            assertTrue(!PathGuard.isInside(root, sib))
            sib.deleteRecursively()
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun symlinkEscape() {
        val root = tmp()
        val outside = tmp()
        try {
            File(outside, "secret.txt").writeText("s")
            Files.createSymbolicLink(File(root, "link").toPath(), outside.toPath())
            Files.createSymbolicLink(File(root, "file.jpg").toPath(), File(outside, "secret.txt").toPath())
            File(root, "in").mkdirs()
            Files.createSymbolicLink(File(root, "inner").toPath(), File(root, "in").toPath())
            forbidden(root, "link/secret.txt")
            forbidden(root, "link")
            forbidden(root, "file.jpg")
            assertEquals(File(root, "in"), PathGuard.resolve(root, "inner")) // symlink that stays inside is fine
            assertTrue(!PathGuard.allowed(File(root, "link/secret.txt"), listOf(root)))
            assertTrue(PathGuard.allowed(File(root, "in"), listOf(root)))
            // the root itself reached through a symlink: canonical root is used
            val alias = File(outside, "alias")
            Files.createSymbolicLink(alias.toPath(), root.toPath())
            assertEquals(File(root, "in"), PathGuard.resolve(alias, "in"))
        } finally {
            root.deleteRecursively()
            outside.deleteRecursively()
        }
    }

    @Test
    fun names() {
        val taken = mutableSetOf("IMG_1.jpg", "IMG_1 (1).jpg", "notes", ".hidden", "a.b.c.tar")
        assertEquals("IMG_2.jpg", PathGuard.uniqueName("IMG_2.jpg") { it in taken })
        assertEquals("IMG_1 (2).jpg", PathGuard.uniqueName("IMG_1.jpg") { it in taken })
        assertEquals("notes (1)", PathGuard.uniqueName("notes") { it in taken })
        assertEquals(".hidden (1)", PathGuard.uniqueName(".hidden") { it in taken })
        assertEquals("a.b.c (1).tar", PathGuard.uniqueName("a.b.c.tar") { it in taken })
        assertEquals("IMG.jpg", PathGuard.cleanName(" IMG.jpg "))
        for (bad in listOf("", ".", "..", "a/b", "a\\b", "x\ny")) {
            try {
                PathGuard.cleanName(bad)
                fail(bad)
            } catch (e: LinkError) {
                assertEquals(400, e.status)
            }
        }
        val root = tmp()
        try {
            File(root, "IMG.jpg").writeText("1")
            assertEquals("IMG (1).jpg", PeekFs.uniqueFile(root, "IMG.jpg").name)
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun ranges() {
        assertEquals(ByteRange.Full, Ranges.parse(null, 100))
        assertEquals(ByteRange.Full, Ranges.parse("items=0-1", 100))
        assertEquals(ByteRange.Part(0, 99), Ranges.parse("bytes=0-", 100))
        assertEquals(ByteRange.Part(10, 19), Ranges.parse("bytes=10-19", 100))
        assertEquals(ByteRange.Part(10, 99), Ranges.parse("bytes=10-1000", 100)) // end clamped
        assertEquals(ByteRange.Part(90, 99), Ranges.parse("bytes=-10", 100)) // suffix
        assertEquals(ByteRange.Part(0, 99), Ranges.parse("bytes=-500", 100)) // suffix longer than file
        assertEquals(ByteRange.Part(5, 9), Ranges.parse("bytes=5-9, 20-30", 100)) // first of many
        assertEquals(ByteRange.Part(99, 99), Ranges.parse("bytes=99-", 100))
        assertEquals(ByteRange.Unsatisfiable, Ranges.parse("bytes=100-", 100))
        assertEquals(ByteRange.Unsatisfiable, Ranges.parse("bytes=5000-", 4096))
        assertEquals(ByteRange.Unsatisfiable, Ranges.parse("bytes=-0", 100))
        assertEquals(ByteRange.Full, Ranges.parse("bytes=20-10", 100)) // invalid → ignore
        assertEquals(ByteRange.Full, Ranges.parse("bytes=abc", 100))
        val r = FileReplies.build("bytes=-10", 100, 1, "image/jpeg")
        assertEquals(206, r.status)
        assertEquals("bytes 90-99/100", r.headers["X-Peek-Content-Range"])
        assertEquals("10", r.headers["Content-Length"])
        assertEquals(416, FileReplies.build("bytes=100-", 100, 1, "image/jpeg").status)
    }

    @Test
    fun iso6709() {
        assertArrayEquals(doubleArrayOf(41.0082, 28.9784), Iso6709.parse("+41.0082+028.9784/"), 1e-9)
        assertArrayEquals(doubleArrayOf(37.4219, -122.084), Iso6709.parse("+37.4219-122.0840/"), 1e-9)
        assertArrayEquals(doubleArrayOf(-33.8688, 151.2093), Iso6709.parse("-33.8688+151.2093+012.300/"), 1e-9) // altitude
        assertArrayEquals(doubleArrayOf(41.0082, 28.9784), Iso6709.parse("+41.0082+028.9784"), 1e-9) // no slash
        assertArrayEquals(doubleArrayOf(40.5, -73.75), Iso6709.parse("+4030-07345/"), 1e-9) // DDMM
        assertArrayEquals(doubleArrayOf(40.0 + 30.0 / 60 + 36.0 / 3600, -(73 + 45.0 / 60 + 18.0 / 3600)), Iso6709.parse("+403036-0734518/"), 1e-9) // DDMMSS
        assertArrayEquals(doubleArrayOf(41.0, 29.0), Iso6709.parse("+41+029/"), 1e-9)
        assertArrayEquals(doubleArrayOf(48.8584, 2.2945), Iso6709.parse("+48.8584+002.2945+035.000CRSWGS_84/"), 1e-9)
        assertNull(Iso6709.parse("+00.0000+000.0000/")) // null island
        assertNull(Iso6709.parse("+91.0000+000.0000/"))
        assertNull(Iso6709.parse("+41.0+190.0/"))
        assertNull(Iso6709.parse(""))
        assertNull(Iso6709.parse(null))
        assertNull(Iso6709.parse("garbage"))
        assertNull(Iso6709.parse("+4075-07345/")) // 75 minutes
    }

    @Test
    fun backoff() {
        val r = Random(1)
        var last = 0L
        for (a in 0..10) {
            val d = Backoff.delayMs(a, r)
            assertTrue(d in 500..72_000)
            if (a in 1..5) assertTrue(d > last / 3)
            last = d
        }
        assertTrue(Backoff.delayMs(20, r) <= 72_000)
        assertTrue(Backoff.delayMs(0, r) in 800..1200)
    }

    @Test
    fun json() {
        assertEquals("\"a\\\"b\\\\c\\n\\u0001\"", Json.quote("a\"b\\c\n\u0001"))
        assertEquals("12", Json.num(12.0))
        assertEquals("12.5", Json.num(12.5))
        assertEquals("{\"p\":\"Ş/ğ.jpg\",\"k\":\"image\",\"s\":1,\"m\":2,\"w\":null,\"h\":null,\"d\":null,\"t\":null,\"id\":null}", LinkCodec.listLine("Ş/ğ.jpg", "image", 1, 2, null, null, null, null, null))
    }
}
