package app.peekaround

import android.media.MediaMetadataRetriever
import androidx.exifinterface.media.ExifInterface
import org.json.JSONObject
import java.io.File
import java.io.FileInputStream

/**
 * Grouping metadata read on the phone (GPS, camera, capture-time offset, panorama / animation
 * hints) for the link `details` op and the local PeekIndex.
 * GPS in files of other apps is only visible with ACCESS_MEDIA_LOCATION (Android 10+).
 */
object PeekMeta {
    /** PeekIndex flags hint bits */
    const val FLAG_GPANO = 1
    const val FLAG_ANIMATED = 2

    data class Row(
        val lat: Double?, val lon: Double?, val make: String?, val model: String?, val lens: String?,
        /** capture-time UTC offset in minutes (OffsetTimeOriginal) */
        val offset: Int?, val flags: Int,
    )

    private fun str(v: String?): String? = v?.replace("\u0000", "")?.trim()?.takeIf { it.isNotEmpty() }?.take(80)

    /** "+03:00" / "-0530" / "Z" → minutes */
    fun parseOffset(s: String?): Int? {
        val v = s?.trim() ?: return null
        if (v == "Z") return 0
        val m = Regex("^([+-])(\\d{2}):?(\\d{2})$").matchEntire(v) ?: return null
        val n = m.groupValues[2].toInt() * 60 + m.groupValues[3].toInt()
        if (n > 14 * 60) return null
        return if (m.groupValues[1] == "-") -n else n
    }

    private fun validGps(lat: Double, lon: Double) = lat.isFinite() && lon.isFinite() && lat in -90.0..90.0 && lon in -180.0..180.0 && !(lat == 0.0 && lon == 0.0)

    fun gpano(ex: ExifInterface): Boolean {
        val xmp = try { ex.getAttributeBytes(ExifInterface.TAG_XMP) } catch (_: Exception) { null } ?: return false
        val s = String(xmp, Charsets.UTF_8)
        if (!s.contains("GPano")) return false
        return Regex("ProjectionType[\"'>=\\s]*(equirectangular|cylindrical)", RegexOption.IGNORE_CASE).containsMatchIn(s) ||
            Regex("UsePanoramaViewer[\"'>=\\s]*true", RegexOption.IGNORE_CASE).containsMatchIn(s)
    }

    /** GIF with ≥ 2 frames / animated WebP (VP8X animation flag) — header sniffing only. */
    fun animated(f: File): Boolean {
        val ext = PeekFs.ext(f.name)
        if (ext != "gif" && ext != "webp") return false
        return try {
            FileInputStream(f).use { input ->
                if (ext == "webp") {
                    val h = ByteArray(21)
                    if (input.read(h) < 21) return false
                    String(h, 12, 4, Charsets.US_ASCII) == "VP8X" && (h[20].toInt() and 0x02) != 0
                } else {
                    val buf = ByteArray(1 shl 20)
                    val n = input.read(buf).coerceAtLeast(0)
                    var gce = 0
                    for (i in 0 until n - 2) if (buf[i] == 0x21.toByte() && buf[i + 1] == 0xF9.toByte() && buf[i + 2] == 0x04.toByte()) {
                        gce++
                        if (gce >= 2) return true
                    }
                    false
                }
            }
        } catch (_: Exception) {
            false
        }
    }

