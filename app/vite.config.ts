/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import type { Plugin } from 'vite';

/** `vite preview` only: /__shot-delay?ms=N answers after N ms. scripts/screenshots.mjs uses it
 *  to hold the page's load event (and so Firefox's --screenshot) until the window has drawn. */
function screenshotDelay(): Plugin {
  return {
    name: 'linoptions-screenshot-delay',
    configurePreviewServer(server) {
      server.middlewares.use('/__shot-delay', (req, res) => {
        const ms = Math.min(10000, Number(new URL(req.url ?? '', 'http://x').searchParams.get('ms')) || 1000);
        setTimeout(() => {
          res.setHeader('Content-Type', 'image/svg+xml');
          res.end('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>');
        }, ms);
      });
    },
  };
}

// Tauri expects a fixed dev port and serves the built files from `dist`.
// https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [svelte(), screenshotDelay()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
    // The .po catalogues live in ../locale (shared with the Rust crates).
    fs: { allow: ['..'] },
  },
  preview: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    outDir: 'dist',
    // WebView2 (Chromium) on Windows, WebKit on macOS and Linux.
    target: ['es2022', 'chrome110', 'safari15'],
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 400,
  },
  test: {
    environment: 'jsdom',
    include: ['tests/**/*.test.ts'],
    setupFiles: ['tests/setup.ts'],
    restoreMocks: true,
  },
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
});
