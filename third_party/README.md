# third_party

- `i-slint-backend-winit` 1.18.1 (crates.io source, GPL-3.0-only OR LicenseRef-Slint-*), patched in
  `frame_throttle.rs`: a monitor refresh rate of 0 mHz (Windows reports 0/1 for "default hardware
  rate"; wine on Xvfb, some VMs / RDP sessions) made `TimerBasedFrameThrottle` divide by zero and
  panic at the first redraw. Drop the `[patch.crates-io]` entry once upstream filters it.
