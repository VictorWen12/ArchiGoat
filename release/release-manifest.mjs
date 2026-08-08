// Emits and re-verifies the immutable release feed every installed macOS daemon parses.
// Contract source of truth: daemon/src/update/release.rs fetch() and daemon/src/api.rs.
import { readFileSync, writeFileSync } from "node:fs";

const [version, commit, macosName, macosSha, windowsName, output = "dist/release.json"] = process.argv.slice(2);
const stem = process.env.ARCHIGOAT_ASSET_STEM || "archigoat";
const api = readFileSync("daemon/src/api.rs", "utf8");
const constant = (name) => {
  const match = api.match(new RegExp(`const ${name}: u16 = (\\d+);`, "u"));
  if (!match) throw new Error(`daemon/src/api.rs does not declare ${name}`);
  return Number(match[1]);
};
const protocol = constant("PROTOCOL");
// The feed floor is the oldest daemon protocol admitted to this release, not the api.rs client floor:
// every installed daemon below it refuses the update forever, so it stays pinned at the 1.0 launch protocol.
const minProtocol = 15;

writeFileSync(output, `${JSON.stringify({
  version,
  commit,
  protocol,
  minProtocol,
  macosApp: { name: macosName, sha256: macosSha, signed: true },
  windows: { name: windowsName, sha256: "0".repeat(64), signed: false },
}, null, 2)}\n`);

const feed = JSON.parse(readFileSync(output, "utf8"));
const digest = (value) => typeof value === "string" && /^[0-9a-f]{64}$/u.test(value);
const failures = [];
if (feed.version !== version || !/^[0-9]+\.[0-9]+\.[0-9]+$/u.test(feed.version)) failures.push("version");
if (Number(feed.version.split(".")[0]) < 1) failures.push("version-floor");
if (feed.commit !== commit || !/^[0-9a-f]{40}$/u.test(feed.commit)) failures.push("commit");
if (feed.protocol !== protocol || feed.minProtocol !== minProtocol) failures.push("protocol");
if (feed.macosApp?.name !== `${stem}-macos.dmg` || feed.macosApp.name !== macosName) failures.push("macosApp.name");
if (feed.macosApp?.signed !== true) failures.push("macosApp.signed");
if (!digest(feed.macosApp?.sha256)) failures.push("macosApp.sha256");
if (feed.windows?.name !== "archigoat-windows-x64-setup.exe" || feed.windows.name !== windowsName) failures.push("windows.name");
if (feed.windows?.signed !== false) failures.push("windows.signed");
if (!digest(feed.windows?.sha256)) failures.push("windows.sha256");
if (failures.length) {
  process.stderr.write(`Release manifest would strand installed daemons: ${failures.join(", ")}\n`);
  process.exit(1);
}
