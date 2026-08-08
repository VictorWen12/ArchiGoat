// Packs the resized product logo into Apple's PNG-backed ICNS container.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [source, output] = process.argv.slice(2);
if (!source || !output) throw new Error("icon source and output are required");

const formats = [["icp4", 16], ["icp5", 32], ["icp6", 64], ["ic07", 128], ["ic08", 256], ["ic09", 512], ["ic10", 1024]];
const chunk = (type, payload) => {
  const header = Buffer.alloc(8);
  header.write(type, 0, "ascii");
  header.writeUInt32BE(payload.length + header.length, 4);
  return Buffer.concat([header, payload]);
};
const body = Buffer.concat(formats.map(([type, size]) => chunk(type, readFileSync(join(source, `${size}.png`)))));
writeFileSync(output, chunk("icns", body));
