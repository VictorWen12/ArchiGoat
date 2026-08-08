import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);

function jobSection(workflow, job) {
  const start = workflow.indexOf(`\n  ${job}:\n`);
  assert.notEqual(start, -1, `${job} job must exist`);
  const body = workflow.slice(start + 1);
  const nextJob = body.search(/\n  [A-Za-z0-9_-]+:\n/u);
  return nextJob === -1 ? body : body.slice(0, nextJob);
}

test("Windows candidate stays current-user, reproducible, and unpublished", async () => {
  const [configText, packageText, lockText, workflow, manifest] = await Promise.all([
    readFile(new URL("shell/src-tauri/tauri.conf.json", root), "utf8"),
    readFile(new URL("shell/package.json", root), "utf8"),
    readFile(new URL("shell/package-lock.json", root), "utf8"),
    readFile(new URL(".github/workflows/release.yml", root), "utf8"),
    readFile(new URL("release/release-manifest.mjs", root), "utf8"),
  ]);
  const config = JSON.parse(configText);
  const packageJson = JSON.parse(packageText);
  const lock = JSON.parse(lockText);
  const windowsJob = jobSection(workflow, "windows-candidate");

  assert.ok(config.bundle.targets.includes("nsis"));
  assert.ok(config.bundle.icon.includes("icons/icon.ico"));
  assert.equal(config.bundle.windows?.nsis?.installMode, "currentUser");

  assert.equal(packageJson.optionalDependencies["@tauri-apps/cli-win32-x64-msvc"], "2.6.2");
  assert.equal(lock.packages[""].optionalDependencies["@tauri-apps/cli-win32-x64-msvc"], "2.6.2");
  assert.equal(lock.packages["node_modules/@tauri-apps/cli-win32-x64-msvc"]?.version, "2.6.2");

  assert.match(workflow, /^  workflow_dispatch:\n    inputs:\n      version:/mu);
  assert.match(windowsJob, /github\.event_name == 'workflow_dispatch'/u);
  assert.match(windowsJob, /MANUAL_VERSION: \$\{\{ inputs\.version \}\}/u);
  assert.match(windowsJob, /GITHUB_EVENT_NAME -eq 'workflow_dispatch'/u);
  assert.match(windowsJob, /runs-on: windows-latest/u);
  assert.match(windowsJob, /x86_64-pc-windows-msvc/u);
  assert.match(windowsJob, /tauri .*build .*--bundles nsis/u);
  assert.match(windowsJob, /archigoat-windows-x64-setup\.exe/u);
  assert.match(windowsJob, /actions\/upload-artifact@v4/u);
  assert.match(windowsJob, /rustc .*test\/function\/windows-runner-liveness\.rs/u);
  assert.match(windowsJob, /ArgumentList @\('verify', '\/pa', '\/v'/u);
  assert.match(windowsJob, /Start-Process[\s\S]*-Wait -PassThru/u);
  assert.match(windowsJob, /\.ExitCode/u);
  assert.doesNotMatch(windowsJob, /& \$signtool/u);
  assert.match(windowsJob, /\$\{env:ProgramFiles\(x86\)\}/u);
  assert.doesNotMatch(windowsJob, /\$env:ProgramFiles\(x86\)/u);
  assert.match(windowsJob, /Get-AuthenticodeSignature/u);
  assert.doesNotMatch(windowsJob, /gh release/u);
  assert.match(manifest, /windows-x64-setup\.exe/u);
  assert.doesNotMatch(manifest, /windows-x64\.msi/u);
  assert.match(manifest, /sha256: "0"\.repeat\(64\), signed: false/u);

  const publish = workflow.indexOf("gh release");
  const verify = workflow.indexOf("Get-AuthenticodeSignature");
  assert.ok(publish > verify, "Authenticode verification must precede every publish step");
});
