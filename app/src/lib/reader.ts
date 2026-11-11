// The reader in WebAssembly. One tab owns the endpoint, so that two tabs do
// not use the same iroh identity at the same time.
declare const __YAPPR_BUILD__: string;
declare const __YAPPR_BASE__: string;

type Handler = (payload: unknown) => void;
interface Reader { invoke(command: string, args: string): Promise<string>; }

const handlers = new Set<Handler>();
let loading: Promise<Reader> | null = null;

async function load(): Promise<Reader> {
  const version = encodeURIComponent(__YAPPR_BUILD__);
  const root = `${__YAPPR_BASE__}/wasm`;
  const module = await import(/* @vite-ignore */ `${root}/yappr_browser.js?v=${version}`);
  await module.default({ module_or_path: `${root}/yappr_browser_bg.wasm?v=${version}` });
  return new module.App((json: string) => {
    const payload: unknown = JSON.parse(json);
    handlers.forEach((handler) => handler(payload));
  }) as Reader;
}

function reader(): Promise<Reader> {
  loading ??= new Promise<Reader>((resolve, reject) => {
    if (!navigator.locks) { reject(new Error("Yappr needs a browser with Web Locks support.")); return; }
    const locks = navigator.locks;
    const controller = new AbortController();
    const alreadyOpen = () => reject(new Error("Yappr is already open in another tab. Close that tab, then reload."));
    const ownEndpoint = async () => {
      try {
        resolve(await load());
        // The browser releases this lock when the owning page closes.
        await new Promise<void>(() => {});
      } catch (error) { reject(error); }
    };
    void locks.request("yappr.endpoint", { ifAvailable: true }, async (lock) => {
      if (lock) return ownEndpoint();
      const { held } = await locks.query();
      if (!held?.some((item) => item.name === "yappr.notification-reply")) return alreadyOpen();
      // A notification may still be sending an answer when this page opens.
      const timer = setTimeout(() => controller.abort(), 12000);
      return locks.request("yappr.endpoint", { signal: controller.signal }, async () => {
        clearTimeout(timer);
        await ownEndpoint();
      }).catch(alreadyOpen);
    }).catch(reject);
  });
  return loading;
}

// Runs a reader command. A failed command rejects with its message.
export async function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  const app = await reader();
  try {
    return JSON.parse(await app.invoke(command, JSON.stringify(args))) as T;
  } catch (error) {
    throw typeof error === "string" ? error : String(error);
  }
}

// Receives each reader event. Returns a function that stops it.
export async function listen<T>(handler: (payload: T) => void): Promise<() => void> {
  await reader();
  handlers.add(handler as Handler);
  return () => handlers.delete(handler as Handler);
}

// Copy text after a user selects a copy action.
export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}

export const deviceName = "My browser";
