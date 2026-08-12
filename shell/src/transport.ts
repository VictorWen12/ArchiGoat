// Keeps Account and the local Agent on the public ArchiGoat bridge only.

export const PROTOCOL = 16;
export const ACCOUNT_ORIGIN = "https://triangoat.com";
export const ACCOUNT_AUTHORIZATION_URL = `${ACCOUNT_ORIGIN}/?authorize=archigoat`;

export type Session = { id: string; title: string; updatedAt: number; folderId: string | null; pinnedAt: number | null };
export type Attachment = { id: string; name: string; media: string; bytes: number; sha256: string; image: boolean; url: string };
export type Turn = { id: number; role: "me" | "goat"; text: string; at: number; workId?: string; deliveryId?: string; attachments: Attachment[]; product?: Product };
export type ProductFile = { id: string; path: string; form: string; size: number; sha256: string };
export type Product = { id: string; name: string; description: string; tags: string[]; form: string; size: number; previewKind: "image" | "video" | "html" | "pdf" | "text" | null; published: boolean; files: ProductFile[] };
export type WorkIntent = "brief" | "build";
// Account's status.rs owns this wire vocabulary; the shell only validates and displays it.
export type CreatorStatus = "designing" | "ready_to_build" | "building" | "preview" | "published" | "failed" | "stopped";
export type PublishMetadata = { id: string; name: string; description: string; tags: string[] };
export type WorkProgress = { sequence: number; text: string };
export type WorkContext = { author: string; source: string; provenance: "user" | "agent" | "external"; text: string; attachments: string[] };
export type PendingInput = { id: string; name: string; media: string; bytes: number; sha256: string; image: boolean; url: string };
export type PendingWork = { deliveryId: string; workId: string; scopeKind: "goat"; scopeId: string; goal: string; context: WorkContext[]; attachments: PendingInput[]; computer: string | null };
export type WorkFile = { artifactId: string; workId: string; name: string; bytes: number; sha256: string; format: string; width?: number; height?: number };
// One conversation fact from the Agent, in its own append-only order. Field names are the wire's.
export type WorkEvent =
  | { seq: number; at: string; kind: "agent_message"; id: string; text: string }
  | { seq: number; at: string; kind: "user_message"; steer_id: string; text: string; attachments: string[] }
  | { seq: number; at: string; kind: "stage"; label: string }
  | { seq: number; at: string; kind: "artifact"; name: string }
  | { seq: number; at: string; kind: "turn_boundary"; reason: string; elapsed_seconds: number };
export type WorkSnapshot = { phase: string; awaiting: boolean; text: string; events: WorkEvent[]; startedAt: number; progress?: WorkProgress; tokens?: number; model?: string; kind?: "answer" | "artifact"; run?: string; files: WorkFile[] };
export type PendingSummon = { deliveryId: string; workId: string; scopeId: string; goal: string; computer: string | null; intent: WorkIntent };
export type RemoteWork = { workId: string; computer: string; status: CreatorStatus; state: string; availability: string; live: boolean; progress: string; reason: string; events: WorkEvent[]; text: string; awaiting: boolean; intent: WorkIntent };
export type InputReceipt = { session: string; nonce: string; id: string; name: string; media: string; bytes: number; sha256: string; proof?: string };
// TypicalMs is the account's one measured median over recently finished builds, so every creator reads
// the same number; it is null until enough builds have finished to measure one. Only the account can
// know it — this computer alone has no such history, and a per-computer figure would be a different fact.
export type WorkStatus = { deliveryId: string; workId: string; status: CreatorStatus; typicalMs: number | null };
export type AgentModel = { id: string; label: string };
export type AgentTier = { model?: string; effort?: string };
export type AgentPresets = { best: AgentTier; fast: AgentTier };
export type PairOffer = { code: string; expiresAt: number };
export type PairedPhone = { pairId: string; device: string; name: string };
export type AccountIdentity = { email: string };

let bearer: string | null = null;
let voucher: { value: string; expiresAt: number } | null = null;

type TauriBridge = { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> };
type CloseRequest = { preventDefault(): void };
type NativeWindow = { onCloseRequested(handler: (event: CloseRequest) => Promise<void>): Promise<() => unknown>; destroy(): Promise<void> };
type ArchiWindow = Window & { __TAURI_INTERNALS__?: TauriBridge; __TAURI__?: { window?: { getCurrentWindow(): NativeWindow } } };

type NativePayload = { status: number; body: number[] };

async function nativeInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
  const bridge = (window as ArchiWindow).__TAURI_INTERNALS__;
  if (!bridge) return null;
  try { return await bridge.invoke<T>(command, args); } catch { return null; }
}

