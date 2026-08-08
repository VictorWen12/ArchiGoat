import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);

test("Codex receives Designer instructions and lifts native gates", async () => {
  const source = await readFile(new URL("daemon/src/provider/codex.rs", root), "utf8");

  assert.match(source, /words\(&\["exec"\]\)/u);
  assert.match(source, /"developer_instructions"/u);
  assert.match(source, /"model_reasoning_effort"/u);
  assert.match(source, /"--skip-git-repo-check"/u);
  assert.match(source, /"--dangerously-bypass-approvals-and-sandbox"/u);
  assert.match(source, /"--json"/u);

  for (const forbidden of [
    /approval_policy/u,
    /default_permissions/u,
    /permissions\.app_work/u,
    /background_terminal_max_timeout/u,
    /boundary\.(?:readable|writable|denied|network|ceiling)/u,
    /--search/u,
  ]) {
    assert.doesNotMatch(source, forbidden, `Codex still contains an AG override: ${forbidden}`);
  }
});
