import { CalendarDays, MapPinOff, Clapperboard, Compass, Eye, Film, Grid3x3, Image as ImageIcon, LayoutGrid, Monitor, Moon, Palette, Presentation, ShieldCheck, Sun } from 'lucide-react';
import type { SlideTransition, SortKey } from '@peekaround/shared';
import { useLang, useT, type MessageKey } from '@/i18n';
import { formatDate, formatLocal } from '@/lib/format';
import { useIsMobile } from '@/lib/useMedia';
import { ACCENTS, usePrefs, type DateFormatPref, type GroupByPref, type HourCyclePref, type InfoDefault, type ViewerBg, type WeekStart } from '@/store/prefs';
import { ColorSwatches, Segmented, Slider, Switch } from '@/components/ui';
import { useSlideshow } from '@/features/slideshow/store';
import { ExploreSettings } from '@/features/explore/ExploreSettings';
import { MetaDiagnostics } from '@/features/explore/MetaDiagnostics';
import { Card, Opaque, Row, Section, Select } from './parts';

/** a boolean pref as a row */
function PrefSwitch({ k, hint, pref, kw }: { k: MessageKey; hint?: MessageKey; pref: BoolPref; kw?: string }) {
  const t = useT();
  const v = usePrefs((s) => s[pref]);
  const set = usePrefs((s) => s.set);
  return (
    <Row label={t(k)} k={k} hint={hint ? t(hint) : undefined} kw={kw} setting={pref}>
      <Switch checked={v} label={t(k)} onChange={(x) => set({ [pref]: x })} />
    </Row>
  );
}

type BoolPref = 'hoverPreview' | 'dateHeaders' | 'tileNames' | 'tileDuration' | 'tileFavorites' | 'confirmDelete' | 'filmstrip' | 'videoAutoplay' | 'videoMuted' | 'rememberVolume' | 'showMemories' | 'hideLocation' | 'wideSidebar';