// Tauri rejects with a plain string, so the real reason reaches the screen instead of an opaque object.
async function nativeInvokeRequired(command: string, args?: Record<string, unknown>): Promise<void> {
  await nativeInvokeValue(command, args);
}

async function nativeInvokeValue<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
  const bridge = (window as ArchiWindow).__TAURI_INTERNALS__;
  if (!bridge) return null;
  try {
    return await bridge.invoke<T>(command, args);
  } catch (reason) {
    if (reason instanceof Error) throw reason;
    throw new BridgeError(0, typeof reason === "string" && reason.trim() ? reason.trim() : "ArchiGoat could not reach its native bridge.");
  }
}

// Holds the native quit gesture while a Work runs; the returned release restores the plain close.
export async function holdQuit(decide: () => Promise<boolean>): Promise<() => void> {
  const native = (window as ArchiWindow).__TAURI__?.window?.getCurrentWindow();
  if (!native) return () => undefined;
  let unlisten: (() => unknown) | null = null;
  const release = async (): Promise<void> => { const stop = unlisten; unlisten = null; if (stop) await stop(); };
  unlisten = await native.onCloseRequested(async (event) => {
    event.preventDefault();
    if (!await decide()) return;
    await release();
    await native.destroy().catch(() => undefined);
  });
  return () => { void release(); };
}

// Opens the exact Account origin in the OS browser, with a plain-browser development fallback.
export async function openAccount(): Promise<void> {
  const opened = await nativeInvoke<boolean>("open_account");
  if (opened !== true) window.open(ACCOUNT_ORIGIN, "_blank", "noopener,noreferrer");
}

export async function authorizeAccount(): Promise<void> {
  const opened = await nativeInvoke<boolean>("authorize_account");
  if (opened !== true) window.open(ACCOUNT_AUTHORIZATION_URL, "_blank", "noopener,noreferrer");
}

class BridgeResponse {
  constructor(readonly status: number, private readonly bytes: Uint8Array) {}
  get ok(): boolean { return this.status >= 200 && this.status < 300; }
  clone(): BridgeResponse { return new BridgeResponse(this.status, this.bytes.slice()); }
  async json(): Promise<unknown> { try { return JSON.parse(new TextDecoder().decode(this.bytes)); } catch { return null; } }
  text(): string { return new TextDecoder().decode(this.bytes); }
  async blob(): Promise<Blob> { return new Blob([this.bytes.slice().buffer as ArrayBuffer]); }
}

async function requestBytes(body: BodyInit | null | undefined): Promise<number[] | null> {
  if (body == null) return null;
  if (typeof body === "string") return Array.from(new TextEncoder().encode(body));
  if (body instanceof Blob) return Array.from(new Uint8Array(await body.arrayBuffer()));
  throw new BridgeError(0, "This request body is not supported.");
}

async function nativeRequest(command: "account_request" | "loopback_request", init: { method: string; path: string; headers: Headers; body: BodyInit | null | undefined; authenticated?: boolean; voucher?: string | null }): Promise<BridgeResponse> {
  const bridge = (window as ArchiWindow).__TAURI_INTERNALS__;
  if (!bridge) throw new BridgeError(0, "ArchiGoat's native bridge is unavailable.");
  let value: NativePayload;
  try {
    const request = {
      method: init.method,
      path: init.path,
      headers: Array.from(init.headers.entries()),
      body: await requestBytes(init.body),
      ...(command === "loopback_request" ? { authenticated: init.authenticated === true, voucher: init.voucher ?? null } : {}),
    };
    value = await bridge.invoke<NativePayload>(command, { request });
  } catch (reason) {
    if (reason instanceof Error) throw reason;
    throw new BridgeError(0, typeof reason === "string" && reason.trim() ? reason.trim() : "ArchiGoat could not reach its native bridge.");
  }
  if (!value || !Number.isSafeInteger(value.status) || !Array.isArray(value.body)) throw new BridgeError(0, "ArchiGoat received an invalid native response.");
  return new BridgeResponse(value.status, Uint8Array.from(value.body));
}

const validToken = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const validVoucher = (value: unknown): value is string => typeof value === "string" && value.length <= 1024 && /^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(value);
const text = (value: unknown): value is string => typeof value === "string";
const creatorStatus = (value: unknown): value is CreatorStatus =>
  value === "designing" || value === "ready_to_build" || value === "building"
  || value === "preview" || value === "published" || value === "failed" || value === "stopped";
const nonnegative = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const validTags = (value: unknown): value is string[] => Array.isArray(value) && value.every((tag) => text(tag) && !!tag.trim() && !/[\u0000-\u001f\u007f]/.test(tag));

export class BridgeError extends Error {
  constructor(readonly status: number, message: string) { super(message); }
}

