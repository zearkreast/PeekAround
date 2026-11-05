import { NavLink, useLocation } from 'react-router';
import { FolderOpen, Smartphone } from 'lucide-react';
import type { DeviceInfo } from '@peekaround/shared';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { relativeTime } from '@/lib/format';
import { folderHref } from '@/lib/routes';
import { useLinkedDevices } from './useDevices';

export function OnlineDot({ online, className }: { online: boolean; className?: string }) {
  return <span className={cn('inline-block size-2 shrink-0 rounded-full', online ? 'bg-ok shadow-[0_0_0_3px_color-mix(in_oklab,var(--ok)_22%,transparent)]' : 'bg-faint/60', className)} />;
}

export function deviceStatus(d: DeviceInfo, t: ReturnType<typeof useT>, lang: ReturnType<typeof useLang>): string {
  return d.online ? t('devices.online') : d.lastSeen ? t('devices.lastSeen', { time: relativeTime(d.lastSeen, lang) }) : t('devices.offline');
}

/** Sidebar "Devices" section: each linked phone with its shared folders. */
export function DevicesNav({ collapsed, title }: { collapsed: boolean; title: (children: React.ReactNode) => React.ReactNode }) {
  const t = useT();
  const lang = useLang();
  const devices = useLinkedDevices();
  const loc = useLocation();
  if (!devices.length) return null;
  return (
    <>
      {title(t('devices.title'))}
      <div className="flex flex-col gap-0.5" data-testid="devices-nav">
        {devices.map((d) => (
          <div key={d.id}>
            {!collapsed && (
              <div className="flex h-8 items-center gap-2 px-2.5 text-[12.5px] text-dim" title={deviceStatus(d, t, lang)}>
                <Smartphone className="size-4 shrink-0" />
                <span className="min-w-0 flex-1 truncate font-medium">{d.name}</span>
                <OnlineDot online={d.online} />
              </div>
            )}
            {d.folders.map((f) => {
              const active = loc.pathname.startsWith(`/folder/${f.id}`);
              const tip = d.online ? `${d.name} · ${f.name}` : t('devices.offlineTip');
              return (
                <NavLink
                  key={f.id}
                  to={folderHref(f.id)}
                  title={collapsed ? `${d.name} · ${f.name}${d.online ? '' : ` — ${t('devices.offlineTip')}`}` : tip}
                  className={cn(
                    'group relative flex h-9 items-center gap-2.5 rounded-xl text-[13px] transition-colors',
                    collapsed ? 'justify-center px-0' : 'pr-2.5 pl-5',
                    active ? 'bg-surface-2 text-fg' : 'text-dim hover:bg-surface hover:text-fg',
                    !d.online && 'opacity-55',
                  )}
                >
                  {collapsed ? (
                    <span className="relative">
                      <Smartphone className="size-[18px]" />
                      <OnlineDot online={d.online} className="absolute -right-0.5 -bottom-0.5 size-1.5" />
                    </span>
                  ) : (
                    <>
                      <FolderOpen className="size-4 shrink-0" />
                      <span className="min-w-0 flex-1 truncate">{f.name}</span>
                      <span className="text-[11px] text-faint tabular-nums">{f.mediaCount || ''}</span>
                    </>
                  )}
                </NavLink>
              );
            })}
          </div>
        ))}
      </div>
    </>
  );
}
