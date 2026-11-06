import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
    environment: 'node',
    pool: 'forks',
    // the integration tests run real ffmpeg jobs
    testTimeout: 180_000,
    hookTimeout: 120_000,
    // shared CI runners are slow and noisy: one retry for timing-sensitive integration tests
    retry: process.env.CI ? 1 : 0,
  },
});
