// This cross-platform proof runs the shipped binary exactly as production launches it — the Work helper
// headlessly, as a direct background child against a Provider child that inherits its output pipes, and
// removal as the command a person runs to take ArchiGoat off their machine.

// Node primitives create one private signed Work, observe its native processes, and remove its exact fixture root.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHmac, randomBytes } from "node:crypto";
import { chmod, cp, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import test from "node:test";

// The repository and built ArchiGoat path keep this proof on the same binary CI releases.
const repository = resolve(import.meta.dirname, "../..");
const defaultBinary = join(repository, "daemon/target/debug", process.platform === "win32" ? "archigoat.exe" : "archigoat");
const appBinary = resolve(process.env.ARCHIGOAT_BINARY || defaultBinary);

// One real helper run must finish, journal Done, and reap the Provider grandchild that holds both output pipes.
test("finished native Work exits and reaps a pipe-holding Provider grandchild", { timeout: 15_000 }, async (context) => {
  const fixture = await prepareFixture();
  let helper;
  context.after(async () => {
    if (helper?.exitCode === null) helper.kill("SIGKILL");
    await killIfAlive(await readPid(fixture.marker));
    // The frozen receipt is intentionally read-only; its root needs owner write access for fixture deletion.
    await chmod(fixture.frozen, 0o700).catch(() => {});
    await rm(fixture.root, { recursive: true, force: true });
  });

  helper = spawn(appBinary, [fixture.helper, fixture.job], {
    cwd: repository,
    env: { ...process.env, ARCHIGOAT_STATE: fixture.state },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  helper.stderr.on("data", (bytes) => { stderr += bytes; });
  const code = await exitWithin(helper, 10_000);

  assert.equal(code, 0, stderr || "native Work helper did not exit");
  assert.equal(await journalKinds(fixture.events).then((kinds) => kinds.at(-1)), 3, "native Work did not journal Done");
  assert.equal(isAlive(await readPid(fixture.marker)), false, "Provider grandchild survived finished Work");
});

// A finished Work must close the browser session it opened, or the user cannot quit their own Chrome.
test("finished macOS native Work stops the browser session it opened", {
  skip: process.platform !== "darwin",
  timeout: 15_000,
}, async (context) => {
  const fixture = await prepareBrowserFixture();
  let helper;
  context.after(() => cleanupBrowserFixture(fixture, helper));

  helper = spawn(appBinary, [fixture.helper, fixture.job], {
    cwd: repository,
    env: { ...process.env, ARCHIGOAT_STATE: fixture.state },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  helper.stderr.on("data", (bytes) => { stderr += bytes; });
  const code = await exitWithin(helper, 10_000).catch(async (error) => {
    error.message += `; ready=${await readFile(fixture.readyMarker, "utf8").catch(() => "missing")}; daemon=${await readFile(fixture.errorMarker, "utf8").catch(() => "no error")}`;
    throw error;
  });
  const daemon = await readPid(fixture.daemonPid);
  const stop = await readJsonWithin(fixture.stopMarker, 2_000);

  assert.equal(code, 0, stderr || "native Work helper did not exit");
  assert.deepEqual(stop, { id: 1, method: "stop", params: {} },
    `runner did not ask the browser session to close; ${await readFile(fixture.errorMarker, "utf8").catch(() => "no daemon error")}`);
  assert.equal(await exitsWithin(daemon, 2_000), true, "browser session survived finished Work");
  assert.deepEqual(JSON.parse(await readFile(fixture.envMarker, "utf8")), {
    PWTEST_DAEMON_SESSION_DIR: fixture.browserSessions,
    PWTEST_SOCKETS_DIR: fixture.browserTemp,
    TMPDIR: fixture.browserTemp,
  }, "runner injected the wrong private browser roots");
});

// A Work the owner stops mid-run must release the browser just as completely as one that finished.
test("stopped macOS native Work stops the browser session it opened", {
  skip: process.platform !== "darwin",
  timeout: 15_000,
}, async (context) => {
  const fixture = await prepareBrowserFixture(true);
  let helper;
  context.after(() => cleanupBrowserFixture(fixture, helper));

  helper = spawn(appBinary, [fixture.helper, fixture.job], {
    cwd: repository,
    env: { ...process.env, ARCHIGOAT_STATE: fixture.state },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  helper.stderr.on("data", (bytes) => { stderr += bytes; });
  // The Work is only stopped once its browser session is genuinely open.
  assert.ok(await readJsonWithin(fixture.readyMarker, 5_000, false), "browser session never opened");
  await writeFile(fixture.ownerStop, fixture.ownerStopProof);
  const daemon = await readPid(fixture.daemonPid);
  const code = await exitWithin(helper, 10_000);

  assert.equal(code, 0, stderr || "stopped native Work helper did not exit");
  assert.deepEqual(await readJsonWithin(fixture.stopMarker, 2_000), { id: 1, method: "stop", params: {} },
    `stopped runner did not ask the browser session to close; ${await readFile(fixture.errorMarker, "utf8").catch(() => "no daemon error")}`);
  assert.equal(await exitsWithin(daemon, 2_000), true, "browser session survived stopped Work");
  assert.equal(isAlive(await readPid(fixture.marker)), false, "Provider survived stopped Work");
});

// An app that leaves files, a bundle, and a login item behind after removal is what makes people call software a virus.
test("removal deletes every ArchiGoat file except delivered artifacts", {
  skip: process.platform !== "darwin",
  timeout: 30_000,
}, async (context) => {
  const root = await mkdtemp(join(tmpdir(), "product-uninstall-"));
  context.after(() => rm(root, { recursive: true, force: true }));
  const home = join(root, "home");
  const temp = join(root, "temp");
  const state = join(root, "state");
  const bundle = join(home, "Applications", "ArchiGoat.app");
  const installed = join(bundle, "Contents", "MacOS", "archigoat");

  // Removal deletes only the App it is running from, so this fixture runs the real binary from a real bundle path.
  await mkdir(dirname(installed), { recursive: true });
  await cp(appBinary, installed);
  await chmod(installed, 0o755);

  // The delivered artifact is the one thing removal must keep; everything else here is app bookkeeping.
  const kept = join(state, "Deliveries", "run-1", "product.html");
  const removed = [
    join(state, "Works", "a".repeat(64), "scratch.txt"),
    join(state, "Inputs", "b".repeat(64), "attachment"),
    join(state, "InputReceipts", "c".repeat(64), "receipt"),
    join(state, "Relay", "queued"),
    join(state, "archigoat.json"),
    join(state, "works.json"),
    join(state, `login-${"d".repeat(64)}.command`),
    join(temp, "ArchiGoat", "archigoat.log"),
    join(temp, "ag-0123456789abcdef", "session"),
    join(temp, ["Pl", "ugin"].join(""), ["pl", "ugin.log"].join("")),
    join(temp, `f${"o-"}0123456789abcdef`, "session"),
  ];
  // Temporary neighbours prove the browser-tree match is exact rather than a prefix sweep of the temp directory.
  const strangers = [
    join(temp, "ag-notahexnonce0", "keep"),
    join(temp, "ag-0123456789abcdef0", "keep"),
    join(temp, "unrelated", "keep"),
  ];
  for (const file of [kept, ...removed, ...strangers]) {
    await mkdir(dirname(file), { recursive: true });
    await writeFile(file, "seeded");
  }

  // Exists reports presence without throwing, so one assertion reads either survival or removal.
  const exists = (path) => stat(path).then(() => true, () => false);
  const removal = spawnSync(installed, ["--uninstall"], {
    // The opt-out keeps this proof off the developer's own launchd and running ArchiGoat.
    env: { ...process.env, HOME: home, TMPDIR: temp, ARCHIGOAT_STATE: join(state, "archigoat.json"), ARCHIGOAT_KEEPALIVE: "off" },
    encoding: "utf8",
  });

  assert.equal(removal.status, 0, removal.stderr || "removal did not succeed");
  assert.deepEqual(await readdir(state), ["Deliveries"], "removal left ArchiGoat state behind");
  assert.equal(await readFile(kept, "utf8"), "seeded", "removal destroyed a delivered artifact");
  for (const file of removed) {
    assert.equal(await exists(file), false, `removal left ${file}`);
  }
  for (const file of strangers) {
    assert.equal(await exists(file), true, `removal deleted an unrelated temporary path ${file}`);
  }
  assert.equal(await exists(bundle), false, "removal left the installed App");
});

// Online removal retires the durable Account computer before deleting the local identity.
test("online removal adopts the old identity, retires the App, then keeps deliveries", {
  skip: process.platform !== "darwin",
  timeout: 15_000,
}, async (context) => {
  const root = await mkdtemp(join(tmpdir(), "product-uninstall-retire-"));
  const home = join(root, "home");
  const temp = join(root, "temp");
  const support = join(home, "Library", "Application Support");
  const legacyRoot = join(support, ["Pl", "ugin"].join(""));
  const currentRoot = join(support, "ArchiGoat");
  const legacyState = join(legacyRoot, ["pl", "ugin.json"].join(""));
  const bundle = join(home, "Applications", "ArchiGoat.app");
  const installed = join(bundle, "Contents", "MacOS", "archigoat");
  const device = "a".repeat(64);
  const credential = "b".repeat(64);
  const requests = [];
  const account = createServer((request, response) => {
    requests.push({
      method: request.method,
      path: request.url,
      headers: request.headers,
    });
    response.writeHead(204).end();
  });
  await new Promise((resolve, reject) => {
    account.once("error", reject);
    account.listen(0, "127.0.0.1", resolve);
  });
  context.after(async () => {
    await new Promise((resolve) => account.close(resolve));
    await rm(root, { recursive: true, force: true });
  });
  const address = account.address();
  assert.ok(address && typeof address === "object", "Account fixture did not bind");
  await mkdir(dirname(installed), { recursive: true });
  await cp(appBinary, installed);
  await chmod(installed, 0o755);
  await mkdir(legacyRoot, { recursive: true });
  await writeFile(legacyState, JSON.stringify({
    device_id: device,
    instance_secret: "c".repeat(64),
    [["pl", "ugin_credential"].join("")]: credential,
    provider: null,
  }));
  const kept = join("Deliveries", "run-1", "product.html");
  await mkdir(dirname(join(legacyRoot, kept)), { recursive: true });
  await writeFile(join(legacyRoot, kept), "seeded");

  const removal = spawn(installed, ["--uninstall"], {
    env: {
      ...process.env,
      HOME: home,
      TMPDIR: temp,
      ACCOUNT_URL: `http://127.0.0.1:${address.port}`,
      ARCHIGOAT_KEEPALIVE: "off",
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  removal.stderr.on("data", (bytes) => { stderr += bytes; });
  const code = await exitWithin(removal, 10_000);

  assert.equal(code, 0, stderr || "removal did not succeed");
  assert.equal(requests.length, 1, "removal did not notify Account");
  assert.equal(requests[0].method, "POST");
  assert.equal(requests[0].path, "/auth/app/retire");
  assert.equal(requests[0].headers.authorization, `Bearer ${credential}`);
  assert.equal(requests[0].headers["x-app-device"], device);
  assert.match(requests[0].headers["x-app-instance"], /^[0-9a-f]{64}$/);
  assert.equal(requests[0].headers["x-app-protocol"], "16");
  assert.equal(await stat(legacyRoot).then(() => true, () => false), false, "legacy state was not adopted");
  assert.equal(await readFile(join(currentRoot, kept), "utf8"), "seeded", "removal destroyed a delivered artifact");
  assert.equal(await stat(bundle).then(() => true, () => false), false, "removal left the installed App");
});

// Fixture preparation signs the exact platform request accepted by the production helper.
async function prepareFixture(provider = providerCommand) {
  const root = await mkdtemp(join(tmpdir(), "product-native-runner-"));
  const state = join(root, "archigoat.json");
  const secret = "a".repeat(64);
  const nonce = randomBytes(32).toString("hex");
  const desktop = join(root, "Desktop", "Product");
  const workspace = join(desktop, "RunnerCleanup");
  const frozen = join(root, "Frozen", "delivery");
  const marker = join(root, "grandchild.pid");
  await mkdir(workspace, { recursive: true });
  await writeFile(state, JSON.stringify({ instance_secret: secret, provider: null, pairing: null }));

  const runner = process.platform === "win32"
    ? join(dirname(state), "WindowsWork", nonce)
    : join(workspace, ".app", "terminal", nonce);
  const platform = await provider(root, marker, runner);
  const request = {
    work_id: "native-runner-cleanup",
    nonce,
    state,
    program: platform.program,
    prefix: platform.prefix,
    args: platform.args,
    input: "finish and exit",
    cwd: workspace,
    ...(process.platform === "win32" ? { provider: "codex" } : {}),
    desktop_root: desktop,
    freeze_root: frozen,
  };
  const payload = JSON.stringify(request);
  const proof = createHmac("sha256", secret).update("terminal-work:").update(payload).digest("hex");
  await mkdir(runner, { recursive: true });
  const job = join(runner, process.platform === "win32" ? "work.json" : "job.json");
  await writeFile(job, JSON.stringify({ request, proof }));
  return {
    root,
    state,
    marker,
    frozen,
    job,
    helper: process.platform === "win32" ? "--windows-work" : "--terminal-work",
    events: join(runner, "events.bin"),
    // The owner Stop file and its proof let one test end a live Work exactly as ArchiGoat does.
    ownerStop: join(runner, "stop"),
    ownerStopProof: createHmac("sha256", secret).update("terminal-work:").update(`stop:${nonce}`).digest("hex"),
  };
}

// This Provider double launches one detached daemon holding Playwright's session file and stop socket.
async function prepareBrowserFixture(hold = false) {
  let browser;
  const fixture = await prepareFixture(async (root, marker, runner) => {
    browser = await browserProviderCommand(root, marker, runner, hold);
    return browser;
  });
  return { ...fixture, ...browser };
}

// Cleanup removes the daemon, its socket, and the private browser tree however the Work ended.
async function cleanupBrowserFixture(fixture, helper) {
  if (helper?.exitCode === null) helper.kill("SIGKILL");
  await killIfAlive(await readPid(fixture.marker));
  await killIfAlive(await readPid(fixture.daemonPid));
  spawnSync("pkill", ["-KILL", "-f", fixture.root], { stdio: "ignore" });
  await rm(await readFile(fixture.socketMarker, "utf8").catch(() => ""), { force: true }).catch(() => {});
  await rm(fixture.browserTemp, { recursive: true, force: true });
  await chmod(fixture.frozen, 0o700).catch(() => {});
  await rm(fixture.root, { recursive: true, force: true });
}

// A held Provider keeps running so the Work can be stopped mid-run instead of finishing on its own.
async function browserProviderCommand(root, marker, runner, hold) {
  const script = join(root, "browser-provider.mjs");
  const daemonPid = join(root, "browser-daemon.pid");
  const errorMarker = join(root, "browser-daemon.error");
  const envMarker = join(root, "browser-env");
  const stopMarker = join(root, "browser-stop.json");
  const socketMarker = join(root, "browser-socket");
  const readyMarker = join(root, "browser-ready");
  const browserSessions = join(runner, "browser-sessions");
  const browserTemp = join(tmpdir(), `ag-${basename(runner).slice(0, 16)}`);
  await writeFile(script, [
    'import { spawn } from "node:child_process";',
    'import { mkdir, open, readFile, writeFile } from "node:fs/promises";',
    'import net from "node:net";',
    'import { join } from "node:path";',
    'const [mode, expectedRoot, expectedTemp, providerPidFile, pidFile, envFile, stopFile, socketFile, readyFile, errorFile, holdFlag] = process.argv.slice(2);',
    'if (mode === "provider") {',
    '  await writeFile(providerPidFile, String(process.pid));',
    '  await writeFile(envFile, JSON.stringify({ PWTEST_DAEMON_SESSION_DIR: process.env.PWTEST_DAEMON_SESSION_DIR || "", PWTEST_SOCKETS_DIR: process.env.PWTEST_SOCKETS_DIR || "", TMPDIR: process.env.TMPDIR || "" }));',
    '  const errors = await open(errorFile, "w");',
    '  const child = spawn(process.execPath, [process.argv[1], "daemon", expectedRoot, expectedTemp, providerPidFile, pidFile, envFile, stopFile, socketFile, readyFile, errorFile, holdFlag], { detached: true, stdio: ["ignore", "ignore", errors.fd], env: process.env });',
    '  child.once("error", (error) => writeFile(errorFile, String(error?.stack || error)));',
    '  child.unref();',
    '  const deadline = Date.now() + 2000;',
    '  while (Date.now() < deadline) {',
    '    if (await readFile(readyFile).catch(() => null)) {',
    // A held Provider never finishes, so only owner Stop can end this Work.
    '      if (holdFlag === "hold") await new Promise(() => {});',
    '      process.exit(0);',
    '    }',
    '    await new Promise((resolve) => setTimeout(resolve, 10));',
    '  }',
    '  process.exit(32);',
    '}',
    'const sessionRoot = process.env.PWTEST_DAEMON_SESSION_DIR || expectedRoot;',
    'const tempRoot = process.env.TMPDIR === expectedTemp ? process.env.TMPDIR : expectedTemp;',
    'const profile = join(sessionRoot, "workspace-hash");',
    'const socketPath = join(process.env.PWTEST_SOCKETS_DIR || tempRoot, `fixture-${process.pid}.sock`);',
    'await mkdir(profile, { recursive: true });',
    'await mkdir(tempRoot, { recursive: true });',
    'const heldFile = await open(join(tempRoot, "chrome-profile.lock"), "w");',
    'await writeFile(pidFile, String(process.pid));',
    'await writeFile(socketFile, socketPath);',
    'const server = net.createServer((socket) => {',
    '  let input = "";',
    '  socket.on("data", async (bytes) => {',
    '    input += bytes;',
    '    const end = input.indexOf("\\n");',
    '    if (end < 0) return;',
    '    const request = JSON.parse(input.slice(0, end));',
    '    await writeFile(stopFile, JSON.stringify(request));',
    // The daemon acknowledges but keeps holding the private tree, so only a real kill can end it.
    '    socket.end(`${JSON.stringify({ id: request.id, result: "ok" })}\\n`);',
    '  });',
    '});',
    'await new Promise((resolve, reject) => server.listen(socketPath, resolve).once("error", reject));',
    'await writeFile(join(profile, "default.session"), JSON.stringify({ name: "default", version: "1.55.0", timestamp: Date.now(), socketPath, workspaceDir: "fixture", cli: { persistent: false }, browser: { browserName: "chromium", launchOptions: {} } }));',
    'await writeFile(readyFile, "ready");',
  ].join("\n"));
  return {
    program: process.execPath,
    prefix: [script, "provider"],
    args: [browserSessions, browserTemp, marker, daemonPid, envMarker, stopMarker, socketMarker, readyMarker, errorMarker, hold ? "hold" : "exit"],
    daemonPid,
    errorMarker,
    envMarker,
    stopMarker,
    socketMarker,
    readyMarker,
    browserSessions,
    browserTemp,
  };
}

// The Provider parent exits after launching one quiet child that inherits stdout and stderr.
async function providerCommand(root, marker) {
  if (process.platform === "win32") {
    const script = join(root, "provider.ps1");
    await writeFile(script, [
      "$process = New-Object System.Diagnostics.Process",
      "$process.StartInfo.FileName = Join-Path $PSHOME 'powershell.exe'",
      "$process.StartInfo.Arguments = '-NoLogo -NoProfile -Command Start-Sleep -Seconds 300'",
      "$process.StartInfo.UseShellExecute = $false",
      "if (-not $process.Start()) { exit 31 }",
      `$process.Id | Set-Content -NoNewline -LiteralPath '${quotePowerShell(marker)}'`,
      "Write-Output 'provider finished'",
    ].join("\r\n"));
    const powershell = join(process.env.SystemRoot || "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
    return { program: powershell, prefix: ["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script], args: [] };
  }
  const script = join(root, "provider");
  await writeFile(script, `#!/bin/sh\nsleep 300 &\nprintf '%s' "$!" > "$1"\nprintf '%s\\n' 'provider finished'\n`);
  await chmod(script, 0o700);
  return { program: script, prefix: [], args: [marker] };
}

// A short bound turns the production hang into direct evidence without leaving the test runner stuck.
async function exitWithin(child, milliseconds) {
  return new Promise((resolveExit, rejectExit) => {
    const deadline = setTimeout(
      () => rejectExit(new Error("native Work helper did not exit after Provider completion")),
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

// Marker reading waits only for the Provider's already-started child identity.
async function readPid(path) {
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline) {
    const value = await readFile(path, "utf8").catch(() => "");
    if (/^\d+$/.test(value)) return Number(value);
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
  return 0;
}

// A marker file appears only once the process it proves has reached that exact point.
async function readJsonWithin(path, milliseconds, parse = true) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    const value = await readFile(path, "utf8").catch(() => "");
    if (value) return parse ? JSON.parse(value) : value;
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
  return null;
}

// A process that must die gets a bounded window to do it before the proof calls it a survivor.
async function exitsWithin(pid, milliseconds) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    if (!isAlive(pid)) return true;
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
  return false;
}

// Process observation uses signal zero without changing a living process.
function isAlive(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

// Cleanup kills only the fixture Provider child when the old runner reproduces its leak.
async function killIfAlive(pid) {
  if (!isAlive(pid)) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
  } else {
    try { process.kill(pid, "SIGKILL"); } catch {}
  }
}

// PowerShell single-quoted literals escape only their delimiter.
function quotePowerShell(value) {
  return value.replaceAll("'", "''");
}
