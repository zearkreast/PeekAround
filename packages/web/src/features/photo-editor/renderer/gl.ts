import { straightenZoom } from '../geometry';
import type { Renderer } from './types';

const VERT = `#version 300 es
in vec2 a_pos;
void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }`;

/**
 * One pass does everything: view → frame → (straighten, flips, quarter turns) → source uv,
 * then blur/sharpen taps, white balance, tone, colour, split tone, fade, vignette and grain.
 * Rendering the same shader into a screen-sized canvas (preview) or into full-resolution
 * tiles (export) keeps both identical.
 */
const FRAG = `#version 300 es
precision highp float;
precision highp int;
uniform sampler2D u_tex;
uniform vec2 u_canvas;
uniform vec3 u_view;
uniform vec4 u_clip;
uniform vec4 u_crop;
uniform vec2 u_src;
uniform vec2 u_frame;
uniform float u_angle;
uniform float u_zoom;
uniform int u_rot;
uniform vec2 u_flip;
uniform float u_texScale;
uniform float u_exposure;
uniform float u_brightness;
uniform float u_contrast;
uniform float u_highlights;
uniform float u_shadows;
uniform float u_saturation;
uniform float u_vibrance;
uniform float u_temp;
uniform float u_tint;
uniform float u_hue;
uniform float u_fade;
uniform float u_vignette;
uniform float u_grain;
uniform float u_grainCell;
uniform float u_sharpen;
uniform float u_sharpStep;
uniform float u_blurRadius;
uniform vec3 u_splitS;
uniform vec3 u_splitH;
out vec4 outColor;

vec2 frameToSrc(vec2 f) {
  vec2 c = u_frame * 0.5;
  vec2 d = (f - c) / u_zoom;
  float cs = cos(-u_angle);
  float sn = sin(-u_angle);
  vec2 p = c + vec2(cs * d.x - sn * d.y, sn * d.x + cs * d.y);
  if (u_flip.x > 0.5) p.x = u_frame.x - p.x;
  if (u_flip.y > 0.5) p.y = u_frame.y - p.y;
  if (u_rot == 1) return vec2(p.y, u_src.y - p.x);
  if (u_rot == 2) return vec2(u_src.x - p.x, u_src.y - p.y);
  if (u_rot == 3) return vec2(u_src.x - p.y, p.x);
  return p;
}

float luma(vec3 c) { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }

float hash(vec2 p) {
  uvec2 q = uvec2(ivec2(p) + 65536);
  uint h = (q.x * 1597334677u) ^ (q.y * 3812015801u);
  h = (h ^ (h >> 16u)) * 0x7feb352du;
  h = (h ^ (h >> 15u)) * 0x846ca68bu;
  h = h ^ (h >> 16u);
  return float(h) / 4294967295.0;
}

vec3 hueRotate(vec3 c, float a) {
  const vec3 k = vec3(0.57735026);
  float ca = cos(a);
  return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca);
}

void main() {
  vec2 px = vec2(gl_FragCoord.x, u_canvas.y - gl_FragCoord.y);
  vec2 f = u_view.xy + px * u_view.z;
  if (f.x < u_clip.x || f.y < u_clip.y || f.x > u_clip.x + u_clip.z || f.y > u_clip.y + u_clip.w) {
    outColor = vec4(0.0);
    return;
  }
  vec2 uv = frameToSrc(f) / u_src;
  vec4 base = texture(u_tex, uv);
  vec3 c = base.rgb;

  if (u_blurRadius > 0.0) {
    float lod = max(0.0, log2(max(1.0, u_blurRadius * u_texScale / 3.0)));
    vec3 acc = textureLod(u_tex, uv, lod).rgb;
    for (int i = 0; i < 24; i++) {
      float fi = float(i) + 0.5;
      float rr = sqrt(fi / 24.0) * u_blurRadius;
      float th = fi * 2.3999632;
      acc += textureLod(u_tex, uv + vec2(cos(th), sin(th)) * rr / u_src, lod).rgb;
    }
    c = acc / 25.0;
  }
  if (u_sharpen > 0.0) {
    vec2 st = vec2(u_sharpStep) / u_src;
    vec3 n = texture(u_tex, uv + vec2(st.x, 0.0)).rgb + texture(u_tex, uv - vec2(st.x, 0.0)).rgb
           + texture(u_tex, uv + vec2(0.0, st.y)).rgb + texture(u_tex, uv - vec2(0.0, st.y)).rgb;
    c += (c - n * 0.25) * u_sharpen * 2.2;
  }

  // white balance
  c *= vec3(1.0 + u_temp * 0.2, 1.0 - u_tint * 0.14 + abs(u_temp) * 0.02, 1.0 - u_temp * 0.2);
  // exposure (in stops) & brightness (midtone gamma)
  c = pow(pow(max(c, 0.0), vec3(2.2)) * exp2(u_exposure * 1.6), vec3(1.0 / 2.2));
  c = pow(max(c, 0.0), vec3(exp2(-u_brightness * 0.75)));
  // contrast around mid grey
  float ck = u_contrast >= 0.0 ? 1.0 + u_contrast * 1.1 : 1.0 + u_contrast * 0.75;
  c = (c - 0.5) * ck + 0.5;
  // highlights & shadows
  float l = clamp(luma(c), 0.0, 1.0);
  float sh = 1.0 - smoothstep(0.0, 0.62, l);
  float hi = smoothstep(0.38, 1.0, l);
  c += u_shadows * 0.32 * sh + u_highlights * 0.3 * hi;
  // saturation & vibrance
  float L = luma(c);
  float mx = max(c.r, max(c.g, c.b));
  float mn = min(c.r, min(c.g, c.b));
  float vib = u_vibrance * (1.0 - smoothstep(0.0, 0.7, mx - mn)) * 1.2;
  c = mix(vec3(L), c, max(0.0, 1.0 + u_saturation + vib));
  if (u_hue != 0.0) c = hueRotate(c, u_hue);
  // split toning
  L = clamp(luma(c), 0.0, 1.0);
  c += u_splitS * (1.0 - L) + u_splitH * L;
  // fade / matte
  c = mix(c, c * 0.8 + 0.13, u_fade);
  // vignette, relative to the output rect
  vec2 q = (f - u_crop.xy) / u_crop.zw - 0.5;
  float v = smoothstep(0.3, 1.12, length(q) * 1.41421);
  if (u_vignette > 0.0) c *= 1.0 - u_vignette * 0.88 * v;
  else c = mix(c, vec3(1.0), -u_vignette * 0.75 * v);
  // film grain
  if (u_grain > 0.0) {
    float n = hash(floor(f / u_grainCell)) - 0.5;
    float lg = clamp(luma(c), 0.0, 1.0);
    c += n * u_grain * 0.22 * (1.0 - abs(lg - 0.5));
  }
  c = clamp(c, 0.0, 1.0);
  outColor = vec4(c * base.a, base.a);
}`;

