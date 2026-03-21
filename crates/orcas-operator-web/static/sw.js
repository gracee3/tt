self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

function readPushPayload(event) {
  if (!event.data) {
    return null;
  }
  try {
    return event.data.json();
  } catch (_error) {
    try {
      return JSON.parse(event.data.text());
    } catch (_parse_error) {
      return null;
    }
  }
}

function resolveRoutePath(payload) {
  if (!payload || !payload.route || !payload.notification_id) {
    return "/notifications";
  }

  const notificationId = payload.notification_id;
  const route = payload.route;
  const query = new URLSearchParams();
  query.set("notification_id", notificationId);
  query.set("push", "1");

  switch (route.kind) {
    case "inbox_item":
      if (!route.origin_node_id || !route.item_id || !route.candidate_id) {
        return "/notifications";
      }
      query.set("origin_node_id", route.origin_node_id);
      query.set("candidate_id", route.candidate_id);
      return `/inbox/${encodeURIComponent(route.item_id)}?${query.toString()}`;
    case "remote_action_request":
      if (!route.origin_node_id || !route.request_id) {
        return "/notifications";
      }
      query.set("origin_node_id", route.origin_node_id);
      return `/actions/${encodeURIComponent(route.request_id)}?${query.toString()}`;
    case "notifications":
      if (!route.origin_node_id) {
        return "/notifications";
      }
      query.set("origin_node_id", route.origin_node_id);
      return `/notifications?${query.toString()}`;
    case "deliveries":
      if (!route.origin_node_id) {
        return "/notifications";
      }
      query.set("origin_node_id", route.origin_node_id);
      return `/deliveries?${query.toString()}`;
    default:
      return "/notifications";
  }
}

async function focusOrOpenClient(routePath) {
  const absoluteRoute = new URL(routePath, self.location.origin).href;
  const windowClients = await self.clients.matchAll({
    type: "window",
    includeUncontrolled: true,
  });

  for (const client of windowClients) {
    if ("navigate" in client) {
      try {
        await client.navigate(absoluteRoute);
        await client.focus();
        return;
      } catch (_error) {
        // Fall through and try the next client or open a new tab.
      }
    }
  }

  if (windowClients.length > 0) {
    try {
      await windowClients[0].focus();
      if ("navigate" in windowClients[0]) {
        await windowClients[0].navigate(absoluteRoute);
        return;
      }
    } catch (_error) {
      // Fall through and open a new tab.
    }
  }

  if (self.clients.openWindow) {
    await self.clients.openWindow(absoluteRoute);
  }
}

self.addEventListener("push", (event) => {
  event.waitUntil(
    (async () => {
      const payload = readPushPayload(event) || {};
      const routePath = resolveRoutePath(payload);
      const title = payload.title || "Orcas operator notification";
      const body =
        payload.body ||
        "Open Orcas to review mirrored inbox state and any available actions.";
      const notificationOptions = {
        body,
        icon: payload.icon || "/icon-192.svg",
        badge: payload.badge || "/icon-192.svg",
        tag: payload.notification_id || routePath,
        renotify: false,
        data: {
          payload,
          routePath,
        },
      };
      await self.registration.showNotification(title, notificationOptions);
    })(),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(
    (async () => {
      const data = event.notification.data || {};
      const routePath = data.routePath || resolveRoutePath(data.payload || {});
      await focusOrOpenClient(routePath);
    })(),
  );
});
