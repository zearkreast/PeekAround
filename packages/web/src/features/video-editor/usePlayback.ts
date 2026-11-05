import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from 'react';
import type { MediaDetails } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { clamp } from './model';
import { useEditor, usePlayhead } from './store';

export interface Playback {
  videoRef: RefObject<HTMLVideoElement | null>;
  src: string;
  /** server-side start of the transcode stream (0 for direct playback) */
  offset: number;
  transcode: boolean;
  autoPlay: boolean;
  fps: number;
  play(): void;
  pause(): void;
  toggle(): void;
  /** jump to a source time */
  seek(t: number): void;
  /** live preview while dragging (no stream restart for transcoded sources) */
  scrub(t: number): void;
  endScrub(t?: number): void;
  step(frames: number): void;
  onVideoEvent: {
    onPlay(): void;
    onPause(): void;
    onLoadedMetadata(): void;
  };
}

/**
 * Drives the preview <video>: source-time bookkeeping (incl. restart-on-seek transcode
 * streams), looping inside the trim range and applying speed / volume.
 */
export function usePlayback(item: MediaDetails): Playback {
  const backend = useBackend();
  const transcode = item.playback === 'transcode';
  const videoRef = useRef<HTMLVideoElement>(null);
  const [offset, setOffset] = useState(0);
  const offsetRef = useRef(0);
  const wantPlay = useRef(false);
  const wasPlaying = useRef(false);
  const duration = useEditor((s) => s.duration);
  const speed = useEditor((s) => s.s.speed);
  const volume = useEditor((s) => s.s.volume);
  const mute = useEditor((s) => s.s.mute);
  const fps = item.video?.fps || 30;
  const src = transcode ? backend.streamUrl(item.id, offset) : backend.fileUrl(item.id);

  const current = useCallback(() => {
    const v = videoRef.current;
    return v ? offsetRef.current + v.currentTime : usePlayhead.getState().time;
  }, []);

  const seekTo = useCallback(
    (t: number, keepPlaying: boolean) => {
      const time = clamp(t, 0, duration);
      usePlayhead.setState({ time });
      const v = videoRef.current;
      if (transcode) {
        wantPlay.current = keepPlaying;
        offsetRef.current = time;
        setOffset(time);
      } else if (v) {
        v.currentTime = time;
        if (keepPlaying && v.paused) void v.play().catch(() => {});
      }
    },
    [duration, transcode],
  );

  // 60 fps clock + loop within the trim range
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      raf = requestAnimationFrame(tick);
      const v = videoRef.current;
      if (!v || usePlayhead.getState().scrubbing) return;
      const t = offsetRef.current + v.currentTime;
      const { trim } = useEditor.getState().s;
      if (!v.paused && t >= trim.end - 0.02) {
        seekTo(trim.start, true);
        return;
      }
      if (Math.abs(t - usePlayhead.getState().time) > 1e-4) usePlayhead.setState({ time: t });
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [seekTo]);

  const applyAudio = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    v.defaultPlaybackRate = speed;
    v.playbackRate = speed;
    v.volume = clamp(volume, 0, 1);
    v.muted = mute || volume === 0;
  }, [speed, volume, mute]);
  useEffect(applyAudio, [applyAudio, src]);

  const play = useCallback(() => {
    const { trim } = useEditor.getState().s;
    const t = current();
    if (t < trim.start - 0.01 || t >= trim.end - 0.05) seekTo(trim.start, true);
    else {
      wantPlay.current = true;
      void videoRef.current?.play().catch(() => {});
    }
  }, [current, seekTo]);

  const pause = useCallback(() => {
    wantPlay.current = false;
    videoRef.current?.pause();
  }, []);

  const toggle = useCallback(() => {
    if (videoRef.current && !videoRef.current.paused) pause();
    else play();
  }, [play, pause]);

  const seek = useCallback((t: number) => seekTo(t, !!videoRef.current && !videoRef.current.paused), [seekTo]);

  const scrub = useCallback(
    (t: number) => {
      const v = videoRef.current;
      if (!usePlayhead.getState().scrubbing) {
        wasPlaying.current = !!v && !v.paused;
        v?.pause();
      }
      const time = clamp(t, 0, duration);
      usePlayhead.setState({ time, scrubbing: true });
      if (!transcode && v) v.currentTime = time;
    },
    [duration, transcode],
  );

  const endScrub = useCallback(
    (t?: number) => {
      const time = t ?? usePlayhead.getState().time;
      usePlayhead.setState({ scrubbing: false });
      seekTo(time, wasPlaying.current);
      wasPlaying.current = false;
    },
    [seekTo],
  );

  const step = useCallback(
    (frames: number) => {
      pause();
      seekTo(current() + frames / fps, false);
    },
    [pause, seekTo, current, fps],
  );

  const onVideoEvent = useMemo(
    () => ({
      onPlay: () => usePlayhead.setState({ playing: true }),
      onPause: () => {
        if (!wantPlay.current || !transcode) usePlayhead.setState({ playing: false });
      },
      onLoadedMetadata: () => {
        applyAudio();
        if (wantPlay.current) void videoRef.current?.play().catch(() => {});
      },
    }),
    [applyAudio, transcode],
  );

  useEffect(() => () => usePlayhead.setState({ time: 0, playing: false, scrubbing: false }), []);

  const autoPlay = wantPlay.current;
  return useMemo(
    () => ({ videoRef, src, offset, transcode, autoPlay, fps, play, pause, toggle, seek, scrub, endScrub, step, onVideoEvent }),
    [src, offset, transcode, autoPlay, fps, play, pause, toggle, seek, scrub, endScrub, step, onVideoEvent],
  );
}
