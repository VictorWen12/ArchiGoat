import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);

test("a parked Work reports failure instead of running without a native owner", async () => {
  const model = await readFile(new URL("daemon/src/work_state/model.rs", root), "utf8");
  const store = await readFile(new URL("daemon/src/work_state/store.rs", root), "utf8");

  assert.match(model, /Entry::Running\(work\) if work\.attention => failed_snapshot\(work\)/u);
  assert.match(model, /phase:\s*RunPhase::Failed/u);
  assert.match(model, /attention_text\(work\.provider/u);
  assert.match(store, /work\.attention \|\| self\.native_owned\.contains/u);
});
