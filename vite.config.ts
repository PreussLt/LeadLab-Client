import { defineConfig } from 'vite'

// Tauri erwartet einen festen Port im Entwicklungsmodus und keine
// Bildschirmlöschung, damit Rust-Fehler sichtbar bleiben.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  // Zweite Seite für die Fenster des Betriebsmodus (Hauptrepo #1377).
  build: {
    target: 'es2021', outDir: 'dist', emptyOutDir: true,
    rollupOptions: { input: { main: 'index.html', betrieb: 'betrieb.html' } },
  },
  test: { environment: 'jsdom' },
})
