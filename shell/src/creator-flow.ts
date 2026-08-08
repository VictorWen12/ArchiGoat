import { useEffect, useState } from "react";
import type { BuildState, PreviewProduct } from "./build-preview";
import { previewUrl as minePreviewUrl, type MineProduct } from "./mine";
import type { SessionState } from "./projects";
import {
  finished,
  type CreatorStatus,
  type Product,
  type RemoteWork,
  type Turn,
  type WorkEvent,
  type WorkIntent,
} from "./transport";

export type CreatorRun = {
  workId: string;
  deliveryId: string;
  status: CreatorStatus;
  intent: WorkIntent;
  phase: string;
  awaiting: boolean;
  text: string;
  events: WorkEvent[];
  startedAt: number;
  typicalMs: number | null;
  model?: string;
  tokens?: number;
  controller: AbortController;
};

export type PreviewTarget =
  | { source: "turn"; product: Product; session: string }
  | { source: "mine"; product: MineProduct; session: string | null };

export type PreviewLoad = { url: string | null; error: string };

const FRAMEWORK_FIELDS = ["Mechanic", "Hook", "Looks", "Sound", "Effects", "Assumption"] as const;

function framework(text: string): boolean {
  return FRAMEWORK_FIELDS.filter((field) => new RegExp(`(?:^|\\n)\\s*(?:#{1,3}\\s*)?${field}\\s*:`, "iu").test(text)).length >= 3;
}

// Chat owns creator briefs and the locked Framework. Runtime output, paths, classifiers, and
// products never become conversation UI.
export function creatorChatTurns(turns: readonly Turn[]): readonly Turn[] {
  return turns.filter((turn) => turn.role === "me" || (!turn.product && framework(turn.text)));
}

export function latestBrief(turns: readonly Turn[]): string {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (turn.role === "goat" && !turn.product && framework(turn.text)) return turn.text.trim();
  }
  return "";
}

export function latestProduct(turns: readonly Turn[]): Product | null {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    if (turns[index].product) return turns[index].product!;
  }
  return null;
}

// A finished Work is read from its own delivered turn: a product opens Preview, and words with no
// product are the Agent's reply, which belongs in the conversation the creator answers in.
export function deliveredTurn(turns: readonly Turn[]): { product: Product | null; words: string } {
  const product = latestProduct(turns);
  if (product) return { product, words: "" };
  const turn = turns.at(-1);
  if (!turn || turn.role !== "goat") return { product: null, words: "" };
  return { product: turn.product ?? null, words: turn.text.trim() };
}

export function agentReady(agent: { registered: boolean; state: string } | null): boolean {
  return !!agent?.registered && agent.state === "online";
}

// One unfinished local or pending phone-owned Work owns a session until Account delivery closes it.
export function liveWork(
  run: Pick<CreatorRun, "phase"> | null,
  remote: RemoteWork | null,
): boolean {
  return (!!run && !finished(run.phase)) || remote !== null;
}

// Work state chooses the only creator surface that can accept the next action.
export function workSurface(
  run: Pick<CreatorRun, "status"> | null,
  remote: RemoteWork | null,
): "chat" | "build" {
  return (remote?.status ?? run?.status) === "building" ? "build" : "chat";
}

export function runBuildState(run: CreatorRun): BuildState {
  return {
    status: run.status,
    startedAt: run.startedAt,
    typicalMs: run.typicalMs,
  };
}

// Account status.rs supplies the status; this function only supplies its visible sentence.
export function workStage(status: CreatorStatus): Pick<SessionState, "stage" | "detail"> {
  switch (status) {
    case "designing": return { stage: status, detail: "Designing…" };
    case "ready_to_build": return { stage: status, detail: "Ready to build" };
    case "building": return { stage: status, detail: "Building…" };
    case "failed": return { stage: status, detail: "Needs attention" };
    case "stopped": return { stage: status, detail: "Stopped" };
    case "preview": return { stage: status, detail: "Preview ready" };
    case "published": return { stage: status, detail: "Published" };
  }
}

export function creatorSessionStates(
  runs: ReadonlyMap<string, CreatorRun>,
  remote: ReadonlyMap<string, RemoteWork>,
  known: ReadonlyMap<string, CreatorStatus>,
): Map<string, SessionState> {
  const states = new Map<string, SessionState>();
  for (const [session, status] of known) {
    if (status !== "preview" && status !== "published") states.set(session, { ...workStage(status), remote: null });
  }
  for (const [session, run] of runs) {
    states.set(session, { ...workStage(run.status), remote: null });
  }
  for (const [session, work] of remote) {
    const state = workStage(work.status);
    // Only the other computer can say why its Work failed, so its own reason wins that one word.
    states.set(session, { ...state, detail: state.stage === "failed" ? (work.reason || state.detail) : state.detail, remote: work });
  }
  return states;
}

export function previewLeaf(target: PreviewTarget, load: PreviewLoad): PreviewProduct {
  return {
    id: target.product.id,
    name: target.product.name,
    previewKind: target.product.previewKind,
    published: target.source === "turn" ? target.product.published : target.product.visibility === "public",
    url: load.url,
    sourceError: load.error,
    editable: !!target.session,
  };
}

export function usePreviewTarget(target: PreviewTarget | null): PreviewLoad {
  const [load, setLoad] = useState<PreviewLoad>({ url: null, error: "" });
  const key = target ? `${target.source}:${target.product.id}:${target.source === "mine" ? target.product.sha256 : target.product.files[0]?.sha256 ?? ""}` : "";

  useEffect(() => {
    setLoad({ url: null, error: "" });
    if (!target || target.product.previewKind === null) return;
    const file = target.product.files[0];
    if (target.source === "turn" && !file) {
      setLoad({ url: null, error: "The delivered preview file is missing." });
      return;
    }
    let alive = true;
    // A blob: frame inherits this app's own policy, where a freshly built card's inline scripts are
    // dead. Both a just-delivered product and a saved one play from the preview scheme instead,
    // under the boundary the card's own page declares; staging needs its identity and digest alone.
    const source = minePreviewUrl(target.source === "mine"
      ? target.product
      : { id: target.product.id, sha256: file!.sha256 } as MineProduct);
    void source.then((url) => { if (alive) setLoad({ url, error: "" }); })
      .catch((reason) => { if (alive) setLoad({ url: null, error: reason instanceof Error ? reason.message : "Preview could not load." }); });
    return () => {
      alive = false;
    };
  }, [key]);

  return load;
}
