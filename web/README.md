# F1-photo Web

Vue 3 + Vite + TS + Naive UI + Tailwind + Pinia frontend.

## Dev

```
npm install
npm run dev
```

Dev server runs on http://127.0.0.1:5173 and proxies `/api`, `/healthz`, `/readyz` to the backend at 127.0.0.1:18080.

## Build

```
npm run build
```

Produces `dist/` for rust-embed packaging.