function errorMessage(status: number, fallback: string): string {
  if (status === 401) return "Sign in to continue";
  if (status === 403) return "This action is not allowed";
  if (status === 404) return "Not found";
  if (status === 409) return "This Work has already changed";
  if (status === 410) return "This Work is already finished";
  if (status === 413) return "File too large";
  if (status === 426) return "Update ArchiGoat to continue.";
  return fallback;
}

// The refusal the other side wrote reaches the screen, whether it answered JSON or plain words.
async function bodyMessage(response: BridgeResponse): Promise<string | null> {
  const value: unknown = await response.clone().json().catch(() => null);
  if (value && typeof value === "object") {
    const message = (value as { message?: unknown; error?: unknown }).message ?? (value as { error?: unknown }).error;
    if (text(message) && message.trim()) return message.trim();
    return null;
  }
  const plain = response.clone().text().trim();
  return plain && plain.length <= 200 && !/[\u0000-\u001f]/.test(plain) ? plain : null;
}

async function accountRequest(path: string, init: RequestInit = {}, token = bearer): Promise<BridgeResponse> {
  const headers = new Headers(init.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  headers.delete("Origin");
  const response = await nativeRequest("account_request", { method: init.method || "GET", path, headers, body: init.body });
  if (!response.ok) throw new BridgeError(response.status, (await bodyMessage(response)) ?? errorMessage(response.status, "TrianGoat did not respond."));
  return response;
}

async function json<T>(response: BridgeResponse): Promise<T> {
  const value: unknown = await response.json().catch(() => null);
  if (value === null) throw new BridgeError(0, "ArchiGoat received an invalid response.");
  return value as T;
}

// The launcher reports a failed daemon start through argv; the shell renders it instead of staying silent.
export async function startError(): Promise<string | null> {
  const message = await nativeInvoke<unknown>("start_error");
  return typeof message === "string" && message.length > 0 ? message : null;
}

export async function nativeSession(): Promise<string | null> {
  if (bearer) return bearer;
  // A rejected read carries the real session-file fault to the screen; only a missing file is null.
  const value = await nativeInvokeValue<unknown>("credential_get");
  bearer = validToken(value) ? value : null;
  return bearer;
}

// A failed handoff never hides an already stored session; it only speaks up when nothing else signs the user in.
export async function restoreSession(): Promise<string | null> {
  const url = await nativeInvoke<unknown>("handoff_argument");
  let handoffFault: unknown = null;
  if (text(url)) {
    try {
      await nativeInvokeRequired("forward_handoff", { url });
      // A fresh handoff supersedes any cached bearer; the next read takes the newer session from the session file.
      bearer = null;
      for (let attempt = 0; attempt < 20; attempt += 1) {
        const token = await nativeSession();
        if (token) break;
        await new Promise((resolve) => window.setTimeout(resolve, 250));
      }
    } catch (reason) { handoffFault = reason; }
  }
  const token = await nativeSession();
  if (!token) {
    if (handoffFault) throw handoffFault;
    return null;
  }
  try {
    await accountRequest("/auth/me");
    return token;
  } catch (error) {
    if (error instanceof BridgeError && error.status === 401) {
      await clearSession();
      if (handoffFault) throw handoffFault;
      return null;
    }
    throw error;
  }
}

export async function fetchIdentity(): Promise<AccountIdentity> {
  const value = await json<{ email?: unknown }>(await accountRequest("/auth/me"));
  if (!text(value.email) || !value.email) throw new BridgeError(0, "TrianGoat returned an invalid account.");
  return { email: value.email };
}

export async function clearSession(): Promise<void> {
  await endLocalSession();
  bearer = null;
  voucher = null;
  await nativeInvokeRequired("credential_clear");
}

export async function logout(): Promise<void> {
  try {
    if (bearer) await accountRequest("/auth/logout", { method: "POST" });
  } finally {
    await clearSession();
  }
}

// EndLocalSession is revocation-only, so it never depends on already-expired Account authority.
async function endLocalSession(): Promise<void> {
  await loopbackRequest("/v1/session/end", { method: "POST" }, false).catch(() => undefined);
}

async function appVoucher(): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  if (voucher && voucher.expiresAt > now + 15) return voucher.value;
  const value = await json<{ voucher?: unknown; expiresAt?: unknown }>(await accountRequest("/auth/app/voucher"));
  if (!validVoucher(value.voucher) || !Number.isSafeInteger(value.expiresAt)) throw new BridgeError(0, "TrianGoat returned an invalid Agent voucher.");
  voucher = { value: value.voucher, expiresAt: value.expiresAt as number };
  return voucher.value;
}

