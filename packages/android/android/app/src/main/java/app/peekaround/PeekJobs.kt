package app.peekaround

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.media3.common.C
import androidx.media3.common.Effect
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.ChannelMixingAudioProcessor
import androidx.media3.common.audio.ChannelMixingMatrix
import androidx.media3.common.audio.SpeedProvider
import androidx.media3.common.util.UnstableApi
import androidx.media3.effect.BitmapOverlay
import androidx.media3.effect.Brightness
import androidx.media3.effect.Contrast
import androidx.media3.effect.Crop
import androidx.media3.effect.HslAdjustment
import androidx.media3.effect.MatrixTransformation
import androidx.media3.effect.OverlayEffect
import androidx.media3.effect.Presentation
import androidx.media3.effect.RgbAdjustment
import androidx.media3.effect.RgbMatrix
import androidx.media3.effect.ScaleAndRotateTransformation
import androidx.media3.transformer.Composition
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.EditedMediaItemSequence
import androidx.media3.transformer.Effects
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.ProgressHolder
import androidx.media3.transformer.Transformer
import com.getcapacitor.JSObject
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Video editing + slideshow export on the phone with Media3 Transformer (hardware encoders). */
@UnstableApi
class PeekJobs(private val ctx: Context, private val emit: (JSObject) -> Unit) {
    private val main = Handler(Looper.getMainLooper())
    private val running = HashMap<String, Transformer>()

    private fun event(id: String, title: String, type: String, status: String, progress: Double, error: String? = null, result: File? = null) {
        val o = JSObject()
        o.put("id", id); o.put("title", title); o.put("type", type); o.put("status", status)
        o.put("progress", progress); o.put("error", error)
        if (result != null) o.put("resultPath", result.absolutePath)
        emit(o)
    }

    fun cancel(id: String) = main.post { running.remove(id)?.cancel() }

    private fun run(id: String, title: String, type: String, part: File, dest: File, build: () -> Pair<Transformer.Builder, Any>) {
        main.post {
            try {
                val (builder, what) = build()
                val holder = ProgressHolder()
                lateinit var transformer: Transformer
                val listener = object : Transformer.Listener {
                    override fun onCompleted(composition: Composition, exportResult: ExportResult) {
                        running.remove(id)
                        val ok = part.exists() && part.length() > 0 && part.renameTo(dest)
                        if (ok) {
                            PeekFs.scan(ctx, dest.absolutePath)
                            event(id, title, type, "done", 1.0, result = dest)
                        } else event(id, title, type, "error", 1.0, "Output could not be written")
                    }

                    override fun onError(composition: Composition, exportResult: ExportResult, exportException: ExportException) {
                        running.remove(id)
                        part.delete()
                        event(id, title, type, "error", 0.0, exportException.message ?: "Export failed")
                    }
                }
                transformer = builder.addListener(listener).build()
                running[id] = transformer
                event(id, title, type, "running", 0.0)
                when (what) {
                    is Composition -> transformer.start(what, part.absolutePath)
                    is EditedMediaItem -> transformer.start(what, part.absolutePath)
                }
                val poll = object : Runnable {
                    override fun run() {
                        if (!running.containsKey(id)) return
                        if (transformer.getProgress(holder) == Transformer.PROGRESS_STATE_AVAILABLE) {
                            event(id, title, type, "running", holder.progress / 100.0)
                        }
                        main.postDelayed(this, 400)
                    }
                }
                main.postDelayed(poll, 400)
            } catch (e: Exception) {
                part.delete()
                event(id, title, type, "error", 0.0, e.message ?: e.toString())
            }
        }
    }

    // ───────────────────────── video edit ─────────────────────────

