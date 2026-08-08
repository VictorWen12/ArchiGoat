import { useEffect, useRef, useState } from "react";
import { deleteProduct, type RemoteWork, type Session } from "./transport";
import { fetchMine, previewUrl, renameProduct, type MineProduct, type MinePage } from "./mine";
import "./projects.css";

const NEW_APP = "New App";

// The conductor supplies the server/runtime truth for one unfinished session.
export type SessionState = {
  stage: "designing" | "building" | "waiting" | "failed";
  detail: string;
  remote: RemoteWork | null;
};

type ProjectsProps = {
  identity: string;
  onSignOut(): void;
  sessions: Session[];
  sessionStates: Map<string, SessionState>;
  reload: number;
  onTry(product: MineProduct): void;
  onEdit(session: string): void;
  onRenameSession(session: string, title: string): void;
  onDeleteSession(session: string): Promise<void>;
  onStopRemote(session: string, work: RemoteWork): void;
};

// Apps owns the server's complete product shelf and keeps account utility quiet.
export function ProjectsView({ identity, onSignOut, sessions, sessionStates, reload, onTry, onEdit, onRenameSession, onDeleteSession, onStopRemote }: ProjectsProps) {
  const [page, setPage] = useState<MinePage | null>(null);
  const [stage, setStage] = useState<"loading" | "error" | "ready">("loading");
  const [error, setError] = useState("");
  const [more, setMore] = useState(false);
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    let alive = true;
    setStage("loading");
    setError("");
    void fetchMine().then((next) => {
      if (!alive) return;
      setPage(next);
      setStage("ready");
    }).catch((reason) => {
      if (!alive) return;
      setError(messageOf(reason, "Could not load your apps"));
      setStage("error");
    });
    return () => { alive = false; };
  }, [reload, retry]);

  async function loadMore(): Promise<void> {
    if (!page?.nextCursor) return;
    setMore(true);
    setError("");
    try {
      const next = await fetchMine(page.nextCursor);
      setPage((current) => current ? { ...next, products: [...current.products, ...next.products] } : next);
    } catch (reason) { setError(messageOf(reason, "Could not load more apps")); }
    finally { setMore(false); }
  }

  function removeProduct(product: MineProduct): void {
    setPage((current) => current ? {
      ...current,
      products: current.products.filter((item) => item.id !== product.id),
      totalCount: Math.max(0, current.totalCount - 1),
    } : current);
    if (product.sessionId) void onDeleteSession(product.sessionId);
  }

  function renameProductInPlace(id: string, name: string): void {
    setPage((current) => current ? { ...current, products: current.products.map((item) => item.id === id ? { ...item, name } : item) } : current);
  }

  const products = page?.products ?? [];
  const productSessions = new Set(products.map((product) => product.sessionId).filter((session): session is string => !!session));
  const inProgress = sessions.filter((session) => !productSessions.has(session.id) && sessionStates.has(session.id));

  return <div className="projects">
    <header className="apps-account-head">
      <div className="apps-account">
        <span className="apps-avatar" aria-hidden="true">{identity.trim().charAt(0).toUpperCase() || "A"}</span>
        <span className="apps-email">{identity || "Signed in"}</span>
      </div>
      <button type="button" className="apps-signout" onClick={onSignOut}>Sign out</button>
    </header>
    <header className="projects-head"><h1>Apps</h1></header>
    {stage === "loading" && <p className="projects-note">Loading apps…</p>}
    {stage === "error" && <div className="projects-note"><p role="alert">{error}</p><button type="button" className="projects-retry" onClick={() => setRetry((value) => value + 1)}>Try again</button></div>}
    {stage === "ready" && page && (inProgress.length > 0 || page.products.length > 0) && <ul className="projects-grid">
      {inProgress.map((session) => {
        const state = sessionStates.get(session.id);
        if (!state) return null;
        return <InProgressRow
          key={`session-${session.id}`}
          session={session}
          state={state}
          onEdit={() => onEdit(session.id)}
          onRename={(title) => onRenameSession(session.id, title)}
          onDelete={() => onDeleteSession(session.id)}
          onStopRemote={(work) => onStopRemote(session.id, work)}
        />;
      })}
      {page.products.map((product) => <ProjectRow
        key={product.id}
        product={product}
        editable={product.sessionId !== null && sessions.some((session) => session.id === product.sessionId)}
        onTry={() => onTry(product)}
        onEdit={() => onEdit(product.sessionId!)}
        onRenamed={(name) => renameProductInPlace(product.id, name)}
        onDeleted={() => removeProduct(product)}
      />)}
    </ul>}
    {stage === "ready" && page?.nextCursor && <button type="button" className="projects-more" disabled={more} onClick={() => void loadMore()}>{more ? "Loading…" : "Load more"}</button>}
    {stage === "ready" && error && <p className="projects-error" role="alert">{error}</p>}
  </div>;
}