async function loopbackRequest(path: string, init: RequestInit = {}, authenticate = true): Promise<BridgeResponse> {
  const headers = new Headers(init.headers);
  const loopbackVoucher = authenticate ? await appVoucher() : null;
  const response = await nativeRequest("loopback_request", { method: init.method || "GET", path, headers, body: init.body, authenticated: authenticate, voucher: loopbackVoucher });
  // The Agent's own words win, including on 409, where it names either a moved Work or a stale identity.
  if (!response.ok) {
    const message = await bodyMessage(response);
    throw new BridgeError(response.status, message
      ?? (response.status === 409 ? "Sign in again to reconnect your Agent." : errorMessage(response.status, "Your Agent did not respond.")));
  }
  return response;
}

export async function agentHealth(): Promise<{ device: string; registered: boolean; version: string; protocol: number }> {
  const value = await json<{ device?: unknown; registered?: unknown; version?: unknown; protocol?: unknown }>(await loopbackRequest("/v1/health", {}, false));
  if (!text(value.device) || !value.device || typeof value.registered !== "boolean" || !text(value.version) || value.protocol !== PROTOCOL) throw new BridgeError(426, "Update ArchiGoat to continue.");
  return { device: value.device, registered: value.registered, version: value.version, protocol: value.protocol };
}

function agentModelValue(value: unknown): value is AgentModel {
  if (!value || typeof value !== "object") return false;
  const item = value as AgentModel;
  return text(item.id) && !!item.id && text(item.label) && !!item.label;
}

// A published tier names a model, a reasoning depth, or neither.
function agentTier(value: unknown): AgentTier {
  const tier = (value ?? {}) as AgentTier;
  return {
    model: text(tier.model) && tier.model ? tier.model : undefined,
    effort: text(tier.effort) && tier.effort ? tier.effort : undefined,
  };
}

export async function agentStatus(): Promise<{ state: string; provider: string | null; device: string; installed: string[] | null; model: string | null; effort: string | null; models: AgentModel[]; presets: AgentPresets | null }> {
  const value = await json<{ state?: unknown; provider?: unknown; device?: unknown; installed?: unknown; model?: unknown; effort?: unknown; models?: unknown; presets?: unknown }>(await loopbackRequest("/v1/status"));
  if (!text(value.state) || !text(value.device) || !value.device) throw new BridgeError(0, "The Agent returned an invalid status.");
  const installed = value.installed === undefined
    ? null
    : Array.isArray(value.installed) && value.installed.every((item) => text(item) && !!item)
      ? value.installed
      : null;
  const published = value.presets as { best?: unknown; fast?: unknown } | null | undefined;
  return {
    state: value.state,
    provider: text(value.provider) ? value.provider : null,
    device: value.device,
    installed,
    model: text(value.model) && value.model ? value.model : null,
    effort: text(value.effort) && value.effort ? value.effort : null,
    models: Array.isArray(value.models) && value.models.every(agentModelValue) ? value.models : [],
    presets: published && typeof published === "object"
      ? { best: agentTier(published.best), fast: agentTier(published.fast) }
      : null,
  };
}

export async function submitSignInCode(code: string): Promise<void> {
  await loopbackRequest("/v1/connect/code", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code }),
  });
}

export async function connectAgent(provider: string, model?: string, effort?: string): Promise<void> {
  const body: { provider: string; model?: string; effort?: string } = { provider };
  if (model?.trim()) body.model = model;
  if (effort?.trim()) body.effort = effort;
  await loopbackRequest("/v1/connect", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
  });
}

export async function createPairOffer(device: string): Promise<PairOffer> {
  const value = await json<{ code?: unknown; expiresAt?: unknown }>(await accountRequest("/auth/pair/offer", {
    method: "POST",
    headers: { "Content-Type": "application/json", "x-app-device": device },
    body: JSON.stringify({ computer: device }),
  }));
  if (!validToken(value.code) || !Number.isSafeInteger(value.expiresAt)) throw new BridgeError(0, "The Agent returned an invalid pairing code.");
  return { code: value.code, expiresAt: value.expiresAt as number };
}

export async function pairedPhones(device: string): Promise<PairedPhone[]> {
  const value = await json<{ viewer?: unknown; devices?: unknown }>(await accountRequest("/auth/pair/roster", {
    headers: { "x-app-device": device },
  }));
  if (value.viewer !== "computer" || !Array.isArray(value.devices)) throw new BridgeError(0, "The Agent returned an invalid device list.");
  if (value.devices.some((item) => {
    if (!item || typeof item !== "object") return true;
    const row = item as Partial<PairedPhone>;
    return !text(row.pairId) || !row.pairId || !text(row.device) || !row.device || !text(row.name) || !row.name;
  })) throw new BridgeError(0, "The Agent returned an invalid device list.");
  return value.devices as PairedPhone[];
}

export async function revokePair(pairId: string, device: string): Promise<void> {
  await accountRequest(`/auth/pair/${encodeURIComponent(pairId)}`, {
    method: "DELETE",
    headers: { "x-app-device": device },
  });
}