export function AppearanceSection() {
  const t = useT();
  const lang = useLang();
  const prefs = usePrefs();
  const mobile = useIsMobile();
  const sample = Date.UTC(2024, 6, 5, 14, 30);
  return (
    <Section id="appearance" title="pref.sec.appearance" icon={<Palette />}>
      <Card title={t('pref.themeCard')} titleKey="pref.themeCard" icon={<Palette />} scope="device">
        <Row label={t('settings.language')} k="settings.language" kw="language dil türkçe english" setting="lang">
          <Segmented
            value={prefs.lang}
            onChange={(l) => prefs.set({ lang: l })}
            options={[
              { value: 'tr', label: 'Türkçe' },
              { value: 'en', label: 'English' },
            ]}
          />
        </Row>
        <Row label={t('settings.theme')} k="settings.theme" kw="dark light karanlık" setting="theme">
          <Segmented
            value={prefs.theme}
            onChange={(theme) => prefs.set({ theme })}
            options={[
              { value: 'dark', label: t('settings.theme.dark'), icon: <Moon /> },
              { value: 'light', label: t('settings.theme.light'), icon: <Sun /> },
              { value: 'system', label: t('settings.theme.system'), icon: <Monitor /> },
            ]}
          />
        </Row>
        <Row label={t('settings.accent')} k="settings.accent" kw="color renk" setting="accent">
          <ColorSwatches value={prefs.accent} onChange={(accent) => prefs.set({ accent })} colors={ACCENTS} />
        </Row>
        <Row label={t('settings.motion')} k="settings.motion" kw="animation reduce" setting="motion">
          <Segmented
            value={prefs.motion}
            onChange={(motion) => prefs.set({ motion })}
            options={[
              { value: 'full', label: t('settings.motion.full') },
              { value: 'reduced', label: t('settings.motion.reduced') },
              { value: 'off', label: t('settings.motion.off') },
            ]}
          />
        </Row>
        {!mobile && <PrefSwitch k="pref.wideSidebar" hint="pref.wideSidebarHint" pref="wideSidebar" kw="sidebar kenar çubuğu rail menu panel sol left" />}
      </Card>
      <Card title={t('pref.format')} titleKey="pref.format" icon={<CalendarDays />} scope="device">
        <Row label={t('pref.dateFormat')} k="pref.dateFormat" hint={t('pref.preview', { date: formatLocal(sample, lang, { dateStyle: 'medium', timeStyle: 'short' }) })} kw="iso tarih date" setting="dateFormat">
          <Segmented<DateFormatPref>
            size="sm"
            value={prefs.dateFormat}
            onChange={(dateFormat) => prefs.set({ dateFormat })}
            options={[
              { value: 'locale', label: t('pref.dateFormat.locale') },
              { value: 'iso', label: t('pref.dateFormat.iso') },
            ]}
          />
        </Row>
        <Row label={t('pref.hourCycle')} k="pref.hourCycle" hint={t('pref.preview', { date: formatDate(Date.now(), lang, { timeStyle: 'short' }) })} kw="am pm saat clock" setting="hourCycle">
          <Segmented<HourCyclePref>
            size="sm"
            value={prefs.hourCycle}
            onChange={(hourCycle) => prefs.set({ hourCycle })}
            options={[
              { value: 'auto', label: t('pref.hourCycle.auto') },
              { value: 'h23', label: t('pref.hourCycle.h23') },
              { value: 'h12', label: t('pref.hourCycle.h12') },
            ]}
          />
        </Row>
        <Row label={t('pref.weekStart')} k="pref.weekStart" hint={t('pref.weekStartHint')} kw="calendar takvim hafta week" setting="weekStart">
          <Segmented<WeekStart>
            size="sm"
            value={prefs.weekStart}
            onChange={(weekStart) => prefs.set({ weekStart })}
            options={[
              { value: 'mon', label: t('pref.weekStart.mon') },
              { value: 'sun', label: t('pref.weekStart.sun') },
              { value: 'sat', label: t('pref.weekStart.sat') },
            ]}
          />
        </Row>
      </Card>
    </Section>
  );
}

const SORTS: SortKey[] = ['takenAt', 'mtime', 'name', 'size', 'kind'];

export function GallerySection() {
  const t = useT();
  const prefs = usePrefs();
  return (
    <Section id="gallery" title="pref.sec.gallery" icon={<LayoutGrid />}>
      <Card title={t('pref.grid')} titleKey="pref.grid" icon={<Grid3x3 />} scope="device">
        <Row label={t('pref.defaultSort')} k="pref.defaultSort" kw="sort sırala order" setting="sort">
          <Select<SortKey> label={t('pref.defaultSort')} value={prefs.sort} onChange={(sort) => prefs.set({ sort })} options={SORTS.map((s) => ({ value: s, label: t(`pref.sort.${s}` as MessageKey) }))} />
          <Segmented
            size="sm"
            value={prefs.order}
            onChange={(order) => prefs.set({ order })}
            options={[
              { value: 'desc', label: t('pref.order.desc') },
              { value: 'asc', label: t('pref.order.asc') },
            ]}
          />
        </Row>
        <Row label={t('pref.gridMode')} k="pref.gridMode" kw="justified square kare layout" setting="gridMode">
          <Segmented
            size="sm"
            value={prefs.gridMode}
            onChange={(gridMode) => prefs.set({ gridMode })}
            options={[
              { value: 'justified', label: t('pref.gridMode.justified') },
              { value: 'square', label: t('pref.gridMode.square') },
            ]}
          />
        </Row>
        <Row label={t('pref.rowHeight')} k="pref.rowHeight" hint={t('pref.rowHeightHint', { px: prefs.rowHeight })} kw="size row height boyut" setting="rowHeight">
          <Slider className="w-44" value={prefs.rowHeight} min={110} max={420} step={10} center={220} onChange={(rowHeight) => prefs.set({ rowHeight })} />
        </Row>
        <PrefSwitch k="pref.dateHeaders" hint="pref.dateHeadersHint" pref="dateHeaders" kw="header başlık" />
        <Row label={t('pref.groupBy')} k="pref.groupBy" kw="group day month gün ay" setting="groupBy">
          <Segmented<GroupByPref>
            size="sm"
            value={prefs.groupBy}
            onChange={(groupBy) => prefs.set({ groupBy })}
            options={[
              { value: 'auto', label: t('pref.groupBy.auto') },
              { value: 'day', label: t('pref.groupBy.day') },
              { value: 'month', label: t('pref.groupBy.month') },
            ]}
          />
        </Row>
      </Card>
      <Card title={t('pref.tiles')} titleKey="pref.tiles" icon={<ImageIcon />} scope="device">
        <PrefSwitch k="pref.tileNames" pref="tileNames" kw="filename caption" />
        <PrefSwitch k="pref.tileDuration" pref="tileDuration" kw="badge rozet" />
        <PrefSwitch k="pref.tileFavorites" pref="tileFavorites" kw="heart favorite" />
        <PrefSwitch k="settings.hoverPreview" pref="hoverPreview" kw="hover preview önizleme" />
        <PrefSwitch k="pref.confirmDelete" hint="pref.confirmDeleteHint" pref="confirmDelete" kw="trash çöp delete sil" />
      </Card>
    </Section>
  );
}

