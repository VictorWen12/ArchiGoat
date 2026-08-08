import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { inflateSync } from "node:zlib";

const source = new URL("../../shell/src/", import.meta.url);

// Reads a non-interlaced 8-bit RGBA PNG so the welcome marks can be proven trimmed, not merely present.
function rgbaPixels(png) {
  const width = png.readUInt32BE(16);
  const height = png.readUInt32BE(20);
  assert.equal(png[24], 8, "welcome marks must be 8-bit");
  assert.equal(png[25], 6, "welcome marks must carry an alpha channel");
  assert.equal(png[28], 0, "welcome marks must not be interlaced");
  const parts = [];
  for (let at = 8; at + 8 <= png.length;) {
    const length = png.readUInt32BE(at);
    if (png.toString("ascii", at + 4, at + 8) === "IDAT") parts.push(png.subarray(at + 8, at + 8 + length));
    at += length + 12;
  }
  const raw = inflateSync(Buffer.concat(parts));
  const stride = width * 4;
  const pixels = Buffer.alloc(height * stride);
  let read = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[read];
    read += 1;
    for (let x = 0; x < stride; x += 1) {
      const left = x >= 4 ? pixels[y * stride + x - 4] : 0;
      const up = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const corner = x >= 4 && y > 0 ? pixels[(y - 1) * stride + x - 4] : 0;
      let value = raw[read + x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const guess = left + up - corner;
        const dl = Math.abs(guess - left);
        const du = Math.abs(guess - up);
        const dc = Math.abs(guess - corner);
        value += dl <= du && dl <= dc ? left : du <= dc ? up : corner;
      }
      pixels[y * stride + x] = value & 255;
    }
    read += stride;
  }
  return { width, height, stride, pixels };
}

function edgeAlpha({ width, height, stride, pixels }) {
  let highest = 0;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      if (x === 0 || y === 0 || x === width - 1 || y === height - 1) highest = Math.max(highest, pixels[y * stride + x * 4 + 3]);
    }
  }
  return highest;
}

test("desktop sign-in is one TrianGoat authorization action", async () => {
  const [app, transport, native, uiLogo, appIcon, publicLogo] = await Promise.all([
    readFile(new URL("App.tsx", source), "utf8"),
    readFile(new URL("transport.ts", source), "utf8"),
    readFile(new URL("../src-tauri/src/lib.rs", source), "utf8"),
    readFile(new URL("../public/Logo.png", source)),
    readFile(new URL("../../release/archigoat-icon.png", import.meta.url)),
    readFile(new URL("../../Logo.png", import.meta.url)),
  ]);

  assert.match(app, /Sign in with TrianGoat/);
  assert.match(app, /await authorizeAccount\(\)/, "sign-in must open authorization, not the generic account page");
  assert.doesNotMatch(app, /Use the TrianGoat account|password is never shared/i);
  assert.doesNotMatch(app, /Create account|6-digit PIN|Email code/);
  assert.doesNotMatch(transport, /auth\/(?:login|register|pin)/);
  assert.match(transport, /authorize=archigoat/);
  assert.doesNotMatch(native, /"\/auth\/(?:login|register|pin)/);
  assert.match(native, /authorize=archigoat/);
  assert.match(native, /fn authorize_account\(\)/, "authorization must be a distinct trusted native action");
  const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
  assert.equal(sha256(uiLogo), sha256(appIcon), "the UI must use the same enlarged goat as the macOS icon");
  assert.equal(sha256(uiLogo), sha256(publicLogo), "README and App branding must not drift");
  assert.equal(uiLogo.readUInt32BE(16), 1024);
  assert.equal(uiLogo.readUInt32BE(20), 1024);
  assert.ok([4, 6].includes(uiLogo[25]), "the macOS icon needs real alpha for rounded corners");
});

test("the welcome window uses trimmed marks, not the app icon plate", async () => {
  const [archiMark, trianMark, appIcon] = await Promise.all([
    readFile(new URL("../public/archigoat-mark.png", source)),
    readFile(new URL("../public/triangoat-mark.png", source)),
    readFile(new URL("../../release/archigoat-icon.png", import.meta.url)),
  ]);

  const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
  assert.notEqual(sha256(archiMark), sha256(appIcon), "the welcome mark must be the trimmed art, not the packaged icon plate");

  const archi = rgbaPixels(archiMark);
  assert.equal(edgeAlpha(archi), 0, "the ArchiGoat mark must be trimmed to a fully transparent border");
  const trian = rgbaPixels(trianMark);
  assert.equal(edgeAlpha(trian), 0, "the TrianGoat mark must be trimmed to a fully transparent border");
});
