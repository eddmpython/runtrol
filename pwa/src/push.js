import { base64UrlDecode, equalBytes } from "./bytes.js";

export function pushAvailable(environment = globalThis) {
  return "Notification" in environment
    && "serviceWorker" in environment.navigator
    && "PushManager" in environment;
}

export async function synchronizePush(client, applicationServerKey, registration) {
  if (!applicationServerKey) return { enabled: false, reason: "PC push identity is unavailable." };
  const subscription = await registration.pushManager.getSubscription();
  if (!subscription) return { enabled: false, reason: "Notifications are off." };
  if (!subscriptionUsesKey(subscription, applicationServerKey)) {
    await subscription.unsubscribe();
    await client.setPushSubscription(null);
    return { enabled: false, reason: "Notifications need to be enabled again for this PC." };
  }
  await client.setPushSubscription(subscription.endpoint);
  return { enabled: true, reason: "Notifications are on." };
}

export async function enablePush(client, applicationServerKey, registration) {
  if (!applicationServerKey) throw new Error("This PC has no protected push identity.");
  let subscription = await registration.pushManager.getSubscription();
  if (subscription && !subscriptionUsesKey(subscription, applicationServerKey)) {
    await subscription.unsubscribe();
    subscription = null;
  }
  if (!subscription) {
    subscription = await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: base64UrlDecode(applicationServerKey, 65),
    });
  }
  await client.setPushSubscription(subscription.endpoint);
  return subscription;
}

export async function disablePush(client, registration) {
  const subscription = await registration.pushManager.getSubscription();
  if (subscription) await subscription.unsubscribe();
  await client.setPushSubscription(null);
}

function subscriptionUsesKey(subscription, encodedKey) {
  const bound = subscription.options?.applicationServerKey;
  return bound instanceof ArrayBuffer
    && equalBytes(new Uint8Array(bound), base64UrlDecode(encodedKey, 65));
}