    fun videoLocation(f: File): DoubleArray? {
        val r = MediaMetadataRetriever()
        return try {
            r.setDataSource(f.absolutePath)
            Iso6709.parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_LOCATION))
        } catch (_: Exception) {
            null
        } finally {
            try { r.release() } catch (_: Exception) {}
        }
    }

    /** PeekIndex row for an image / video. */
    fun row(f: File, kind: String): Row {
        if (kind == "video") {
            val loc = videoLocation(f)
            return Row(loc?.get(0), loc?.get(1), null, null, null, null, 0)
        }
        return try {
            val ex = ExifInterface(f.absolutePath)
            val ll = ex.latLong?.takeIf { validGps(it[0], it[1]) }
            var flags = 0
            if (gpano(ex)) flags = flags or FLAG_GPANO
            if (animated(f)) flags = flags or FLAG_ANIMATED
            Row(
                ll?.get(0), ll?.get(1),
                str(ex.getAttribute(ExifInterface.TAG_MAKE)), str(ex.getAttribute(ExifInterface.TAG_MODEL)), str(ex.getAttribute(ExifInterface.TAG_LENS_MODEL)),
                parseOffset(ex.getAttribute(ExifInterface.TAG_OFFSET_TIME_ORIGINAL) ?: ex.getAttribute(ExifInterface.TAG_OFFSET_TIME)),
                flags,
            )
        } catch (_: Exception) {
            Row(null, null, null, null, null, null, if (animated(f)) FLAG_ANIMATED else 0)
        }
    }

    /** "12/1,30/1,5/1" → "12:30:05" */
    private fun rationalTime(s: String?): String? {
        val parts = s?.split(',') ?: return null
        if (parts.size != 3) return null
        val v = parts.map { p ->
            val (a, b) = p.split('/').let { it[0].toDoubleOrNull() to (it.getOrNull(1)?.toDoubleOrNull() ?: 1.0) }
            if (a == null || b == 0.0) return null
            a / b
        }
        return String.format(java.util.Locale.ROOT, "%02d:%02d:%02d", v[0].toInt(), v[1].toInt(), v[2].toInt())
    }

    /** `details` op body: {"exif":{…},"gps":{"lat","lon"}|null,"make","model","lens"} (exifr-like tag names). */
    fun details(f: File, kind: String): JSONObject {
        val exif = JSONObject()
        var gps: DoubleArray? = null
        var make: String? = null
        var model: String? = null
        var lens: String? = null
        if (kind == "image") {
            try {
                val ex = ExifInterface(f.absolutePath)
                fun s(tag: String, key: String) { str(ex.getAttribute(tag))?.let { exif.put(key, it) } }
                fun d(tag: String, key: String) { ex.getAttributeDouble(tag, 0.0).takeIf { it > 0 }?.let { exif.put(key, it) } }
                fun i(tag: String, key: String) { ex.getAttributeInt(tag, 0).takeIf { it > 0 }?.let { exif.put(key, it) } }
                s(ExifInterface.TAG_MAKE, "Make"); s(ExifInterface.TAG_MODEL, "Model"); s(ExifInterface.TAG_LENS_MODEL, "LensModel")
                s(ExifInterface.TAG_DATETIME_ORIGINAL, "DateTimeOriginal"); s(ExifInterface.TAG_DATETIME_DIGITIZED, "CreateDate"); s(ExifInterface.TAG_DATETIME, "ModifyDate")
                s(ExifInterface.TAG_OFFSET_TIME_ORIGINAL, "OffsetTimeOriginal"); s(ExifInterface.TAG_OFFSET_TIME_DIGITIZED, "OffsetTimeDigitized"); s(ExifInterface.TAG_OFFSET_TIME, "OffsetTime")
                s(ExifInterface.TAG_GPS_DATESTAMP, "GPSDateStamp")
                rationalTime(ex.getAttribute(ExifInterface.TAG_GPS_TIMESTAMP))?.let { exif.put("GPSTimeStamp", it) }
                d(ExifInterface.TAG_F_NUMBER, "FNumber"); d(ExifInterface.TAG_EXPOSURE_TIME, "ExposureTime"); d(ExifInterface.TAG_FOCAL_LENGTH, "FocalLength")
                i(ExifInterface.TAG_PHOTOGRAPHIC_SENSITIVITY, "ISO"); i(ExifInterface.TAG_FOCAL_LENGTH_IN_35MM_FILM, "FocalLengthIn35mmFormat")
                i(ExifInterface.TAG_PIXEL_X_DIMENSION, "ExifImageWidth"); i(ExifInterface.TAG_PIXEL_Y_DIMENSION, "ExifImageHeight")
                i(ExifInterface.TAG_IMAGE_WIDTH, "ImageWidth"); i(ExifInterface.TAG_IMAGE_LENGTH, "ImageHeight")
                i(ExifInterface.TAG_ORIENTATION, "Orientation")
                s(ExifInterface.TAG_SOFTWARE, "Software")
                if (gpano(ex)) exif.put("ProjectionType", "equirectangular").put("UsePanoramaViewer", true)
                gps = ex.latLong?.takeIf { validGps(it[0], it[1]) }
                make = str(ex.getAttribute(ExifInterface.TAG_MAKE))
                model = str(ex.getAttribute(ExifInterface.TAG_MODEL))
                lens = str(ex.getAttribute(ExifInterface.TAG_LENS_MODEL))
            } catch (_: Exception) {
            }
        } else if (kind == "video") {
            gps = videoLocation(f)
        }
        return JSONObject()
            .put("exif", exif)
            .put("gps", gps?.let { JSONObject().put("lat", it[0]).put("lon", it[1]) } ?: JSONObject.NULL)
            .put("make", make ?: JSONObject.NULL)
            .put("model", model ?: JSONObject.NULL)
            .put("lens", lens ?: JSONObject.NULL)
    }
}
