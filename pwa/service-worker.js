const CACHE_NAME = "runtrol-phone-v4";
const APP_SHELL = [
  "./",
  "index.html",
  "styles.css",
  "manifest.webmanifest",
  "src/attention.js",
  "src/app.js",
  "src/bytes.js",
  "src/core.js",
  "src/identityStore.js",
  "src/missions.js",
  "src/noise.js",
  "src/pairing.js",
  "src/presentation.js",
  "src/push.js",
  "src/records.js",
  "src/relay.js",
  "assets/event-presentation.json",
  "assets/brand/favicon.svg",
  "assets/brand/lockup-dark.svg",
  "assets/brand/lockup-light.svg",
  "assets/brand/apple-touch-icon.png",
  "assets/brand/icon-192.png",
  "assets/brand/icon-512.png"
];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE_NAME).then((cache) => cache.addAll(APP_SHELL)));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(caches.keys().then((names) => Promise.all(
    names.filter((name) => name !== CACHE_NAME).map((name) => caches.delete(name)),
  )).then(() => self.clients.claim()));
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== self.location.origin) return;
  event.respondWith(caches.match(event.request).then((cached) => cached ?? fetch(event.request)));
});

self.addEventListener("push", (event) => {
  event.waitUntil(self.registration.showNotification("Runtrol needs attention", {
    body: "Open Runtrol to check your PC.",
    icon: "assets/brand/icon-192.png",
    badge: "assets/brand/icon-192.png",
    tag: "runtrol-attention",
    renotify: true,
  }));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((clients) => {
    const current = clients.find((client) => "focus" in client);
    if (current) {
      current.postMessage({ kind: "runtrolAttention" });
      return current.focus();
    }
    return self.clients.openWindow("./?attention=1");
  }));
});