    fun videoEdit(id: String, o: JSONObject, suffix: String): File {
        val src = File(o.getString("input"))
        require(src.isFile) { "Source not found" }
        val base = src.name.substringBeforeLast('.')
        val dest = PeekFs.uniqueEditFile(src.parentFile!!, base, suffix, "mp4")
        val part = File(dest.path + ".part.mp4")
        run(id, src.name, "video-edit", part, dest) {
            val clip = MediaItem.ClippingConfiguration.Builder()
            o.optJSONObject("trim")?.let {
                clip.setStartPositionMs((it.optDouble("start", 0.0) * 1000).toLong())
                clip.setEndPositionMs((it.optDouble("end", 0.0) * 1000).toLong())
            }
            val item = MediaItem.Builder().setUri(Uri.fromFile(src)).setClippingConfiguration(clip.build()).build()

            val video = ArrayList<Effect>()
            val srcW = o.optInt("srcWidth", 0)
            val srcH = o.optInt("srcHeight", 0)
            o.optJSONObject("crop")?.let { c ->
                if (srcW > 0 && srcH > 0) {
                    val x = c.getDouble("x"); val y = c.getDouble("y"); val w = c.getDouble("w"); val h = c.getDouble("h")
                    val left = (x / srcW * 2 - 1).toFloat()
                    val right = ((x + w) / srcW * 2 - 1).toFloat()
                    val top = (1 - y / srcH * 2).toFloat()
                    val bottom = (1 - (y + h) / srcH * 2).toFloat()
                    video.add(Crop(left, right, bottom, top))
                }
            }
            val rot = o.optInt("rotate", 0)
            val fh = o.optBoolean("flipH", false)
            val fv = o.optBoolean("flipV", false)
            if (rot != 0 || fh || fv) {
                video.add(
                    ScaleAndRotateTransformation.Builder()
                        .setScale(if (fh) -1f else 1f, if (fv) -1f else 1f)
                        .setRotationDegrees(-rot.toFloat()) // media3 rotates counter-clockwise
                        .build(),
                )
            }
            o.optJSONObject("adjust")?.let { a ->
                val b = a.optDouble("brightness", 0.0)
                val c = a.optDouble("contrast", 0.0)
                val s = a.optDouble("saturation", 0.0)
                val hue = a.optDouble("hue", 0.0)
                val warm = a.optDouble("warmth", 0.0)
                if (b != 0.0) video.add(Brightness((b * 0.5).toFloat().coerceIn(-1f, 1f)))
                if (c != 0.0) video.add(Contrast(c.toFloat().coerceIn(-1f, 1f)))
                if (s != 0.0 || hue != 0.0) video.add(HslAdjustment.Builder().adjustSaturation((s * 100).toFloat()).adjustHue(hue.toFloat()).build())
                if (warm != 0.0) {
                    val w = warm.toFloat()
                    video.add(RgbAdjustment.Builder().setRedScale(1f + 0.18f * w).setGreenScale(1f + 0.03f * w).setBlueScale(1f - 0.18f * w).build())
                }
            }
            val outH = o.optInt("height", 0)
            if (outH > 0) video.add(Presentation.createForHeight(outH))
            val overlays = o.optJSONArray("overlays") ?: JSONArray()
            if (overlays.length() > 0) {
                val list = (0 until overlays.length()).map { i ->
                    val ov = overlays.getJSONObject(i)
                    TimedBitmapOverlay(File(ov.getString("path")), (ov.getDouble("start") * 1e6).toLong(), (ov.getDouble("end") * 1e6).toLong())
                }
                video.add(OverlayEffect(list))
            }

            val audio = ArrayList<AudioProcessor>()
            val volume = o.optDouble("volume", 1.0).toFloat()
            if (volume != 1f && !o.optBoolean("mute", false)) {
                val mixer = ChannelMixingAudioProcessor()
                for (ch in 1..2) mixer.putChannelMixingMatrix(ChannelMixingMatrix.createForConstantGain(ch, ch).scaleBy(volume))
                audio.add(mixer)
            }

            val edited = EditedMediaItem.Builder(item)
                .setRemoveAudio(o.optBoolean("mute", false))
                .setEffects(Effects(audio, video))
            val speed = o.optDouble("speed", 1.0).toFloat()
            if (speed != 1f) edited.setSpeed(ConstantSpeed(speed))
            Pair(Transformer.Builder(ctx).setVideoMimeType(MimeTypes.VIDEO_H264).setAudioMimeType(MimeTypes.AUDIO_AAC), edited.build())
        }
        return dest
    }

    // ───────────────────────── slideshow ─────────────────────────

