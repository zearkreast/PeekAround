/**
 * Free-text search parsing — pure. "istanbul 2023 video", "Ağustos 2024", "haziran selfie".
 * Date words become a localAt range (or a month across all years), type words a MediaFlagName;
 * everything else is left as folded terms for the backend to match against
 * file name, place (city / province / country) and camera.
 */
import type { MediaFlagName } from './index';

const TR_FOLD: Record<string, string> = {
  İ: 'i', I: 'i', ı: 'i', i: 'i',
  Ş: 's', ş: 's',
  Ğ: 'g', ğ: 'g',
  Ü: 'u', ü: 'u',
  Ö: 'o', ö: 'o',
  Ç: 'c', ç: 'c',
};

/**
 * Case + diacritic folding with an explicit Turkish table, so "ISTANBUL", "istanbul", "İstanbul",
 * "Istanbul" and "ıstanbul" are all "istanbul" (independent of the runtime locale).
 */
export function foldText(s: string | null | undefined): string {
  if (!s) return '';
  let out = '';
  for (const ch of s) out += TR_FOLD[ch] ?? ch;
  return out
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .replace(/ß/g, 'ss')
    .toLowerCase();
}

/** [month, full names, abbreviations (only honoured next to a year: "agu 2024")] */
const monthNames: [number, string[], string[]][] = [
  [1, ['ocak', 'january'], ['oca', 'jan']],
  [2, ['subat', 'february'], ['sub', 'feb']],
  [3, ['mart', 'march'], ['mar']],
  [4, ['nisan', 'april'], ['nis', 'apr']],
  [5, ['mayis', 'may'], []],
  [6, ['haziran', 'june'], ['haz', 'jun']],
  [7, ['temmuz', 'july'], ['tem', 'jul']],
  [8, ['agustos', 'august'], ['agu', 'aug']],
  [9, ['eylul', 'september'], ['eyl', 'sep', 'sept']],
  [10, ['ekim', 'october'], ['eki', 'oct']],
  [11, ['kasim', 'november'], ['kas', 'nov']],
  [12, ['aralik', 'december'], ['ara', 'dec']],
];
const MONTHS = new Map<string, number>();
const MONTHS_SHORT = new Map<string, number>();
for (const [m, full, short] of monthNames) {
  for (const n of full) MONTHS.set(n, m);
  for (const n of short) MONTHS_SHORT.set(n, m);
}

/** multi-word phrases first (matched on folded text) */
const FLAG_WORDS: [string, MediaFlagName][] = [
  ['ekran goruntuleri', 'screenshot'],
  ['ekran goruntusu', 'screenshot'],
  ['ekran resmi', 'screenshot'],
  ['screen shots', 'screenshot'],
  ['screenshots', 'screenshot'],
  ['screenshot', 'screenshot'],
  ['videolar', 'video'],
  ['videos', 'video'],
  ['video', 'video'],
  ['selfies', 'selfie'],
  ['selfieler', 'selfie'],
  ['selfie', 'selfie'],
  ['ozcekim', 'selfie'],
  ['panoramalar', 'panorama'],
  ['panoramas', 'panorama'],
  ['panorama', 'panorama'],
  ['animasyonlar', 'animated'],
  ['animasyon', 'animated'],
  ['animated', 'animated'],
  ['animations', 'animated'],
  ['hareketli', 'animated'],
  ['gifler', 'animated'],
  ['gifs', 'animated'],
  ['gif', 'animated'],
];

export interface ParsedSearch {
  /** remaining free text (folded, single spaced) */
  text: string;
  /** remaining folded words; each must match name / place / camera */
  terms: string[];
  /** localAt range [from, to) */
  from?: number;
  to?: number;
  /** month (1-12) across all years when no year was given */
  month?: number;
  flag?: MediaFlagName;
}

const PUNCT = "._'-";

/** strip leading/trailing . _ ' - (linear; a `[...]+$` regex backtracks quadratically) */
function trimPunct(w: string): string {
  let a = 0;
  let b = w.length;
  while (a < b && PUNCT.includes(w[a])) a++;
  while (b > a && PUNCT.includes(w[b - 1])) b--;
  return w.slice(a, b);
}

/** @param _now reserved for relative words ("dün", "geçen yıl"); keeps the function deterministic */
export function parseSearch(input: string, _now?: number): ParsedSearch {
  let s = ` ${foldText(input).replace(/[^\p{L}\p{N}\s'._-]+/gu, ' ').replace(/\s+/g, ' ').trim()} `;
  let flag: MediaFlagName | undefined;
  for (const [w, f] of FLAG_WORDS) {
    const needle = ` ${w} `;
    if (s.includes(needle)) {
      if (!flag) flag = f;
      s = s.replace(needle, ' ');
    }
  }
  const words = s.split(' ').map(trimPunct).filter(Boolean);
  const yearAt = words.findIndex((w) => /^(19[7-9]\d|2[01]\d\d)$/.test(w));
  const year = yearAt >= 0 ? Number(words[yearAt]) : undefined;
  let month: number | undefined;
  const terms: string[] = [];
  words.forEach((w, i) => {
    if (i === yearAt) return;
    const m = MONTHS.get(w) ?? (year !== undefined ? MONTHS_SHORT.get(w) : undefined);
    if (m !== undefined && month === undefined) month = m;
    else terms.push(w);
  });
  const out: ParsedSearch = { text: terms.join(' '), terms };
  if (flag) out.flag = flag;
  if (year !== undefined) {
    if (month !== undefined) {
      out.from = Date.UTC(year, month - 1, 1);
      out.to = Date.UTC(year, month, 1);
    } else {
      out.from = Date.UTC(year, 0, 1);
      out.to = Date.UTC(year + 1, 0, 1);
    }
  } else if (month !== undefined) out.month = month;
  return out;
}
