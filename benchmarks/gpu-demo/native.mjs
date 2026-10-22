/** Native CPU reference and native/WASI packet parity. Node standard library. */
import {spawn} from 'node:child_process';
import {readFile, readdir, writeFile, mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {dirname, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {samples} from './samples.mjs';
import {median, hash} from './measure.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const option = (key, fallback) => { const i = process.argv.indexOf(key); return i < 0 ? fallback : process.argv[i + 1]; };
const mode = process.argv[2] ?? 'bench';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

class Native {
  constructor() {
    this.child = spawn(resolve(option('--binary', resolve(root, 'target/gpu-demo/release/pagura-gpu-demo'))), [], {stdio: ['pipe', 'pipe', 'inherit']});
    this.chunks = []; this.length = 0; this.pending = null;
    this.child.stdout.on('data', bytes => {this.chunks.push(bytes);this.length += bytes.length;this.receive();});
    this.child.on('error', error => this.pending?.reject(error));
    this.child.on('exit', code => this.pending?.reject(new Error(`Native host exited: ${code}`)));
  }
  receive() {
    if (this.length < 4) return;
    const header = Buffer.alloc(4);let offset = 0;
    for (const chunk of this.chunks) { const n = Math.min(4 - offset, chunk.length); chunk.copy(header, offset, 0, n);offset += n;if (offset === 4) break; }
    const size = header.readUInt32LE();
    if (size > 68 * 1024 * 1024) {this.pending?.reject(new Error('Native frame too large'));this.close();return;}
    if (this.length < size + 4) return;
    const data = Buffer.concat(this.chunks);this.chunks = [];this.length = 0;
    const pending = this.pending;this.pending = null;
    try {
      if (!pending || data.length !== size + 4) throw new Error('Unexpected native frame');
      const n = data.readUInt32LE(4), meta = JSON.parse(data.subarray(8, 8 + n));
      if (!meta.ok) throw new Error(meta.error);
      pending.resolve({meta, data: data.subarray(8 + n)});
    } catch (error) {pending?.reject(error);}
  }
  request(bytes) {
    return new Promise((resolve, reject) => {
      if (this.pending) {reject(new Error('Concurrent native requests'));return;}
      this.pending = {resolve, reject};const header = Buffer.alloc(4);header.writeUInt32LE(bytes.length);
      this.child.stdin.write(header);this.child.stdin.write(bytes);
    });
  }
  open(pdf) {const bytes = Buffer.alloc(pdf.length + 4);bytes.writeUInt32LE(1);bytes.set(pdf, 4);return this.request(bytes);}
  render(op, dpi) {const bytes = Buffer.alloc(32);bytes.writeUInt32LE(op);bytes.writeUInt32LE(dpi, 8);bytes.writeUInt32LE(32, 12);return this.request(bytes);}
  close() {this.child.stdin.end();this.child.kill('SIGTERM');}
}

const native = new Native();
try {
  let report;
  if (mode === 'parity') {
    const browser = JSON.parse(await readFile(resolve(option('--browser-report', resolve(root, 'target/gpu-demo/hardware-tests.json'))), 'utf8'));
    if (browser.failures.length || browser.cases.some(c => !c.cpu_sha256 || !c.fallback && !c.program_sha256)) throw new Error('Rerun the browser sweep to obtain complete successful SHA-256 evidence');
    const paths = new Map();
    for (const file of await readdir(resolve(root, 'corpus/manifests'))) {
      if (!file.endsWith('.json')) continue;
      const manifest = JSON.parse(await readFile(resolve(root, 'corpus/manifests', file), 'utf8'));
      for (const c of manifest.cases) paths.set(c.id, c);
    }
    const failures = [];let packets = 0;
    for (const entry of browser.cases) {
      const c = paths.get(entry.id);
      await native.open(await readFile(resolve(root, c.path)));
      const cpu = await native.render(2, c.dpi ?? 72);
      if (hash(cpu.data) !== entry.cpu_hash || entry.cpu_sha256 && sha256(cpu.data) !== entry.cpu_sha256) failures.push(`${entry.id}: CPU bytes`);
      const packet = await native.render(3, c.dpi ?? 72);
      if (packet.meta.fallback !== entry.fallback) failures.push(`${entry.id}: admission`);
      if (entry.program_sha256) {
        if (sha256(packet.data) !== entry.program_sha256) failures.push(`${entry.id}: packet bytes`);
        packets++;
      }
    }
    report = {schema: 1, fixtures: browser.cases.length, packets, failures};
  } else if (mode === 'bench') {
    const inputs = samples.map(sample => ({label: sample.label, bytes: sample.make()}));
    for (let i = 3; i < process.argv.length; i++) if (process.argv[i] === '--pdf') inputs.push({label: `Document ${inputs.length - samples.length + 1}`, bytes: await readFile(resolve(process.argv[++i]))});
    const count = Number(option('--samples', '100')), warmups = Number(option('--warmups', '5'));
    const dpis = option('--dpis', '72,150,300').split(',').map(Number), cases = [];
    if (!Number.isInteger(count) || count < 1 || count > 1000 || !Number.isInteger(warmups) || warmups < 0 || warmups > 100
        || !dpis.length || dpis.some(dpi => !Number.isInteger(dpi) || dpi < 1 || dpi > 600)) throw new Error('Invalid native benchmark settings');
    for (const input of inputs) {
      await native.open(input.bytes);
      for (const dpi of dpis) {
        const cold = await native.render(2, dpi), digest = sha256(cold.data), times = [];
        for (let i = 0; i < warmups + count; i++) {
          const result = await native.render(2, dpi);
          if (sha256(result.data) !== digest) throw new Error('Native CPU output changed');
          if (i >= warmups) times.push(result.meta);
        }
        const sorted = times.map(t => t.engine_ms).sort((a, b) => a - b);
        const entry = {label: input.label, dpi, input_sha256: sha256(input.bytes), pixel_sha256: digest,
          first_cpu: cold.meta, median_ms: median(sorted), p95_ms: sorted[Math.ceil(count * .95) - 1], samples: times};
        cases.push(entry);console.log(`${input.label} ${dpi}: native CPU ${entry.median_ms.toFixed(3)} ms`);
      }
    }
    report = {schema: 1, timestamp: new Date().toISOString(), scope: 'Exploratory native CPU context, measured separately from browser hybrid rendering.', warmups, cases};
  } else throw new Error(`Unknown native mode: ${mode}`);
  const output = resolve(option('--output', resolve(root, `target/gpu-demo/native-${mode}.json`)));
  await mkdir(dirname(output), {recursive: true});await writeFile(output, JSON.stringify(report, null, 2) + '\n');
  console.log(mode === 'parity' ? report : `Evidence: ${output}`);
  if (report.failures?.length) process.exitCode = 1;
} finally {native.close();}