    fun slideshow(id: String, o: JSONObject): File {
        val items = o.getJSONArray("items")
        require(items.length() > 0) { "No items" }
        val dir = File(o.getString("dir"))
        dir.mkdirs()
        val dest = PeekFs.uniqueFile(dir, "${o.optString("name", "Slayt").ifBlank { "Slayt" }}.mp4")
        val part = File(dest.path + ".part.mp4")
        val w = o.optInt("width", 1920)
        val h = o.optInt("height", 1080)
        val fps = o.optInt("fps", 30)
        val kenBurns = o.optBoolean("kenBurns", true)
        val fade = o.optString("transition", "fade") != "none"
        val fadeUs = (o.optDouble("transitionDuration", 0.8) * 1e6 / 2).toLong()
        run(id, dest.name, "slideshow", part, dest) {
            val seq = ArrayList<EditedMediaItem>()
            for (i in 0 until items.length()) {
                val it = items.getJSONObject(i)
                val f = File(it.getString("path"))
                val isVideo = it.optString("kind") == "video"
                val durMs = (it.optDouble("duration", o.optDouble("duration", 4.0)) * 1000).toLong()
                val mi = MediaItem.Builder().setUri(Uri.fromFile(f))
                if (isVideo) {
                    val c = MediaItem.ClippingConfiguration.Builder()
                    it.optJSONObject("trim")?.let { t ->
                        c.setStartPositionMs((t.optDouble("start", 0.0) * 1000).toLong()); c.setEndPositionMs((t.optDouble("end", 0.0) * 1000).toLong())
                    } ?: c.setEndPositionMs(durMs)
                    mi.setClippingConfiguration(c.build())
                } else {
                    mi.setImageDurationMs(durMs)
                }
                val effects = ArrayList<Effect>()
                effects.add(Presentation.createForWidthAndHeight(w, h, if (kenBurns && !isVideo) Presentation.LAYOUT_SCALE_TO_FIT_WITH_CROP else Presentation.LAYOUT_SCALE_TO_FIT))
                if (kenBurns && !isVideo) effects.add(kenBurnsEffect(i, durMs * 1000))
                if (fade) effects.add(FadeMatrix(durMs * 1000, fadeUs))
                val e = EditedMediaItem.Builder(mi.build()).setRemoveAudio(true).setEffects(Effects(emptyList(), effects))
                if (!isVideo) e.setFrameRate(fps)
                seq.add(e.build())
            }
            val sequences = ArrayList<EditedMediaItemSequence>()
            sequences.add(EditedMediaItemSequence.Builder(seq).build())
            val audioPath = o.optString("audio", "")
            if (audioPath.isNotEmpty() && File(audioPath).isFile) {
                val music = EditedMediaItem.Builder(MediaItem.fromUri(Uri.fromFile(File(audioPath)))).setRemoveVideo(true).build()
                sequences.add(EditedMediaItemSequence.Builder(music).setIsLooping(true).build())
            }
            val comp = Composition.Builder(sequences).build()
            Pair(Transformer.Builder(ctx).setVideoMimeType(MimeTypes.VIDEO_H264).setAudioMimeType(MimeTypes.AUDIO_AAC), comp)
        }
        return dest
    }

    /** Slow zoom + drift; alternates direction per slide. Timestamps are made relative to the first frame. */
    private fun kenBurnsEffect(index: Int, durUs: Long): MatrixTransformation {
        var start = -1L
        val zoomIn = index % 2 == 0
        val dx = if (index % 4 < 2) 0.04f else -0.04f
        return MatrixTransformation { t ->
            if (start < 0) start = t
            val p = ((t - start).toFloat() / durUs).coerceIn(0f, 1f)
            val s = if (zoomIn) 1.0f + 0.12f * p else 1.12f - 0.12f * p
            Matrix().apply {
                postScale(s, s)
                postTranslate(dx * (p - 0.5f), 0f)
            }
        }
    }

    private class FadeMatrix(private val durUs: Long, private val fadeUs: Long) : RgbMatrix {
        private var start = -1L
        override fun getMatrix(presentationTimeUs: Long, useHdr: Boolean): FloatArray {
            if (start < 0) start = presentationTimeUs
            val t = presentationTimeUs - start
            val a = if (fadeUs <= 0) 1f else minOf(1f, t.toFloat() / fadeUs, (durUs - t).toFloat() / fadeUs).coerceIn(0f, 1f)
            return floatArrayOf(a, 0f, 0f, 0f, 0f, a, 0f, 0f, 0f, 0f, a, 0f, 0f, 0f, 0f, 1f)
        }
    }

    private class ConstantSpeed(private val speed: Float) : SpeedProvider {
        override fun getSpeed(timeUs: Long): Float = speed
        override fun getNextSpeedChangeTimeUs(timeUs: Long): Long = C.TIME_UNSET
    }

    /** Full-frame transparent PNG overlay that is only visible between start and end (output time). */
    private class TimedBitmapOverlay(file: File, private val startUs: Long, private val endUs: Long) : BitmapOverlay() {
        private val bitmap: Bitmap = BitmapFactory.decodeFile(file.absolutePath)
        private val empty: Bitmap = Bitmap.createBitmap(1, 1, Bitmap.Config.ARGB_8888)
        override fun getBitmap(presentationTimeUs: Long): Bitmap = if (presentationTimeUs in startUs..endUs) bitmap else empty
    }
}
