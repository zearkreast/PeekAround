import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import type { DeviceInfo, FileRef, FolderRef, MediaItem } from '@peekaround/shared';
import { create } from 'zustand';
import { useBackend, useInfo } from '@/backend';
import { HttpBackend } from '@/backend/http';
import { isNative } from '@/backend/android/plugin';
import { useLibraries } from '@/lib/queries';

/** All devices the server shows this client (paired + pending pairing requests). */
export function useDevices() {
  const b = useBackend();
  return useQuery({
    queryKey: ['devices', b.id],
    queryFn: async () => (b.devices ? await b.devices() : []),
    enabled: !!b.devices,
    staleTime: 10_000,
  });
}

/** Paired devices with their shared folders (from /api/libraries). */
export function useLinkedDevices(): DeviceInfo[] {
  const libs = useLibraries();
  return useMemo(() => (libs.data?.devices ?? []).filter((d) => d.paired !== false && d.folders.length > 0), [libs.data?.devices]);
}

/** folder id → its device (or null for a PC folder) */
export function useDeviceFolders(): Map<string, { device: DeviceInfo; folder: FolderRef }> {
  const libs = useLibraries();
  return useMemo(() => {
    const m = new Map<string, { device: DeviceInfo; folder: FolderRef }>();
    for (const d of libs.data?.devices ?? []) for (const f of d.folders) m.set(f.id, { device: d, folder: f });
    return m;
  }, [libs.data?.devices]);
}

export function useDeviceFolder(folderId: string | undefined | null) {
  const map = useDeviceFolders();
  return folderId ? (map.get(folderId) ?? null) : null;
}

/** This client may approve pairing requests (desktop window, or a headless server's web UI). */
export function useCanApprove(): boolean {
  const info = useInfo();
  const b = useBackend();
  if (!b.devices || !info) return false;
  if (isNative()) return false;
  // desktop: LAN clients never get pending devices from /api/devices (and approval answers 403 there)
  return info.mode === 'server' || info.mode === 'desktop';
}

/** The Android app talking to a PeekAround server (downloads are saved on the phone natively). */
export function isPhoneClient(b: { kind: string }): boolean {
  return isNative() && b.kind === 'http' && b instanceof HttpBackend;
}

// ── dialogs opened by actions / menus ──

export type DeviceDialog =
  | { kind: 'copyToPc'; refs: FileRef[]; label: string }
  | { kind: 'sendToPhone'; refs: FileRef[]; label: string }
  | { kind: 'saveToPhone'; items: MediaItem[] }
  | { kind: 'saveFolderToPhone'; folderId: string; dir: string; name: string }
  | { kind: 'uploadFromPhone'; target: { folderId: string; dir: string } };

interface DeviceUi {
  dialog: DeviceDialog | null;
  /** pending pairing requests to show (desktop window) */
  pairs: DeviceInfo[];
  /** pairing requests the user closed without deciding */
  dismissed: Set<string>;
  open(d: DeviceDialog): void;
  close(): void;
  pushPair(d: DeviceInfo): void;
  dropPair(id: string, dismiss?: boolean): void;
}

export const useDeviceUi = create<DeviceUi>((set) => ({
  dialog: null,
  pairs: [],
  dismissed: new Set(),
  open: (dialog) => set({ dialog }),
  close: () => set({ dialog: null }),
  pushPair: (d) =>
    set((s) => {
      const dismissed = new Set(s.dismissed);
      dismissed.delete(d.id);
      return { pairs: [...s.pairs.filter((x) => x.id !== d.id), d], dismissed };
    }),
  dropPair: (id, dismiss) =>
    set((s) => {
      const dismissed = new Set(s.dismissed);
      if (dismiss) dismissed.add(id);
      return { pairs: s.pairs.filter((x) => x.id !== id), dismissed };
    }),
}));
