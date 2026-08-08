import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import {
  ACCOUNT_ORIGIN,
  connectAgent,
  createPairOffer,
  pairedPhones,
  revokePair,
  submitSignInCode,
  type AgentPresets,
  type PairedPhone,
} from "./transport";
import { BridgeError } from "./transport";
import { qrModules } from "./qr";
import "./agent-connections.css";

const PROVIDERS = [["codex", "ChatGPT"], ["claude", "Claude"], ["cursor", "Cursor"]] as const;
type ProviderId = (typeof PROVIDERS)[number][0];

export type AgentConnectionState = {
  registered: boolean;
  state: string;
  provider: string | null;
  installed: string[] | null;
  presets?: AgentPresets | null;
};

export type AgentConnectionsProps = {
  agent: AgentConnectionState | null;
  device: string | null;
  issue?: string;
};

// Connections owns Provider choice, native sign-in handoff, and remote-device pairing as one leaf.
export function AgentConnections({ agent, device, issue = "" }: AgentConnectionsProps) {
  const [selectedProvider, setSelectedProvider] = useState<ProviderId>(() => providerId(agent?.provider));
  const [pendingProvider, setPendingProvider] = useState<ProviderId | null>(null);
  const [busy, setBusy] = useState(false);
  const [localIssue, setLocalIssue] = useState("");

  useEffect(() => {
    if (agent?.provider) setSelectedProvider(providerId(agent.provider));
    if (agent?.state === "online") {
      setPendingProvider(null);
      setLocalIssue("");
    }
  }, [agent?.provider, agent?.state]);

  function choose(provider: ProviderId, confirmedInstall = false): void {
    if (!confirmedInstall && agent?.installed && !agent.installed.includes(provider)) {
      setPendingProvider(provider);
      return;
    }
    setBusy(true);
    setLocalIssue("");
    void connectAgent(provider, agent?.presets?.best.model, agent?.presets?.best.effort)
      .then(() => setPendingProvider(null))
      .catch((reason) => setLocalIssue(messageOf(reason, "Could not connect this Agent")))
      .finally(() => setBusy(false));
  }

  return <main className="agent-connections" aria-label="Connections">
    <AgentPanel
      agent={agent}
      issue={localIssue || issue}
      busy={busy}
      pendingProvider={pendingProvider}
      selectedProvider={selectedProvider}
      onSelect={(provider) => {
        setSelectedProvider(provider);
        setPendingProvider(null);
        setLocalIssue("");
      }}
      onChoose={() => choose(selectedProvider)}
      onInstall={() => { if (pendingProvider) choose(pendingProvider, true); }}
      />
    <ConnectionsView device={device} />
  </main>;
}

export function AgentPanel({ agent, issue, busy, pendingProvider, selectedProvider, onSelect, onChoose, onInstall }: {
  agent: AgentConnectionState | null;
  issue: string;
  busy: boolean;
  pendingProvider: ProviderId | null;
  selectedProvider: ProviderId;
  onSelect(provider: ProviderId): void;
  onChoose(): void;
  onInstall(): void;
}) {
  const label = providerLabel(agent?.provider);
  const state = agent?.state === "online" && label ? `${label} connected`
    : agent?.state === "authorizing" && label ? `Finish ${label} sign-in`
      : label ? `${label} ready` : "No Agent connected";
  const connected = agent?.provider === selectedProvider && agent.state === "online";
  const pendingLabel = providerLabel(pendingProvider);
  const authorizing = agent?.state === "authorizing" && !pendingProvider;

  function moveSelection(event: KeyboardEvent<HTMLInputElement>, index: number): void {
    const delta = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1
      : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
    if (!delta) return;
    event.preventDefault();
    const next = (index + delta + PROVIDERS.length) % PROVIDERS.length;
    const radios = event.currentTarget.closest('[role="radiogroup"]')?.querySelectorAll<HTMLInputElement>('input[type="radio"]');
    onSelect(PROVIDERS[next][0]);
    window.requestAnimationFrame(() => radios?.[next]?.focus());
  }

  return <section className="agent-connection-card" aria-labelledby="connect-agent-title">
    <h1 className="connections-section-title" id="connect-agent-title">Connect your Agent</h1>
    <div className="agent-connection-head">
      <div className="agent-connection-state">
        <span className={`agent-connection-dot ${agent?.state === "online" ? "online" : ""}`} aria-hidden="true" />
        <div><strong>{state}</strong></div>
      </div>
    </div>
    <div className="agent-provider-control">
      <div className="agent-provider-selector" role="radiogroup" aria-label="Choose an Agent">
        {PROVIDERS.map(([id, name], index) => <label className="agent-provider-option" key={id}>
          <input
            type="radio"
            name="agent-provider"
            value={id}
            checked={selectedProvider === id}
            tabIndex={selectedProvider === id ? 0 : -1}
            disabled={busy}
            onChange={() => onSelect(id)}
            onKeyDown={(event) => moveSelection(event, index)}
          />
          <span>{name}</span>
        </label>)}
      </div>
      <button
        className="agent-connect-button"
        type="button"
        disabled={busy || connected}
        onClick={onChoose}
      >{busy ? "Connecting…" : connected ? "Connected" : "Connect"}</button>
    </div>
    {pendingProvider && <div className="agent-install-card" role="status">
      <div><strong>{pendingLabel} is not installed</strong><span>Install it on this Mac, then connect.</span></div>
      <button type="button" className="agent-connect-action" onClick={onInstall} disabled={busy}>{busy ? "Connecting…" : "Install & connect"}</button>
    </div>}
    {authorizing && <SignInCode label={label} />}
    {issue && <p className="agent-connection-error" role="alert">{issue}</p>}
  </section>;
}

