import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const drivePath = new URL("../../daemon/src/work_state/drive.rs", import.meta.url);
const modelPath = new URL("../../daemon/src/work_state/model.rs", import.meta.url);
const terminalPath = new URL("../../daemon/src/work_state/terminal.rs", import.meta.url);
const relayPath = new URL("../../daemon/src/account_relay.rs", import.meta.url);

test("Work waits for its turn instead of an ArchiGoat wall clock", async () => {
  const drive = await readFile(drivePath, "utf8");

  assert.doesNotMatch(drive, /\b(?:FRAMEWORK|FIRST_APP_BYTES|PLAYABLE_PREVIEW)(?:_TIMEOUT)?\b/);
  assert.doesNotMatch(drive, /\bwork_deadline\b|\bremaining\s*\(/);
  assert.match(drive, /let next = turn\.await;/);
});

test("continuous Work still keeps its real terminal owners", async () => {
  const drive = await readFile(drivePath, "utf8");

  assert.match(drive, /mark_owner_stopped/);
  assert.match(drive, /observer\.terminal_failure\(\)/);
  assert.match(drive, /complete_observed/);
  assert.match(drive, /const DELIVERED/);
});

test("a delivered turn checkpoints the same Work and only Publish releases it", async () => {
  const [model, terminal, relay] = await Promise.all([
    readFile(modelPath, "utf8"),
    readFile(terminalPath, "utf8"),
    readFile(relayPath, "utf8"),
  ]);

  assert.match(model, /Entry::Checkpoint|Checkpoint\(CheckpointWork\)/u);
  assert.match(terminal, /settle_checkpoint/u);
  assert.match(terminal, /publish_work/u);
  assert.match(relay, /"publish"\s*=>/u);
});
