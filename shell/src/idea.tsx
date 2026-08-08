import { useEffect, useRef, type ChangeEvent, type ClipboardEvent, type FormEvent, type KeyboardEvent } from "react";
import type { Attachment } from "./transport";
import "./creator.css";

/// The small attachment shape shared by the idea and chat composers.
export type CreatorAttachment = Pick<Attachment, "id" | "name" | "image"> & {
  imageUrl?: string;
  busy?: boolean;
};

/// PastedFiles keeps clipboard images on the same upload path as picked files.
export function pastedFiles(event: ClipboardEvent<HTMLElement>): File[] {
  return Array.from(event.clipboardData.items).filter((item) => item.kind === "file").map((item) => item.getAsFile()).filter((file): file is File => !!file);
}

export type IdeaViewProps = {
  value: string;
  attachments: readonly CreatorAttachment[];
  busy?: boolean;
  error?: string;
  onChange(value: string): void;
  onSubmit(value: string): void | Promise<void>;
  onAttach(files: File[]): void | Promise<void>;
  onRemoveAttachment?(id: string): void | Promise<void>;
};

/// IdeaView is the local, session-free entry point for one new app idea.
export function IdeaView({ value, attachments, busy = false, error = "", onChange, onSubmit, onAttach, onRemoveAttachment }: IdeaViewProps) {
  const composer = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);

  useEffect(() => { composer.current?.focus(); }, []);

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

  const canSubmit = !busy && !attachments.some((item) => item.busy) && (!!value.trim() || attachments.length > 0);
  return <main className="creator-idea" aria-label="New idea">
    <form className="creator-composer creator-idea-composer" onSubmit={submit}>
      {attachments.length > 0 && <AttachmentList attachments={attachments} onRemove={onRemoveAttachment} />}
      <textarea
        ref={composer}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={keyDown}
        onPaste={paste}
        placeholder="Describe the app you have in mind…"
        aria-label="Your app idea"
        rows={5}
        disabled={busy}
      />
      <div className="creator-composer-actions">
        <input ref={picker} type="file" multiple onChange={attach} aria-label="Choose attachments" />
        <button type="button" className="creator-secondary" onClick={() => picker.current?.click()} disabled={busy}>
          <span aria-hidden="true">＋</span> Attach
        </button>
        <button type="submit" className="creator-primary" disabled={!canSubmit}>
          {busy ? "Designing…" : "Start with this idea"}
        </button>
      </div>
      {error && <p className="creator-error" role="alert">{error}</p>}
    </form>
  </main>;
}

/// AttachmentList keeps attachment state visible without adding another work surface.
export function AttachmentList({ attachments, onRemove }: { attachments: readonly CreatorAttachment[]; onRemove?(id: string): void | Promise<void> }) {
  return <ul className="creator-attachments" aria-label="Attachments">
    {attachments.map((item) => <li key={item.id}>
      {item.image && item.imageUrl ? <img src={item.imageUrl} alt="" /> : <span className="creator-file" aria-hidden="true">FILE</span>}
      <span className="creator-attachment-name">{item.name}</span>
      {item.busy ? <span className="creator-attachment-state">Uploading…</span> : onRemove && <button type="button" className="creator-remove" onClick={() => void onRemove(item.id)} aria-label={`Remove ${item.name}`}>×</button>}
    </li>)}
  </ul>;
}

export const IdeaComposer = IdeaView;
