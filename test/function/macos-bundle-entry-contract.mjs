import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const release = new URL("../../release/", import.meta.url);
const shell = new URL("../../shell/src/", import.meta.url);

const readOptional = (url) => readFile(url, "utf8").catch(() => "");

test("the signed Tauri shell is the one macOS application executable", async () => {
  const [info, stage, pack, workflow, native, runtime, keepalive, updater, app, connections, connectionsStyles, local, voucher, packageLockText] = await Promise.all([
    readFile(new URL("Info.plist", release), "utf8"),
    readFile(new URL("stage-app-macos.sh", release), "utf8"),
    readFile(new URL("package-macos.sh", release), "utf8"),
    readFile(new URL("../../.github/workflows/release.yml", import.meta.url), "utf8"),
    readFile(new URL("../../shell/src-tauri/src/macos.rs", import.meta.url), "utf8"),
    readFile(new URL("../../daemon/src/runtime.rs", import.meta.url), "utf8"),
    readFile(new URL("../../daemon/src/keepalive.rs", import.meta.url), "utf8"),
    readFile(new URL("../../daemon/src/update/release.rs", import.meta.url), "utf8"),
    readFile(new URL("App.tsx", shell), "utf8"),
    readOptional(new URL("agent-connections.tsx", shell)),
    readOptional(new URL("agent-connections.css", shell)),
    readFile(new URL("../../daemon/src/local.rs", import.meta.url), "utf8"),
    readFile(new URL("../../daemon/src/local/voucher.rs", import.meta.url), "utf8"),
    readFile(new URL("../../shell/package-lock.json", import.meta.url), "utf8"),
  ]);
  const locked = JSON.parse(packageLockText).packages;

  assert.match(info, /<key>CFBundleExecutable<\/key>\s*<string>archigoat-shell<\/string>/u);
  for (const source of [stage, pack, workflow]) {
    assert.doesNotMatch(source, /archigoat-launcher|swiftc|macos-launcher\.swift/);
  }
  assert.match(pack, /CFBundleExecutable/);
  assert.match(pack, /archigoat-shell/);
  assert.match(pack, /CFBundleURLTypes\.0\.CFBundleURLSchemes -json '\[\]'/u,
    "packaging must replace any stale URL scheme array");
  assert.match(pack, /CFBundleURLTypes\.0\.CFBundleURLSchemes\.0 -string "[$]ARCHIGOAT_URL_SCHEME"/u,
    "the bundle must register the ArchiGoat scheme exactly once");
  assert.match(native, /install\(&source, &installed\)\?;[\s\S]*relaunch\(&installed\)\?/u,
    "a replaced bundle must relaunch the installed shell that owns its daemon");
  assert.match(native, /stderr\(Stdio::piped\(\)\)[\s\S]*read_to_end[\s\S]*failed with \{code\}.*detail/u,
    "native command failures must preserve stderr diagnostics");
  assert.match(native, /fn install_destination\b[\s\S]*if system\.exists\(\) \|\| source == canonical\(&system\)[\s\S]*system[\s\S]*else[\s\S]*user/u,
    "an existing system Applications bundle must be the one canonical install");
  assert.match(native, /retire_agents\(label\)[\s\S]*start_daemon\(&installed\)\?[\s\S]*wait_for_health\(\)\?/u,
    "the shell must retire obsolete agents, launch its child daemon, and prove readiness");
  assert.match(native, /fn start_daemon\b[\s\S]*Command::new\(&daemon\)[\s\S]*"--autostart"[\s\S]*spawn\(\)[\s\S]*child\.wait\(\)/u,
    "the shell must own and reap the daemon it starts");
  assert.match(native, /fn retire_agents\b[\s\S]*"bootout"[\s\S]*remove_file/u,
    "every launch must remove the obsolete hidden login item");
  assert.doesNotMatch(native, /start_agent|agent_plist|RunAtLoad|KeepAlive|"bootstrap"|"kickstart"/u,
    "the shell must never install or revive a hidden LaunchAgent");
  assert.match(runtime, /#\[cfg\(target_os = "macos"\)\][\s\S]*crate::keepalive::watch_parent\(\);/u,
    "the macOS daemon must always die with its shell parent");
  assert.doesNotMatch(runtime, /crate::keepalive::ensure\(\)/u,
    "the daemon must never restore persistence behind the shell");
  assert.doesNotMatch(keepalive, /launch_agent_plist|RunAtLoad|KeepAlive|"bootstrap"|"kickstart"|job_loaded|write_plist|launchd_started/u,
    "the daemon keepalive module must retain cleanup but no registration path");
  assert.doesNotMatch(updater, /keepalive::disabled|launchctl|kickstart/u,
    "parent-bound installs must still update without restoring launchd persistence");
  assert.match(updater, /Command::new\("\/bin\/sh"\)[\s\S]*kill -TERM[\s\S]*\/usr\/bin\/open[\s\S]*current\.app/u,
    "a verified update must replace the owning shell through the App bundle");
  assert.match(native, /127\.0\.0\.1:17891\/v1\/health[\s\S]*did not become healthy/u,
    "health failure must name the real daemon readiness problem");
  assert.match(pack, /verify_image[\s\S]*hdiutil verify[\s\S]*hdiutil attach[\s\S]*spctl --assess[\s\S]*\/v1\/health/u,
    "the final local DMG must be mounted and proven healthy before upload");
  assert.match(workflow, /name: Verify final mounted DMG[\s\S]*package-macos\.sh --verify[\s\S]*Build and verify release feed/u,
    "the release job must verify the final image before publishing its feed");
  assert.match(workflow, /gh release delete "[$]TAG" --yes[\s\S]*gh release create "[$]TAG"[\s\S]*--verify-tag/u,
    "publishing must replace any stale or draft record with one public release");
  assert.doesNotMatch(workflow, /gh release upload[\s\S]*--clobber/u,
    "a hidden draft must never be reused as a false-green release");
  for (const arch of ["arm64", "x64"]) {
    assert.equal(locked[`node_modules/@tauri-apps/cli-darwin-${arch}`]?.version, "2.6.2",
      `the clean lock must carry the Darwin ${arch} Tauri CLI binary`);
  }
  assert.match(connections, /export function AgentConnections\b/u);
  assert.match(connections, /ChatGPT[\s\S]*Claude[\s\S]*Cursor/u,
    "Connections must own all three Provider choices");
  assert.match(connections, /Connect your Agent/u);
  assert.match(connections, /Pair your device/u);
  assert.match(connections, /"Connect"/u,
    "every installed Provider must expose the connection action");
  assert.match(connections, /role="radiogroup"[\s\S]*type="radio"/u,
    "Providers must be one mutually exclusive horizontal selector");
  assert.equal(connections.match(/className="agent-connect-button"/gu)?.length, 1,
    "the selected Provider must expose exactly one Connect button");
  assert.match(connectionsStyles, /\.agent-provider-selector\s*\{[^}]*grid-template-columns:\s*repeat\(3,/iu,
    "all three Provider choices must stay on one row");
  assert.match(connectionsStyles, /\.agent-provider-selector\s*\{[^}]*background:\s*#fff/iu,
    "the Provider selector must stay white");
  assert.match(connectionsStyles, /\.agent-connect-button\s*\{[^}]*background:\s*#87b7eb/iu,
    "Connect must use ArchiGoat's Argentina blue");
  assert.doesNotMatch(connections, /Available|One active connection/u,
    "Connections must use actions, not weak availability commentary or counts");
  assert.doesNotMatch(connections, /Connect your Agent first/u,
    "device pairing must never depend on Agent connection");
  assert.match(connections, /AgentConnections\(\{ agent, device,/u,
    "device identity must enter Connections independently from Agent state");
  assert.match(app, /setDevice\(health\.device\)/u);
  assert.match(app, /<AgentConnections agent=\{agent\} device=\{device\}/u);
  assert.match(local, /struct Health\s*\{[\s\S]*device:\s*String/u,
    "native health must expose this Mac independently from Agent authorization");
  assert.match(voucher, /StatusCode::UNAUTHORIZED\s*=>\s*\{[\s\S]*retire\(state, credential\)[\s\S]*Authorization::Invalid/u,
    "a server-rejected installation credential must be retired locally");
  assert.match(voucher, /verify_shared\(state, voucher\)\.await[\s\S]*register_missing/u,
    "the same valid voucher must repair a retired installation without restart");
  assert.match(connections, /submitSignInCode[\s\S]*export function ConnectionsView/u,
    "Connections must own Provider sign-in and device pairing");
  assert.match(connectionsStyles, /\.agent-connections\b/u,
    "Connections must have a dedicated visual surface");
});