function InProgressRow({ session, state, onEdit, onRename, onDelete, onStopRemote }: {
  session: Session;
  state: SessionState;
  onEdit(): void;
  onRename(title: string): void;
  onDelete(): Promise<void>;
  onStopRemote(work: RemoteWork): void;
}) {
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [naming, setNaming] = useState<string | null>(null);
  const title = session.title || NEW_APP;

  function save(): void {
    const name = (naming ?? "").trim();
    setNaming(null);
    if (name && name !== title) onRename(name);
  }

  const label = stateLabel(state.stage);
  const remote = state.remote;

  async function remove(): Promise<void> {
    setBusy("delete");
    setError("");
    try { await onDelete(); }
    catch (reason) { setError(messageOf(reason, "Could not delete this app")); }
    finally { setBusy(""); }
  }

  return <li className="projects-card projects-in-progress">
    <div className={`projects-thumb projects-progress-thumb${state.stage === "failed" ? " projects-failed-thumb" : ""}`}>
      {(state.stage === "designing" || state.stage === "building") && <span className="progress-ring" aria-hidden="true" />}
      <strong>{label}</strong>
    </div>
    <div className="projects-body">
      {naming === null
        ? <h2 title={title}>{title}</h2>
        : <input className="projects-inline-name" value={naming} autoFocus aria-label={`Name for ${title}`} onChange={(event) => setNaming(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); save(); } if (event.key === "Escape") { event.preventDefault(); setNaming(null); } }} onBlur={() => setNaming(null)} />}
      <p className="projects-facts"><span>{label}</span>{state.detail.trim() && state.detail.trim() !== label && <span>{state.detail}</span>}</p>
      {remote && <div className="projects-remote"><span>{remote.availability}</span><button type="button" onClick={() => onStopRemote(remote)} disabled={busy !== ""}>Stop</button></div>}
      <div className="projects-actions">
        <button type="button" onClick={onEdit}>Edit</button>
        {naming === null && <button type="button" onClick={() => setNaming("")}>Rename</button>}
        <button type="button" className="projects-danger" disabled={busy !== ""} onClick={() => void remove()}>{busy === "delete" ? "Deleting…" : "Delete"}</button>
      </div>
      {error && <p className="projects-error" role="alert">{error}</p>}
    </div>
  </li>;
}

