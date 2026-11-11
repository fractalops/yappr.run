export type PushSubscriptionData = {
  endpoint: string;
  p256dh: string;
  auth: string;
  vapid_private: string;
  vapid_public: string;
  app_url: string;
};

export type PushKeys = { publicKey: string; privateKey: string };
const STORAGE_KEY = "yappr.push.keys";

export function isWebPage() {
  return typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);
}

export function supportsPush() {
  return isWebPage() && window.isSecureContext && "serviceWorker" in navigator
    && "PushManager" in window && "Notification" in window && !!crypto.subtle;
}

function keys(): PushKeys | null {
  const value = localStorage.getItem(STORAGE_KEY);
  return value ? JSON.parse(value) as PushKeys : null;
}

function base64url(bytes: Uint8Array) {
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function decode(key: string) {
  return Uint8Array.from(atob(key.replaceAll("-", "+").replaceAll("_", "/")), (c) => c.charCodeAt(0));
}

export async function createPushKeys(): Promise<PushKeys> {
  const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const pub = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  const priv = await crypto.subtle.exportKey("jwk", pair.privateKey);
  if (!priv.d) throw new Error("Cannot create the notification key.");
  return { publicKey: base64url(pub), privateKey: priv.d };
}

async function restoreSubscription(manager: PushManager, saved: PushKeys): Promise<PushSubscriptionData> {
  const current = await manager.getSubscription();
  const currentKey = current?.options.applicationServerKey;
  if (current && (!currentKey || base64url(new Uint8Array(currentKey)) !== saved.publicKey)) {
    await unsubscribe(manager);
  }
  const subscription = await manager.getSubscription() ?? await subscribe(manager, saved);
  return subscriptionData(subscription, saved);
}

function subscribe(manager: PushManager, saved: PushKeys) {
  return manager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: decode(saved.publicKey),
  });
}

function subscriptionData(subscription: PushSubscription, saved: PushKeys): PushSubscriptionData {
  const auth = subscription.getKey("auth");
  const p256dh = subscription.getKey("p256dh");
  if (!auth || !p256dh) throw new Error("The browser did not supply the notification keys.");
  return {
    endpoint: subscription.endpoint,
    auth: base64url(new Uint8Array(auth)),
    p256dh: base64url(new Uint8Array(p256dh)),
    vapid_private: saved.privateKey,
    vapid_public: saved.publicKey,
    app_url: new URL("./", location.href).href,
  };
}

async function unsubscribe(manager: PushManager) {
  const subscription = await manager.getSubscription();
  if (subscription && !await subscription.unsubscribe() && await manager.getSubscription()) {
    throw new Error("The browser could not stop notifications. Try again.");
  }
}

export async function enablePush(manager: PushManager, saved: PushKeys) {
  let subscription;
  try {
    subscription = await subscribe(manager, saved);
  } catch (error) {
    if (Notification.permission === "denied" || (error instanceof DOMException && error.name === "NotAllowedError")) {
      throw new Error("Check the notification permissions for Yappr, then try again.");
    }
    throw error;
  }
  const data = subscriptionData(subscription, saved);
  localStorage.setItem(STORAGE_KEY, JSON.stringify(saved));
  return data;
}

export async function restorePush(manager: PushManager) {
  const saved = keys();
  return saved && Notification.permission === "granted" ? restoreSubscription(manager, saved) : null;
}

export async function disablePush(manager: PushManager) {
  await unsubscribe(manager);
  localStorage.removeItem(STORAGE_KEY);
}

export async function rotatePush(manager: PushManager) {
  const saved = await createPushKeys();
  await disablePush(manager);
  return enablePush(manager, saved);
}
