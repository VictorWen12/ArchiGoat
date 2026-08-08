// Small, dependency-free byte-mode QR encoder for the short-lived pairing URL.
// Version 6 / M carries the Account URL plus its 64-byte offer with useful recovery.

const SIZE = 41;
const DATA_CODEWORDS = 108;
const BLOCKS = 4;
const BLOCK_DATA = 27;
const BLOCK_TOTAL = 43;
const ECC = BLOCK_TOTAL - BLOCK_DATA;
const ALIGNMENT = [6, 34];

const EXP = new Uint16Array(510);
const LOG = new Uint16Array(256);
for (let i = 0, value = 1; i < 255; i += 1) {
  EXP[i] = value;
  LOG[value] = i;
  value <<= 1;
  if (value & 0x100) value ^= 0x11d;
}
for (let i = 255; i < EXP.length; i += 1) EXP[i] = EXP[i - 255];

// Multiplies two QR finite-field bytes for error recovery.
function multiply(a: number, b: number): number {
  return a === 0 || b === 0 ? 0 : EXP[LOG[a] + LOG[b]];
}

// Builds the Reed-Solomon generator for one fixed block.
function generator(length: number): number[] {
  let result = [1];
  for (let i = 0; i < length; i += 1) {
    const next = new Array<number>(result.length + 1).fill(0);
    for (let j = 0; j < result.length; j += 1) {
      next[j] ^= result[j];
      next[j + 1] ^= multiply(result[j], EXP[i]);
    }
    result = next;
  }
  return result;
}

// Produces the recovery bytes that keep a damaged pairing code scannable.
function errorCorrection(data: number[]): number[] {
  const poly = generator(ECC);
  const working = [...data, ...new Array<number>(ECC).fill(0)];
  for (let i = 0; i < data.length; i += 1) {
    const factor = working[i];
    if (!factor) continue;
    for (let j = 0; j < poly.length; j += 1) working[i + j] ^= multiply(poly[j], factor);
  }
  return working.slice(-ECC);
}

// Encodes the pairing URL into the fixed byte-mode data capacity.
function bitsFor(value: string): number[] {
  const bytes = Array.from(new TextEncoder().encode(value));
  if (bytes.length > 91) throw new Error("Pairing URL is too long");
  const bits: number[] = [];
  const put = (number: number, length: number): void => {
    for (let bit = length - 1; bit >= 0; bit -= 1) bits.push((number >>> bit) & 1);
  };
  put(0b0100, 4); // byte mode
  put(bytes.length, 8); // version 6 uses an eight-bit byte count
  for (const byte of bytes) put(byte, 8);
  for (let i = 0; i < 4 && bits.length < DATA_CODEWORDS * 8; i += 1) bits.push(0);
  while (bits.length % 8 !== 0) bits.push(0);
  const data: number[] = [];
  for (let i = 0; i < bits.length; i += 8) data.push(bits.slice(i, i + 8).reduce((byte, bit) => (byte << 1) | bit, 0));
  let pad = 0xec;
  while (data.length < DATA_CODEWORDS) { data.push(pad); pad ^= 0xfd; }
  return data;
}

// Interleaves data and recovery blocks in scanner order.
function qrCodewords(value: string): number[] {
  const data = bitsFor(value);
  const blocks: number[][] = [];
  const corrections: number[][] = [];
  for (let block = 0; block < BLOCKS; block += 1) {
    const part = data.slice(block * BLOCK_DATA, (block + 1) * BLOCK_DATA);
    blocks.push(part);
    corrections.push(errorCorrection(part));
  }
  const result: number[] = [];
  for (let i = 0; i < BLOCK_DATA; i += 1) for (const block of blocks) result.push(block[i]);
  for (let i = 0; i < ECC; i += 1) for (const block of corrections) result.push(block[i]);
  return result;
}

// Protects the QR recovery-level and mask metadata.
function bchTypeInfo(data: number): number {
  const generator = 0x537;
  let value = data << 10;
  while (value.toString(2).length - generator.toString(2).length >= 0) {
    value ^= generator << (value.toString(2).length - generator.toString(2).length);
  }
  return ((data << 10) | value) ^ 0x5412;
}

// Mask zero breaks up scan-hostile solid regions.
function mask(row: number, col: number): boolean { return (row + col) % 2 === 0; }

// Places one finder target and its white separator.
function finder(modules: (boolean | null)[][], row: number, col: number): void {
  for (let r = -1; r <= 7; r += 1) {
    if (row + r < 0 || row + r >= SIZE) continue;
    for (let c = -1; c <= 7; c += 1) {
      if (col + c < 0 || col + c >= SIZE) continue;
      modules[row + r][col + c] = (r >= 0 && r <= 6 && (c === 0 || c === 6))
        || (c >= 0 && c <= 6 && (r === 0 || r === 6))
        || (r >= 2 && r <= 4 && c >= 2 && c <= 4);
    }
  }
}

// Places the version-six alignment targets used by angled phone scans.
function alignment(modules: (boolean | null)[][]): void {
  for (const row of ALIGNMENT) for (const col of ALIGNMENT) {
    if (modules[row][col] !== null) continue;
    for (let r = -2; r <= 2; r += 1) for (let c = -2; c <= 2; c += 1) {
      modules[row + r][col + c] = Math.max(Math.abs(r), Math.abs(c)) !== 1;
    }
  }
}

// Writes level-M recovery and mask-zero format bits.
function format(modules: (boolean | null)[][]): void {
  const bits = bchTypeInfo(0); // level M and mask 0
  for (let i = 0; i < 15; i += 1) {
    const dark = ((bits >>> i) & 1) === 1;
    if (i < 6) modules[i][8] = dark;
    else if (i < 8) modules[i + 1][8] = dark;
    else modules[SIZE - 15 + i][8] = dark;
    if (i < 8) modules[8][SIZE - i - 1] = dark;
    else if (i < 9) modules[8][15 - i] = dark;
    else modules[8][15 - i - 1] = dark;
  }
  modules[SIZE - 8][8] = true;
}

// Maps protected codewords through every unreserved module.
function map(modules: (boolean | null)[][], bytes: number[]): void {
  let row = SIZE - 1;
  let direction = -1;
  let bit = 7;
  let byte = 0;
  for (let col = SIZE - 1; col > 0; col -= 2) {
    if (col === 6) col -= 1;
    for (;;) {
      for (let side = 0; side < 2; side += 1) {
        const currentCol = col - side;
        if (modules[row][currentCol] !== null) continue;
        const dark = byte < bytes.length && ((bytes[byte] >>> bit) & 1) === 1;
        modules[row][currentCol] = dark !== mask(row, currentCol);
        bit -= 1;
        if (bit < 0) { byte += 1; bit = 7; }
      }
      row += direction;
      if (row < 0 || row >= SIZE) { row -= direction; direction = -direction; break; }
    }
  }
}

// QrModules returns the complete local pairing code without sharing its token.
export function qrModules(value: string): boolean[][] {
  const modules = Array.from({ length: SIZE }, () => Array<boolean | null>(SIZE).fill(null));
  finder(modules, 0, 0);
  finder(modules, SIZE - 7, 0);
  finder(modules, 0, SIZE - 7);
  alignment(modules);
  for (let i = 8; i < SIZE - 8; i += 1) {
    if (modules[i][6] === null) modules[i][6] = i % 2 === 0;
    if (modules[6][i] === null) modules[6][i] = i % 2 === 0;
  }
  format(modules);
  map(modules, qrCodewords(value));
  return modules.map((row) => row.map((cell) => cell === true));
}