// Some Providers hand the browser a code to finish sign-in; this delivers it to the waiting flow.
function SignInCode({ label }: { label: string }) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [issue, setIssue] = useState("");

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!code.trim()) return;
    setBusy(true);
    setIssue("");
    try {
      await submitSignInCode(code.trim());
      setCode("");
    } catch (reason) {
      setIssue(messageOf(reason, "Could not deliver the sign-in code"));
    } finally {
      setBusy(false);
    }
  }

  return <form className="agent-signin-code" onSubmit={(event) => void submit(event)}>
    <p>Approve the browser sign-in. Paste its code here if one appears.</p>
    <div className="agent-signin-row">
      <input
        value={code}
        onChange={(event) => setCode(event.target.value)}
        placeholder={`${label} code`}
        aria-label={`${label} sign-in code`}
        autoComplete="off"
        spellCheck={false}
      />
      <button type="submit" className="agent-connect-action" disabled={busy || !code.trim()}>{busy ? "Sending…" : "Send code"}</button>
    </div>
    {issue && <p className="agent-connection-error" role="alert">{issue}</p>}
  </form>;
}

export function ConnectionsView({ device }: { device: string | null }) {
  const [offer, setOffer] = useState<{ code: string; expiresAt: number } | null>(null);
  const [phones, setPhones] = useState<PairedPhone[] | null>(null);
  const [adding, setAdding] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const [busy, setBusy] = useState(false);
  const [revokeBusy, setRevokeBusy] = useState("");
  const [error, setError] = useState("");
  const rosterSize = useRef(-1);

  async function refreshRoster(currentDevice = device): Promise<void> {
    if (!currentDevice) return;
    try {
      const next = await pairedPhones(currentDevice);
      // A phone that appears mid-pairing consumed the open code, so the code leaves with it.
      if (rosterSize.current >= 0 && next.length > rosterSize.current) {
        setAdding(false);
        setOffer(null);
      }
      rosterSize.current = next.length;
      setPhones(next);
      setError("");
    } catch (reason) {
      setError(messageOf(reason, "Could not check paired devices"));
    }
  }

  async function mint(currentDevice = device): Promise<void> {
    if (!currentDevice) return;
    setBusy(true);
    setError("");
    setOffer(null);
    try {
      setOffer(await createPairOffer(currentDevice));
      setNow(Date.now());
    } catch (reason) {
      setError(messageOf(reason, "Could not create a pairing code"));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    let alive = true;
    setOffer(null);
    setAdding(false);
    setPhones(null);
    rosterSize.current = -1;
    if (!device) return () => { alive = false; };
    void pairedPhones(device).then((next) => {
      if (!alive) return;
      rosterSize.current = next.length;
      setPhones(next);
      setError("");
      if (next.length === 0) {
        setAdding(true);
        void mint(device);
      }
    }).catch((reason) => { if (alive) setError(messageOf(reason, "Could not open Connections")); });
    const timer = window.setInterval(() => void refreshRoster(device), 10_000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [device]);

  useEffect(() => {
    if (!adding || !offer) return;
    const clock = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(clock);
  }, [adding, offer]);

  function toggleAdd(): void {
    if (adding) {
      setAdding(false);
      setOffer(null);
      return;
    }
    setAdding(true);
    void mint();
  }

  async function revoke(pairId: string): Promise<void> {
    if (!device) return;
    setRevokeBusy(pairId);
    setError("");
    try {
      await revokePair(pairId, device);
      setPhones((current) => current?.filter((phone) => phone.pairId !== pairId) ?? []);
      if (rosterSize.current > 0) rosterSize.current -= 1;
    } catch (reason) {
      setError(messageOf(reason, "Could not revoke access"));
    } finally {
      setRevokeBusy("");
    }
  }

  if (!device) return <section className="connections-view" aria-labelledby="devices-title">
    <header className="connections-view-head"><h2 className="connections-section-title" id="devices-title">Pair your device</h2></header>
    <p className="connections-blocker">Mac identity unavailable. Reopen ArchiGoat to try again.</p>
  </section>;

  const secondsLeft = offer ? Math.max(0, Math.ceil((offer.expiresAt * 1_000 - now) / 1_000)) : 0;
  const expired = !!offer && secondsLeft === 0;
  const qrValue = offer ? `${ACCOUNT_ORIGIN}/pair#${offer.code}` : "";
  return <section className="connections-view" aria-labelledby="devices-title">
    <header className="connections-view-head">
      <h2 className="connections-section-title" id="devices-title">Pair your device</h2>
      <div className="connections-view-actions">
        <button className="connections-round-action" type="button" aria-label="Refresh paired devices" onClick={() => void refreshRoster()} disabled={!phones}>↻</button>
        <button className="connections-add-action" type="button" aria-expanded={adding} onClick={toggleAdd}>{adding ? "Close" : "Pair device"}</button>
      </div>
    </header>
    {adding && <div className="connections-pair-card">
      {offer ? <PairQr value={qrValue} cover={expired ? "Expired" : ""} /> : <div className="connections-pair-loading" aria-hidden="true" />}
      {offer && !expired && <p className="connections-pair-expiry" aria-live="polite">{`Expires in ${Math.floor(secondsLeft / 60)}:${String(secondsLeft % 60).padStart(2, "0")}`}</p>}
      {expired && <button className="connections-round-action connections-renew" type="button" aria-label="New pairing code" onClick={() => void mint()} disabled={busy}>↻</button>}
      <p className="connections-pair-scan">Open TrianGoat and scan.</p>
    </div>}
    <ul className="connections-device-rows">{phones?.map((phone) => <li key={phone.pairId}>
      <span className="connections-device-icon" aria-hidden="true">⌁</span>
      <span><strong>{phone.name}</strong><small>{phone.device}</small></span>
      <button type="button" className="connections-revoke-action" onClick={() => void revoke(phone.pairId)} disabled={revokeBusy === phone.pairId}>{revokeBusy === phone.pairId ? "Revoking…" : "Revoke access"}</button>
    </li>)}</ul>
    {error && <p className="agent-connection-error" role="alert">{error}</p>}
  </section>;
}

function PairQr({ value, cover }: { value: string; cover: string }) {
  const modules = useMemo(() => qrModules(value), [value]);
  const path = useMemo(() => modules.flatMap((row, rowIndex) => row.flatMap((dark, column) => dark ? `M${column + 4} ${rowIndex + 4}h1v1h-1z` : [])).join(" "), [modules]);
  const size = modules.length + 8;
  return <div className="connections-pair-qr" aria-label="Pairing QR code" aria-disabled={!!cover}>
    <svg viewBox={`0 0 ${size} ${size}`} role="img"><title>Pairing code</title><rect width={size} height={size} fill="#fff" /><path d={path} fill="#111827" shapeRendering="crispEdges" /></svg>
    {cover && <span className="connections-pair-cover" aria-hidden="true">{cover}</span>}
  </div>;
}

function providerLabel(provider: string | null | undefined): string {
  return PROVIDERS.find(([id]) => id === provider)?.[1] ?? "";
}

function providerId(provider: string | null | undefined): ProviderId {
  return PROVIDERS.find(([id]) => id === provider)?.[0] ?? PROVIDERS[0][0];
}

function messageOf(reason: unknown, fallback: string): string {
  return reason instanceof BridgeError && reason.message.trim() ? reason.message : fallback;
}
