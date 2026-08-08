import { useEffect, useState } from "react";
import type { BuildState, PreviewProduct } from "./build-preview";
import { previewUrl as minePreviewUrl, type MineProduct } from "./mine";
import type { SessionState } from "./projects";
import {
  finished,
  type Product,
  type RemoteWork,
  type Turn,
  type WorkEvent,
  type WorkIntent,
} from "./transport";

export type CreatorRun = {
  workId: string;
  deliveryId: string;
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

export function latestBrief(turns: readonly Turn[]): string {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (turn.role === "goat" && !turn.product && turn.text.trim()) return turn.text.trim();
  }
  return "";
}

export function latestProduct(turns: readonly Turn[]): Product | null {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    if (turns[index].product) return turns[index].product!;
  }
  return null;
}

export function latestTurnIsBrief(turns: readonly Turn[]): boolean {
  const turn = turns.at(-1);
  return !!turn && turn.role === "goat" && (!!turn.product || !!turn.text.trim());
}

// A finished Work is read from its own delivered turn: a product opens Preview, and words with no
// product are the Agent's reply, which belongs in the conversation the creator answers in.
export function deliveredTurn(turns: readonly Turn[]): { product: Product | null; words: string } {
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
  run: Pick<CreatorRun, "intent" | "phase" | "awaiting"> | null,
  remote: RemoteWork | null,
): "chat" | "build" {
  if (remote?.intent === "build") return "build";
  // A turn the Agent parked on the creator is the creator's turn again: it belongs in the conversation.
  if (run?.awaiting && !finished(run.phase)) return "chat";
  if (run?.intent === "build" && !finished(run.phase)) return "build";
  return "chat";
}

export function runBuildState(run: CreatorRun): BuildState {
  return {
    phase: run.phase,
    startedAt: run.startedAt,
    typicalMs: run.typicalMs,
    awaiting: run.awaiting,
    words: run.text,
  };
}

// The Agent's own stage events say when a turn stopped shaping the design and started making the app.
function makingApp(events: readonly WorkEvent[]): boolean {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (event.kind !== "stage") continue;
    return event.label === "Building" || event.label === "Verifying" || event.label === "Delivering";
  }
  return false;
}

// Every status word the creator reads comes from Work truth: whether the turn ended, whether the
// Agent parked it on the creator, and the last stage the Agent itself published.
export function workStage(phase: string, awaiting: boolean, events: readonly WorkEvent[]): Pick<SessionState, "stage" | "detail"> {
  if (phase === "failed" || phase === "stopped") return { stage: "failed", detail: "Needs attention" };
  if (awaiting) return { stage: "waiting", detail: "Needs your answer" };
  return makingApp(events) ? { stage: "building", detail: "Building…" } : { stage: "designing", detail: "Designing…" };
}

export function creatorSessionStates(
  runs: ReadonlyMap<string, CreatorRun>,
  remote: ReadonlyMap<string, RemoteWork>,
  ready: ReadonlySet<string>,
): Map<string, SessionState> {
  const states = new Map<string, SessionState>();
  for (const [session, run] of runs) {
    states.set(session, { ...workStage(run.phase, run.awaiting, run.events), remote: null });
  }
  for (const [session, work] of remote) {
    const state = workStage(work.state, work.awaiting, work.events);
    // Only the other computer can say why its Work failed, so its own reason wins that one word.
    states.set(session, { ...state, detail: state.stage === "failed" ? (work.reason || state.detail) : state.detail, remote: work });
  }
  for (const session of ready) {
    if (!states.has(session)) states.set(session, { stage: "designing", detail: "Ready to build", remote: null });
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
