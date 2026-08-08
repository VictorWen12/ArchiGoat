import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("Cursor lifts native gates with its documented Work arguments", async () => {
  const source = await readFile(
    new URL("../../daemon/src/provider/cursor.rs", import.meta.url),
    "utf8",
  );

  for (const argument of ["-p", "--workspace", "--add-dir", "--output-format", "stream-json", "--model", "--resume", "--force", "--sandbox", "disabled", "--approve-mcps", "--trust"]) {
    assert.match(source, new RegExp(`\\"${argument}\\"`), `${argument} is required by the native launch`);
  }
});
