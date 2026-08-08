import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");
const read = (file) => readFileSync(join(root, file), "utf8");

test("every Work is one build Work with no intent alias", () => {
  const source = read("daemon/src/work/request.rs");
  assert.doesNotMatch(source, /WorkIntent|intent:\s*/);
  assert.match(source, /take_designer_guidance/);
});

test("Designer guidance becomes instructions with no envelope labeling", () => {
  const request = read("daemon/src/work/request.rs");
  const source = read("daemon/src/work/runtime.rs");
  assert.match(request, /TrianGoat Designer/);
  assert.match(request, /source == "guidance"/);
  assert.match(request, /Provenance::Agent/);
  assert.match(source, /instructions:\s*Option<&'a str>/);
  assert.match(source, /without_native_instructions/);
  assert.doesNotMatch(source, /AUTHORITY|UNTRUSTED|untrusted/);
  assert.doesNotMatch(source, /stored_intent|is_brief|request\.intent/);
});

test("shared runtime carries launch data without an ArchiGoat permission layer", () => {
  const provider = read("daemon/src/provider/mod.rs");
  const runtime = read("daemon/src/work/runtime.rs");
  const egress = read("daemon/src/work/egress.rs");
  for (const [file, source] of [
    ["provider/mod.rs", provider],
    ["work/runtime.rs", runtime],
    ["work/egress.rs", egress],
  ]) {
    assert.doesNotMatch(
      source,
      /WorkBoundary|WorkGrant|capability_map|CAPABILITY/,
      `${file} still contains the deleted AG permission layer`,
    );
  }
  assert.match(provider, /workspace:\s*&Path/);
  assert.match(provider, /readable:\s*&\[PathBuf\]/);
  assert.match(provider, /instructions:\s*Option<&str>/);
});

test("provider lifecycle carries no unreachable legacy diagnostics", () => {
  const execution = read("daemon/src/execution.rs");
  const observer = read("daemon/src/process/observe.rs");
  const provider = read("daemon/src/provider/mod.rs");
  assert.doesNotMatch(execution, /Stalled\(Vec<u8>\)/);
  assert.doesNotMatch(observer, /machine_stop/);
  assert.doesNotMatch(provider, /retry_notice_line/);
});

test("creator leaves expose the idea gate, full-screen chat, and accessible motion-safe styling", () => {
  for (const file of ["shell/src/idea.tsx", "shell/src/chat.tsx", "shell/src/creator.css"]) {
    assert.ok(existsSync(join(root, file)), `${file} is missing`);
  }
  const idea = read("shell/src/idea.tsx");
  const chat = read("shell/src/chat.tsx");
  const css = read("shell/src/creator.css");
  assert.match(idea, /IdeaView/);
  assert.match(idea, /Attach/);
  assert.match(idea, /onSubmit/);
  assert.doesNotMatch(idea, /creator-idea-mark|creator-eyebrow|creator-idea-lede|<h1/);
  assert.doesNotMatch(idea, /Preview/);
  assert.doesNotMatch(idea, /Agent picker/i);
  assert.match(chat, /ChatView/);
  assert.match(chat, /Build my idea/);
  assert.match(chat, /aria-live/);
  assert.match(css, /44px/);
  assert.match(css, /prefers-reduced-motion/);
});

test("chat renders the returned Framework naturally without fixed header chips", () => {
  const chat = read("shell/src/chat.tsx");
  assert.doesNotMatch(chat, /FRAMEWORK_PARTS/);
  assert.doesNotMatch(chat, /creator-framework/);
  assert.doesNotMatch(chat, /Core play|Visual direction|BGM\/SFX|Key features/);
  assert.match(chat, /message\.text/);
  assert.match(chat, /Build my idea/);
  assert.doesNotMatch(chat, /\b(?:ETA|Preview|tool log|engineering steps|\d+%)\b/i);
});