function ProjectRow({ product, editable, onTry, onEdit, onRenamed, onDeleted }: {
  product: MineProduct;
  editable: boolean;
  onTry(): void;
  onEdit(): void;
  onRenamed(name: string): void;
  onDeleted(): void;
}) {
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [naming, setNaming] = useState<string | null>(null);

  async function act(kind: string, run: () => Promise<void>, done: () => void, fallback: string): Promise<void> {
    setBusy(kind);
    setError("");
    try { await run(); done(); }
    catch (reason) { setError(messageOf(reason, fallback)); }
    finally { setBusy(""); }
  }

  function save(): void {
    const name = (naming ?? "").trim();
    setNaming(null);
    if (name && name !== product.name) void act("rename", () => renameProduct(product.id, name), () => onRenamed(name), "Could not rename this app");
  }

  return <li className="projects-card">
    <ProjectThumb product={product} />
    <div className="projects-body">
      {naming === null
        ? <h2 title={product.name}>{product.name}</h2>
        : <input className="projects-inline-name" value={naming} autoFocus aria-label={`Name for ${product.name}`} onChange={(event) => setNaming(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); save(); } if (event.key === "Escape") { event.preventDefault(); setNaming(null); } }} onBlur={() => setNaming(null)} />}
      {product.tags.length > 0 && <div className="projects-tags" aria-label="Tags">{product.tags.map((tag, index) => <span key={`${tag}-${index}`} className="projects-tag">{tag}</span>)}</div>}
      <p className="projects-facts"><span>{product.form}</span><span>{humanSize(product.size)}</span><span>{product.files.length === 1 ? "1 file" : `${product.files.length} files`}</span><span>Created {when(product.createdAt)}</span></p>
      <div className="projects-actions">
        {product.previewKind !== null && <button type="button" className="projects-primary" onClick={onTry}>Try</button>}
        {editable && <button type="button" onClick={onEdit}>Edit</button>}
        {naming === null && <button type="button" onClick={() => setNaming("")}>Rename</button>}
        <button type="button" className="projects-danger" disabled={!!busy} onClick={() => void act("delete", () => deleteProduct(product.id), onDeleted, "Could not delete this app")}>{busy === "delete" ? "Deleting…" : "Delete"}</button>
      </div>
      {error && <p className="projects-error" role="alert">{error}</p>}
    </div>
  </li>;
}

// The thumbnail is the product's own verified file, fetched once the row reaches the screen.
function ProjectThumb({ product }: { product: MineProduct }) {
  const holder = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(false);
  const url = usePreview(near ? product : null);

  useEffect(() => {
    const node = holder.current;
    if (!node || near) return;
    const watcher = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) { setNear(true); watcher.disconnect(); }
    }, { rootMargin: "300px" });
    watcher.observe(node);
    return () => watcher.disconnect();
  }, [near]);

  const kind = product.previewKind;
  return <div className="projects-thumb" ref={holder}>
    {url && kind === "image" && <img src={url} alt="" />}
    {url && kind === "video" && <video src={url} muted playsInline preload="metadata" aria-hidden="true" />}
    {url && (kind === "html" || kind === "pdf" || kind === "text") && <iframe src={url} title={`Preview of ${product.name}`} sandbox="" scrolling="no" tabIndex={-1} aria-hidden="true" />}
    {kind === null && <span className="projects-file">Preview unavailable</span>}
  </div>;
}

function usePreview(product: MineProduct | null): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    if (!product || product.previewKind === null) return;
    let alive = true;
    void previewUrl(product).then((next) => { if (alive) setUrl(next); }).catch(() => { if (alive) setUrl(null); });
    return () => { alive = false; };
  }, [product?.id, product?.sha256, product?.previewKind]);
  return url;
}

function stateLabel(stage: SessionState["stage"]): string {
  switch (stage) {
    case "designing": return "Designing…";
    case "building": return "Building…";
    case "waiting": return "Needs your answer";
    case "failed": return "Needs attention";
  }
}

function humanSize(bytes: number): string {
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb >= 10 ? Math.round(kb) : Number(kb.toFixed(1))} KB`;
  const mb = kb / 1024;
  return `${mb >= 10 ? Math.round(mb) : Number(mb.toFixed(1))} MB`;
}

function when(seconds: number): string {
  const date = new Date(seconds * 1000);
  return date.toDateString() === new Date().toDateString()
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString([], { month: "short", day: "numeric" });
}

function messageOf(reason: unknown, fallback: string): string { return reason instanceof Error ? reason.message : fallback; }
