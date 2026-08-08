import { useEffect, useRef, useState } from "react";
import { type Product, type WorkSnapshot } from "./transport";
import { bindProductFrame, productRuntimeName } from "./parent-bridge";
import "./build-preview.css";

// The wait shows only Work truth: when it started, how long one typically takes, whether the Agent
// parked this turn on the creator, and the words it said. A Work watched from another computer has fewer.
export type BuildState = Pick<WorkSnapshot, "phase"> & { startedAt?: number; typicalMs?: number | null; awaiting?: boolean; words?: string };

export type BuildScreenProps = {
  brief?: string;
  snapshot: BuildState | null;
  issue?: string;
  onStop(): void;
  onRetry(): void;
};

export type PreviewProduct = Pick<Product, "id" | "name" | "previewKind" | "published"> & {
  url: string | null;
  sourceError?: string;
  editable?: boolean;
};

export type PreviewScreenProps = {
  product: PreviewProduct;
  editable?: boolean;
  onEdit(): void;
  onContinue(): void;
};

export type BuildPreviewProps =
  | ({ surface: "build" } & BuildScreenProps)
  | ({ surface: "preview" } & PreviewScreenProps);

// The one line the build wait says, owned here and never taken from the Agent.
const BUILD_LINE = "Leave whenever you like. Your app keeps building and lands in Apps on its own.";

function clock(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

// Counts the wait in the creator's own time; a Work with no known start simply has no clock.
function useElapsed(startedAt: number | undefined, running: boolean): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running || !startedAt) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, [running, startedAt]);
  return startedAt ? Math.max(0, now - startedAt) : null;
}

// Build keeps backstage Work details out of the creator surface: one line, the clock, and the bar.
export function BuildScreen({ snapshot, issue, onStop, onRetry }: BuildScreenProps) {
  const terminal = snapshot?.phase === "failed" || snapshot?.phase === "stopped";
  const stopped = snapshot?.phase === "stopped";
  const failure = issue?.trim() || (stopped ? "The build was stopped." : "The build could not finish.");
  // A parked turn says what the Agent asked instead of pretending the app is still being made.
  const parked = !terminal && snapshot?.awaiting === true;
  const line = parked ? snapshot?.words?.trim() ?? "" : BUILD_LINE;
  const elapsed = useElapsed(snapshot?.startedAt, !terminal);
  const typical = snapshot?.typicalMs && snapshot.typicalMs > 0 ? snapshot.typicalMs : null;
  // Past the typical the bar holds just short of full and the clock keeps counting, so a long build never reads as finished.
  const fill = elapsed !== null && typical ? 6 + Math.min(elapsed / typical, 1) * 91 : null;

  return <main className="build-preview-screen build-screen" aria-label="Build" data-phase={snapshot?.phase ?? "starting"}>
    <div className="build-shell">
      <h1>{terminal ? (stopped ? "Build stopped" : "Build failed") : parked ? "The Agent needs your answer" : "Building…"}</h1>
      {terminal
        ? <>
            <p className="build-failure" role="alert">{failure}</p>
            <div className="build-actions">
              <button type="button" className="build-retry" onClick={onRetry}>Retry</button>
            </div>
          </>
        : <>
            {line && <p className="build-line">{line}</p>}
            {elapsed !== null && <p className="build-clock">
              <span className="build-elapsed">{clock(elapsed)}</span>
              {typical !== null && <span className="build-typical">typical {clock(typical)}</span>}
            </p>}
            {fill !== null && <div className="build-bar" aria-hidden="true"><span style={{ width: `${fill}%` }} /></div>}
            <div className="build-actions">
              <button type="button" className="build-stop" onClick={onStop}>Stop</button>
            </div>
          </>}
    </div>
  </main>;
}

// Preview gives the delivered source the whole canvas while keeping only creator actions nearby.
export function PreviewScreen({ product, editable: editableOverride, onEdit, onContinue }: PreviewScreenProps) {
  const frame = useRef<HTMLIFrameElement>(null);
  const title = product.name.trim() || "Untitled app";
  const framed = product.previewKind === "html" || product.previewKind === "pdf" || product.previewKind === "text";
  const editable = editableOverride ?? product.editable ?? true;

  useEffect(() => {
    const node = frame.current;
    if (!node || !product.url) return;
    return bindProductFrame(node, product.id, product.published);
  }, [product.id, product.published, product.url]);

  return <main className="build-preview-screen preview-screen" aria-label="Preview">
    <header className="preview-toolbar">
      <div className="preview-title">
        <span>Preview</span>
        <h1 title={title}>{title}</h1>
      </div>
      <nav className="preview-actions" aria-label="Preview actions">
        {editable && <button type="button" className="preview-edit" onClick={onEdit}>Edit</button>}
        <button type="button" className="preview-continue" onClick={onContinue}>Publish</button>
      </nav>
    </header>
    <section className="preview-canvas" aria-label={`${title} playable preview`}>
      {!product.url && product.previewKind !== null && !product.sourceError && <p className="preview-loading" role="status" aria-live="polite">Loading your app…</p>}
      {product.sourceError && <p className="preview-error" role="alert">{product.sourceError}</p>}
      {product.url && product.previewKind === "image" && <img src={product.url} alt={title} tabIndex={0} />}
      {product.url && product.previewKind === "video" && <video src={product.url} controls playsInline aria-label={title} tabIndex={0} />}
      {product.url && framed && <iframe
        ref={frame}
        src={product.url}
        name={productRuntimeName(product.id)}
        title={`${title} playable preview`}
        sandbox="allow-scripts allow-forms"
        tabIndex={0}
      />}
      {product.previewKind === null && <p className="preview-unavailable" role="status">Preview unavailable for this delivery.</p>}
    </section>
  </main>;
}

// The conductor chooses the state; this leaf never starts a Work or changes a product identity.
export function BuildPreview(props: BuildPreviewProps) {
  return props.surface === "build"
    ? <BuildScreen brief={props.brief} snapshot={props.snapshot} issue={props.issue} onStop={props.onStop} onRetry={props.onRetry} />
    : <PreviewScreen product={props.product} editable={props.editable} onEdit={props.onEdit} onContinue={props.onContinue} />;
}

export { BuildScreen as BuildView, PreviewScreen as PreviewView };
