import { afterEach, describe, expect, it, vi } from 'vitest';
import { HttpBackend, withDeviceAuth } from './http';
import { authFor, sameServer } from './android/linkAuth';

vi.mock('@capacitor/core', () => ({ registerPlugin: () => ({}) }));

const AUTH = { deviceId: 'dev-1234-abcd', token: 'tok_EN-xyz' };

describe('HttpBackend device auth (linked phone in client mode)', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('leaves URLs alone without auth', () => {
    const b = new HttpBackend('http://pc:8080/');
    expect(b.thumbUrl('7', 300)).toBe('http://pc:8080/api/media/7/thumb?s=320');
    expect(b.fileUrl('7')).toBe('http://pc:8080/api/media/7/file');
    expect(b.authHeaders()).toEqual({});
  });

  it('appends pdev/ptok to every media URL', () => {
    const b = new HttpBackend('http://pc:8080');
    b.setDeviceAuth(AUTH);
    const q = 'pdev=dev-1234-abcd&ptok=tok_EN-xyz';
    expect(b.thumbUrl('7', 300)).toBe(`http://pc:8080/api/media/7/thumb?s=320&${q}`);
    expect(b.fileUrl('7')).toBe(`http://pc:8080/api/media/7/file?${q}`);
    expect(b.fileUrl('7', true)).toBe(`http://pc:8080/api/media/7/file?download=1&${q}`);
    expect(b.displayUrl({ id: '7', kind: 'image', playback: 'convert' })).toBe(`http://pc:8080/api/media/7/display?${q}`);
    expect(b.displayUrl({ id: '7', kind: 'image', playback: 'direct' })).toBe(`http://pc:8080/api/media/7/file?${q}`);
    expect(b.zoomUrl({ id: '7', kind: 'image', playback: 'direct' })).toBe(`http://pc:8080/api/media/7/display?max=4096&${q}`);
    expect(b.streamUrl('7', 2)).toBe(`http://pc:8080/api/media/7/stream?t=2.00&${q}`);
    expect(b.previewUrl('7')).toBe(`http://pc:8080/api/media/7/preview?${q}`);
    expect(b.filmstripUrl('7', 10, 60)).toBe(`http://pc:8080/api/media/7/filmstrip?n=10&h=60&${q}`);
    expect(b.downloadUrl(['1', '2'])).toBe(`http://pc:8080/api/download?ids=1%2C2&${q}`);
    expect(b.folderDownloadUrl('3', 'a b')).toBe(`http://pc:8080/api/download?folderId=3&dir=a+b&${q}`);
    b.setDeviceAuth(null);
    expect(b.fileUrl('7')).toBe('http://pc:8080/api/media/7/file');
  });

  it('encodes credentials and keeps a #fragment last', () => {
    expect(withDeviceAuth('/x?a=1#t=3', { deviceId: 'a/b', token: 'c&d' })).toBe('/x?a=1&pdev=a%2Fb&ptok=c%26d#t=3');
    expect(withDeviceAuth('', AUTH)).toBe('');
  });

  it('sends Authorization + X-Peek-Device headers on API calls (merged with JSON headers)', async () => {
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ libraries: [], temp: [] }), { headers: { 'content-type': 'application/json' } }));
    vi.stubGlobal('fetch', fetchMock);
    const b = new HttpBackend('http://pc:8080');
    b.setDeviceAuth(AUTH);
    await b.libraries();
    await b.updateSettings({ showHidden: true });
    const calls = fetchMock.mock.calls as unknown as [string, RequestInit][];
    expect(calls[0][0]).toBe('http://pc:8080/api/libraries');
    expect(calls[0][1].headers).toEqual({ Authorization: 'Bearer tok_EN-xyz', 'X-Peek-Device': 'dev-1234-abcd' });
    expect(calls[1][1].headers).toEqual({ 'Content-Type': 'application/json', Authorization: 'Bearer tok_EN-xyz', 'X-Peek-Device': 'dev-1234-abcd' });
  });

  it('only authenticates against the server the phone is paired with', () => {
    expect(sameServer('http://192.168.1.20:47213/', 'HTTP://192.168.1.20:47213')).toBe(true);
    expect(sameServer('http://pc', 'http://pc:80')).toBe(true);
    expect(sameServer('http://pc:8080', 'http://pc:8081')).toBe(false);
    const st = { paired: true, token: 't', deviceId: 'd', server: 'http://pc:8080' };
    expect(authFor(st, 'http://pc:8080/')).toEqual({ deviceId: 'd', token: 't' });
    expect(authFor(st, 'http://other:8080')).toBeNull();
    expect(authFor({ ...st, paired: false }, 'http://pc:8080')).toBeNull();
    expect(authFor(null, 'http://pc:8080')).toBeNull();
  });
});
