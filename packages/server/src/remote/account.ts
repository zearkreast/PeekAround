import type { MegaAccountStatus, MegaLoginRefusal, MegaLoginRequest, MegaLoginResult } from '@peekaround/shared';
import type { Ctx } from '../context.js';
import { HttpError } from '../paths.js';
import { clearDownloadUrls, isSessionError, sessionApi, type MegaSessionSource } from './mega.js';

/**
 * Optional MEGA account login (Settings → MEGA hesabı).
 *
 * Why: anonymous downloads of public links count against the IP address's MEGA transfer quota; with
 * a session every API request carries `&sid=` (mega.ts sessionApi) and downloads count against the
 * account's own quota instead.
 *
 * What is stored: ONLY the session id (+ the account email for display and the login time), in the
 * settings table under `megaSession` — never the password, never the master key / RSA key, and the
 * sid never leaves the server (no route or event returns it). The password lives in memory for the
 * duration of the login request only (megajs derives the login hash from it).
 *
 * A session MEGA rejects (ESID; EBLOCKED / ENOENT confirmed by a "ug" check) is dropped: the status
 * shows `expired` and requests fall back to anonymous.
 * Logout sends "sml" best-effort and deletes the stored sid.
 */

export interface MegaAccountDetails {
  email: string | null;
  /** MEGA "utype": 0 free, 1–3 Pro I–III, 4 Pro Lite, 100 Business, 101 Pro Flexi */
  utype: number | null;
  /** bytes, when MEGA reports them */
  transferUsed: number | null;
  transferTotal: number | null;
}

export type MegaAuthLogin = { sid: string; email: string } | { refused: MegaLoginRefusal };

/** the MEGA account API (injectable for tests) */
export interface MegaAuthClient {
  login(req: { email: string; password: string; code?: string }): Promise<MegaAuthLogin>;
  /** "ug" + "uq" with the session; rejects with the MEGA error (ESID, …) */
  details(sid: string): Promise<MegaAccountDetails>;
  /** "sml" — ends the session on MEGA's side */
  logout(sid: string): Promise<void>;
}

interface Stored {
  sid?: string;
  email?: string;
  since?: number;
  expired?: boolean;
}

const KEY = 'megaSession';
const DETAILS_TTL = 60_000;
const SUSPECT_GAP = 60_000;

