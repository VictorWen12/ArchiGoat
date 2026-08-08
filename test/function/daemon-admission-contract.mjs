import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const sourcePath = new URL("../../daemon/src/account_relay.rs", import.meta.url);

test("a queued same-Work lease renews before waiting for native admission", async () => {
  const source = await readFile(sourcePath, "utf8");
  const start = source.indexOf("async fn execution_worker");
  const end = source.indexOf("// RenewWorker keeps", start);
  assert.notEqual(start, -1, "execution worker is missing");
  assert.notEqual(end, -1, "renewal worker boundary is missing");

  const worker = source.slice(start, end);
  const renewal = worker.indexOf("tokio::spawn(renew_worker");
  const ordered = worker.indexOf("order.lock().await");
  assert.ok(renewal >= 0, "leased Work must start renewal");
  assert.ok(ordered >= 0, "same-Work admission must stay ordered");
  assert.ok(
    renewal < ordered,
    "same-Work jobs must renew their lease while waiting for the ordered lane",
  );
});
