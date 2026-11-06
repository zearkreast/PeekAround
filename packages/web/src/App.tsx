import { lazy } from 'react';
import { BrowserRouter, Navigate, Route, Routes, useLocation } from 'react-router';
import { Layout } from './Layout';
import { GalleryPage } from '@/features/gallery/GalleryPage';
import { SettingsPage } from '@/features/settings/SettingsPage';

const FontsPage = lazy(() => import('@/features/fonts'));
const PhotoEditorPage = lazy(() => import('@/features/photo-editor'));
const CollagePage = lazy(() => import('@/features/collage'));
const VideoEditorPage = lazy(() => import('@/features/video-editor'));
const SlideshowPage = lazy(() => import('@/features/slideshow'));
const ExplorePage = lazy(() => import('@/features/explore'));
const LibraryPage = lazy(() => import('@/features/nav/LibraryPage'));
const AlbumsPage = lazy(() => import('@/features/library/AlbumsPage'));
const AlbumPage = lazy(() => import('@/features/library/AlbumPage'));

/** old routes → their 1.2 places, keeping ?view= / ?q= (open viewer, search) */
function Redirect({ to }: { to: string }) {
  const loc = useLocation();
  return <Navigate to={{ pathname: to, search: loc.search }} replace />;
}

export function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Layout />}>
          {/* Photos (timeline + memories) is the landing page; the old Home lives on in the Library hub */}
          <Route index element={<GalleryPage mode="timeline" />} />
          <Route path="library/:libraryId" element={<GalleryPage mode="library" />} />
          <Route path="folder/:folderId/*" element={<GalleryPage mode="folder" />} />
          <Route path="timeline" element={<Redirect to="/" />} />
          <Route path="home" element={<Redirect to="/library" />} />
          <Route path="albums" element={<AlbumsPage />} />
          <Route path="album/:albumId" element={<AlbumPage />} />
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
          <Route path="*" element={<Redirect to="/" />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
