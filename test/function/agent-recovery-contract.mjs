import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);

test("a wiped Account self-heals the installed Mac without coupling Pair to Agent", async () => {
  const [app, transport, local, voucher, flow, codex, claude, cursor] = await Promise.all([
    readFile(new URL("shell/src/App.tsx", root), "utf8"),
    readFile(new URL("shell/src/transport.ts", root), "utf8"),
    readFile(new URL("daemon/src/local.rs", root), "utf8"),
    readFile(new URL("daemon/src/local/voucher.rs", root), "utf8"),
    readFile(new URL("daemon/src/connection/flow.rs", root), "utf8"),
    readFile(new URL("daemon/src/provider/codex.rs", root), "utf8"),
    readFile(new URL("daemon/src/provider/claude.rs", root), "utf8"),
    readFile(new URL("daemon/src/provider/cursor.rs", root), "utf8"),
  ]);

  assert.match(voucher, /StatusCode::UNAUTHORIZED\s*=>\s*\{[\s\S]*retire\(state, credential\)[\s\S]*Authorization::Invalid/u,
    "401 means the durable installation credential is dead and must be retired");
  assert.match(voucher, /let verdict = self\.verify_shared\(state, voucher\)\.await;[\s\S]*register_missing/u,
    "the current valid voucher must immediately register after retirement");

  assert.match(local, /struct Health\s*\{[\s\S]*device:\s*String/u);
  assert.match(transport, /agentHealth\(\)[\s\S]*device:\s*string/u);
  assert.match(app, /const \[device, setDevice\] = useState<string \| null>\(null\)/u);
  assert.match(app, /setDevice\(health\.device\)/u);
  assert.match(app, /<AgentConnections agent=\{agent\} device=\{device\}/u);

  assert.match(flow, /spawn_login\(program, &provider\.login_args\(\)\)/u,
    "Connect must launch each Provider's official browser authorization flow");
  assert.match(codex, /words\(&\["login"\]\)/u);
  assert.match(claude, /words\(&\["auth", "login", "--claudeai"\]\)/u);
  assert.match(cursor, /words\(&\["login"\]\)/u);
});
