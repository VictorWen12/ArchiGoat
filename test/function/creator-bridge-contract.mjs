import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);
const read = (path) => readFile(new URL(path, root), "utf8");

test("creator bridge carries intent, publish metadata, and one-screen routing", async () => {
  const [transport, mine, app, projects, publish, creatorFlow] = await Promise.all([
    read("shell/src/transport.ts"),
    read("shell/src/mine.ts"),
    read("shell/src/App.tsx"),
    read("shell/src/projects.tsx"),
    read("shell/src/publish.tsx"),
    read("shell/src/creator-flow.ts"),
  ]);

  assert.match(transport, /type WorkIntent = "brief" \| "build"/);
  assert.match(transport, /appendTurn\(session: string, textValue: string, attachments: string\[\], intent: WorkIntent\)/u);
  assert.match(transport, /created:\s*response\.status === 201/u,
    "the Account's 201\/200 owner signal must survive the transport boundary");
  assert.match(transport, /JSON\.stringify\(\{ session, role: "me", text: textValue, attachments, intent \}\)/u,
    "Account must freeze the same brief/build intent that the daemon executes");
  assert.match(transport, /JSON\.stringify\(\{ conversation, goal, context, attachments, intent \}\)/);
  assert.match(transport, /type PendingSummon = \{[^}]*intent: WorkIntent/,
    "desktop must preserve the phone brief/build intent already published by Account");
  assert.match(transport, /summons\.push\(\{[^}]*intent: item\.intent === "brief" \? "brief" : "build"/u,
    "pending phone Work must carry its frozen intent into the desktop phase state");
  assert.match(transport, /type PublishMetadata = \{[^}]*description: string; tags: string\[\]/);
  assert.match(transport, /publishProduct\(metadata: PublishMetadata\)[\s\S]*JSON\.stringify\(metadata\)/);
  assert.match(mine, /description:\s*string/);

  assert.match(app, /<IdeaView\b/);
  assert.match(app, /<ChatView\b/);
  assert.match(app, /<BuildPreview\b/);
  assert.match(app, /<PublishView\b/);
  assert.match(app, /sessionStates=/);
  assert.match(app, /onTry=/);
  assert.doesNotMatch(app, /grid-template-columns:[^;]*1fr[^;]*1fr/);
  assert.match(projects, /stage === "error"[\s\S]*role="alert"/,
    "a real Account failure must remain visible until the bridge recovers");
  assert.doesNotMatch(projects, /Your next app starts with/u,
    "a successful empty Apps shelf must stay visually blank");
  assert.match(publish, /placeholder="#puzzle, #rhythm, #friends"/u);
  assert.match(publish, /const \[tagText, setTagText\] = useState\(\(\) => tagsFrom\(product\.tags\.join\(", "\)\)\.join\(", "\)\)/u,
    "generated artifact tags must enter Publish as valid #tags");
  assert.match(publish, /tags\.push\(`#\$\{name\}`\)/u,
    "AG must send the app-owned #tag contract consumed by TG");
  assert.match(creatorFlow, /return !!turn && turn\.role === "goat" && \(!!turn\.product \|\| !!turn\.text\.trim\(\)\);/u,
    "editing a delivered product must keep Build available in the same session");
});

test("creator edit has one local or phone-owned Work owner", async () => {
  const [app, creatorFlow] = await Promise.all([
    read("shell/src/App.tsx"),
    read("shell/src/creator-flow.ts"),
  ]);

  assert.match(creatorFlow, /export function liveWork\([\s\S]*?return \(!!run && !finished\(run\.phase\)\) \|\| remote !== null;/u,
    "every pending remote Work must own the session, including queued and unreachable states");
  assert.match(creatorFlow, /export function workSurface\([\s\S]*?if \(remote\?\.intent === "build"\) return "build";[\s\S]*?return "chat";/u,
    "only a phone Build may open Building; a phone Framework turn stays in Chat");
  assert.match(app, /function openWork\(session: string\): void \{[\s\S]*?setView\(workSurface\(runs\.get\(session\) \?\? null, remote\.get\(session\) \?\? null\)\);/u,
    "Preview to Edit must return to the same session's Chat unless it is still Building");
  assert.match(app, /function tryProduct\(product: MineProduct\): void \{[\s\S]*?if \(product\.sessionId && liveWork\([\s\S]*?setView\(workSurface\(/u,
    "an old playable result cannot bypass a live Build phase");
  assert.match(app, /const editingDelivered = !!latestProduct\(turns\)[\s\S]*?const intent: WorkIntent = editingDelivered \? "build" : "brief";[\s\S]*?startCreatorTurn\(active, value, intent, files\)/u,
    "a delivered Preview edit must start one fresh build Work for its replacement Preview");
  assert.match(app, /async function startCreatorTurn\([\s\S]*?if \(liveWork\(runs\.get\(session\) \?\? null, remote\.get\(session\) \?\? null\)\) \{[\s\S]*?setView\(workSurface\(runs\.get\(session\) \?\? null, remote\.get\(session\) \?\? null\)\);[\s\S]*?return;[\s\S]*?const saved = await appendTurn\(session,/u,
    "a live owner must gate appendTurn before any new Work can start");
  assert.match(app, /appendTurn\(session, textValue\.trim\(\), attachmentIds, intent\)/u,
    "the durable Account turn and local Work must share one intent");
  const follower = app.match(/if \(!saved\.created\) \{([\s\S]*?)\n    \}\n    putTurns/u)?.[1] ?? "";
  assert.match(follower, /fetchTurns\(session\)/u,
    "a converged follower must adopt the canonical Account turn");
  assert.match(follower, /saved\.pending\.computer[\s\S]*?remoteWork\(saved\.workId, intent\)/u,
    "a phone-owned follower must immediately adopt the existing remote Work");
  assert.doesNotMatch(follower, /startWork|stagePendingInput/u,
    "a 200 follower must never stage or start the first writer's Work");
  assert.match(app, /const completed = \[\.\.\.remoteRef\.current\.keys\(\)\]\.filter\(\(session\) => !next\.has\(session\)\);[\s\S]*?adoptRemoteDelivery\(session\)/u,
    "a delivered phone Work must move the desktop out of Building");
  assert.match(app, /\(view === "chat" \|\| view === "preview" \|\| view === "publish"\)[\s\S]*?remote\.get\(active\)\?\.intent === "build"[\s\S]*?setView\("build"\)/u,
    "a phone Edit must replace every open creator phase, including Publish, with Building");
  assert.match(app, /async function adoptRemoteDelivery\(session: string\)[\s\S]*?fetchTurns\(session\)[\s\S]*?setPreviewTarget\([\s\S]*?setView\("preview"\)/u,
    "remote delivery must refresh the same session and open its replacement Preview");
  assert.doesNotMatch(app.match(/async function startCreatorTurn[\s\S]*?\n  \}\n\n  async function stop/u)?.[0] ?? "", /createSession\(/u,
    "Edit must reuse the existing conversation/native session");
  assert.doesNotMatch(app, /CreatorSwitch|creator-switch/u,
    "creator surfaces must be phase-driven, without a manual Chat/Preview switch");
  assert.match(app, /view === "build"[\s\S]*?<BuildPreview[\s\S]*?surface="build"/u,
    "Build must own its own screen until delivery");
  assert.match(app, /onStop=\{\(\) => \{ if \(active && activeRemote\) void stopRemote\(active, activeRemote\); else void stop\(\); \}\}/u,
    "Building Stop must target the actual local or phone-owned Work");
  assert.match(app, /view === "preview"[\s\S]*?<BuildPreview[\s\S]*?surface="preview"[\s\S]*?onEdit=/u,
    "Edit must exist on delivered Preview");
  assert.match(app, /view === "publish" && previewTarget[\s\S]*?<PublishView/u,
    "Publish must remain reachable only from delivered Preview");
});
