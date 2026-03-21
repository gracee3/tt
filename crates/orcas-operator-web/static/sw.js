self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

// Push delivery is intentionally a later layer. The current worker only
// establishes the install/activate scaffold for the PWA shell.
