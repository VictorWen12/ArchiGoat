// Release profiles validate only the public ArchiGoat build and signing boundary.

const fields = {
  PRODUCT_NAME: nonempty,
  ACCOUNT_URL: exactHttpsOrigin,
  RELEASE_FEED_ORIGIN: releaseFeedOrigin,
  ARTIFACT_ORIGIN: exactHttpsOrigin,
  ARCHIGOAT_BUNDLE_ID: value => /^(?:[a-z0-9]+\.)+[a-z0-9-]+$/u.test(value),
  ARCHIGOAT_URL_SCHEME: value => /^[a-z][a-z0-9+.-]*$/u.test(value),
  ARCHIGOAT_ASSET_STEM: value => /^[a-z0-9]+(?:-[a-z0-9]+)*$/u.test(value),
  MACOS_APP_IDENTITY: nonempty,
  APPLE_TEAM_ID: value => /^[A-Z0-9]{10}$/u.test(value),
};

// Signing bytes are read from the protected Actions environment and never committed.
const secrets = new Set([
  "MACOS_CERT_P12",
  "MACOS_CERT_PASSWORD",
  "APPLE_ID",
  "APPLE_APP_PASSWORD",
]);

export const profiles = {
  "archigoat-build": [
    "PRODUCT_NAME",
    "ACCOUNT_URL",
    "RELEASE_FEED_ORIGIN",
    "ARTIFACT_ORIGIN",
    "ARCHIGOAT_BUNDLE_ID",
    "ARCHIGOAT_URL_SCHEME",
    "ARCHIGOAT_ASSET_STEM",
  ],
  "archigoat-macos": [
    "PRODUCT_NAME",
    "ACCOUNT_URL",
    "RELEASE_FEED_ORIGIN",
    "ARTIFACT_ORIGIN",
    "ARCHIGOAT_BUNDLE_ID",
    "ARCHIGOAT_URL_SCHEME",
    "ARCHIGOAT_ASSET_STEM",
    "MACOS_APP_IDENTITY",
    "APPLE_TEAM_ID",
    "MACOS_CERT_P12",
    "MACOS_CERT_PASSWORD",
    "APPLE_ID",
    "APPLE_APP_PASSWORD",
  ],
};

// Validation returns field names only, so secret bytes never enter logs.
export function validate(profile, environment = process.env) {
  const required = profiles[profile];
  if (!required) return [`unknown profile ${profile}`];
  const invalid = [];
  for (const name of required) {
    const value = environment[name] ?? "";
    const valid = secrets.has(name) ? nonempty(value) : fields[name]?.(value);
    if (!valid) invalid.push(name);
  }
  return invalid;
}

function nonempty(value) {
  return typeof value === "string" && value.trim() === value && value.length > 0;
}

function exactHttpsOrigin(value) {
  try {
    const url = new URL(value);
    return url.protocol === "https:" && url.origin === value && url.pathname === "/" && !url.search && !url.hash;
  } catch {
    return false;
  }
}

function releaseFeedOrigin(value) {
  try {
    const url = new URL(value);
    return url.protocol === "https:" && url.hostname === "github.com" && !url.username && !url.password && !url.search && !url.hash && url.pathname.endsWith("/releases/latest/download");
  } catch {
    return false;
  }
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  const invalid = validate(process.argv[2]);
  if (invalid.length) {
    process.stderr.write(`Invalid production configuration: ${invalid.join(", ")}\n`);
    process.exitCode = 1;
  }
}
