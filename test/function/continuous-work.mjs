import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const drivePath = new URL("../../daemon/src/work_state/drive.rs", import.meta.url);

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
