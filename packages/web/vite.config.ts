import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { VitePWA } from 'vite-plugin-pwa';
import { fileURLToPath } from 'node:url';

const api = process.env.PEEK_API ?? 'http://127.0.0.1:8787';

export default defineConfig({
  base: '/',
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  plugins: [
    react(),
    tailwindcss(),
    VitePWA({
      registerType: 'autoUpdate',
      injectRegister: null,
      manifest: {
        name: 'PeekAround',
        short_name: 'PeekAround',
        description: 'Your folders, beautifully.',
        theme_color: '#0b0b10',
        background_color: '#0b0b10',
        display: 'standalone',
        start_url: './',
        icons: [
          { src: 'icons/icon-192.png', sizes: '192x192', type: 'image/png' },
          { src: 'icons/icon-512.png', sizes: '512x512', type: 'image/png' },
          { src: 'icons/icon-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
      },
      workbox: {
        navigateFallbackDenylist: [/^\/api\//],
        globPatterns: ['**/*.{js,css,html,woff2,svg,png}'],
        maximumFileSizeToCacheInBytes: 6 * 1024 * 1024,
        runtimeCaching: [
          {
            // Explore map tiles (CARTO / OpenStreetMap): offline-friendly, bounded
            urlPattern: ({ url }) => /(^|\.)basemaps\.cartocdn\.com$/.test(url.hostname) || url.hostname === 'tile.openstreetmap.org',
            handler: 'CacheFirst',
            options: {
              cacheName: 'peek-map-tiles',
              expiration: { maxEntries: 2000, maxAgeSeconds: 30 * 24 * 60 * 60, purgeOnQuotaError: true },
              cacheableResponse: { statuses: [0, 200] },
            },
          },
        ],
      },
    }),
  ],
  server: {
    port: 5173,
    host: true,
    proxy: {
      '/api': { target: api, changeOrigin: true, ws: true },
    },
  },
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 1500,
  },
});