const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];
const TRANSITIONS: SlideTransition[] = ['fade', 'slide', 'zoom', 'kenburns', 'blur', 'flip', 'circle', 'random', 'none'];

export function ViewerSection() {
  const t = useT();
  const prefs = usePrefs();
  const slide = useSlideshow((s) => s.settings);
  const setSlide = useSlideshow((s) => s.setSettings);
  return (
    <Section id="viewer" title="pref.sec.viewer" icon={<Eye />}>
      <Card title={t('pref.viewer')} titleKey="pref.viewer" icon={<Eye />} scope="device">
        <Row label={t('pref.viewerBg')} k="pref.viewerBg" kw="background black blur" setting="viewerBg">
          <Segmented<ViewerBg>
            size="sm"
            value={prefs.viewerBg}
            onChange={(viewerBg) => prefs.set({ viewerBg })}
            options={[
              { value: 'blur', label: t('pref.viewerBg.blur') },
              { value: 'black', label: t('pref.viewerBg.black') },
              { value: 'tint', label: t('pref.viewerBg.tint') },
            ]}
          />
        </Row>
        <PrefSwitch k="pref.filmstrip" pref="filmstrip" kw="filmstrip strip şerit" />
        <Row label={t('pref.infoDefault')} k="pref.infoDefault" kw="info details bilgi" setting="infoDefault">
          <Segmented<InfoDefault>
            size="sm"
            value={prefs.infoDefault}
            onChange={(infoDefault) => prefs.set({ infoDefault })}
            options={[
              { value: 'remember', label: t('pref.infoDefault.remember') },
              { value: 'open', label: t('pref.infoDefault.open') },
              { value: 'closed', label: t('pref.infoDefault.closed') },
            ]}
          />
        </Row>
        <Row label={t('pref.zoomStep')} k="pref.zoomStep" hint={t('pref.zoomStepHint')} kw="zoom" setting="zoomStep">
          <Select<number> label={t('pref.zoomStep')} value={prefs.zoomStep} onChange={(zoomStep) => prefs.set({ zoomStep })} options={[1.2, 1.4, 1.6, 2].map((v) => ({ value: v, label: `×${v}` }))} />
        </Row>
        <Row label={t('pref.dblZoom')} k="pref.dblZoom" kw="double tap zoom" setting="dblZoom">
          <Select<number> label={t('pref.dblZoom')} value={prefs.dblZoom} onChange={(dblZoom) => prefs.set({ dblZoom })} options={[2, 2.5, 3, 4].map((v) => ({ value: v, label: `${v * 100}%` }))} />
        </Row>
      </Card>
      <Card title={t('pref.video')} titleKey="pref.video" icon={<Film />} scope="device">
        <PrefSwitch k="pref.videoAutoplay" pref="videoAutoplay" kw="autoplay" />
        <PrefSwitch k="pref.videoMuted" pref="videoMuted" kw="mute ses sound" />
        <PrefSwitch k="pref.rememberVolume" pref="rememberVolume" kw="volume ses" />
        <Row label={t('pref.loopShort')} k="pref.loopShort" hint={t('pref.loopShortHint')} kw="loop repeat tekrar" setting="loopShortSec">
          <Select<number>
            label={t('pref.loopShort')}
            value={prefs.loopShortSec}
            onChange={(loopShortSec) => prefs.set({ loopShortSec })}
            options={[0, 5, 10, 15, 30, 60].map((n) => ({ value: n, label: n ? t('pref.loopShort.sec', { n }) : t('pref.loopShort.off') }))}
          />
        </Row>
        <Row label={t('pref.videoSpeed')} k="pref.videoSpeed" kw="speed playback rate hız" setting="videoSpeed">
          <Select<number> label={t('pref.videoSpeed')} value={prefs.videoSpeed} onChange={(videoSpeed) => prefs.set({ videoSpeed })} options={SPEEDS.map((v) => ({ value: v, label: `${v}×` }))} />
        </Row>
      </Card>
      <Card title={t('pref.slideshow')} titleKey="pref.slideshow" icon={<Presentation />} scope="device">
        <Row label={t('pref.slideDuration')} k="pref.slideDuration" kw="slideshow interval süre" setting="slideDuration">
          <Select<number>
            label={t('pref.slideDuration')}
            value={slide.duration}
            onChange={(duration) => setSlide({ duration })}
            options={[...new Set([2, 3, 4, 5, 7, 10, 15, slide.duration])].sort((a, b) => a - b).map((n) => ({ value: n, label: t('pref.slideDurationValue', { n }) }))}
          />
        </Row>
        <Row label={t('pref.slideTransition')} k="pref.slideTransition" kw="transition geçiş" setting="slideTransition">
          <Select<SlideTransition> label={t('pref.slideTransition')} value={slide.transition} onChange={(transition) => setSlide({ transition })} options={TRANSITIONS.map((v) => ({ value: v, label: t(`slide.tr.${v}` as MessageKey) }))} />
        </Row>
      </Card>
    </Section>
  );
}

export function ExploreSection() {
  const t = useT();
  return (
    <Section id="explore" title="pref.sec.explore" icon={<Compass />}>
      <Card title={t('explore.memories')} titleKey="explore.memories" icon={<Clapperboard />} scope="device">
        <PrefSwitch k="pref.memories" hint="pref.memoriesHint" pref="showMemories" kw="memories anılar on this day" />
      </Card>
      <Opaque keys={['explore.settings.title', 'map.settings.style', 'map.settings.cache', 'explore.settings.homeTz']} kw="map harita tile time zone saat dilimi osm satellite uydu">
        <ExploreSettings delay={0} />
      </Opaque>
      <Opaque keys={['meta.title']} kw="gps metadata exif diagnostics tanı">
        <MetaDiagnostics delay={0} />
      </Opaque>
    </Section>
  );
}

export function PrivacySection() {
  const t = useT();
  return (
    <Section id="privacy" title="pref.sec.privacy" icon={<ShieldCheck />}>
      <Card title={t('pref.location')} titleKey="pref.location" icon={<MapPinOff />} scope="device">
        <PrefSwitch k="pref.hideLocation" hint="pref.hideLocationHint" pref="hideLocation" kw="gps location konum map privacy" />
      </Card>
    </Section>
  );
}
