/** Minimal TrueType/OpenType 'name' + 'OS/2' reader (family, subfamily, weight) — no dependencies. */
export function readFontInfo(buf: ArrayBuffer, fileName: string): { family: string; subfamily: string; weight: number; italic: boolean } {
  const fallback = { family: fileName.replace(/\.[^.]+$/, '').replace(/[-_]+/g, ' '), subfamily: 'Regular', weight: 400, italic: false };
  try {
    const v = new DataView(buf);
    const tag = v.getUint32(0);
    // only plain sfnt (TrueType 0x00010000 / 'OTTO' / 'true'); WOFF(2) are compressed → fallback
    if (tag !== 0x00010000 && tag !== 0x4f54544f && tag !== 0x74727565) return fallback;
    const numTables = v.getUint16(4);
    const tables: Record<string, { offset: number; length: number }> = {};
    for (let i = 0; i < numTables; i++) {
      const o = 12 + i * 16;
      const t = String.fromCharCode(v.getUint8(o), v.getUint8(o + 1), v.getUint8(o + 2), v.getUint8(o + 3));
      tables[t] = { offset: v.getUint32(o + 8), length: v.getUint32(o + 12) };
    }
    const names: Record<number, string> = {};
    const name = tables['name'];
    if (name) {
      const count = v.getUint16(name.offset + 2);
      const strOff = name.offset + v.getUint16(name.offset + 4);
      for (let i = 0; i < count; i++) {
        const r = name.offset + 6 + i * 12;
        const platform = v.getUint16(r);
        const encoding = v.getUint16(r + 2);
        const id = v.getUint16(r + 6);
        const len = v.getUint16(r + 8);
        const off = strOff + v.getUint16(r + 10);
        if (![1, 2, 4, 16, 17].includes(id)) continue;
        let s = '';
        if (platform === 3 || platform === 0) {
          for (let k = 0; k < len; k += 2) s += String.fromCharCode(v.getUint16(off + k));
        } else if (platform === 1 && encoding === 0) {
          for (let k = 0; k < len; k++) s += String.fromCharCode(v.getUint8(off + k));
        } else continue;
        if (!names[id] || platform === 3) names[id] = s;
      }
    }
    let weight = 400;
    let italic = false;
    const os2 = tables['OS/2'];
    if (os2 && os2.length >= 64) {
      weight = v.getUint16(os2.offset + 4) || 400;
      italic = (v.getUint16(os2.offset + 62) & 1) === 1;
    }
    const subfamily = names[17] || names[2] || 'Regular';
    return {
      family: (names[16] || names[1] || fallback.family).trim(),
      subfamily,
      weight,
      italic: italic || /italic|oblique/i.test(subfamily),
    };
  } catch {
    return fallback;
  }
}