function sessionValue(value: unknown): value is Session {
  if (!value || typeof value !== "object") return false;
  const item = value as Session;
  return text(item.id) && !!item.id && text(item.title) && Number.isSafeInteger(item.updatedAt) && (item.folderId === null || text(item.folderId))
    && (item.pinnedAt === null || Number.isSafeInteger(item.pinnedAt));
}

function attachmentValue(value: unknown): value is Attachment {
  if (!value || typeof value !== "object") return false;
  const item = value as Attachment;
  return text(item.id) && !!item.id && text(item.name) && !!item.name && text(item.media)
    && nonnegative(item.bytes) && validToken(item.sha256) && typeof item.image === "boolean" && text(item.url) && item.url.startsWith("/auth/");
}

function productValue(value: unknown): value is Product {
  if (!value || typeof value !== "object") return false;
  const item = value as Product;
  return text(item.id) && !!item.id && text(item.name) && !!item.name && (item.description === undefined || text(item.description)) && validTags(item.tags) && text(item.form) && nonnegative(item.size)
    && Array.isArray(item.files) && item.files.every((file) => !!file && typeof file === "object" && text((file as ProductFile).id)
      && text((file as ProductFile).path) && text((file as ProductFile).form) && nonnegative((file as ProductFile).size) && validToken((file as ProductFile).sha256));
}

function turnValue(value: unknown): value is Turn {
  if (!value || typeof value !== "object") return false;
  const item = value as Turn;
  return Number.isSafeInteger(item.id) && (item.role === "me" || item.role === "goat") && text(item.text) && Number.isSafeInteger(item.at)
    && (item.workId === undefined || (text(item.workId) && !!item.workId))
    && (item.deliveryId === undefined || (text(item.deliveryId) && !!item.deliveryId))
    && Array.isArray(item.attachments) && item.attachments.every(attachmentValue) && (item.product === undefined || productValue(item.product));
}

function workContextValue(value: unknown): value is WorkContext {
  if (!value || typeof value !== "object") return false;
  const item = value as WorkContext;
  return text(item.author) && !!item.author && text(item.source) && !!item.source
    && (item.provenance === "user" || item.provenance === "agent" || item.provenance === "external")
    && text(item.text) && Array.isArray(item.attachments) && item.attachments.every((id) => text(id) && !!id);
}

function pendingInputValue(value: unknown): value is PendingInput {
  if (!value || typeof value !== "object") return false;
  const item = value as PendingInput;
  return text(item.id) && !!item.id && text(item.name) && !!item.name && text(item.media) && !!item.media
    && nonnegative(item.bytes) && validToken(item.sha256) && typeof item.image === "boolean"
    && text(item.url) && item.url.startsWith("/auth/work/pending/input?");
}

function fetchPendingWork(value: unknown, deliveryId: string, workId: string, session: string): PendingWork {
  if (!value || typeof value !== "object") throw new BridgeError(0, "TrianGoat returned invalid pending Work.");
  const item = value as PendingWork;
  if (item.deliveryId !== deliveryId || item.workId !== workId || item.scopeKind !== "goat" || item.scopeId !== session
    || !text(item.goal) || !Array.isArray(item.context) || item.context.some((entry) => !workContextValue(entry))
    || !Array.isArray(item.attachments) || item.attachments.some((entry) => !pendingInputValue(entry))
    || (item.computer !== null && !text(item.computer))) throw new BridgeError(0, "TrianGoat returned invalid pending Work.");
  return item;
}

export async function fetchSessions(): Promise<Session[]> {
  const value = await json<{ sessions?: unknown }>(await accountRequest("/auth/goat/sessions"));
  if (!Array.isArray(value.sessions) || value.sessions.some((item) => !sessionValue(item))) throw new BridgeError(0, "TrianGoat returned an invalid app list.");
  return value.sessions;
}

export async function createSession(): Promise<string> {
  // The server derives the title from the first brief; the client never invents one.
  const value = await json<{ id?: unknown }>(await accountRequest("/auth/goat/sessions", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({}),
  }));
  if (!text(value.id) || !value.id) throw new BridgeError(0, "TrianGoat did not create the app session.");
  return value.id;
}

export async function fetchTurns(session: string): Promise<Turn[]> {
  const value = await json<{ turns?: unknown }>(await accountRequest(`/auth/goat/turns?session=${encodeURIComponent(session)}`));
  if (!Array.isArray(value.turns) || value.turns.some((item) => !turnValue(item))) throw new BridgeError(0, "TrianGoat returned an invalid conversation.");
  return value.turns.map((turn) => turn.product
    ? { ...turn, product: { ...turn.product, description: text(turn.product.description) ? turn.product.description : "" } }
    : turn);
}

