import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const sourcePath = new URL("../../daemon/src/account_relay.rs", import.meta.url);

test("two stale retry lanes cannot starve a new Done Work", async () => {
  const source = await readFile(sourcePath, "utf8");
  const worker = source.slice(source.indexOf("async fn delivery_worker"));

  const running = ["stale-retry-a", "stale-retry-b"];
  const cappedAdmission = (work) => {
    if (running.length >= 2 || running.includes(work)) return false;
    running.push(work);
    return true;
  };
  assert.equal(cappedAdmission("new-done-work"), false,
    "the old global two-slot gate starves a new Done Work");

  assert.doesNotMatch(source, /\bMAX_DELIVERIES\b/u);
  assert.doesNotMatch(worker, /deliveries\.len\(\)/u);
  assert.match(worker, /!active\.insert\(work\.clone\(\)\)/u,
    "each Work must still have only one active delivery lane");
});
