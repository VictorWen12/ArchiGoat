import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);
const read = (path) => readFile(new URL(path, root), "utf8");

function section(source, start, end) {
  const match = source.match(new RegExp(`${start}[\\s\\S]*?(?=${end})`));
  assert.ok(match, `expected ${start} section`);
  return match[0];
}

test("creator build phase owns honest active and terminal actions", async () => {
  const [source, styles] = await Promise.all([
    read("shell/src/build-preview.tsx"),
    read("shell/src/build-preview.css"),
  ]);
  const build = section(source, "export function BuildScreen", "\\n// Preview");

  assert.match(build, /snapshot\?\.status === "failed"[\s\S]*snapshot\?\.status === "stopped"/);
  assert.doesNotMatch(build, /snapshot\?\.phase|snapshot\?\.awaiting|needs your answer/i);
  assert.match(build, /issue\?\.trim\(\) \|\|/);
  assert.match(build, /className="build-failure"[\s\S]*role="alert"/);
  assert.match(build, /className="build-retry"[\s\S]*onClick=\{onRetry\}/);
  assert.match(build, /className="build-stop"[\s\S]*onClick=\{onStop\}/);
  assert.doesNotMatch(build, /Preview|Edit|Publish/);
  assert.match(styles, /\.build-failure\s*\{/);
  assert.match(styles, /\.build-stop,\s*\.build-retry,\s*\.preview-edit\s*\{/);
});

test("creator Preview owns Edit and Publish", async () => {
  const source = await read("shell/src/build-preview.tsx");
  const preview = section(source, "export function PreviewScreen", "\\n// The conductor");

  assert.match(preview, /className="preview-edit"[\s\S]*onClick=\{onEdit\}/);
  assert.match(preview, /className="preview-continue"[\s\S]*onClick=\{onContinue\}/);
  assert.match(preview, />Publish<\/button>/u);
});

test("creator Chat labels delivered-app edits as builds", async () => {
  const [source, app] = await Promise.all([
    read("shell/src/chat.tsx"),
    read("shell/src/App.tsx"),
  ]);

  assert.match(source, /editing: boolean;/);
  assert.match(source, /editing \? "Building…" : "Thinking…"/);
  assert.match(source, /editing \? "Apply changes" : "Revise brief"/);
});

test("creator composers attach pasted images", async () => {
  const [chat, idea] = await Promise.all([
    read("shell/src/chat.tsx"),
    read("shell/src/idea.tsx"),
  ]);

  assert.match(idea, /export function pastedFiles[\s\S]*clipboardData\.items[\s\S]*getAsFile/u);
  assert.match(idea, /<textarea[\s\S]*onPaste=\{paste\}/u);
  assert.match(chat, /<textarea[\s\S]*onPaste=\{paste\}/u);
});

test("creator Chat keeps every turn and the latest app in Preview", async () => {
  const [app, flow] = await Promise.all([
    read("shell/src/App.tsx"),
    read("shell/src/creator-flow.ts"),
  ]);

  function body(name) {
    const start = flow.indexOf(`export function ${name}`);
    assert.notEqual(start, -1, `expected ${name}`);
    const open = flow.indexOf("{", start);
    let depth = 0;
    for (let index = open; index < flow.length; index += 1) {
      if (flow[index] === "{") depth += 1;
      if (flow[index] === "}" && --depth === 0) return flow.slice(open + 1, index);
    }
    assert.fail(`${name} has no closing brace`);
  }

  const framework = (text) => ["Mechanic", "Hook", "Looks", "Sound", "Effects", "Assumption"]
    .every((field) => new RegExp(`(?:^|\\n)\\s*(?:#{1,3}\\s*)?${field}:`, "iu").test(text));
  const creatorChatTurns = new Function("framework", `return function (turns) {${body("creatorChatTurns")}}`)(framework);
  const latestBrief = new Function("framework", `return function (turns) {${body("latestBrief")}}`)(framework);
  const design = "Mechanic: tap\nHook: duel\nLooks: bright\nSound: crisp\nEffects: sparks\nAssumption: thumb play";
  const turns = [
    { id: 1, role: "me", text: "Make it quick", at: 1, attachments: [] },
    { id: 2, role: "goat", text: "I need one decision", at: 2, attachments: [] },
    { id: 3, role: "goat", text: design, at: 3, attachments: [] },
    { id: 4, role: "goat", text: "A delivered app reply", at: 4, attachments: [], product: { id: "app" } },
  ];

  assert.deepEqual(creatorChatTurns(turns), turns,
    "Chat must keep every creator and Agent turn, including ordinary and delivered replies");
  assert.equal(latestBrief(turns), design,
    "field recognition may still decorate the latest six-field design reply");
  assert.match(app, /creatorChatTurns\(turns\)/u,
    "Chat must render the complete conversation projection");
  assert.match(app, /imageUrl: attachment\.image \? attachment\.url : undefined/u,
    "history images must retain their authorized attachment URL");
  assert.match(flow, /export function deliveredTurn[\s\S]*const product = latestProduct\(turns\)[\s\S]*if \(product\) return \{ product, words: "" \}/u,
    "later Agent text must not replace an already delivered Preview");
});
