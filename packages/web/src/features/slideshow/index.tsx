import { Route, Routes } from 'react-router';
import { Builder } from './Builder';
import { Player } from './Player';

/** /slideshow → builder (inside the app chrome); /slideshow/play → immersive full-screen player. */
export default function SlideshowPage() {
  return (
    <Routes>
      <Route path="play" element={<Player />} />
      <Route path="*" element={<Builder />} />
    </Routes>
  );
}
