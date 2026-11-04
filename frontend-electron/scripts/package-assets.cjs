// Run after tsc and the frontend build. Reject links and unknown resource types
// so private package files cannot enter the runtime handler's namespace.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { assetType, assetIndex } = require('../compiled/assets.js');
const source = path.resolve(__dirname, '../../frontend/dist');
const output = path.resolve(__dirname, '../compiled/ui');
const assets = [];
function walk(directory, prefix = '') {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    const relative = prefix + '/' + entry.name;
    if (entry.isSymbolicLink()) throw new Error('Symlink in frontend build: ' + relative);
    if (entry.isDirectory()) { walk(file, relative); continue; }
    const type = assetType(relative);
    if (!entry.isFile() || !type) throw new Error('Unsupported frontend build resource: ' + relative);
    const bytes = fs.readFileSync(file);
    assets.push({ path: relative, type, bytes: bytes.length, sha256: crypto.createHash('sha256').update(bytes).digest('hex') });
  }
}
walk(source);
assets.sort((a,b) => a.path.localeCompare(b.path));
const manifest = { format: 1, assets };
assetIndex(manifest);
fs.rmSync(output, { recursive: true, force: true });
fs.mkdirSync(path.join(output, 'files'), { recursive: true });
for (const asset of assets) {
  const target = path.join(output, 'files', asset.path.slice(1));
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.copyFileSync(path.join(source, asset.path.slice(1)), target);
}
fs.writeFileSync(path.join(output, 'manifest.json'), JSON.stringify(manifest) + '\n');
console.log('Packaged ' + assets.length + ' declared frontend resources.');
