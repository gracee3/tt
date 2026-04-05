# tt-operator-web

`tt-operator-web` is the browser-facing TT operator app. It is built with Leptos CSR and served locally with Trunk.

## Local development

Prerequisites:

- Rust toolchain with the `wasm32-unknown-unknown` target installed
- `trunk`
- `tt-server` running locally

Suggested setup:

```bash
rustup target add wasm32-unknown-unknown
cd crates/tt-operator-web
trunk serve
```

Then open the Trunk-served URL in a browser and configure:

- server URL
- origin node id
- operator token

Those settings are persisted in localStorage.

## PWA assets

The app ships a minimal PWA shell:

- `static/manifest.webmanifest`
- `static/sw.js`
- `static/icon-192.svg`
- `static/icon-512.svg`

Trunk copies those into the served output. The service worker is used for browser push registration and click-through handling only; actual workflow mutation still runs through `tt-server` and the daemon behind it.

## Notes

If your environment sets `NO_COLOR=1`, Trunk 0.21 may reject it when run directly. In that case, unset `NO_COLOR` for the Trunk command:

```bash
NO_COLOR= trunk serve
```

