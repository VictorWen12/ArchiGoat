// Reads the owner's product truth from TrianGoat and renames one product; Projects keeps no copy of it.

import { BridgeError, nativeSession } from "./transport";

export type MineVisibility = "private" | "public";
export type PreviewKind = "html" | "image" | "video" | "pdf" | "text";
export type MineFile = { id: string; path: string; form: string; size: number; sha256: string };
export type ProductMetrics = { served: number; uses: number; useRate: number | null; likes: number; comments: number; shares: number; saves: number };
export type MineProduct = {
  id: string;
  name: string;
  description: string;
  tags: string[];
  form: string;
  size: number;
  sha256: string;
  createdAt: number;
  visibility: MineVisibility;
  publishedAt: number | null;
  previewKind: PreviewKind | null;
  bytesUrl: string | null;
  sessionId: string | null;
  metrics: ProductMetrics | null;
  files: MineFile[];
};
export type MinePage = { products: MineProduct[]; nextCursor: string | null; totalCount: number; privateCount: number; publicCount: number };

type TauriBridge = { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> };
type ArchiWindow = Window & { __TAURI_INTERNALS__?: TauriBridge };
type NativePayload = { status: number; body: number[] };

const text = (value: unknown): value is string => typeof value === "string";
const nonnegative = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const digest = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const validTags = (value: unknown): value is string[] => Array.isArray(value) && value.every((tag) => text(tag) && !!tag.trim() && !/[\u0000-\u001f\u007f]/.test(tag));

function reason(status: number, bytes: Uint8Array): string {
  try {
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    const message = value && typeof value === "object"
      ? (value as { message?: unknown; error?: unknown }).message ?? (value as { error?: unknown }).error
      : null;
    if (text(message) && message.trim()) return message.trim();
  } catch { /* a non-JSON answer falls back to the status meaning */ }
  if (status === 401) return "Sign in to continue";
  if (status === 403) return "This action is not allowed";
  if (status === 404) return "Not found";
  if (status === 409) return "This Work has already changed";
  if (status === 410) return "This Work is already finished";
  if (status === 413) return "File too large";
  if (status === 426) return "Update ArchiGoat to continue.";
  return "TrianGoat did not respond.";
}

// Every Mine read and the rename ride the same allowlisted native call, so one session answers for all three.
async function mineRequest(method: string, path: string, body?: string): Promise<Uint8Array> {
  const bridge = (window as ArchiWindow).__TAURI_INTERNALS__;
  if (!bridge) throw new BridgeError(0, "ArchiGoat's native bridge is unavailable.");
  const token = await nativeSession();
  if (!token) throw new BridgeError(401, "Sign in to continue");
  const headers: Array<[string, string]> = [["Authorization", `Bearer ${token}`]];
  if (body !== undefined) headers.push(["Content-Type", "application/json"]);
  let value: NativePayload;
  try {
    value = await bridge.invoke<NativePayload>("account_request", {
      request: { method, path, headers, body: body === undefined ? null : Array.from(new TextEncoder().encode(body)) },
    });
  } catch (fault) {
    throw new BridgeError(0, typeof fault === "string" && fault.trim() ? fault.trim() : "ArchiGoat could not reach its native bridge.");
  }
  if (!value || !Number.isSafeInteger(value.status) || !Array.isArray(value.body)) throw new BridgeError(0, "TrianGoat returned an invalid response.");
  const bytes = Uint8Array.from(value.body);
  if (value.status < 200 || value.status >= 300) throw new BridgeError(value.status, reason(value.status, bytes));
  return bytes;
}

function fileValue(value: unknown): value is MineFile {
  if (!value || typeof value !== "object") return false;
  const item = value as MineFile;
  return text(item.id) && !!item.id && text(item.path) && !!item.path && text(item.form) && nonnegative(item.size) && digest(item.sha256);
}

function metricsValue(value: unknown): value is ProductMetrics {
  if (!value || typeof value !== "object") return false;
  const item = value as ProductMetrics;
  return nonnegative(item.served) && nonnegative(item.uses) && nonnegative(item.likes) && nonnegative(item.comments)
    && nonnegative(item.shares) && nonnegative(item.saves)
    && (item.useRate === null || (typeof item.useRate === "number" && Number.isFinite(item.useRate)));
}

function previewKind(value: unknown): PreviewKind | null {
  return value === "html" || value === "image" || value === "video" || value === "pdf" || value === "text" ? value : null;
}

function productValue(value: unknown): value is MineProduct {
  if (!value || typeof value !== "object") return false;
  const item = value as MineProduct;
  return text(item.id) && !!item.id && text(item.name) && !!item.name && (item.description === undefined || text(item.description)) && validTags(item.tags) && text(item.form) && nonnegative(item.size) && digest(item.sha256)
    && Number.isSafeInteger(item.createdAt)
    && (item.visibility === "private" || item.visibility === "public")
    && (item.publishedAt === null || Number.isSafeInteger(item.publishedAt))
    && (item.sessionId === null || (text(item.sessionId) && !!item.sessionId))
    && (item.metrics === null || metricsValue(item.metrics))
    && Array.isArray(item.files) && item.files.every(fileValue);
}

// One page of the owner's delivered products, drafts included, exactly as the server counts them.
export async function fetchMine(cursor?: string | null): Promise<MinePage> {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  const value: unknown = JSON.parse(new TextDecoder().decode(await mineRequest("GET", `/auth/mine${query}`)));
  if (!value || typeof value !== "object") throw new BridgeError(0, "TrianGoat returned an invalid app list.");
  const page = value as MinePage;
  if (!Array.isArray(page.products) || !page.products.every(productValue)
    || (page.nextCursor !== null && !text(page.nextCursor))
    || !nonnegative(page.totalCount) || !nonnegative(page.privateCount) || !nonnegative(page.publicCount)) {
    throw new BridgeError(0, "TrianGoat returned an invalid app list.");
  }
  return {
    products: page.products.map((product) => ({ ...product, description: text(product.description) ? product.description : "", previewKind: previewKind(product.previewKind), bytesUrl: text(product.bytesUrl) ? product.bytesUrl : null })),
    nextCursor: page.nextCursor,
    totalCount: page.totalCount,
    privateCount: page.privateCount,
    publicCount: page.publicCount,
  };
}

type PreviewHandle = { status: number; url: string };

// The card's bytes are held for its own frame and never cross this bridge, so a product opened
// twice is fetched once and plays under the boundary its own page declares.
export async function previewUrl(product: MineProduct): Promise<string> {
  const bridge = (window as ArchiWindow).__TAURI_INTERNALS__;
  if (!bridge) throw new BridgeError(0, "ArchiGoat's native bridge is unavailable.");
  let value: PreviewHandle;
  try {
    value = await bridge.invoke<PreviewHandle>("stage_preview", { id: product.id, sha256: product.sha256 });
  } catch (fault) {
    throw new BridgeError(0, typeof fault === "string" && fault.trim() ? fault.trim() : "ArchiGoat could not stage this preview.");
  }
  if (!value || !Number.isSafeInteger(value.status) || !text(value.url)) throw new BridgeError(0, "ArchiGoat returned an invalid preview.");
  if (!value.url) throw new BridgeError(value.status, reason(value.status, new Uint8Array()));
  return value.url;
}

export async function renameProduct(id: string, name: string): Promise<void> {
  await mineRequest("POST", "/auth/mine/rename", JSON.stringify({ id, name }));
}
