import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = new URL("../../shell/src/", import.meta.url);

test("Build and Preview keep the product surface full-screen and keyboard reachable", async () => {
  const [component, styles] = await Promise.all([
    readFile(new URL("build-preview.tsx", source), "utf8"),
    readFile(new URL("build-preview.css", source), "utf8"),
  ]);

  assert.match(component, /export function BuildScreen\b/);
  assert.match(component, /export function PreviewScreen\b/);
  assert.match(component, /editable\?: boolean/);
  assert.match(component, /const editable = editableOverride \?\? product\.editable \?\? true;/);
  assert.match(component, /aria-live="polite"/);
  assert.match(component, /onStop/);
  assert.match(component, /onRetry/);
  assert.match(component, /"Building…"/);
  assert.match(component, /"Build stopped"[\s\S]*"Build failed"/);
  assert.match(component, /className="build-failure" role="alert"/);
  assert.doesNotMatch(component, /\b(stageText|latestStage|progress|events)\b/);
  assert.doesNotMatch(component, /build-(brief|status|error|recovery|pulse)|<time\b|Approved brief/);
  assert.doesNotMatch(component, /allow="autoplay"/);
  assert.match(component, /<iframe[\s\S]*tabIndex=\{0\}/);
  assert.match(component, /editable && <button[^>]+>Edit<\/button>/);
  assert.match(component, /<button[^>]+>Publish<\/button>/);
  assert.doesNotMatch(component, /<progress\b|aria-valuenow|\b\d+%/);
  assert.doesNotMatch(component, />\s*(Private|Delete|Download)\s*</i);

  assert.match(styles, /\.build-preview-screen\s*\{[\s\S]*min-height:\s*100dvh/);
  assert.match(styles, /\.preview-screen\s*\{[\s\S]*display:\s*flex/);
  assert.match(styles, /\.preview-canvas\s*\{[\s\S]*flex:\s*1/);
  assert.match(styles, /\.build-actions button,[\s\S]*?\.preview-actions button\s*\{[\s\S]*min-height:\s*44px/);
  assert.doesNotMatch(styles, /\.preview-canvas[^}]*pointer-events:\s*none/);
});