const UNIFORMS = [
  'u_tex',
  'u_canvas',
  'u_view',
  'u_clip',
  'u_crop',
  'u_src',
  'u_frame',
  'u_angle',
  'u_zoom',
  'u_rot',
  'u_flip',
  'u_texScale',
  'u_exposure',
  'u_brightness',
  'u_contrast',
  'u_highlights',
  'u_shadows',
  'u_saturation',
  'u_vibrance',
  'u_temp',
  'u_tint',
  'u_hue',
  'u_fade',
  'u_vignette',
  'u_grain',
  'u_grainCell',
  'u_sharpen',
  'u_sharpStep',
  'u_blurRadius',
  'u_splitS',
  'u_splitH',
] as const;

type Uniform = (typeof UNIFORMS)[number];

function compile(gl: WebGL2RenderingContext, type: number, src: string) {
  const s = gl.createShader(type)!;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader compile failed');
  return s;
}

/** Returns null when WebGL2 is unavailable (caller falls back to canvas 2D). */
export function createGlRenderer(opts: { preserve?: boolean } = {}): Renderer | null {
  const canvas = document.createElement('canvas');
  const gl = canvas.getContext('webgl2', { premultipliedAlpha: true, preserveDrawingBuffer: !!opts.preserve, antialias: false, alpha: true });
  if (!gl) return null;

  let program: WebGLProgram;
  let loc: Record<Uniform, WebGLUniformLocation | null>;
  let tex: WebGLTexture | null = null;
  let source: { img: HTMLImageElement | HTMLCanvasElement | ImageBitmap; w: number; h: number } | null = null;
  let texScale = 1;
  let lost = false;

  const maxTex = gl.getParameter(gl.MAX_TEXTURE_SIZE) as number;
  const vp = gl.getParameter(gl.MAX_VIEWPORT_DIMS) as Int32Array;
  const maxSize = Math.min(4096, vp[0], vp[1], gl.getParameter(gl.MAX_RENDERBUFFER_SIZE) as number);

  const init = () => {
    program = gl.createProgram()!;
    gl.attachShader(program, compile(gl, gl.VERTEX_SHADER, VERT));
    gl.attachShader(program, compile(gl, gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program) ?? 'link failed');
    gl.useProgram(program);
    loc = Object.fromEntries(UNIFORMS.map((u) => [u, gl.getUniformLocation(program, u)])) as Record<Uniform, WebGLUniformLocation | null>;
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const a = gl.getAttribLocation(program, 'a_pos');
    gl.enableVertexAttribArray(a);
    gl.vertexAttribPointer(a, 2, gl.FLOAT, false, 0, 0);
    tex = null;
    if (source) upload();
  };

  const upload = () => {
    if (!source) return;
    let img: TexImageSource = source.img;
    let tw = source.w;
    let th = source.h;
    if (Math.max(tw, th) > maxTex) {
      // too big for a texture → downscale once (rare: > 16k px on most GPUs)
      const k = maxTex / Math.max(tw, th);
      tw = Math.floor(tw * k);
      th = Math.floor(th * k);
      const c = document.createElement('canvas');
      c.width = tw;
      c.height = th;
      c.getContext('2d')!.drawImage(source.img, 0, 0, tw, th);
      img = c;
    }
    texScale = tw / source.w;
    if (tex) gl.deleteTexture(tex);
    tex = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, img);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  };

  canvas.addEventListener('webglcontextlost', (e) => {
    e.preventDefault();
    lost = true;
  });
  canvas.addEventListener('webglcontextrestored', () => {
    lost = false;
    init();
  });

  try {
    init();
  } catch (e) {
    console.error('[photo-editor] WebGL init failed', e);
    return null;
  }

  return {
    kind: 'webgl2',
    canvas,
    maxSize,
    setSource(img, w, h) {
      source = { img, w, h };
      if (!lost) upload();
    },
    hasSource: () => !!tex,
    render(p, width, height) {
      if (lost) return;
      width = Math.max(1, Math.round(width));
      height = Math.max(1, Math.round(height));
      if (canvas.width !== width) canvas.width = width;
      if (canvas.height !== height) canvas.height = height;
      gl.viewport(0, 0, width, height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      if (!tex) return;
      gl.useProgram(program);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, tex);
      const a = p.adjust;
      const zoom = straightenZoom(p.frameW, p.frameH, p.straighten);
      gl.uniform1i(loc.u_tex, 0);
      gl.uniform2f(loc.u_canvas, width, height);
      gl.uniform3f(loc.u_view, p.ox, p.oy, p.k);
      gl.uniform4f(loc.u_clip, p.clip.x, p.clip.y, p.clip.w, p.clip.h);
      gl.uniform4f(loc.u_crop, p.crop.x, p.crop.y, p.crop.w, p.crop.h);
      gl.uniform2f(loc.u_src, p.srcW, p.srcH);
      gl.uniform2f(loc.u_frame, p.frameW, p.frameH);
      gl.uniform1f(loc.u_angle, (p.straighten * Math.PI) / 180);
      gl.uniform1f(loc.u_zoom, zoom);
      gl.uniform1i(loc.u_rot, p.rot);
      gl.uniform2f(loc.u_flip, p.flipH ? 1 : 0, p.flipV ? 1 : 0);
      gl.uniform1f(loc.u_texScale, texScale);
      gl.uniform1f(loc.u_exposure, a.exposure);
      gl.uniform1f(loc.u_brightness, a.brightness);
      gl.uniform1f(loc.u_contrast, a.contrast);
      gl.uniform1f(loc.u_highlights, a.highlights);
      gl.uniform1f(loc.u_shadows, a.shadows);
      gl.uniform1f(loc.u_saturation, a.saturation);
      gl.uniform1f(loc.u_vibrance, a.vibrance);
      gl.uniform1f(loc.u_temp, a.temperature);
      gl.uniform1f(loc.u_tint, a.tint);
      gl.uniform1f(loc.u_hue, a.hue);
      gl.uniform1f(loc.u_fade, a.fade);
      gl.uniform1f(loc.u_vignette, a.vignette);
      gl.uniform1f(loc.u_grain, a.grain);
      gl.uniform1f(loc.u_grainCell, p.grainCell);
      gl.uniform1f(loc.u_sharpen, a.sharpen);
      gl.uniform1f(loc.u_sharpStep, p.sharpenStep);
      gl.uniform1f(loc.u_blurRadius, a.blur > 0 ? a.blur * Math.min(p.srcW, p.srcH) * 0.025 : 0);
      gl.uniform3f(loc.u_splitS, ...a.split.shadows);
      gl.uniform3f(loc.u_splitH, ...a.split.highlights);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    },
    dispose() {
      if (tex) gl.deleteTexture(tex);
      tex = null;
      source = null;
      gl.getExtension('WEBGL_lose_context')?.loseContext();
      canvas.width = canvas.height = 1;
    },
  };
}