export async function appendTurn(session: string, textValue: string, attachments: string[], intent: WorkIntent): Promise<{ id: number; at: number; deliveryId: string; workId: string; status: "designing"; pending: PendingWork; created: boolean }> {
  const response = await accountRequest("/auth/goat/append", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session, role: "me", text: textValue, attachments, intent }),
  });
  const value = await json<{ id?: unknown; at?: unknown; deliveryId?: unknown; workId?: unknown; status?: unknown; pending?: unknown }>(response);
  if (!Number.isSafeInteger(value.id) || !Number.isSafeInteger(value.at) || !text(value.deliveryId) || !value.deliveryId || !text(value.workId) || !value.workId) {
    throw new BridgeError(0, "TrianGoat did not accept this idea.");
  }
  if (value.status !== "designing") throw new BridgeError(0, "TrianGoat returned invalid Creator status.");
  return {
    id: value.id as number,
    at: value.at as number,
    deliveryId: value.deliveryId,
    workId: value.workId,
    status: value.status,
    pending: fetchPendingWork(value.pending, value.deliveryId, value.workId, session),
    created: response.status === 201,
  };
}

export async function steerTurn(session: string, workId: string, textValue: string, attachments: string[], computer?: string): Promise<{ id: number; at: number; deliveryId: string; workId: string; computer: string | null; status: "building" }> {
  const response = await accountRequest("/auth/goat/steer", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ session, workId, text: textValue, attachments, ...(computer ? { computer } : {}) }),
  });
  const value = await json<{ id?: unknown; at?: unknown; deliveryId?: unknown; workId?: unknown; computer?: unknown; status?: unknown }>(response);
  if (!Number.isSafeInteger(value.id) || !Number.isSafeInteger(value.at) || !text(value.deliveryId) || !value.deliveryId
    || value.workId !== workId || (value.computer !== null && !text(value.computer)) || value.status !== "building") {
    throw new BridgeError(0, "TrianGoat did not continue this Work.");
  }
  return { id: value.id as number, at: value.at as number, deliveryId: value.deliveryId, workId, computer: value.computer, status: value.status };
}

export async function removeSession(session: string): Promise<void> {
  await accountRequest("/auth/goat/remove", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session }) });
}

export async function renameSession(session: string, title: string): Promise<void> {
  await accountRequest("/auth/goat/rename", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session, title }) });
}

export async function uploadAttachment(file: File): Promise<Attachment> {
  const digest = await sha256(file);
  const value = await json<{ id?: unknown }>(await accountRequest("/auth/attachments", {
    method: "POST", headers: { "x-file-name": encodeURIComponent(file.name), "Content-Type": file.type || "application/octet-stream" }, body: file,
  }));
  if (!text(value.id) || !value.id) throw new BridgeError(0, "TrianGoat did not save the attachment.");
  return { id: value.id, name: file.name, media: file.type || "application/octet-stream", bytes: file.size, sha256: digest, image: file.type.startsWith("image/"), url: `/auth/attachments/${encodeURIComponent(value.id)}` };
}

// AttachmentImageUrl turns authorized history bytes into a CSP-safe image source after verifying their identity.
export async function attachmentImageUrl(attachment: Attachment): Promise<string> {
  const bytes = await (await accountRequest(attachment.url)).blob();
  if (!attachment.image || bytes.size !== attachment.bytes || await sha256(bytes) !== attachment.sha256) {
    throw new BridgeError(0, "TrianGoat returned an invalid attachment.");
  }
  return URL.createObjectURL(new Blob([bytes], { type: attachment.media }));
}

export async function deleteAttachment(id: string): Promise<void> {
  await accountRequest(`/auth/attachments/${encodeURIComponent(id)}`, { method: "DELETE" });
}

async function stageBytes(workId: string, item: Pick<Attachment, "id" | "name" | "media" | "bytes" | "sha256">, body: Blob): Promise<InputReceipt> {
  const response = await loopbackRequest(`/v1/input?workId=${encodeURIComponent(workId)}`, {
    method: "POST",
    headers: {
      "Content-Type": item.media,
      "x-work-id": workId,
      "x-work-input-id": item.id,
      "x-work-input-bytes": String(item.bytes),
      "x-work-input-sha256": item.sha256,
      "x-file-name": encodeURIComponent(item.name),
    },
    body,
  });
  const value = await json<Partial<InputReceipt>>(response);
  if (!text(value.session) || !text(value.nonce) || !text(value.id) || !text(value.name) || !text(value.media) || !nonnegative(value.bytes) || !validToken(value.sha256)) throw new BridgeError(0, "The Agent did not accept the attachment.");
  return value as InputReceipt;
}

