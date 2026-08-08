import { useEffect, useRef, type ChangeEvent, type ClipboardEvent, type FormEvent, type KeyboardEvent } from "react";
import { AttachmentList, pastedFiles, type CreatorAttachment } from "./idea";
import "./creator.css";

export type ChatMessage = {
  id: string | number;
  role: "user" | "agent";
  text: string;
  attachments?: readonly CreatorAttachment[];
};

export type ChatViewProps = {
  messages: readonly ChatMessage[];
  value: string;
  attachments: readonly CreatorAttachment[];
  briefDelivered: boolean;
  editing: boolean;
  busy?: boolean;
  building?: boolean;
  status?: string;
  error?: string;
  onChange(value: string): void;
  onSubmit(value: string): void | Promise<void>;
  onAttach(files: File[]): void | Promise<void>;
  onRemoveAttachment?(id: string): void | Promise<void>;
  onBuild(): void | Promise<void>;
};

/// ChatView keeps the delivered brief and each revision on one focused surface.
export function ChatView({ messages, value, attachments, briefDelivered, editing, busy = false, building = false, status = "", error = "", onChange, onSubmit, onAttach, onRemoveAttachment, onBuild }: ChatViewProps) {
  const composer = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const stream = useRef<HTMLElement>(null);
  const atBottom = useRef(true);

  useEffect(() => { composer.current?.focus(); }, []);
  useEffect(() => {
    const node = stream.current;
    if (node && atBottom.current) node.scrollTop = node.scrollHeight;
  }, [messages.length, status]);

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (busy || (!value.trim() && attachments.length === 0) || attachments.some((item) => item.busy)) return;
    void onSubmit(value.trim());
  }

  function keyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  }

  function attach(event: ChangeEvent<HTMLInputElement>): void {
    const files = Array.from(event.target.files ?? []);
    event.target.value = "";
    if (files.length > 0) void onAttach(files);
  }

  function paste(event: ClipboardEvent<HTMLTextAreaElement>): void {
    const files = pastedFiles(event);
    if (files.length > 0) void onAttach(files);
  }

  function scroll(event: React.UIEvent<HTMLElement>): void {
    const node = event.currentTarget;
    atBottom.current = node.scrollHeight - node.scrollTop - node.clientHeight < 32;
  }

  const canSubmit = !busy && !attachments.some((item) => item.busy) && (!!value.trim() || attachments.length > 0);
  return <main className="creator-chat" aria-label="Idea chat">
    <header className="creator-chat-head">
      <p className="creator-eyebrow">Your idea</p>
      <h1>Shape the design</h1>
    </header>
    <section ref={stream} className="creator-chat-stream" onScroll={scroll} aria-live="polite" aria-busy={busy}>
      {messages.length === 0 && <p className="creator-chat-empty">Your conversation will appear here.</p>}
      {messages.map((message) => <article className={`creator-message ${message.role}`} key={message.id}>
        <p className="creator-message-role">{message.role === "user" ? "You" : "Agent"}</p>
        {message.text && <p className="creator-message-text">{message.text}</p>}
        {message.attachments && message.attachments.length > 0 && <AttachmentList attachments={message.attachments} />}
      </article>)}
    </section>
    {status && <p className="creator-status" aria-live="polite"><span className="creator-status-dot" aria-hidden="true" />{status}</p>}
    <form className="creator-composer creator-chat-composer" onSubmit={submit}>
      {attachments.length > 0 && <AttachmentList attachments={attachments} onRemove={onRemoveAttachment} />}
      <textarea
        ref={composer}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={keyDown}
        onPaste={paste}
        placeholder="Tell the Agent what to change…"
        aria-label="Revise your idea"
        rows={3}
        disabled={busy || building}
      />
      <div className="creator-composer-actions">
        <input ref={picker} type="file" multiple onChange={attach} aria-label="Choose attachments" />
        <button type="button" className="creator-secondary" onClick={() => picker.current?.click()} disabled={busy || building}>
          <span aria-hidden="true">＋</span> Attach
        </button>
        <div className="creator-chat-actions">
          <button type="submit" className="creator-primary" disabled={!canSubmit || building}>
            {busy ? (editing ? "Building…" : "Thinking…") : (editing ? "Apply changes" : "Revise brief")}
          </button>
          {briefDelivered && <button type="button" className="creator-build" onClick={() => void onBuild()} disabled={busy || building}>
            {building ? "Building…" : "Build my idea"}
          </button>}
        </div>
      </div>
      {error && <p className="creator-error" role="alert">{error}</p>}
    </form>
  </main>;
}

export const Chat = ChatView;
