import { lazy, Suspense } from 'react';
import { Route, Routes } from 'react-router';
import { Spinner } from '@/components/ui';
import { ExploreHome } from './Home';
import { MemoryPlayerRoute } from './MemoryPlayer';
import { PlacesPage } from './PlacesPage';
import { ResultPage } from './ResultPage';
import './explore.css';

// Leaflet + supercluster only load with the map
const MapPage = lazy(() => import('./MapPage'));

/**
 * /explore                    → groups (memories, places, types, cameras, years)
 * /explore/places             → every place by country
 * /explore/map[?place=TR-34]  → photo map (clusters)
 * /explore/place/:key  /type/:flag  /camera/:key  /year/:year  /memory/:id → result grids
 * /explore/memory/:id/play    → story-style memory player (full-screen overlay)
 */
export default function ExplorePage() {
  return (
    <Routes>
      <Route index element={<ExploreHome />} />
      <Route path="places" element={<PlacesPage />} />
      <Route
        path="map"
        element={
          <Suspense
            fallback={
              <div className="grid h-[60vh] place-items-center">
                <Spinner className="size-7" />
              </div>
            }
          >
            <MapPage />
          </Suspense>
        }
      />
      <Route path="place/:key" element={<ResultPage key="place" kind="place" />} />
      <Route path="type/:flag" element={<ResultPage key="type" kind="type" />} />
      <Route path="camera/:key" element={<ResultPage key="camera" kind="camera" />} />
      <Route path="year/:year" element={<ResultPage key="year" kind="year" />} />
      <Route path="memory/:id" element={<ResultPage key="memory" kind="memory" />} />
      <Route path="memory/:id/play" element={<MemoryPlayerRoute />} />
      <Route path="*" element={<ExploreHome />} />
    </Routes>
  );
}
