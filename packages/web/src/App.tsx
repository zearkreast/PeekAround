import { lazy } from 'react';
import { BrowserRouter, Route, Routes } from 'react-router';
import { Layout } from './Layout';
import { HomePage } from '@/features/home/HomePage';
import { GalleryPage } from '@/features/gallery/GalleryPage';
import { SettingsPage } from '@/features/settings/SettingsPage';

const FontsPage = lazy(() => import('@/features/fonts'));
const PhotoEditorPage = lazy(() => import('@/features/photo-editor'));
const CollagePage = lazy(() => import('@/features/collage'));
const VideoEditorPage = lazy(() => import('@/features/video-editor'));
const SlideshowPage = lazy(() => import('@/features/slideshow'));
const ExplorePage = lazy(() => import('@/features/explore'));
const LibraryPage = lazy(() => import('@/features/nav/LibraryPage'));

export function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<HomePage />} />
          <Route path="library/:libraryId" element={<GalleryPage mode="library" />} />
          <Route path="folder/:folderId/*" element={<GalleryPage mode="folder" />} />
          <Route path="timeline" element={<GalleryPage mode="timeline" />} />
          <Route path="favorites" element={<GalleryPage mode="favorites" />} />
          <Route path="search" element={<GalleryPage mode="search" />} />
          <Route path="explore/*" element={<ExplorePage />} />
          <Route path="library" element={<LibraryPage />} />
          <Route path="settings" element={<SettingsPage />} />
          <Route path="fonts" element={<FontsPage />} />
          <Route path="collage" element={<CollagePage />} />
          <Route path="slideshow/*" element={<SlideshowPage />} />
          <Route path="edit/photo/:id" element={<PhotoEditorPage />} />
          <Route path="edit/video/:id" element={<VideoEditorPage />} />
          <Route path="*" element={<HomePage />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
