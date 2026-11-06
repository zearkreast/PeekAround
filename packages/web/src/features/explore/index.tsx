import { lazy, Suspense } from 'react';
import { Route, Routes } from 'react-router';
import { Spinner } from '@/components/ui';
import { ExploreHome } from './Home';
import { MemoryPlayerRoute } from './MemoryPlayer';
import { PlacesPage } from './PlacesPage';
import { ResultPage } from './ResultPage';
import { DuplicatesPage } from './DuplicatesPage';
import './explore.css';

// Leaflet + supercluster only load with the map
const MapPage = lazy(() => import('./MapPage'));

/**
 * /explore                    → groups (memories, places, types, cameras, years)
 * /explore/places             → every place by country
 * /explore/map[?place=TR-34]  → photo map (clusters)
 * /explore/place/:key  /type/:flag  /camera/:key  /year/:year  /memory/:id → result grids
 * /explore/country/:cc  /city/:placeKey|city  /color/:c  /time/night|golden  /shape/:o  /length/short|long
 *   /day/YYYY-MM-DD  /largest  /recent  /no-location                       → insight result grids
 * /explore/duplicates         → exact duplicate groups
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
      <Route path="country/:key" element={<ResultPage key="country" kind="country" />} />
      <Route path="city/:key" element={<ResultPage key="city" kind="city" />} />
      <Route path="color/:value" element={<ResultPage key="color" kind="color" />} />
      <Route path="time/:value" element={<ResultPage key="daytime" kind="daytime" />} />
      <Route path="shape/:value" element={<ResultPage key="shape" kind="shape" />} />
      <Route path="length/:value" element={<ResultPage key="length" kind="length" />} />
      <Route path="day/:key" element={<ResultPage key="day" kind="day" />} />
      <Route path="largest" element={<ResultPage key="largest" kind="largest" />} />
      <Route path="recent" element={<ResultPage key="recent" kind="recent" />} />
      <Route path="no-location" element={<ResultPage key="nogps" kind="nogps" />} />
      <Route path="duplicates" element={<DuplicatesPage />} />
      <Route path="memory/:id/play" element={<MemoryPlayerRoute />} />
      <Route path="*" element={<ExploreHome />} />
    </Routes>
  );
}
