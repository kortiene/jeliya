import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  test: {
    // Vitest owns src/**/*.test.ts only; ui/e2e/*.spec.ts belongs to
    // Playwright (npm run test:e2e) and must not be collected here.
    include: ['src/**/*.test.ts'],
  },
});
