# GitHub CodeQL alerts on zearkreast/peakaround@main (313c6d0 = 68f19e0 code), 47 open, all High

Line numbers refer to the 1.0 code (68f19e0); map them with `git show 68f19e0:<file>`.

## js/polynomial-redos (2)
- packages/server/src/paths.ts:62  (#47)
- packages/server/src/media/scanner.ts:158  (#46)

## js/missing-rate-limiting (15)
- packages/server/src/routes/files.ts: 49, 127, 161, 170, 198, 226, 261, 306, 331  (#37–#45)
- packages/server/src/routes/core.ts: 141, 161, 186, 187  (#33–#36)
- packages/server/src/routes/media.ts: 58  (#32)

## js/path-injection (8)
- packages/server/src/routes/files.ts:296 (×2)  (#30, #31)
- packages/server/src/routes/core.ts: 149, 166  (#28, #29)
- packages/server/src/paths.ts:107  (#27)
- packages/server/src/library.ts:136  (#26)
- packages/server/src/jobs/index.ts: 78, 82  (#24, #25)

## js/xss-through-dom (22)
- web/src/features/viewer/Viewer.tsx: 132, 140, 147, 277, 427  (#19–#23)
- web/src/features/viewer/VideoPlayer.tsx:128  (#18)
- web/src/features/video-editor/Stage.tsx:108 (#17), Timeline.tsx:235 (#16)
- web/src/features/slideshow/MediaPicker.tsx:188 (#15), ShowStage.tsx:97,109 (#13,#14), ItemList.tsx:65 (#12)
- web/src/features/photo-editor/EditorStage.tsx:293 (#11)
- web/src/features/home/HomePage.tsx:45 (#10)
- web/src/features/gallery/actions.tsx:54 (#9), MediaTile.tsx:118,132 (#7,#8), FolderCards.tsx:30 (#6)
- web/src/features/collage/PhotosPanel.tsx:68 (#5), PhotoPicker.tsx:191 (#4), CollageCanvas.tsx:47,133,495 (#1–#3)
