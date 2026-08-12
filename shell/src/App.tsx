import { useEffect, useMemo, useRef, useState } from "react";
import {
  ACCOUNT_ORIGIN,
  appendTurn,
  authorizeAccount,
  agentHealth,
  agentStatus,
  createSession,
  deleteAttachment,
  deliverLocalWork,
  fetchIdentity,
  fetchSessions,
  fetchTurns,
  finished,
  holdQuit,
  logout,
  openAccount,
  pendingWorks,
  publishLocalWork,
  publishProduct,
  renameSession,
  readWork,
  remoteWork,
  restoreSession,
  removeSession,
  stagePendingInput,
  startError,
  startWork,
  steerLocalWork,
  steerTurn,
  stopRemoteWork,
  stopWork,
  type AgentModel,
  type AgentPresets,
  type Attachment,
  type CreatorStatus,
  type RemoteWork,
  type Session,
  type Turn,
  type WorkIntent,
  uploadAttachment,
  waitForWork,
  workStatus,
} from "./transport";
import { BridgeError } from "./transport";
import { AgentConnections } from "./agent-connections";
import { BuildPreview } from "./build-preview";
import { ChatView, type ChatMessage } from "./chat";
import {
  agentReady,
  creatorChatTurns,
  creatorSessionStates,
  deliveredTurn,
  latestBrief,
  latestProduct,
  liveWork,
  previewLeaf,
  runBuildState,
  usePreviewTarget,
  workStage,
  workSurface,
  type CreatorRun,
  type PreviewTarget,
} from "./creator-flow";
import { IdeaView, type CreatorAttachment } from "./idea";
import { type MineProduct } from "./mine";
import { ProjectsView } from "./projects";
import { PublishView, type PublishMetadata } from "./publish";

type AuthState = "loading" | "signed-out" | "signed-in";
type AuthNotice = "hard" | "soft";
type Agent = { registered: boolean; state: string; provider: string | null; installed: string[] | null; model: string | null; effort: string | null; models: AgentModel[]; presets: AgentPresets | null };
type DraftFile = { attachment: Attachment; file: File; busy: boolean };
type OwedWork = { deliveryId: string; workId: string; intent: WorkIntent };
type Lifecycle = { deliveryId: string; workId: string };
type View = "idea" | "chat" | "build" | "preview" | "publish" | "projects" | "connections";

const OWED = "archigoat.work.owed";

// Names every delivery this computer still owes, by session, so quitting mid-Work strands nothing.
function rememberedWork(): Record<string, OwedWork> {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(OWED) ?? "null");
    if (!value || typeof value !== "object") return {};
    const owed: Record<string, OwedWork> = {};
    // A single record written by an earlier release still names one owed delivery.
    const legacy = value as { session?: unknown; deliveryId?: unknown; workId?: unknown };
    if (typeof legacy.session === "string" && typeof legacy.deliveryId === "string" && typeof legacy.workId === "string") {
      return { [legacy.session]: { deliveryId: legacy.deliveryId, workId: legacy.workId, intent: "build" } };
    }
    for (const [session, entry] of Object.entries(value as Record<string, unknown>)) {
      const item = entry as OwedWork;
      if (session && item && typeof item === "object" && typeof item.deliveryId === "string" && item.deliveryId && typeof item.workId === "string" && item.workId) {
        owed[session] = { deliveryId: item.deliveryId, workId: item.workId, intent: item.intent === "brief" ? "brief" : "build" };
      }
    }
    return owed;
  } catch { return {}; }
}

function rememberWork(session: string, owed: OwedWork | null): void {
  try {
    const all = rememberedWork();
    if (owed) all[session] = owed;
    else delete all[session];
    window.localStorage.setItem(OWED, JSON.stringify(all));
  } catch { /* the studio still runs when local storage refuses */ }
}

function latestLifecycle(turns: Turn[]): Lifecycle | null {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (turn.workId && turn.deliveryId) return { workId: turn.workId, deliveryId: turn.deliveryId };
  }
  return null;
}

