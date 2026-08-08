// Runs a delivered card in its own frame: seeds the state it starts from, keeps what it writes,
// and answers every request it waits on, so a card in Projects plays exactly as it plays on a phone.

type VoteState = { choice: string | null; counts: Record<string, number>; total: number };

const MAX_KEYS = 64;
const MAX_KEY_BYTES = 256;
const MAX_VALUE_BYTES = 32_768;
const MAX_TOTAL_BYTES = 65_536;
const MAX_REQUEST_BYTES = 128;

// A preview keeps its card's state for as long as the app is open; nothing about it is written to disk.
const saved = new Map<string, Record<string, string>>();
const stances = new Map<string, VoteState>();

function bounded(value: unknown, maximum: number, allowEmpty = true): value is string {
  return typeof value === "string" && (allowEmpty || value.length > 0) && new TextEncoder().encode(value).length <= maximum;
}

function choiceOf(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 && value.length <= 32 && /^[a-z0-9_-]+$/.test(value) ? value : null;
}

// A card declares open tokens the server re-checks; one malformed entry declares nothing.
function declaredLabels(value: unknown): Record<string, number> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const entries = Object.entries(value as Record<string, unknown>);
  if (!entries.length || entries.length > 64) return null;
  for (const [name, weight] of entries) {
    if (!/^[a-z0-9]{3,64}$/u.test(name)) return null;
    if (typeof weight !== "number" || !Number.isFinite(weight) || weight < 0 || weight > 1) return null;
  }
  return Object.fromEntries(entries) as Record<string, number>;
}

const NO_STANCE: VoteState = { choice: null, counts: {}, total: 0 };

// The frame reads this as its window name before its first line runs, so the card starts where it stopped.
// The preview surface carries no feed chrome, so both insets are honestly zero rather than absent.
export function productRuntimeName(productId: string): string {
  return JSON.stringify({ type: "product:runtime", resultId: productId, storage: saved.get(productId) ?? {}, bottomInset: 0, rightInset: 0 });
}

function keep(productId: string, values: Record<string, string>): void {
  const total = Object.entries(values).reduce((bytes, [key, item]) => bytes + new TextEncoder().encode(key).length + new TextEncoder().encode(item).length, 0);
  if (Object.keys(values).length <= MAX_KEYS && total <= MAX_TOTAL_BYTES) saved.set(productId, values);
}

function store(productId: string, message: { operation?: unknown; key?: unknown; value?: unknown }): void {
  if (message.operation === "clear") { saved.delete(productId); return; }
  if (!bounded(message.key, MAX_KEY_BYTES)) return;
  const values = { ...saved.get(productId) };
  if (message.operation === "remove") {
    delete values[message.key];
    keep(productId, values);
    return;
  }
  if (message.operation === "set" && bounded(message.value, MAX_VALUE_BYTES)) {
    values[message.key] = message.value;
    keep(productId, values);
  }
}

// Binds one frame to one product: only that frame is heard, and each request it sends is settled once.
// A draft answers with the one stance this session holds; a published card's tally belongs to the
// feed, so this window never invents a number for it.
export function bindProductFrame(frame: HTMLIFrameElement, productId: string, published: boolean): () => void {
  const listen = (event: MessageEvent<unknown>): void => {
    const view = frame.contentWindow;
    if (!view || event.source !== view || !event.data || typeof event.data !== "object") return;
    const message = event.data as { type?: unknown; requestId?: unknown; choice?: unknown; labels?: unknown; operation?: unknown; key?: unknown; value?: unknown };
    if (message.type === "product:storage") { store(productId, message); return; }
    if (message.type !== "product:vote" && message.type !== "product:vote-state" && message.type !== "product:label") return;
    if (!bounded(message.requestId, MAX_REQUEST_BYTES, false)) return;
    const requestId = message.requestId;
    const answer = (result: Record<string, unknown>): void => { view.postMessage({ requestId, ...result }, "*"); };
    if (message.type === "product:label") {
      answer(declaredLabels(message.labels) ? { type: "product:label-result" } : { type: "product:label-result", error: "Labels unavailable" });
      return;
    }
    if (message.type === "product:vote-state") {
      answer({ type: "product:vote-result", state: published ? NO_STANCE : stances.get(productId) ?? NO_STANCE });
      return;
    }
    const choice = published ? null : choiceOf(message.choice);
    if (!choice) { answer({ type: "product:vote-result", error: "Vote unavailable" }); return; }
    const state: VoteState = { choice, counts: { [choice]: 1 }, total: 1 };
    stances.set(productId, state);
    answer({ type: "product:vote-result", state });
  };
  window.addEventListener("message", listen);
  return () => window.removeEventListener("message", listen);
}
