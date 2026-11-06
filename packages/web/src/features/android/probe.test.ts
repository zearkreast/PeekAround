import { describe, expect, it } from 'vitest';
import { candidateUrls, probeServer } from './probe';

const info = { name: 'PeekAround', version: '1.3.0', apiVersion: 1, mode: 'desktop', hostname: 'DESKTOP-AB12' };

/** fake fetch: url → response (or 'down' = network error / timeout) */
function fakeFetch(routes: Record<string, { status?: number; body?: unknown } | 'down' | 'hang'>, calls: string[] = []) {
  return (async (input: string, init?: RequestInit) => {
    calls.push(String(input));
    const r = routes[String(input)] ?? 'down';
    if (r === 'down') throw new TypeError('Failed to fetch');
    if (r === 'hang')
      return new Promise((_res, rej) => init?.signal?.addEventListener('abort', () => rej(new DOMException('aborted', 'AbortError'))));
    const status = r.status ?? 200;
    return new Response(r.body === undefined ? '<html></html>' : JSON.stringify(r.body), { status, headers: { 'content-type': r.body === undefined ? 'text/html' : 'application/json' } });
  }) as typeof fetch;
}

describe('Android connect screen: server address probe', () => {
  it('turns what was typed into the addresses to try', () => {
    expect(candidateUrls('http://192.168.1.20:8080/')).toEqual(['http://192.168.1.20:8080']);
    // no scheme (the field's "http://" prefix deleted) → http://
    expect(candidateUrls(' 192.168.1.20:8080 ')).toEqual(['http://192.168.1.20:8080']);
    // no port → as typed (port 80 / reverse proxy) and the desktop app's LAN port
    expect(candidateUrls('192.168.1.20')).toEqual(['http://192.168.1.20', 'http://192.168.1.20:1313', 'http://192.168.1.20:8080']);
    expect(candidateUrls('https://photos.example.com')).toEqual(['https://photos.example.com', 'https://photos.example.com:1313', 'https://photos.example.com:8080']);
    expect(candidateUrls('http://nas.local/peek')).toEqual(['http://nas.local/peek']);
    for (const bad of ['', 'http://', 'ftp://x', 'http://user:pw@x', 'javascript:alert(1)']) expect(candidateUrls(bad), bad).toEqual([]);
  });

  it('accepts a PeekAround server (GET /api/info, name check)', async () => {
    const calls: string[] = [];
    const r = await probeServer('http://192.168.1.20:8080', { fetch: fakeFetch({ 'http://192.168.1.20:8080/api/info': { body: info } }, calls) });
    expect(r).toEqual({ ok: true, url: 'http://192.168.1.20:8080', info });
    expect(calls).toEqual(['http://192.168.1.20:8080/api/info']);
  });

  it('finds the LAN share when the port was left out', async () => {
    const r = await probeServer('192.168.1.20', { fetch: fakeFetch({ 'http://192.168.1.20:1313/api/info': { body: info } }) });
    expect(r).toMatchObject({ ok: true, url: 'http://192.168.1.20:1313' });
    // a Docker server / a desktop app still on the old port
    const old = await probeServer('192.168.1.20', { fetch: fakeFetch({ 'http://192.168.1.20:8080/api/info': { body: info } }) });
    expect(old).toMatchObject({ ok: true, url: 'http://192.168.1.20:8080' });
  });

  it('tells "nothing answered" apart from "some other server" and HTTP errors', async () => {
    expect(await probeServer('http://192.168.1.20:8080', { fetch: fakeFetch({}) })).toEqual({ ok: false, error: 'unreachable' });
    expect(await probeServer('http://192.168.1.20:8080', { fetch: fakeFetch({ 'http://192.168.1.20:8080/api/info': 'hang' }), timeoutMs: 20 })).toEqual({ ok: false, error: 'unreachable' });
    // e.g. the router's page, or an SPA fallback
    expect(await probeServer('http://192.168.1.1', { fetch: fakeFetch({ 'http://192.168.1.1/api/info': { status: 200 } }) })).toEqual({ ok: false, error: 'not-peekaround' });
    expect(await probeServer('http://pc:8080', { fetch: fakeFetch({ 'http://pc:8080/api/info': { status: 421, body: { error: 'Unknown host' } } }) })).toEqual({ ok: false, error: 'refused', status: 421 });
    // rate limited / refused responses are not taken for a server
    expect(await probeServer('http://pc:8080', { fetch: fakeFetch({ 'http://pc:8080/api/info': { status: 429, body: { name: 'PeekAround' } } }) })).toMatchObject({ ok: false, error: 'refused' });
    expect(await probeServer('nonsense://', { fetch: fakeFetch({}) })).toEqual({ ok: false, error: 'invalid' });
  });
});
