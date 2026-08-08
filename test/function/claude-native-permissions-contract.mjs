import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const sourceUrl = new URL("../../daemon/src/provider/claude.rs", import.meta.url);

test("Claude receives Designer instructions and lifts native gates", async () => {
  const source = await readFile(sourceUrl, "utf8");

  assert.doesNotMatch(
    source,
    /--permission-mode|--settings|\ballow\b|\bdeny\b|\bsandbox\b|\bfilesystem\b|\bnetwork\b|BASH_MAX_TIMEOUT_MS|permission_rule/iu,
    "Claude must not receive an ArchiGoat policy layer",
  );
  assert.match(source, /--dangerously-skip-permissions/u);
  assert.match(source, /let mut args = words\(&\["-p"\]\);/u);
  assert.match(source, /--append-system-prompt/u);
  assert.match(source, /instructions:\s*Option<&str>/u);
  assert.match(source, /--output-format", "stream-json", "--verbose/u);
  assert.match(source, /"--model"/u);
  assert.match(source, /"--resume"\s*\}\s*else\s*\{\s*"--session-id"/u);
  assert.match(source, /args\.push\(session\.to_owned\(\)\);/u);
  assert.doesNotMatch(source, /run_env/u);
});
