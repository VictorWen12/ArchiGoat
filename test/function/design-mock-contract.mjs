import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);
const read = (path) => readFile(new URL(path, root), "utf8");

function section(source, start, end) {
  const match = source.match(new RegExp(`${start}[\\s\\S]*?(?=${end})`));
  assert.ok(match, `expected ${start} section`);
  return match[0];
}

test("present design/mock.png becomes the exact designMock carrier on delivery", async () => {
  const [local, delivery] = await Promise.all([
    read("daemon/src/local.rs"),
    read("daemon/src/account_relay/delivery.rs"),
  ]);
  const mock = section(local, "pub\\(crate\\) struct DesignMock", "// ConnectRequest");
  const request = section(delivery, "struct LocalDeliveryRequest", "struct DeliveryReceipt");

  assert.match(local, /join\("design"\)\.join\("mock\.png"\)/u,
    "delivery must inspect the fixed Work design/mock.png path");
  assert.match(local, /STANDARD\.encode\(bytes\)/u,
    "the present mock must use standard base64 bytes");
  assert.match(mock, /media:\s*&'static str[\s\S]*bytes:\s*String/u,
    "the carrier must contain only media and encoded bytes");
  assert.match(mock, /media:\s*"image\/png"/u,
    "the fixed mock media must be image/png");
  assert.match(request, /design_mock:\s*Option<[^>]+>/u,
    "the Account delivery request must carry an optional mock");
  assert.match(request, /skip_serializing_if\s*=\s*"Option::is_none"/u,
    "an absent mock must be omitted from JSON");
  assert.match(delivery, /\.json\(&LocalDeliveryRequest/u,
    "the existing Account deliver POST must remain the transport");
  assert.match(delivery, /LocalDeliveryRequest(?:<[^>]+>)?\s*\{[\s\S]*design_mock/u,
    "the mock must travel on the existing Account deliver POST");
});

test("absent design/mock.png is an optional no-op", async () => {
  const local = await read("daemon/src/local.rs");
  assert.match(local, /ErrorKind::NotFound[\s\S]*Ok\(None\)/u,
    "missing design/mock.png must produce no attachment and no delivery error");
});

test("the six design field names equal FRAMEWORK_FIELDS", async () => {
  const flow = await read("shell/src/creator-flow.ts");
  const match = flow.match(/const FRAMEWORK_FIELDS = \[([^\]]+)\]/u);
  assert.ok(match, "FRAMEWORK_FIELDS must remain the design field authority");
  const fields = [...match[1].matchAll(/"([^"]+)"/gu)].map((item) => item[1]);
  assert.deepEqual(fields, ["Mechanic", "Hook", "Looks", "Sound", "Effects", "Assumption"]);
});
