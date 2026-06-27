// plaintextesports service worker: show match reminder notifications.
self.addEventListener('push', function (event) {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch (e) {
    data = {};
  }
  const title = data.title || 'plaintextesports';
  const options = {
    body: data.body || '',
    tag: data.tag,
    data: { url: data.url || '/' },
  };
  event.waitUntil(self.registration.showNotification(title, options));
});

// The browser rotated our push subscription (key expiry, etc.). Re-subscribe
// with the same application server key (kept on the old subscription's options)
// and tell the server to move this browser's reminders to the new endpoint, so
// pending notifications survive the rotation instead of waiting for the next
// visit to re-arm.
self.addEventListener('pushsubscriptionchange', function (event) {
  event.waitUntil(
    (async function () {
      try {
        const old = event.oldSubscription;
        let fresh = event.newSubscription;
        if (!fresh) {
          const key = old && old.options && old.options.applicationServerKey;
          if (!key) return;
          fresh = await self.registration.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey: key,
          });
        }
        const j = fresh.toJSON();
        await fetch('/api/push-migrate', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            old_endpoint: old ? old.endpoint : '',
            endpoint: j.endpoint,
            p256dh: j.keys.p256dh,
            auth: j.keys.auth,
          }),
        });
      } catch (e) {
        // Best effort; the next page visit reconciles via the existing path.
      }
    })()
  );
});

self.addEventListener('notificationclick', function (event) {
  event.notification.close();
  const url = (event.notification.data && event.notification.data.url) || '/';
  event.waitUntil(
    clients.matchAll({ type: 'window', includeUncontrolled: true }).then(function (list) {
      for (const client of list) {
        if ('focus' in client) {
          client.navigate(url);
          return client.focus();
        }
      }
      if (clients.openWindow) return clients.openWindow(url);
    })
  );
});
