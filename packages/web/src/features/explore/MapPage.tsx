import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import L from 'leaflet';
import 'leaflet/dist/leaflet.css';
import Supercluster from 'supercluster';
import { ChevronDown, ChevronUp, Layers, Maximize2, Minus, PanelRightClose, PanelRightOpen, Plus, WifiOff, MapPin, MapPinOff, RotateCw, ShieldAlert } from 'lucide-react';
import { MAP_TILE_STYLES, tileStyleOf, type MediaItem, type Settings } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { BACK, useBackHandler } from '@/lib/backStack';
import { useSettings } from '@/lib/queries';
import { useIsMobile } from '@/lib/useMedia';
import { useChrome } from '@/store/chrome';
import { useLocation } from '@/store/location';
import { usePrefs } from '@/store/prefs';
import { IconButton, Spinner } from '@/components/ui';
import { MediaGrid } from '@/features/gallery/MediaGrid';
import { StaticSource } from '@/features/gallery/grid/source';
import { useExplore, useOpenViewer } from './data';
import { exploreQuery, NO_GPS_HREF } from './labels';
import { boundsFilter, normalizeBbox, tileConfig, tileProblemOf, type Point, type TileProblem } from './mapUtil';

type PointProps = { id: string };
type ClusterProps = { cover: string };

/** current data-theme of <html> (dark unless 'light') */
function useDarkTheme(): boolean {
  const [dark, setDark] = useState(() => document.documentElement.dataset.theme !== 'light');
  useEffect(() => {
    const mo = new MutationObserver(() => setDark(document.documentElement.dataset.theme !== 'light'));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    return () => mo.disconnect();
  }, []);
  return dark;
}

function markerIcon(thumb: string, count: number): L.DivIcon {
  const size = count > 1 ? 62 : 54;
  const root = document.createElement('div');
  if (count > 1) {
    const stack = document.createElement('div');
    stack.className = 'pm-stack';
    root.appendChild(stack);
  }
  const tail = document.createElement('div');
  tail.className = 'pm-tail';
  root.appendChild(tail);
  const photo = document.createElement('div');
  photo.className = 'pm-photo';
  const img = document.createElement('img');
  img.src = thumb;
  img.alt = '';
  img.decoding = 'async';
  img.draggable = false;
  photo.appendChild(img);
  root.appendChild(photo);
  if (count > 1) {
    const c = document.createElement('div');
    c.className = 'pm-count';
    c.textContent = count >= 10000 ? `${Math.round(count / 1000)}k` : count >= 1000 ? `${(count / 1000).toFixed(1)}k` : String(count);
    root.appendChild(c);
  }
  return L.divIcon({ className: 'peek-marker', html: root, iconSize: [size, size], iconAnchor: [size / 2, size + 7] });
}