export async function stagePendingInput(workId: string, item: PendingInput, local?: Blob): Promise<InputReceipt> {
  const bytes = local?.size === item.bytes ? local : await (await accountRequest(item.url)).blob();
  if (bytes.size !== item.bytes || await sha256(bytes) !== item.sha256) throw new BridgeError(0, "Attach the file again.");
  return stageBytes(workId, item, bytes);
}

export async function startWork(workId: string, conversation: string, goal: string, context: readonly WorkContext[], attachments: InputReceipt[], intent: WorkIntent = "build"): Promise<void> {
  await loopbackRequest(`/v1/work?workId=${encodeURIComponent(workId)}`, {
    method: "POST", headers: { "Content-Type": "application/json", "x-work-id": workId, "x-work-kind": "start" },
    body: JSON.stringify({ conversation, goal, context, attachments, intent }),
  });
}

export async function steerLocalWork(workId: string, steerId: number, textValue: string, attachments: InputReceipt[]): Promise<void> {
  await loopbackRequest(`/v1/work/steer?workId=${encodeURIComponent(workId)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "x-work-id": workId, "x-work-kind": "steer" },
    body: JSON.stringify({ goal: textValue, attachments, steerId }),
  });
}

export async function publishLocalWork(workId: string): Promise<void> {
  await loopbackRequest(`/v1/work/publish?workId=${encodeURIComponent(workId)}`, {
    method: "POST",
    headers: { "x-work-id": workId, "x-work-kind": "publish" },
  });
}

export async function stopWork(workId: string): Promise<void> {
  await loopbackRequest(`/v1/work?workId=${encodeURIComponent(workId)}`, { method: "DELETE", headers: { "x-work-id": workId, "x-work-kind": "stop" } });
}

// An Agent that publishes no events is not broken; its conversation is the one text field.
function workEvents(value: unknown): WorkEvent[] {
  if (!Array.isArray(value)) return [];
  const events: WorkEvent[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== "object") continue;
    const item = entry as Record<string, unknown>;
    if (!Number.isSafeInteger(item.seq) || !text(item.at)) continue;
    const head = { seq: item.seq as number, at: item.at };
    if (item.kind === "agent_message" && text(item.id) && text(item.text)) events.push({ ...head, kind: "agent_message", id: item.id, text: item.text });
    else if (item.kind === "user_message" && text(item.steer_id) && text(item.text)) {
      const attachments = Array.isArray(item.attachments) ? item.attachments.filter(text) : [];
      events.push({ ...head, kind: "user_message", steer_id: item.steer_id, text: item.text, attachments });
    } else if (item.kind === "stage" && text(item.label)) events.push({ ...head, kind: "stage", label: item.label });
    else if (item.kind === "artifact" && text(item.name)) events.push({ ...head, kind: "artifact", name: item.name });
    else if (item.kind === "turn_boundary" && text(item.reason) && nonnegative(item.elapsed_seconds)) events.push({ ...head, kind: "turn_boundary", reason: item.reason, elapsed_seconds: item.elapsed_seconds });
  }
  return events.sort((left, right) => left.seq - right.seq);
}

// Only these three phases end a Work; any other word the Agent publishes is still live.
export function finished(phase: string): boolean {
  return phase === "done" || phase === "stopped" || phase === "failed";
}

export async function readWork(workId: string): Promise<WorkSnapshot | null> {
  try {
    const response = await loopbackRequest(`/v1/work?workId=${encodeURIComponent(workId)}`);
    if (response.status === 204) return null;
    const value = await json<Partial<WorkSnapshot> & { awaiting?: unknown; events?: unknown }>(response);
    if (!text(value.phase) || !text(value.text) || !Number.isSafeInteger(value.startedAt) || !Array.isArray(value.files)) throw new BridgeError(0, "The Agent returned invalid Work status.");
    return { ...(value as WorkSnapshot), awaiting: value.awaiting === true, events: workEvents(value.events) };
  } catch (error) {
    if (error instanceof BridgeError && error.status === 404) return null;
    throw error;
  }
}

export async function waitForWork(workId: string, onSnapshot: (snapshot: WorkSnapshot) => void, signal: AbortSignal): Promise<WorkSnapshot> {
  let missing = 0;
  for (;;) {
    signal.throwIfAborted();
    const snapshot = await readWork(workId);
    if (!snapshot) {
      missing += 1;
      if (missing > 30) throw new BridgeError(404, "Work was not found");
      await new Promise<void>((resolve, reject) => {
        const timer = window.setTimeout(resolve, 700);
        signal.addEventListener("abort", () => { window.clearTimeout(timer); reject(signal.reason); }, { once: true });
      });
      continue;
    }
    missing = 0;
    onSnapshot(snapshot);
    if (finished(snapshot.phase)) return snapshot;
    await new Promise<void>((resolve, reject) => {
      const timer = window.setTimeout(resolve, 700);
      signal.addEventListener("abort", () => { window.clearTimeout(timer); reject(signal.reason); }, { once: true });
    });
  }
}

// Answers whether one delivery is still owed, so a Work interrupted by quitting is not delivered twice.
export async function workStatus(deliveryId: string): Promise<WorkStatus | null> {
  try {
    const value = await json<Partial<WorkStatus>>(await accountRequest(`/auth/work/status?delivery=${encodeURIComponent(deliveryId)}`));
    if (!text(value.deliveryId) || !text(value.workId) || !creatorStatus(value.status)) throw new BridgeError(0, "TrianGoat returned invalid delivery status.");
    // No measured typical yet is simply no typical; the wait still shows its line and its clock.
    const typicalMs = Number.isSafeInteger(value.typicalMs) && (value.typicalMs as number) > 0 ? (value.typicalMs as number) : null;
    return { ...(value as WorkStatus), typicalMs };
  } catch (error) {
    if (error instanceof BridgeError && error.status === 404) return null;
    throw error;
  }
}

// The account's own list of Work still owed a delivery, whichever device ordered it.
// The account answers in pages, and every page is read, so a long list never hides a Work.
export async function pendingWorks(): Promise<PendingSummon[]> {
  const summons: PendingSummon[] = [];
  let after: string | null = null;
  for (;;) {
    const value: { summons?: unknown; next?: unknown } = await json(await accountRequest(`/auth/work/pending${after ? `?after=${encodeURIComponent(after)}` : ""}`));
    if (!Array.isArray(value.summons)) throw new BridgeError(0, "TrianGoat returned invalid pending deliveries.");
    for (const entry of value.summons) {
      if (!entry || typeof entry !== "object") continue;
      const item = entry as Record<string, unknown>;
      if (item.scopeKind !== "goat" || !text(item.deliveryId) || !text(item.workId) || !text(item.scopeId) || !text(item.goal)) continue;
      summons.push({ deliveryId: item.deliveryId, workId: item.workId, scopeId: item.scopeId, goal: item.goal, computer: text(item.computer) ? item.computer : null, intent: item.intent === "brief" ? "brief" : "build" });
    }
    if (!text(value.next) || !value.next || value.next === after) return summons;
    after = value.next;
  }
}

// A Work this computer did not start still shows its own state, in the account's words.
export async function remoteWork(workId: string, status: CreatorStatus, intent: WorkIntent = "build"): Promise<RemoteWork | null> {
  try {
    const value = await json<Record<string, unknown>>(await accountRequest(`/auth/remote/work?work=${encodeURIComponent(workId)}`));
    if (!text(value.workId) || !text(value.computer) || !text(value.state) || !text(value.availability)) throw new BridgeError(0, "TrianGoat returned invalid remote Work status.");
    const snapshot = value.snapshot as { progress?: { text?: unknown }; events?: unknown; text?: unknown; awaiting?: unknown } | null | undefined;
    const progress = snapshot && typeof snapshot === "object" && text(snapshot.progress?.text) ? snapshot.progress.text : "";
    // The snapshot carries the Agent's whole conversation; this window shows it instead of a bare state word.
    const events = snapshot && typeof snapshot === "object" ? workEvents(snapshot.events) : [];
    const answer = snapshot && typeof snapshot === "object" && text(snapshot.text) ? snapshot.text : "";
    // Awaiting is the other computer's own signal that its Agent parked this turn on the creator.
    const awaiting = !!snapshot && typeof snapshot === "object" && snapshot.awaiting === true;
    return { workId: value.workId, computer: value.computer, status, state: value.state, availability: value.availability, live: value.live === true, progress, reason: text(value.reason) ? value.reason : "", events, text: answer, awaiting, intent };
  } catch (error) {
    if (error instanceof BridgeError && error.status === 404) return null;
    throw error;
  }
}

export async function stopRemoteWork(computer: string, workId: string): Promise<void> {
  await accountRequest("/auth/remote/stop", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ computer, workId }),
  });
}

// Hands a terminal Work to the daemon so it streams frozen artifacts without browser-sized IPC bytes.
export async function deliverLocalWork(session: string, deliveryId: string, workId: string): Promise<void> {
  if (!bearer) throw new BridgeError(401, "Sign in to continue");
  await loopbackRequest(`/v1/work/deliver?workId=${encodeURIComponent(workId)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ accountToken: bearer, scopeKind: "goat", scopeId: session, deliveryId }),
  });
}

export async function publishProduct(metadata: PublishMetadata): Promise<void> {
  await accountRequest("/auth/mine/public", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(metadata),
  });
}

export async function deleteProduct(id: string): Promise<void> {
  await accountRequest("/auth/mine/delete", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ id }),
  });
}

async function sha256(value: Blob): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", await value.arrayBuffer());
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