function withTimeout<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: NodeJS.Timeout;
  return Promise.race([
    p.finally(() => clearTimeout(timer)),
    new Promise<T>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${what} timed out`)), ms);
      timer.unref?.();
    }),
  ]);
}

/** only the MEGA error code of a message ("ENOENT (-9)") — never anything else a message might carry */
const megaCode = (e: unknown) => /\b(E[A-Z]+) \((-\d+)\)/.exec(String((e as Error)?.message ?? e))?.slice(1, 3).join(' ') ?? null;

/** "uq" / "ug" answers → details (pure; unit tested through the fake gateway) */
export function parseDetails(ug: any, uq: any): MegaAccountDetails {
  const num = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) && v >= 0 ? v : null);
  let used = num(uq?.caxfer);
  // free accounts: per-hour transfer history of the current window
  if (used === null && Array.isArray(uq?.tah)) used = uq.tah.reduce((a: number, b: unknown) => a + (num(b) ?? 0), 0);
  return {
    email: typeof ug?.email === 'string' ? ug.email : null,
    utype: num(uq?.utype),
    transferUsed: used,
    transferTotal: num(uq?.mxfer) || null,
  };
}

export function planOf(utype: number | null): { type: MegaAccountStatus['type']; plan: string | null } {
  if (utype === null) return { type: null, plan: null };
  if (utype === 0) return { type: 'free', plan: null };
  if (utype === 100) return { type: 'business', plan: 'Business' };
  const names: Record<number, string> = { 1: 'Pro I', 2: 'Pro II', 3: 'Pro III', 4: 'Pro Lite', 101: 'Pro Flexi' };
  return { type: 'pro', plan: names[utype] ?? null };
}

/** Real client on top of megajs 1.3.10 (Storage does the v1/v2 password → login hash + RSA sid dance). */
export function megaAuthClient(opts: { gateway?: string } = {}): MegaAuthClient {
  const mod = async () => {
    const m: any = await import('megajs');
    return { Storage: m.Storage ?? m.default?.Storage, API: m.API ?? m.default?.API };
  };
  const request = (api: any, json: Record<string, unknown>) =>
    new Promise<any>((resolve, reject) => {
      try {
        api.request(json, (err: unknown, resp: unknown) => (err ? reject(err) : resolve(resp)));
      } catch (e) {
        reject(e);
      }
    });
  const apiFor = async (sid: string) => {
    const { API } = await mod();
    const api = sessionApi(API, { gateway: opts.gateway });
    api.sid = sid;
    return api;
  };
  return {
    async login({ email, password, code }) {
      const { Storage } = await mod();
      // us0 → (v2: PBKDF2) → us {user, uh, mfa?} → sid from csid (RSA) → ug. No keepalive, no tree load.
      let storage: any = new Storage({
        email,
        password,
        ...(code ? { secondFactorCode: code } : {}),
        keepalive: false,
        autoload: false,
        ...(opts.gateway ? { gateway: opts.gateway } : {}),
      });
      try {
        await withTimeout(storage.ready, 45_000, 'MEGA login');
        const sid = typeof storage.sid === 'string' ? storage.sid : '';
        if (!sid) throw new Error('MEGA returned no session');
        return { sid, email: String(storage.email ?? email) };
      } catch (e) {
        const msg = String((e as Error)?.message ?? e);
        if (/EMFAREQUIRED|\(-26\)/.test(msg)) return { refused: code ? 'mfa-invalid' : 'mfa-required' };
        if (code && /EFAILED|\(-5\)/.test(msg)) return { refused: 'mfa-invalid' };
        if (/ENOENT|\(-9\)|invalid credentials|could not be decrypted/i.test(msg)) return { refused: 'credentials' };
        if (/EBLOCKED|\(-16\)/.test(msg)) return { refused: 'blocked' };
        throw e;
      } finally {
        // drop every reference that held the password / keys
        if (storage?.options) {
          delete storage.options.password;
          delete storage.options.secondFactorCode;
        }
        storage = null;
      }
    },
    async details(sid) {
      const api = await apiFor(sid);
      const ug = await withTimeout(request(api, { a: 'ug' }), 15_000, 'MEGA account');
      const uq = await withTimeout(request(api, { a: 'uq', xfer: 1, pro: 1 }), 15_000, 'MEGA account').catch(() => null);
      return parseDetails(ug, uq);
    },
    async logout(sid) {
      const api = await apiFor(sid);
      await withTimeout(request(api, { a: 'sml' }), 10_000, 'MEGA logout');
    },
  };
}

/** login state + the session source of every MEGA request (ctx.megaAccount) */
export class MegaAccount implements MegaSessionSource {
  private details: { at: number; d: MegaAccountDetails } | null = null;
  private loading: Promise<void> | null = null;
  private lastSuspect = 0;

  constructor(
    private ctx: Ctx,
    readonly auth: MegaAuthClient,
  ) {}

  private stored(): Stored {
    const s = this.ctx.db.getSetting<Stored>(KEY, {});
    return s && typeof s === 'object' ? s : {};
  }

  private store(s: Stored) {
    this.ctx.db.setSetting(KEY, s);
  }

  // ── MegaSessionSource ──

  sid(): string | null {
    const s = this.stored().sid;
    return typeof s === 'string' && s ? s : null;
  }

  invalid(sid: string) {
    const s = this.stored();
    if (s.sid !== sid) return;
    this.store({ email: s.email, expired: true });
    this.changed();
  }

  /** EBLOCKED / ENOENT on a link request: usually the link, sometimes the account — ask MEGA (at most once a minute) */
  suspect(sid: string) {
    if (Date.now() - this.lastSuspect < SUSPECT_GAP) return;
    this.lastSuspect = Date.now();
    this.auth.details(sid).then(
      (d) => (this.details = { at: Date.now(), d }),
      (e) => {
        if (isSessionError(e) || /EBLOCKED|\(-16\)|ENOENT|\(-9\)/.test(String((e as Error)?.message))) this.invalid(sid);
      },
    );
  }

  // ── status / login / logout ──

  status(): MegaAccountStatus {
    const s = this.stored();
    const loggedIn = !!s.sid;
    const d = loggedIn ? this.details?.d : undefined;
    const { type, plan } = planOf(d?.utype ?? null);
    return {
      loggedIn,
      email: (loggedIn ? d?.email : null) ?? s.email ?? null,
      type,
      plan,
      transfer: d && (d.transferUsed !== null || d.transferTotal !== null) ? { used: d.transferUsed ?? 0, total: d.transferTotal } : null,
      expired: !loggedIn && !!s.expired,
      since: loggedIn ? (s.since ?? null) : null,
    };
  }

  /** status with fresh account details (type, transfer quota); an ESID answer marks the session expired */
  async refreshedStatus(force = false): Promise<MegaAccountStatus> {
    const sid = this.sid();
    if (sid && (force || !this.details || Date.now() - this.details.at > DETAILS_TTL)) {
      this.loading ??= this.auth
        .details(sid)
        .then(
          (d) => {
            if (this.sid() === sid) this.details = { at: Date.now(), d };
          },
          (e) => {
            if (isSessionError(e) || /EBLOCKED|\(-16\)/.test(String((e as Error)?.message))) this.invalid(sid);
            // network trouble: keep showing what is stored
          },
        )
        .finally(() => (this.loading = null));
      await this.loading;
    }
    return this.status();
  }

  async login(body: MegaLoginRequest): Promise<MegaLoginResult> {
    const email = typeof body?.email === 'string' ? body.email.trim().toLowerCase() : '';
    const password = typeof body?.password === 'string' ? body.password : '';
    const code = typeof body?.code === 'string' ? body.code.replace(/\s+/g, '') : '';
    if (!email || email.length > 254 || !/^[^\s@]+@[^\s@]+$/.test(email)) throw new HttpError(400, 'A valid email address is required');
    if (!password || password.length > 1024) throw new HttpError(400, 'Password required');
    if (code && !/^\d{6}$/.test(code)) throw new HttpError(400, 'The two-factor code has 6 digits');
    let r: MegaAuthLogin;
    try {
      r = await this.auth.login({ email, password, ...(code ? { code } : {}) });
    } catch (e) {
      throw loginError(e);
    }
    if ('refused' in r) return { ok: false, reason: r.refused };
    const old = this.sid();
    this.details = null;
    this.store({ sid: r.sid, email: r.email || email, since: Date.now() });
    if (old && old !== r.sid) void this.auth.logout(old).catch(() => {});
    await this.refreshedStatus(true);
    this.changed();
    return { ok: true, account: this.status() };
  }

  /** forget the session (and the email); "sml" ends it on MEGA's side */
  async logout(): Promise<MegaAccountStatus> {
    const sid = this.sid();
    this.store({});
    this.details = null;
    this.changed();
    // best-effort: the stored sid is gone either way
    if (sid) await this.auth.logout(sid).catch(() => {});
    return this.status();
  }

  /** session started / ended / expired: new download URLs, quota pause lifted (the other quota applies now) */
  private changed() {
    clearDownloadUrls();
    this.ctx.remote?.resumeTransfers();
    this.ctx.events.emit({ type: 'megaAccount' });
  }
}

function loginError(e: unknown): HttpError {
  if (e instanceof HttpError) return e;
  const msg = String((e as Error)?.message ?? e);
  if (/ERATELIMIT|ETOOMANY|EAGAIN|\(-3\)|\(-4\)|\(-6\)/.test(msg)) return new HttpError(429, 'Too many MEGA login attempts, try again later');
  if (/timed out|fetch failed|ENOTFOUND|ECONNRESET|ETIMEDOUT|EAI_AGAIN|network/i.test(msg)) return new HttpError(502, 'Cannot reach MEGA');
  if (/Account version not supported/.test(msg)) return new HttpError(400, 'This MEGA account type is not supported');
  const code = megaCode(e);
  return new HttpError(502, code ? `MEGA login failed (${code})` : 'MEGA login failed');
}
