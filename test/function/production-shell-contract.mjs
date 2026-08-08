import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const manifest = readFileSync(new URL("../../shell/src-tauri/Cargo.toml", import.meta.url), "utf8");
const binary = readFileSync(process.env.ARCHIGOAT_SHELL || "");

assert.match(manifest, /^default\s*=\s*\["custom-protocol"\]$/mu, "release must default to Tauri's production protocol");
assert.match(manifest, /^custom-protocol\s*=\s*\["tauri\/custom-protocol"\]$/mu, "production protocol must reach Tauri");
assert.ok(binary.length > 0, "release shell is missing");
