import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = new URL("../../shell/src/", import.meta.url);
const files = ["app.css", "creator.css", "build-preview.css", "publish.css", "projects.css"];
const styles = (await Promise.all(files.map((file) => readFile(new URL(file, source), "utf8")))).join("\n");
const publish = await readFile(new URL("publish.tsx", source), "utf8");

test("owned AG shell styles keep surfaces light and primary actions blue", () => {
  assert.doesNotMatch(styles, /\b(?:green|teal|cyan)\b|color-scheme:\s*dark/i);
  assert.doesNotMatch(styles, /#(?:080c10|0d1419|121b21|26343c|3a4d57|10252b|0b121c|050a10|03070b|0c1522|121f31|62c6c4|79d5e1|8dcff0)\b/i);
  assert.doesNotMatch(styles, /background(?:-image|-color)?\s*:[^;]*(?:#(?:080c10|0d1419|121b21|10252b|0b121c|050a10|03070b|0c1522)|rgba\(\s*(?:7|113)\s*,)/i);

  for (const rule of [
    /\.new-work\s*\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
    /\.primary\s*\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
    /\.signin-tg\s*\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
    /\.creator-primary[\s\S]*?\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
    /\.creator-build\s*\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
    /\.build-retry,[\s\S]*?\.preview-continue\s*\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
    /\.publish-post\s*\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
    /\.projects-primary\s*\{[^}]*color:\s*#09203a[^}]*background:\s*#87b7eb/i,
  ]) {
    assert.match(styles, rule);
  }
});

test("publish requires the # prefix for every tag", () => {
  assert.match(publish, /startsWith\("#"\)/u);
});
