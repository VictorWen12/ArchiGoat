// Runs the shipped binary exactly as production launches it, then kills the Provider the moment
// its product exists. What the owner receives must be the product, not the way the process died.

// Node primitives create one private signed Work, run it, and read the delivery it froze.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHmac, randomBytes } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

// The repository and built ArchiGoat path keep this check on the same binary CI releases.
const repository = resolve(import.meta.dirname, "../..");
const defaultBinary = join(repository, "daemon/target/debug", "archigoat");
const appBinary = resolve(process.env.ARCHIGOAT_BINARY || defaultBinary);
// The exact bytes the Provider writes; the delivery must carry them unchanged.
const PRODUCT = "<!doctype html>\n<title>Delivered</title>\n";

// A Work whose Provider dies after building the product still owns a product.
test("a Provider killed after writing its product still delivers the frozen bytes", {
  skip: process.platform !== "darwin",
  timeout: 15_000,
}, async (context) => {
  const fixture = await prepareFixture();
  let helper;
  context.after(async () => {
    if (helper?.exitCode === null) helper.kill("SIGKILL");
    // The frozen receipt is intentionally read-only; its root needs owner write access for fixture deletion.
    await chmod(fixture.frozen, 0o700).catch(() => {});
    await rm(fixture.root, { recursive: true, force: true });
  });

  helper = spawn(appBinary, ["--terminal-work", fixture.job], {
    cwd: repository,
    env: { ...process.env, ARCHIGOAT_STATE: fixture.state },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  helper.stderr.on("data", (bytes) => { stderr += bytes; });
  const code = await exitWithin(helper, 10_000);

  assert.equal(code, 0, stderr || "native Work helper did not exit");
  assert.equal(await journalKinds(fixture.events).then((kinds) => kinds.at(-1)), 3,
    "the killed Work did not journal its terminal frame");
  assert.equal(await readFile(join(fixture.frozen, "product.html"), "utf8").catch(() => ""), PRODUCT,
    "the killed Work lost the product it had already built");
  const manifest = JSON.parse(await readFile(join(fixture.frozen, ".manifest.json"), "utf8").catch(() => "[]"));
  assert.deepEqual(manifest.map((receipt) => receipt.name), ["product.html"],
    "the frozen delivery does not name the product");
});

// The fixture signs the exact platform request the production helper accepts.
async function prepareFixture() {
  const root = await mkdtemp(join(tmpdir(), "product-delivery-truth-"));
  const state = join(root, "archigoat.json");
  const secret = "a".repeat(64);
  const nonce = randomBytes(32).toString("hex");
  const desktop = join(root, "Desktop", "Product");
  const workspace = join(desktop, "DeliveryTruth");
  const frozen = join(root, "Frozen", "delivery");
  const runner = join(workspace, ".app", "terminal", nonce);
  await mkdir(workspace, { recursive: true });
  await writeFile(state, JSON.stringify({ instance_secret: secret, provider: null, pairing: null }));

  // The Provider reads its brief, writes one product file, and is killed before any clean exit.
  const source = join(root, "product.source");
  const script = join(root, "provider");
  await writeFile(source, PRODUCT);
  await writeFile(script, `#!/bin/sh\ncat > /dev/null\ncp "$1" product.html\nkill -KILL $$\n`);
  await chmod(script, 0o700);

  const request = {
    work_id: "delivery-truth",
    nonce,
    state,
    program: script,
    prefix: [],
    args: [source],
    input: "build the product",
    cwd: workspace,
    desktop_root: desktop,
    freeze_root: frozen,
  };
  const payload = JSON.stringify(request);
  const proof = createHmac("sha256", secret).update("terminal-work:").update(payload).digest("hex");
  await mkdir(runner, { recursive: true });
  const job = join(runner, "job.json");
  await writeFile(job, JSON.stringify({ request, proof }));
  return { root, state, frozen, job, events: join(runner, "events.bin") };
}

// A short bound turns a production hang into direct evidence without leaving the test runner stuck.
async function exitWithin(child, milliseconds) {
  return new Promise((resolveExit, rejectExit) => {
    const deadline = setTimeout(
      () => rejectExit(new Error("native Work helper did not exit after the Provider died")),
      milliseconds,
    );
    child.once("exit", (code) => {
      clearTimeout(deadline);
      resolveExit(code);
    });
  });
}

// Journal parsing reads only complete production frames and returns their event kinds.
async function journalKinds(path) {
  const bytes = await readFile(path);
  const kinds = [];
  for (let offset = 0; offset + 17 <= bytes.length;) {
    const size = Number(bytes.readBigUInt64LE(offset + 9));
    assert.ok(offset + 17 + size <= bytes.length, "native Work journal ended mid-frame");
    kinds.push(bytes[offset + 8]);
    offset += 17 + size;
  }
  return kinds;
}