export function App() {
  const [auth, setAuth] = useState<AuthState>("loading");
  const [authError, setAuthError] = useState("");
  const [authNotice, setAuthNotice] = useState<AuthNotice>("soft");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [identity, setIdentity] = useState("");
  const [active, setActive] = useState<string | null>(null);
  const [threads, setThreads] = useState<Map<string, Turn[]>>(() => new Map());
  const [error, setError] = useState("");
  const [draft, setDraft] = useState("");
  const [files, setFiles] = useState<DraftFile[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [runs, setRuns] = useState<Map<string, CreatorRun>>(() => new Map());
  const [remote, setRemote] = useState<Map<string, RemoteWork>>(() => new Map());
  const [creatorStatuses, setCreatorStatuses] = useState<Map<string, CreatorStatus>>(() => new Map());
  const [briefs, setBriefs] = useState<Map<string, string>>(() => new Map());
  const [previewTarget, setPreviewTarget] = useState<PreviewTarget | null>(null);
  const [agent, setAgent] = useState<Agent | null>(null);
  const [device, setDevice] = useState<string | null>(null);
  const [agentIssue, setAgentIssue] = useState("");
  const [view, setView] = useState<View>("projects");
  const [projectsReload, setProjectsReload] = useState(0);
  const [quitAsk, setQuitAsk] = useState(false);
  const draftFiles = useRef(new Map<string, File>());
  const quitChoice = useRef<((quit: boolean) => void) | null>(null);
  const activeRef = useRef<string | null>(null);
  const remoteRef = useRef<Map<string, RemoteWork>>(new Map());
  const startFault = useRef("");
  const checking = useRef(false);
  const turns = active ? threads.get(active) ?? [] : [];
  const run = active ? runs.get(active) ?? null : null;
  const activeRemote = active ? remote.get(active) ?? null : null;
  const busy = [...runs.values()].some((item) => !finished(item.phase));
  const sessionStates = useMemo(() => creatorSessionStates(runs, remote, creatorStatuses), [creatorStatuses, remote, runs]);
  const preview = usePreviewTarget(previewTarget);
  const activeBrief = active ? briefs.get(active) ?? latestBrief(turns) : "";
  const editingDelivered = !!latestProduct(turns) || (!!active && previewTarget?.session === active && previewTarget.source === "mine");
  const creatorAttachments = useMemo<CreatorAttachment[]>(() => files.map(({ attachment, file, busy: uploading }) => ({
    id: attachment.id,
    name: attachment.name,
    image: attachment.image,
    imageUrl: attachment.image ? URL.createObjectURL(file) : undefined,
    busy: uploading,
  })), [files]);
  const chatMessages = useMemo<ChatMessage[]>(() => {
    return creatorChatTurns(turns).map((turn) => ({
      id: `${turn.id}-${turn.at}`,
      role: turn.role === "me" ? "user" : "agent",
      text: turn.text,
      attachments: turn.attachments.map((attachment) => ({ id: attachment.id, name: attachment.name, image: attachment.image, imageUrl: attachment.image ? attachment.url : undefined, remote: attachment })),
    }));
  }, [turns]);

  useEffect(() => () => {
    for (const attachment of creatorAttachments) if (attachment.imageUrl) URL.revokeObjectURL(attachment.imageUrl);
  }, [creatorAttachments]);

  useEffect(() => { activeRef.current = active; }, [active]);

  useEffect(() => {
    if (!active || previewTarget?.session === active) return;
    const product = latestProduct(turns);
    if (product) setPreviewTarget({ source: "turn", product, session: active });
  }, [active, previewTarget?.session, turns]);

  useEffect(() => { remoteRef.current = remote; }, [remote]);

  useEffect(() => {
    if (active && (view === "chat" || view === "preview" || view === "publish") && remote.get(active)?.status === "building") setView("build");
  }, [active, remote, view]);

  useEffect(() => {
    let alive = true;
    void startError().then((fault) => {
      if (!alive || !fault) return;
      startFault.current = fault;
      setError(`ArchiGoat could not start: ${fault}`);
      setAuthNotice("hard");
      setAuthError(fault);
    });
    void checkSignIn();
    return () => { alive = false; };
  }, []);

  useEffect(() => {
    if (auth !== "signed-in") return;
    let alive = true;
    setError("");
    void Promise.all([fetchSessions(), fetchIdentity()]).then(([nextSessions, account]) => {
      if (!alive) return;
      setSessions(nextSessions);
      setIdentity(account.email);
      setActive((current) => current && nextSessions.some((session) => session.id === current) ? current : null);
      void Promise.all(nextSessions.map(async (session) => {
        try { return [session.id, await fetchTurns(session.id)] as const; }
        catch { return null; }
      })).then(async (rows) => {
        if (!alive) return;
        const nextThreads = new Map<string, Turn[]>();
        const nextBriefs = new Map<string, string>();
        const nextStatuses = new Map<string, CreatorStatus>();
        for (const row of rows) {
          if (!row) continue;
          const [session, list] = row;
          nextThreads.set(session, list);
          const brief = latestBrief(list);
          if (brief) nextBriefs.set(session, brief);
          const lifecycle = latestLifecycle(list);
          if (lifecycle) {
            const status = await workStatus(lifecycle.deliveryId).catch(() => null);
            if (status) nextStatuses.set(session, status.status);
          }
        }
        if (!alive) return;
        setThreads(nextThreads);
        setBriefs(nextBriefs);
        setCreatorStatuses(nextStatuses);
      });
    }).catch((reason) => { if (alive) setError(messageOf(reason, "Could not load your apps")); });
    return () => { alive = false; };
  }, [auth]);

  useEffect(() => {
    if (auth !== "signed-in" || !active || threads.has(active)) return;
    const session = active;
    let alive = true;
    void fetchTurns(session).then((next) => { if (alive) putTurns(session, next); }).catch((reason) => { if (alive) setError(messageOf(reason, "Could not load this Work")); });
    return () => { alive = false; };
  }, [active, auth, threads]);

  useEffect(() => {
    if (auth !== "signed-in") return;
    let alive = true;
    const refresh = async () => {
      try {
        const health = await agentHealth();
        if (alive) setDevice(health.device);
        let status: Awaited<ReturnType<typeof agentStatus>>;
        try {
          status = await agentStatus();
        } catch (reason) {
          if (alive) {
            setAgent({ registered: health.registered, state: "offline", provider: null, installed: null, model: null, effort: null, models: [], presets: null });
            setAgentIssue(messageOf(reason, "Could not connect your Agent"));
          }
          return;
        }
        if (alive) {
          setAgent({ registered: health.registered, state: status.state, provider: status.provider, installed: status.installed, model: status.model, effort: status.effort, models: status.models, presets: status.presets });
          setAgentIssue("");
        }
      } catch (reason) { if (alive) { setAgent(null); setDevice(null); setAgentIssue(messageOf(reason, "Agent is not reachable")); } }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [auth]);

  // Unfinished Work resumes in its own session's pane, and never steals the one the reader is in.
  useEffect(() => {
    if (auth !== "signed-in") return;
    const controllers: AbortController[] = [];
    for (const [session, entry] of Object.entries(rememberedWork())) {
      const controller = new AbortController();
      controllers.push(controller);
      void resume(session, entry, controller);
    }
    return () => { for (const controller of controllers) controller.abort(); };
  }, [auth]);

  // Work ordered from a phone belongs in this computer's history while it runs, with its own Stop.
  useEffect(() => {
    if (auth !== "signed-in") return;
    let alive = true;
    const refresh = async () => {
      const summons = await pendingWorks().catch(() => null);
      if (!summons || !alive) return;
      const owed = rememberedWork();
      const next = new Map<string, RemoteWork>();
      for (const summon of summons) {
        if (!summon.computer || owed[summon.scopeId]) continue;
        const status = await workStatus(summon.deliveryId).catch(() => null);
        if (!status) continue;
        putCreatorStatus(summon.scopeId, status.status);
        const state = await remoteWork(summon.workId, status.status, summon.intent).catch(() => null);
        if (state?.state === "delivered") continue;
        if (state) next.set(summon.scopeId, state);
      }
      if (!alive) return;
      const completed = [...remoteRef.current.keys()].filter((session) => !next.has(session));
      remoteRef.current = next;
      setRemote(next);
      for (const session of completed) void adoptRemoteDelivery(session);
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 10_000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [auth]);

  useEffect(() => {
    if (!busy) return;
    let release: (() => void) | null = null;
    let dropped = false;
    void holdQuit(() => new Promise<boolean>((resolve) => { quitChoice.current = resolve; setQuitAsk(true); }))
      .then((stop) => { if (dropped) stop(); else release = stop; })
      .catch(() => undefined);
    return () => { dropped = true; release?.(); answerQuit(false); };
  }, [busy]);

  if (auth !== "signed-in") return <AuthView restoring={auth === "loading"} error={authError} notice={authNotice} onClearError={() => setAuthError("")} onRetry={retrySignIn} />;

  // The one restore path: launch, the "Try again" action, and the return from the browser consent all use it.
  async function checkSignIn(): Promise<void> {
    if (checking.current) return;
    checking.current = true;
    try {
      const token = await restoreSession();
      if (token) { setAuthError(""); setAuth("signed-in"); }
      else setAuth("signed-out");
    } catch (reason) {
      setAuth("signed-out");
      if (!startFault.current) { setAuthNotice("soft"); setAuthError(messageOf(reason, "Could not restore sign-in")); }
    } finally { checking.current = false; }
  }

  function retrySignIn(): void {
    if (checking.current) return;
    setAuthError("");
    setAuth("loading");
    void checkSignIn();
  }

  function putTurns(session: string, list: Turn[]): void {
    setThreads((current) => new Map(current).set(session, list));
  }

  function putRun(session: string, value: CreatorRun): void {
    setRuns((current) => new Map(current).set(session, value));
  }

  function putCreatorStatus(session: string, status: CreatorStatus): void {
    setCreatorStatuses((current) => new Map(current).set(session, status));
  }

  function patchRun(session: string, change: (value: CreatorRun) => CreatorRun): void {
    setRuns((current) => {
      const value = current.get(session);
      if (!value) return current;
      return new Map(current).set(session, change(value));
    });
  }

  function dropRun(session: string): void {
    setRuns((current) => {
      if (!current.has(session)) return current;
      const next = new Map(current);
      next.delete(session);
      return next;
    });
  }

  // Words with no product are the Agent's reply: the conversation keeps them and Build stays offered.
  function landWords(session: string, words: string): void {
    if (words) setBriefs((current) => new Map(current).set(session, words));
  }

  // Every title on screen is the server's; this computer creates the thread and then reads it back.
  async function refreshSessions(): Promise<void> {
    setSessions(await fetchSessions());
  }

  async function showProjects(): Promise<void> {
    setView("projects");
    try { await refreshSessions(); }
    catch (reason) { setError(messageOf(reason, "Could not load your apps")); }
    finally { setProjectsReload((value) => value + 1); }
  }

  async function signOut(): Promise<void> {
    try { await logout(); }
    finally { setIdentity(""); setAuth("signed-out"); }
  }

  // A new App begins locally; an Account session exists only after the creator submits the idea.
  function newWork(): void {
    setView("idea");
    setActive(null);
    setPreviewTarget(null);
    setError("");
    setDraft("");
    setFiles([]);
  }

  // Edit returns to the app's own conversation; an active Build keeps its dedicated screen.
  function openWork(session: string): void {
    setError("");
    setView(workSurface(runs.get(session) ?? null, remote.get(session) ?? null));
    setDraft("");
    setActive(session);
  }

  function tryProduct(product: MineProduct): void {
    setError("");
    if (product.sessionId && liveWork(runs.get(product.sessionId) ?? null, remote.get(product.sessionId) ?? null)) {
      setActive(product.sessionId);
      setView(workSurface(runs.get(product.sessionId) ?? null, remote.get(product.sessionId) ?? null));
      return;
    }
    setActive(product.sessionId);
    setPreviewTarget({ source: "mine", product, session: product.sessionId });
    setView("preview");
  }

  // Account removing a phone-owned pending Work is the delivery edge: refresh that same session once.
  async function adoptRemoteDelivery(session: string): Promise<void> {
    try {
      const list = await fetchTurns(session);
      putTurns(session, list);
      await refreshSessions();
      const delivered = deliveredTurn(list);
      if (delivered.product) {
        setPreviewTarget({ source: "turn", product: delivered.product, session });
        if (activeRef.current === session) setView("preview");
      } else {
        landWords(session, delivered.words);
        if (activeRef.current === session) setView("chat");
      }
      setProjectsReload((value) => value + 1);
    } catch (reason) {
      if (activeRef.current === session) setError(messageOf(reason, "Could not load the delivered app"));
    }
  }

  async function remove(id: string): Promise<void> {
    setError("");
    try {
      await removeSession(id);
      rememberWork(id, null);
      runs.get(id)?.controller.abort();
      dropRun(id);
      setThreads((current) => { const next = new Map(current); next.delete(id); return next; });
      setBriefs((current) => { const next = new Map(current); next.delete(id); return next; });
      setCreatorStatuses((current) => { const next = new Map(current); next.delete(id); return next; });
      setSessions((current) => current.filter((session) => session.id !== id));
      setActive((current) => current === id ? null : current);
      setPreviewTarget((current) => current?.session === id ? null : current);
      setProjectsReload((value) => value + 1);
    } catch (reason) { setError(messageOf(reason, "Could not delete this Work")); }
  }

  async function rename(id: string, title: string): Promise<void> {
    setError("");
    try { await renameSession(id, title); await refreshSessions(); }
    catch (reason) { setError(messageOf(reason, "Could not rename this Work")); }
  }

  async function stopRemote(session: string, work: RemoteWork): Promise<void> {
    setError("");
    try {
      await stopRemoteWork(work.computer, work.workId);
      const lifecycle = latestLifecycle(threads.get(session) ?? []);
      const status = lifecycle ? await workStatus(lifecycle.deliveryId) : null;
      const next = status ? await remoteWork(work.workId, status.status, work.intent) : null;
      if (status) putCreatorStatus(session, status.status);
      if (next) setRemote((current) => new Map(current).set(session, next));
    }
    catch (reason) { setError(messageOf(reason, "Could not stop this Work")); }
  }

  // One path adds every draft file, so a picked file and a pasted image share the same dedupe, upload, cap, and error.
  async function addFiles(picked: File[]): Promise<void> {
    for (const file of picked) {
      const key = `${file.name}:${file.size}:${file.lastModified}`;
      if (files.some((item) => item.attachment.name === file.name && item.attachment.bytes === file.size)) continue;
      setFiles((current) => [...current, { attachment: { id: key, name: file.name, media: file.type || "application/octet-stream", bytes: file.size, sha256: "", image: file.type.startsWith("image/"), url: "" }, file, busy: true }]);
      draftFiles.current.set(key, file);
      try {
        const attachment = await uploadAttachment(file);
        setFiles((current) => current.map((item) => item.attachment.id === key ? { attachment, file, busy: false } : item));
        draftFiles.current.delete(key);
        draftFiles.current.set(attachment.id, file);
      } catch (reason) {
        setFiles((current) => current.filter((item) => item.attachment.id !== key));
        draftFiles.current.delete(key);
        setError(messageOf(reason, `${file.name} could not be attached`));
      }
    }
  }

  async function removeFile(id: string): Promise<void> {
    const item = files.find((candidate) => candidate.attachment.id === id);
    if (!item) return;
    setFiles((current) => current.filter((candidate) => candidate.attachment.id !== id));
    draftFiles.current.delete(id);
    if (item.attachment.sha256) await deleteAttachment(id).catch(() => undefined);
  }

  // Reattaches one unfinished Work to its own session without moving the reader anywhere.
  // The account answers for this exact delivery, so a long history never hides one that is still owed.
  async function resume(session: string, owed: OwedWork, controller: AbortController): Promise<void> {
    try {
      const status = await workStatus(owed.deliveryId);
      if (!status) { rememberWork(session, null); return; }
      putCreatorStatus(session, status.status);
      const snapshot = status.status === "preview" || status.status === "published" ? null : await readWork(owed.workId);
      if (!snapshot) { rememberWork(session, null); return; }
      const list = await fetchTurns(session);
      if (controller.signal.aborted) return;
      putTurns(session, list);
      putRun(session, {
        workId: owed.workId, deliveryId: owed.deliveryId, status: status.status, intent: owed.intent, phase: snapshot.phase, awaiting: snapshot.awaiting, text: snapshot.text, events: snapshot.events,
        startedAt: snapshot.startedAt, typicalMs: status?.typicalMs ?? null, model: snapshot.model, tokens: snapshot.tokens,
        controller,
      });
      await follow(session, owed.deliveryId, owed.workId, controller);
    } catch (reason) {
      if (!controller.signal.aborted) { dropRun(session); setError(messageOf(reason, "Could not finish the last Work")); }
    }
  }

  // Watches one Work to its end and finishes the delivery it owes, writing only its own session's pane.
  async function follow(session: string, deliveryId: string, workId: string, controller: AbortController): Promise<void> {
    const result = await waitForWork(workId, (snapshot) => patchRun(session, (current) => ({
      ...current, phase: snapshot.phase, awaiting: snapshot.awaiting, text: snapshot.text, events: snapshot.events,
      // The typical is the account's measured fact, seeded once when this Work started; a daemon poll never rewrites it.
      startedAt: snapshot.startedAt, model: snapshot.model, tokens: snapshot.tokens,
    })), controller.signal);
    if (result.phase === "done") {
      // The delivery stays owed until it lands, so a dropped connection hands the finished result to the next launch.
      await deliverLocalWork(session, deliveryId, workId);
      const accountStatus = await workStatus(deliveryId);
      if (accountStatus) putCreatorStatus(session, accountStatus.status);
      rememberWork(session, null);
      dropRun(session);
      const list = await fetchTurns(session);
      putTurns(session, list);
      await refreshSessions();
      // The delivered turn decides the screen: a product opens Preview, and words with no product
      // become the Agent's reply in the conversation, where the creator answers and the app continues.
      const delivered = deliveredTurn(list);
      if (delivered.product) {
        setPreviewTarget({ source: "turn", product: delivered.product, session });
        if (activeRef.current === session) setView("preview");
      } else {
        landWords(session, delivered.words);
        if (activeRef.current === session) setView("chat");
      }
      setProjectsReload((value) => value + 1);
      return;
    }
    // A stopped or failed Work keeps its conversation; the server rows it wrote join the thread.
    rememberWork(session, null);
    putTurns(session, await fetchTurns(session));
    await refreshSessions();
    if (result.phase === "failed") {
      setError(result.text.trim() || result.progress?.text.trim() || "The Agent could not finish this Work.");
    }
    setProjectsReload((value) => value + 1);
  }

  function answerQuit(quit: boolean): void {
    const decide = quitChoice.current;
    quitChoice.current = null;
    setQuitAsk(false);
    decide?.(quit);
  }

  async function submitIdea(value: string): Promise<void> {
    if (!agentReady(agent)) { setError("Connect ChatGPT, Claude, or Cursor in Connections."); return; }
    setSubmitting(true);
    setError("");
    try {
      const session = await createSession();
      putTurns(session, []);
      setActive(session);
      activeRef.current = session;
      setView("chat");
      await refreshSessions();
      await startCreatorTurn(session, value, "brief", files);
    } catch (reason) {
      setError(messageOf(reason, "Could not start this idea"));
      setView("idea");
    } finally {
      setSubmitting(false);
    }
  }

  async function submitRevision(value: string): Promise<void> {
    if (!active) return;
    if (!agentReady(agent)) { setError("Connect ChatGPT, Claude, or Cursor in Connections."); return; }
    const intent: WorkIntent = editingDelivered ? "build" : "brief";
    setSubmitting(true);
    setError("");
    try { await startCreatorTurn(active, value, intent, files); }
    catch (reason) { setError(messageOf(reason, "Could not revise this idea")); }
    finally { setSubmitting(false); }
  }

  async function build(): Promise<void> {
    if (!active) return;
    if (!agentReady(agent)) { setError("Connect ChatGPT, Claude, or Cursor in Connections."); return; }
    setSubmitting(true);
    setError("");
    try { await startCreatorTurn(active, "Build the latest Framework.", "build", []); }
    catch (reason) { setError(messageOf(reason, "Build could not start")); }
    finally { setSubmitting(false); }
  }

  // Every creator turn is durable before its typed local Work starts.
  async function startCreatorTurn(session: string, textValue: string, intent: WorkIntent, selected: DraftFile[]): Promise<void> {
    const local = runs.get(session) ?? null;
    const phone = remote.get(session) ?? null;
    if (liveWork(local, phone) && !local?.awaiting && !phone?.awaiting) {
      setView(workSurface(local, phone));
      return;
    }
    if (!textValue.trim() && selected.length === 0) return;
    if (selected.some((item) => item.busy)) throw new BridgeError(0, "Wait for attachments to finish uploading.");
    const lifecycle = latestLifecycle(threads.get(session) ?? []);
    if (lifecycle) {
      await continueCreatorWork(session, lifecycle, textValue.trim(), intent, selected, local, phone);
      return;
    }
    const attachmentIds = selected.map((item) => item.attachment.id);
    const attached = selected.map((item) => item.attachment);
    setDraft("");
    setFiles([]);
    setView(intent === "brief" ? "chat" : "build");
    const saved = await appendTurn(session, textValue.trim(), attachmentIds, intent);
    putCreatorStatus(session, saved.status);
    if (!saved.created) {
      putTurns(session, await fetchTurns(session));
      await refreshSessions();
      const existingStatus = await workStatus(saved.deliveryId);
      const existing = saved.pending.computer && existingStatus ? await remoteWork(saved.workId, existingStatus.status, intent) : null;
      if (existing) {
        const next = new Map(remoteRef.current).set(session, existing);
        remoteRef.current = next;
        setRemote(next);
      }
      // No Work of this computer's own follows a converged turn, so the screen is never a build with nothing behind it.
      setView(workSurface(null, existing));
      return;
    }
    putTurns(session, [...(threads.get(session) ?? []), {
      id: saved.id, role: "me", text: textValue.trim(), at: saved.at,
      workId: saved.workId, deliveryId: saved.deliveryId, attachments: attached,
    }]);
    await refreshSessions();
    const controller = new AbortController();
    putRun(session, {
      workId: saved.workId, deliveryId: saved.deliveryId, intent, phase: "queued", awaiting: false, text: "", events: [],
      status: saved.status,
      startedAt: saved.at * 1_000, typicalMs: null, controller,
    });
    try {
      await agentHealth();
      const receipts = await Promise.all(saved.pending.attachments.map((item) => stagePendingInput(saved.workId, item, draftFiles.current.get(item.id))));
      rememberWork(session, { deliveryId: saved.deliveryId, workId: saved.workId, intent });
      await startWork(saved.workId, saved.pending.scopeId, saved.pending.goal, saved.pending.context, receipts, intent);
      patchRun(session, (value) => ({ ...value, phase: "running" }));
      // The typical wait is the account's one measured number, read once for this build. It only ever
      // adds the bar, so a status the account cannot answer leaves the line and the clock untouched.
      void workStatus(saved.deliveryId)
        .then((status) => { if (status?.typicalMs) patchRun(session, (value) => ({ ...value, typicalMs: status.typicalMs })); })
        .catch(() => {});
      void follow(session, saved.deliveryId, saved.workId, controller).catch((reason) => {
        if (!controller.signal.aborted) {
          patchRun(session, (value) => ({ ...value, phase: "failed" }));
          setError(messageOf(reason, "Work could not finish"));
        }
      });
    } catch (reason) {
      rememberWork(session, null);
      if (!controller.signal.aborted) patchRun(session, (value) => ({ ...value, phase: "failed" }));
      throw reason;
    }
  }

  async function continueCreatorWork(session: string, lifecycle: Lifecycle, textValue: string, intent: WorkIntent, selected: DraftFile[], local: CreatorRun | null, phone: RemoteWork | null): Promise<void> {
    const saved = await steerTurn(session, lifecycle.workId, textValue, selected.map((item) => item.attachment.id), phone?.computer);
    putTurns(session, await fetchTurns(session));
    await refreshSessions();
    setDraft("");
    setFiles([]);
    setView(intent === "brief" ? "chat" : "build");
    putCreatorStatus(session, saved.status);
    if (saved.computer) {
      const nextWork = await remoteWork(saved.workId, saved.status, intent);
      if (nextWork) {
        const next = new Map(remoteRef.current).set(session, nextWork);
        remoteRef.current = next;
        setRemote(next);
      }
      return;
    }
    await agentHealth();
    const receipts = await Promise.all(selected.map((item) => stagePendingInput(saved.workId, item.attachment, item.file)));
    const controller = local?.controller ?? new AbortController();
    rememberWork(session, { deliveryId: saved.deliveryId, workId: saved.workId, intent });
    if (local) {
      patchRun(session, (value) => ({ ...value, status: saved.status, intent, phase: "running", awaiting: false, text: "", startedAt: saved.at * 1_000 }));
    } else {
      putRun(session, {
        workId: saved.workId, deliveryId: saved.deliveryId, intent, phase: "queued", awaiting: false, text: "", events: [],
        status: saved.status, startedAt: saved.at * 1_000, typicalMs: null, controller,
      });
    }
    try {
      await steerLocalWork(saved.workId, saved.id, textValue, receipts);
      patchRun(session, (value) => ({ ...value, phase: "running" }));
      if (!local) void follow(session, saved.deliveryId, saved.workId, controller).catch((reason) => {
        if (!controller.signal.aborted) {
          patchRun(session, (value) => ({ ...value, phase: "failed" }));
          setError(messageOf(reason, "Work could not finish"));
        }
      });
    } catch (reason) {
      rememberWork(session, null);
      if (!controller.signal.aborted) patchRun(session, (value) => ({ ...value, phase: "failed" }));
      throw reason;
    }
  }

  async function stop(): Promise<void> {
    if (!active) return;
    // Nothing is running here, so Stop is simply the way back to the conversation.
    if (!run) { setView("chat"); return; }
    const session = active;
    run.controller.abort();
    rememberWork(session, null);
    await stopWork(run.workId).catch((reason) => setError(messageOf(reason, "Could not stop this Work")));
    patchRun(session, (value) => ({ ...value, phase: "stopped" }));
    putTurns(session, await fetchTurns(session).catch(() => threads.get(session) ?? []));
    setProjectsReload((value) => value + 1);
  }

  async function post(metadata: PublishMetadata): Promise<void> {
    setError("");
    const lifecycle = latestLifecycle(turns);
    await publishProduct(metadata);
    if (lifecycle) await publishLocalWork(lifecycle.workId);
    setProjectsReload((value) => value + 1);
    await showProjects();
  }

  const previewProductForView = previewTarget ? previewLeaf(previewTarget, preview) : null;
  const activeStatus = activeRemote?.status ?? run?.status ?? (active ? creatorStatuses.get(active) : undefined);
  const briefRunning = activeStatus === "designing" && !!run && !finished(run.phase);
  const remoteBriefRunning = activeStatus === "designing" && !!activeRemote;
  const buildRunning = activeStatus === "building";
  const liveStage = activeStatus ? workStage(activeStatus) : null;
  const buildSnapshot = activeStatus === "building" && run
    ? runBuildState(run)
    : activeStatus === "building" && activeRemote
      ? { status: activeRemote.status }
      : null;

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand"><img src="/archigoat-mark.png" alt="" /><span>ArchiGoat</span></div>
      </header>
      <div className="workspace">
        <aside className="sidebar" aria-label="Primary">
          <button className="new-work" type="button" onClick={() => void newWork()}>+ New App</button>
          <nav className="sidebar-nav" aria-label="Workspace views">
            <button type="button" className={view === "projects" ? "selected" : ""} onClick={() => void showProjects()}>Apps</button>
            <button type="button" className={view === "connections" ? "selected" : ""} onClick={() => setView("connections")}>Connections</button>
          </nav>
          <div className="sidebar-foot"><a href={ACCOUNT_ORIGIN} onClick={(event) => { event.preventDefault(); void openAccount(); }}>Open TrianGoat</a></div>
        </aside>
        <section className="page">
          {view === "connections"
            ? <AgentConnections agent={agent} device={device} issue={agentIssue || startFault.current} />
            : view === "projects"
            ? <ProjectsView
                identity={identity}
                onSignOut={() => void signOut()}
                sessions={sessions}
                sessionStates={sessionStates}
                reload={projectsReload}
                onTry={tryProduct}
                onEdit={openWork}
                onRenameSession={(session, title) => void rename(session, title)}
                onDeleteSession={(session) => remove(session)}
                onStopRemote={(session, work) => void stopRemote(session, work)}
              />
            : view === "idea"
            ? <IdeaView
                value={draft}
                attachments={creatorAttachments}
                busy={submitting}
                error={error}
                onChange={setDraft}
                onSubmit={submitIdea}
                onAttach={addFiles}
                onRemoveAttachment={removeFile}
              />
            : view === "chat" && active
            ? <div className="creator-surface">
                <ChatView
                  messages={chatMessages}
                  value={draft}
                  attachments={creatorAttachments}
                  briefDelivered={activeStatus === "ready_to_build" && !editingDelivered}
                  editing={editingDelivered}
                  busy={submitting || briefRunning || remoteBriefRunning}
                  building={buildRunning}
                  status={briefRunning || remoteBriefRunning ? liveStage?.detail ?? "" : ""}
                  error={error}
                  onChange={setDraft}
                  onSubmit={submitRevision}
                  onAttach={addFiles}
                  onRemoveAttachment={removeFile}
                  onBuild={build}
                />
              </div>
            : view === "build"
            ? <BuildPreview
                surface="build"
                brief={activeBrief}
                snapshot={buildSnapshot}
                issue={error || activeRemote?.reason || ""}
                onStop={() => { if (active && activeRemote) void stopRemote(active, activeRemote); else void stop(); }}
                onRetry={() => void build()}
              />
            : view === "preview" && previewProductForView
            ? <div className="creator-surface">
                <BuildPreview
                  surface="preview"
                  product={previewProductForView}
                      editable={!!previewTarget?.session}
                      onEdit={() => { if (previewTarget?.session) openWork(previewTarget.session); }}
                      onSaveDraft={() => void showProjects()}
                      onContinue={() => setView("publish")}
                />
              </div>
            : view === "publish" && previewTarget
            ? <PublishView
                product={previewTarget.product}
                previewUrl={preview.url}
                briefDescription={activeBrief}
                onBack={() => setView("preview")}
                onPost={post}
              />
            : <IdeaView
                value={draft}
                attachments={creatorAttachments}
                busy={submitting}
                error={error}
                onChange={setDraft}
                onSubmit={submitIdea}
                onAttach={addFiles}
                onRemoveAttachment={removeFile}
              />}
        </section>
      </div>
      {quitAsk && <div className="quit-ask" role="alertdialog" aria-label="A Work is still running">
        <div className="quit-card">
          <strong>A Work is still running.</strong>
          <p>Delivery finishes next time you open ArchiGoat.</p>
          <div className="quit-actions"><button type="button" className="primary" onClick={() => answerQuit(false)}>Stay</button><button type="button" className="tool" onClick={() => answerQuit(true)}>Quit anyway</button></div>
        </div>
      </div>}
    </main>
  );
}

function AuthView({ restoring, error, notice, onClearError, onRetry }: {
  restoring: boolean;
  error: string;
  notice: AuthNotice;
  onClearError(): void;
  onRetry(): void;
}) {
  const [stage, setStage] = useState<"idle" | "opening" | "waiting">("idle");
  const [openFault, setOpenFault] = useState("");
  const recheck = useRef(onRetry);
  recheck.current = onRetry;

  // A consent finished in the browser lands back on this window, so the return itself re-checks the sign-in.
  useEffect(() => {
    if (stage !== "waiting") return;
    const check = (): void => recheck.current();
    window.addEventListener("focus", check);
    return () => window.removeEventListener("focus", check);
  }, [stage]);

  async function authorize(): Promise<void> {
    setOpenFault("");
    onClearError();
    setStage("opening");
    try {
      await authorizeAccount();
      setStage("waiting");
    } catch (reason) {
      setStage("idle");
      setOpenFault(messageOf(reason, "Could not open TrianGoat"));
    }
  }

  const waiting = stage === "waiting";
  const working = restoring || stage === "opening";
  const message = openFault || error;
  const hard = !openFault && notice === "hard";
  const label = restoring ? "Restoring your sign-in…"
    : stage === "opening" ? "Opening TrianGoat…"
      : waiting ? "Waiting for TrianGoat…"
        : "Sign in with TrianGoat";

  return <main className="welcome">
    <section className="welcome-panel">
      <div className="welcome-card">
        <div className="welcome-brand">
          <span className="welcome-brand-art"><img src="/archigoat-login.png" alt="" width={112} height={72} /></span>
          <h1>ArchiGoat</h1>
        </div>
        <button className="signin-tg" type="button" disabled={working || waiting} onClick={() => void authorize()}>
          {working || waiting
            ? <span className="signin-spin" aria-hidden="true" />
            : <span className="tg-mark" aria-hidden="true"><img src="/triangoat-mark.png" alt="" /></span>}
          <span>{label}</span>
        </button>
        {waiting && <p className="welcome-hint"><button type="button" className="notice-retry" onClick={() => { setStage("idle"); setOpenFault(""); }}>Sign in again</button></p>}
        {message && <p className={hard ? "welcome-notice hard" : "welcome-notice"} role="alert">
          <span>{hard ? `ArchiGoat could not start. ${message}` : expiredSignIn(message) ? "Your last sign-in expired." : message}{!hard && <> <button type="button" className="notice-retry" onClick={onRetry}>Try again</button></>}</span>
        </p>}
      </div>
    </section>
  </main>;
}

function expiredSignIn(message: string): boolean { return /expired|no longer valid|sign in to continue/i.test(message); }

// Only the transport's own sentences reach the screen; any other error shows the owned fallback.
function messageOf(reason: unknown, fallback: string): string { return reason instanceof BridgeError ? reason.message : fallback; }