export function MapPage() {
  const t = useT();
  const backend = useBackend();
  const mobile = useIsMobile();
  const [sp] = useSearchParams();
  const place = sp.get('place') ?? undefined;
  // "Show on map" from the viewer: centre on one item
  const focusId = sp.get('focus') ?? undefined;
  const focusLat = Number(sp.get('lat'));
  const focusLon = Number(sp.get('lon'));
  const focus = focusId && Number.isFinite(focusLat) && Number.isFinite(focusLon) && Math.abs(focusLat) <= 90 && Math.abs(focusLon) <= 180 ? { id: focusId, lat: focusLat, lon: focusLon } : null;
  const setChrome = useChrome((s) => s.set);
  const setView = useLocation((s) => s.setView);
  const motionPref = usePrefs((s) => s.motion);
  const animate = motionPref !== 'off';
  const settings = useSettings();
  const tiles: Settings['mapTiles'] = settings.data?.mapTiles ?? 'osm';
  const dark = useDarkTheme();
  const explore = useExplore();
  const openViewer = useOpenViewer();

  const placeGroup = place ? explore.result.places.find((p) => p.key === place) : undefined;
  const title = placeGroup?.name ?? t('explore.map');
  useEffect(() => setChrome({ crumbs: [], title, viewControls: false }), [setChrome, title]);

  // Android local: places are resolved with metadata loaded by explore() → wait for it there
  const gated = backend.kind === 'android-local' && explore.isLoading;
  const query = useMemo(() => exploreQuery(place ? { place } : {}), [place]);
  const pointsQ = useQuery({
    queryKey: ['explore', backend.id, 'points', query],
    queryFn: () => backend.geoPoints(query),
    enabled: !gated,
    staleTime: 30_000,
  });
  const points: Point[] = useMemo(() => pointsQ.data?.points ?? [], [pointsQ.data]);

  // the media items behind the points (for the in-view list and viewer navigation), newest first
  const itemsQ = useQuery({
    queryKey: ['explore', backend.id, 'map-items', query, points.length],
    enabled: points.length > 0,
    staleTime: 30_000,
    queryFn: async ({ signal }) => {
      const want = new Set(points.map((p) => p[0]));
      const out = new Map<string, MediaItem>();
      const PAGE = 2000;
      for (let offset = 0; ; offset += PAGE) {
        const r = await backend.browse({ ...query, offset, limit: PAGE }, { signal });
        for (const it of r.items) if (want.has(it.id)) out.set(it.id, it);
        if (out.size >= want.size || r.items.length < PAGE || offset + PAGE >= r.total) break;
      }
      return out;
    },
  });
  const items = itemsQ.data;

  // ── clustering ──
  const index = useMemo(() => {
    const sc = new Supercluster<PointProps, ClusterProps>({
      radius: 72,
      maxZoom: 17,
      map: (p) => ({ cover: p.id }),
      reduce: () => {},
    });
    sc.load(points.map(([id, lat, lon]) => ({ type: 'Feature', properties: { id }, geometry: { type: 'Point', coordinates: [lon, lat] } })));
    return sc;
  }, [points]);

  const el = useRef<HTMLDivElement>(null);
  const mapRef = useRef<L.Map | null>(null);
  const markers = useRef(new Map<string, L.Marker>());
  const [inView, setInView] = useState<string[]>([]);
  const [tileProblem, setTileProblem] = useState<TileProblem | null>(null);
  const [tileNonce, setTileNonce] = useState(0);
  const tileError = !!tileProblem;
  const fitted = useRef(false);
  const qc = useQueryClient();

  useEffect(() => {
    if (!el.current) return;
    const map = L.map(el.current, {
      zoomControl: false,
      worldCopyJump: true,
      minZoom: 2,
      maxZoom: 18,
      zoomSnap: 0.5,
      attributionControl: true,
      fadeAnimation: animate,
      zoomAnimation: animate,
      markerZoomAnimation: animate,
    }).setView([30, 15], 2);
    map.attributionControl.setPrefix(false);
    mapRef.current = map;
    const ro = new ResizeObserver(() => map.invalidateSize({ pan: false }));
    ro.observe(el.current);
    return () => {
      ro.disconnect();
      map.remove();
      mapRef.current = null;
      markers.current.clear();
      fitted.current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // tiles (style setting / theme): through the server's tile proxy for HTTP backends, upstream on
  // Android local mode. Failures → banner that says WHY (offline vs. refused); markers keep working.
  const proxy = useMemo(() => (backend.tileUrl ? (st: Parameters<NonNullable<typeof backend.tileUrl>>[0]) => backend.tileUrl!(st) : undefined), [backend]);
  const cfg = useMemo(() => tileConfig(tiles, dark, proxy), [tiles, dark, proxy]);
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    setTileProblem(null);
    if (!cfg) return;
    map.setMaxZoom(Math.max(cfg.maxZoom, 2));
    let ok = 0;
    let bad = 0;
    let probing = false;
    let disposed = false;
    const layer = L.tileLayer(cfg.url, {
      attribution: cfg.attribution,
      subdomains: cfg.subdomains ?? 'abc',
      className: cfg.className,
      maxZoom: cfg.maxZoom,
      maxNativeZoom: cfg.maxZoom,
      detectRetina: false,
      crossOrigin: false,
    });
    layer.on('tileload', () => {
      ok++;
      bad = 0;
      setTileProblem(null);
    });
    layer.on('tileerror', (e: L.TileErrorEvent) => {
      bad++;
      if (bad < 2 || probing) return;
      if (typeof navigator !== 'undefined' && navigator.onLine === false && !cfg.proxied) return setTileProblem({ kind: 'offline' });
      if (!cfg.proxied) {
        if (ok === 0) setTileProblem({ kind: 'unknown' });
        return;
      }
      // ask the server why: X-Peek-Tile-Error = offline | refused:<status>
      probing = true;
      const url = (e.tile as HTMLImageElement | undefined)?.src || layer.getTileUrl(e.coords);
      void fetch(url, { cache: 'no-store' })
        .then((r) => (r.ok ? null : tileProblemOf(r.headers.get('X-Peek-Tile-Error'), r.status)))
        .catch(() => ({ kind: 'offline' }) as TileProblem)
        .then((p) => {
          if (!disposed && p) setTileProblem(p);
        })
        .finally(() => {
          probing = false;
        });
    });
    layer.addTo(map);
    const offline = () => !cfg.proxied && setTileProblem({ kind: 'offline' });
    const online = () => layer.redraw();
    window.addEventListener('offline', offline);
    window.addEventListener('online', online);
    if (typeof navigator !== 'undefined' && navigator.onLine === false && !cfg.proxied) setTileProblem({ kind: 'offline' });
    return () => {
      disposed = true;
      window.removeEventListener('offline', offline);
      window.removeEventListener('online', online);
      layer.remove();
    };
  }, [cfg, tileNonce]);

  // quick style switch on the map (saved as the setting)
  const cycleStyle = useCallback(async () => {
    const cur = tileStyleOf(tiles) ?? 'osm';
    const next = MAP_TILE_STYLES[(MAP_TILE_STYLES.indexOf(cur) + 1) % MAP_TILE_STYLES.length];
    try {
      const s2 = await backend.updateSettings({ mapTiles: next });
      qc.setQueryData(['settings', backend.id], s2);
    } catch {
      /* read-only server: keep the current style */
    }
  }, [tiles, backend, qc]);

  const fitAll = useCallback(
    (anim = animate) => {
      const map = mapRef.current;
      if (!map || !points.length) return;
      // the side / bottom panel may have just appeared: measure before fitting
      map.invalidateSize({ pan: false });
      let [s, w, n, e] = [90, 180, -90, -180];
      for (const [, lat, lon] of points) {
        if (lat < s) s = lat;
        if (lat > n) n = lat;
        if (lon < w) w = lon;
        if (lon > e) e = lon;
      }
      map.fitBounds(
        [
          [s, w],
          [n, e],
        ],
        { padding: [56, 56], maxZoom: 13, animate: anim },
      );
    },
    [points, animate],
  );

  const openItem = useCallback(
    (id: string) => {
      openViewer({ id });
    },
    [openViewer],
  );

  // render clusters for the current viewport + collect the ids in view
  const refresh = useCallback(() => {
    const map = mapRef.current;
    if (!map) return;
    const b = map.getBounds();
    const bbox = normalizeBbox(b.getWest(), b.getSouth(), b.getEast(), b.getNorth());
    const zoom = Math.round(map.getZoom());
    const clusters = bbox ? index.getClusters(bbox, zoom) : [];
    const keep = new Set<string>();
    for (const c of clusters) {
      const [lon, lat] = c.geometry.coordinates;
      const props = c.properties as Supercluster.ClusterProperties & ClusterProps & Partial<PointProps>;
      const isCluster = !!props.cluster;
      const key = isCluster ? `c${props.cluster_id}:${props.point_count}` : `p${props.id}`;
      keep.add(key);
      if (markers.current.has(key)) continue;
      const cover = isCluster ? props.cover : props.id!;
      const m = L.marker([lat, lon], { icon: markerIcon(backend.thumbUrl(cover, 320), isCluster ? props.point_count : 1), keyboard: true, riseOnHover: true });
      m.on('click', () => {
        if (!isCluster) return openItem(props.id!);
        const z = index.getClusterExpansionZoom(props.cluster_id);
        if (z > 17 || map.getZoom() >= 17) {
          // identical locations: open the newest of them
          const leaves = index.getLeaves(props.cluster_id, 1);
          if (leaves[0]) openItem(leaves[0].properties.id);
          return;
        }
        map.flyTo([lat, lon], Math.min(18, z + 0.5), { animate, duration: 0.6 });
      });
      m.addTo(map);
      markers.current.set(key, m);
    }
    for (const [k, m] of markers.current) {
      if (keep.has(k)) continue;
      m.remove();
      markers.current.delete(k);
    }
    setInView(boundsFilter(points, b.getWest(), b.getSouth(), b.getEast(), b.getNorth()));
  }, [index, points, backend, openItem, animate]);

  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    for (const m of markers.current.values()) m.remove();
    markers.current.clear();
    if (focus && !fitted.current) {
      // re-centre once the points (and with them the side panel) are there
      if (points.length) fitted.current = true;
      map.invalidateSize({ pan: false });
      map.setView([focus.lat, focus.lon], 16, { animate: false });
    } else if (points.length && !fitted.current) {
      fitted.current = true;
      fitAll(false);
    }
    refresh();
    map.on('moveend', refresh);
    return () => void map.off('moveend', refresh);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh, fitAll, points.length]);

  // "Show on map" again while the map is open (viewer on top of it): centre on the new item
  const focusKey = focus ? `${focus.id}@${focus.lat},${focus.lon}` : '';
  const lastFocus = useRef(focusKey);
  useEffect(() => {
    if (focusKey === lastFocus.current) return;
    lastFocus.current = focusKey;
    if (focus) mapRef.current?.setView([focus.lat, focus.lon], 16, { animate });
  }, [focusKey]); // eslint-disable-line react-hooks/exhaustive-deps

  // a ring around the focused item
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !focus) return;
    const ring = L.marker([focus.lat, focus.lon], {
      icon: L.divIcon({ className: 'pm-focus', html: '<span></span>', iconSize: [96, 96], iconAnchor: [48, 48] }),
      interactive: false,
      keyboard: false,
      zIndexOffset: -1000,
    }).addTo(map);
    return () => void ring.remove();
  }, [focus?.lat, focus?.lon]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── in-view list (newest first) ──
  const visible = useMemo(() => {
    if (!items) return [];
    const list: MediaItem[] = [];
    for (const id of inView) {
      const it = items.get(id);
      if (it) list.push(it);
    }
    return list.sort((a, b) => b.localAt - a.localAt || (a.id < b.id ? 1 : -1));
  }, [items, inView]);
  const source = useMemo(() => new StaticSource(`map:${place ?? ''}:${visible.length}:${visible.slice(0, 50).map((i) => i.id).join(',')}`, visible), [visible, place]);
  useEffect(() => setView({ target: null, targetLabel: title, source }), [source, setView, title]);
  useEffect(() => () => setView({ target: null, targetLabel: '', source: null }), [setView]);

  // ── list panel ──
  const [panel, setPanel] = useState<'peek' | 'open' | 'closed'>(mobile ? 'peek' : 'open');
  useBackHandler(mobile && panel === 'open', () => setPanel('peek'), BACK.sheet, 'map-list');
  // the list's scroll container must exist before the grid mounts (it measures it in layout effects)
  const listRef = useRef<HTMLDivElement | null>(null);
  const [listEl, setListEl] = useState<HTMLDivElement | null>(null);
  const attachList = useCallback((n: HTMLDivElement | null) => {
    listRef.current = n;
    setListEl(n);
  }, []);
  const loading = gated || pointsQ.isLoading;
  const empty = !loading && !points.length;
  const count = inView.length;

  const listBody =
    items === undefined && points.length ? (
      <div className="grid h-32 place-items-center">
        <Spinner />
      </div>
    ) : !listEl ? null : (
      <MediaGrid key={panel} source={source} scrollRef={listRef} onOpen={(it) => openItem(it.id)} groups={false} fastScroller={false} zoomable={false} />
    );

  return (
    <div className="absolute inset-0 flex overflow-hidden" data-page="explore-map">
      <div className="relative min-w-0 flex-1">
        <div ref={el} className="peek-map absolute inset-0 z-0" data-tiles={tiles} data-points={points.length} />

        {/* header */}
        <div className="pointer-events-none absolute inset-x-0 top-0 z-[500] flex items-start gap-2 p-3">
          <div className="pointer-events-auto flex min-w-0 items-center gap-2 rounded-2xl border border-line bg-elev px-3.5 py-2 shadow-soft">
            <MapPin className="size-4 shrink-0 text-accent" />
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold">{title}</div>
              <div className="text-[11px] text-dim tabular-nums" data-inview={count}>
                {loading ? t('explore.map.loading') : t('explore.map.inView', { n: count })}
              </div>
            </div>
          </div>
          {!place && !!explore.result.noLocation && (
            <Link
              to={NO_GPS_HREF}
              className="pointer-events-auto flex h-10 shrink-0 items-center gap-1.5 self-start rounded-2xl border border-line bg-elev px-3 text-xs font-medium text-dim shadow-soft transition-colors hover:text-fg"
              data-nogps={explore.result.noLocation}
            >
              <MapPinOff className="size-4 text-faint" />
              <span className="max-w-[46vw] truncate">{t('map.noLocation', { n: explore.result.noLocation })}</span>
            </Link>
          )}
          <div className="pointer-events-auto ml-auto flex flex-col gap-2">
            <IconButton label={t('explore.map.fit')} title={t('explore.map.fit')} onClick={() => fitAll()} className="border border-line bg-elev shadow-soft" data-action="fit">
              <Maximize2 />
            </IconButton>
            {tiles !== 'off' && (
              <IconButton
                label={`${t('map.style')}: ${t(`map.style.${tileStyleOf(tiles) ?? 'osm'}`)}`}
                title={`${t('map.style')}: ${t(`map.style.${tileStyleOf(tiles) ?? 'osm'}`)}`}
                onClick={() => void cycleStyle()}
                className="border border-line bg-elev shadow-soft"
                data-action="map-style"
                data-style={tileStyleOf(tiles) ?? 'osm'}
              >
                <Layers />
              </IconButton>
            )}
            {!mobile && (
              <>
                <IconButton label={t('explore.map.zoomIn')} onClick={() => mapRef.current?.zoomIn()} className="border border-line bg-elev shadow-soft">
                  <Plus />
                </IconButton>
                <IconButton label={t('explore.map.zoomOut')} onClick={() => mapRef.current?.zoomOut()} className="border border-line bg-elev shadow-soft">
                  <Minus />
                </IconButton>
                <IconButton
                  label={panel === 'open' ? t('explore.map.hideList') : t('explore.map.list')}
                  onClick={() => setPanel(panel === 'open' ? 'closed' : 'open')}
                  className="border border-line bg-elev shadow-soft"
                >
                  {panel === 'open' ? <PanelRightClose /> : <PanelRightOpen />}
                </IconButton>
              </>
            )}
          </div>
        </div>

        {(tileError || tiles === 'off') && (
          <div className="pointer-events-none absolute inset-x-0 top-[76px] z-[500] flex justify-center px-3" role="status" data-offline={tileProblem?.kind === 'offline' ? '' : undefined} data-tile-problem={tileProblem?.kind}>
            <div className="pointer-events-auto flex max-w-md items-center gap-2 rounded-xl border border-line bg-elev px-3 py-2 text-xs text-dim shadow-soft">
              {tileProblem?.kind === 'refused' ? <ShieldAlert className="size-4 shrink-0 text-warn" /> : <WifiOff className="size-4 shrink-0 text-warn" />}
              <span className="min-w-0 flex-1">
                {tiles === 'off'
                  ? t('explore.map.tilesOff')
                  : tileProblem?.kind === 'refused'
                    ? t('map.error.refused', { status: tileProblem.status || '?' })
                    : tileProblem?.kind === 'offline'
                      ? t('map.error.offline')
                      : t('map.error.refusedDirect')}
              </span>
              {tiles !== 'off' && (
                <button type="button" onClick={() => setTileNonce((n) => n + 1)} className="flex h-7 shrink-0 items-center gap-1 rounded-lg px-2 font-medium text-accent hover:bg-surface-2" data-action="tiles-retry">
                  <RotateCw className="size-3.5" />
                  {t('map.retry')}
                </button>
              )}
            </div>
          </div>
        )}

        {loading && (
          <div className="absolute inset-0 z-[400] grid place-items-center">
            <Spinner className="size-7" />
          </div>
        )}
        {empty && (
          <div className="absolute inset-0 z-[400] grid place-items-center p-6">
            <div className="max-w-sm rounded-2xl border border-line bg-elev p-5 text-center text-sm text-dim shadow-soft">
              <MapPin className="mx-auto mb-2 size-6 text-faint" />
              {t('explore.map.empty')}
              {!place && !!explore.result.noLocation && (
                <>
                  <p className="mt-2 text-xs text-faint">{t('map.noLocationHint')}</p>
                  <Link to={NO_GPS_HREF} className="mt-3 inline-flex h-9 items-center gap-1.5 rounded-lg px-3 text-[13px] font-medium text-accent hover:bg-surface-2">
                    <MapPinOff className="size-4" />
                    {t('map.noLocation', { n: explore.result.noLocation })}
                  </Link>
                </>
              )}
            </div>
          </div>
        )}

        {/* phone: bottom panel above the bottom navigation */}
        {mobile && points.length > 0 && (
          <div
            className={cn('absolute inset-x-0 z-[600] flex flex-col rounded-t-3xl border-t border-line bg-elev shadow-soft transition-[height] duration-300 ease-[var(--ease-out)]')}
            style={{ bottom: 'calc(64px + env(safe-area-inset-bottom, 0px))', height: panel === 'open' ? '62%' : 156 }}
            data-panel={panel}
          >
            <button type="button" onClick={() => setPanel(panel === 'open' ? 'peek' : 'open')} className="flex shrink-0 flex-col items-center px-4 pt-2 pb-1.5">
              <span className="mb-1.5 h-1 w-9 rounded-full bg-surface-3" />
              <span className="flex w-full items-center gap-2">
                <span className="text-sm font-semibold">{t('explore.map.list')}</span>
                <span className="text-xs text-faint tabular-nums">{count}</span>
                <span className="ml-auto text-dim">{panel === 'open' ? <ChevronDown className="size-5" /> : <ChevronUp className="size-5" />}</span>
              </span>
            </button>
            {panel === 'open' ? (
              <div ref={attachList} className="min-h-0 flex-1 overflow-y-auto px-1 pb-2">
                {listBody}
              </div>
            ) : (
              <div className="no-scrollbar flex min-h-0 flex-1 gap-1.5 overflow-x-auto px-3 pb-3" data-strip>
                {visible.slice(0, 40).map((it) => (
                  <button key={it.id} type="button" onClick={() => openItem(it.id)} className="aspect-square h-full shrink-0 overflow-hidden rounded-xl bg-[var(--tile-bg)]">
                    <img src={backend.thumbUrl(it.id, 320)} alt="" loading="lazy" decoding="async" className="size-full object-cover" />
                  </button>
                ))}
                {items === undefined && <Spinner className="m-auto" />}
              </div>
            )}
          </div>
        )}
      </div>

      {/* desktop: side panel */}
      {!mobile && panel === 'open' && points.length > 0 && (
        <aside className="flex w-[380px] shrink-0 flex-col border-l border-line bg-elev xl:w-[440px]" data-panel="open">
          <div className="flex h-12 shrink-0 items-center gap-2 px-4">
            <span className="text-sm font-semibold">{t('explore.map.list')}</span>
            <span className="text-xs text-faint tabular-nums">{count}</span>
          </div>
          <div ref={attachList} className="min-h-0 flex-1 overflow-y-auto px-2 pb-4">
            {listBody}
          </div>
        </aside>
      )}
    </div>
  );
}

export default MapPage;
