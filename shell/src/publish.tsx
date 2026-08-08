import { useEffect, useState, type FormEvent } from "react";
import "./publish.css";

const NAME_LIMIT = 64;
const DESCRIPTION_LIMIT = 280;
const TAG_LIMIT = 3;
const TAG_LENGTH = 64;
const CONTROL = /[\u0000-\u001f\u007f]/u;

export type PublishMetadata = {
  id: string;
  name: string;
  description: string;
  tags: string[];
};

export type PublishProduct = {
  id: string;
  name: string;
  description?: string;
  tags: string[];
  previewKind: "image" | "video" | "html" | "pdf" | "text" | null;
};

export type PublishProps = {
  product: PublishProduct;
  previewUrl: string | null;
  briefDescription?: string;
  onBack(): void;
  onPost(metadata: PublishMetadata): Promise<void>;
};

// Publish owns the final editable draft; transport and navigation remain App-owned bridges.
export function PublishView({ product, previewUrl, briefDescription = "", onBack, onPost }: PublishProps) {
  const [name, setName] = useState(product.name);
  const [description, setDescription] = useState(() => initialDescription(product, briefDescription));
  const [tagText, setTagText] = useState(() => tagsFrom(product.tags.join(", ")).join(", "));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  // A different delivered app starts a fresh form; rerenders of this app never erase typed recovery state.
  useEffect(() => {
    setName(product.name);
    setDescription(initialDescription(product, briefDescription));
    setTagText(tagsFrom(product.tags.join(", ")).join(", "));
    setBusy(false);
    setError("");
  }, [product.id]);

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) return;
    const metadata = publishMetadata(product.id, name, description, tagText);
    if (typeof metadata === "string") {
      setError(metadata);
      return;
    }
    setBusy(true);
    setError("");
    try {
      await onPost(metadata);
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      setBusy(false);
    }
  }

  const visibleTags = tagsFrom(tagText).slice(0, TAG_LIMIT);
  const previewName = name.trim() || product.name;

  return <main className="publish-screen" aria-labelledby="publish-title">
    <div className="publish-layout">
      <header className="publish-head">
        <span className="publish-step">Final step</span>
        <h1 id="publish-title">Publish</h1>
        <p>Give the finished app the words people need, then post it.</p>
      </header>

      <section className="publish-preview" aria-label="App preview">
        <div className="publish-preview-stage">
          <ProductPreview kind={product.previewKind} url={previewUrl} name={previewName} />
        </div>
        <div className="publish-preview-copy">
          <span>Ready to post</span>
          <strong>{previewName}</strong>
          {visibleTags.length > 0 && <div className="publish-preview-tags" aria-label="Preview tags">
            {visibleTags.map((tag) => <span key={tag.toLocaleLowerCase()}>{tag}</span>)}
          </div>}
        </div>
      </section>

      <form className="publish-form" onSubmit={(event) => void submit(event)} noValidate>
        <label className="publish-field">
          <span>Title</span>
          <input
            value={name}
            maxLength={NAME_LIMIT}
            autoFocus
            autoComplete="off"
            onChange={(event) => setName(event.target.value)}
          />
        </label>

        <label className="publish-field publish-description">
          <span>Description</span>
          <textarea
            value={description}
            maxLength={DESCRIPTION_LIMIT}
            rows={6}
            onChange={(event) => setDescription(event.target.value)}
          />
          <small><span>Describe the rules and themes.</span><span>{[...description].length}/{DESCRIPTION_LIMIT}</span></small>
        </label>

        <label className="publish-field">
          <span>Tags</span>
          <input
            value={tagText}
            maxLength={200}
            autoComplete="off"
            placeholder="#puzzle, #rhythm, #friends"
            onChange={(event) => setTagText(event.target.value)}
          />
          <small>Separate up to {TAG_LIMIT} #tags with commas.</small>
        </label>

        {error && <p className="publish-error" role="alert">{error} Your changes are still here.</p>}

        <div className="publish-actions">
          <button type="button" className="publish-back" disabled={busy} onClick={onBack}>Back</button>
          <button type="submit" className="publish-post" disabled={busy}>
            {busy ? "Posting…" : error ? "Post again" : "Post"}
          </button>
        </div>
      </form>
    </div>
  </main>;
}

function ProductPreview({ kind, url, name }: { kind: PublishProduct["previewKind"]; url: string | null; name: string }) {
  if (!url) return <div className="publish-preview-empty">{kind === null ? "Preview unavailable" : "Loading preview…"}</div>;
  if (kind === "image") return <img src={url} alt={name} />;
  if (kind === "video") return <video src={url} controls playsInline />;
  if (kind === "html") return <iframe src={url} title={name} sandbox="allow-scripts allow-forms" allow="autoplay" />;
  if (kind === "pdf" || kind === "text") return <iframe src={url} title={name} sandbox="" />;
  return <div className="publish-preview-empty">Preview unavailable</div>;
}

function initialDescription(product: PublishProduct, brief: string): string {
  return product.description?.trim() || brief.trim();
}

function publishMetadata(id: string, heldName: string, heldDescription: string, heldTags: string): PublishMetadata | string {
  const name = heldName.trim();
  if (!name) return "Add a title.";
  if ([...name].length > NAME_LIMIT || CONTROL.test(name)) return "Keep the title short and on one line.";
  const description = heldDescription.trim();
  if (!description) return "Add a description of the rules and themes.";
  if ([...description].length > DESCRIPTION_LIMIT || CONTROL.test(description)) return "Keep the description to one short paragraph.";
  const rawTags = heldTags.split(",").map((tag) => tag.trim()).filter(Boolean);
  if (rawTags.some((tag) => !tag.startsWith("#"))) return "Start every tag with #.";
  const tags = tagsFrom(heldTags);
  if (tags.length > TAG_LIMIT) return `Use at most ${TAG_LIMIT} tags.`;
  if (tags.some((tag) => [...tag.slice(1)].length > TAG_LENGTH || CONTROL.test(tag))) return "Keep every tag short and on one line.";
  return { id, name, description, tags };
}

function tagsFrom(value: string): string[] {
  const tags: string[] = [];
  const keys = new Set<string>();
  for (const held of value.split(",")) {
    const name = held.trim().replace(/^#+/u, "").trim();
    if (!name) continue;
    const key = name.toLocaleLowerCase();
    if (keys.has(key)) continue;
    keys.add(key);
    tags.push(`#${name}`);
  }
  return tags;
}

function messageOf(reason: unknown): string {
  return reason instanceof Error && reason.message.trim()
    ? reason.message.trim()
    : "Could not post this app. Try again.";
}
