import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { installedJavascriptPackages } from './javascript-packages.mjs';

const root = resolve(import.meta.dirname, '..');
const outputPath = resolve(root, 'LICENSES', 'THIRD_PARTY_NOTICES.md');
const checkOnly = process.argv.includes('--check');
const cargoExecutable = process.platform === 'win32' ? 'cargo.exe' : 'cargo';

function command(executable, arguments_) {
  return execFileSync(executable, arguments_, {
    cwd: root,
    encoding: 'utf8',
    shell: false,
    maxBuffer: 32 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit']
  });
}

function rustPackages() {
  const metadata = JSON.parse(
    command(cargoExecutable, ['metadata', '--locked', '--format-version', '1'])
  );
  return metadata.packages
    .filter((pkg) => pkg.source !== null)
    .map((pkg) => ({
      name: pkg.name,
      version: pkg.version,
      license: pkg.license ?? 'NOASSERTION',
      source: pkg.source
    }))
    .sort((left, right) =>
      `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`)
    );
}

function javascriptPackages() {
  return installedJavascriptPackages(root);
}

function render() {
  const rust = rustPackages();
  const javascript = javascriptPackages();
  const lines = [
    '# Third-party notices',
    '',
    'Generated from the locked Rust and JavaScript dependency graphs. Do not edit by hand;',
    'run `pnpm licenses:generate` after changing either lockfile.',
    '',
    '## Rust dependencies',
    ''
  ];

  for (const pkg of rust) {
    lines.push(`- ${pkg.name} ${pkg.version} — ${pkg.license} — ${pkg.source}`);
  }

  lines.push('', '## JavaScript dependencies', '');
  for (const pkg of javascript) {
    const homepage = pkg.homepage ? ` — ${pkg.homepage}` : '';
    lines.push(`- ${pkg.name} ${pkg.version} — ${pkg.license}${homepage}`);
  }
  lines.push('');
  return `${lines.join('\n')}\n`;
}

const generated = render();
if (checkOnly) {
  const committed = readFileSync(outputPath, 'utf8');
  if (committed !== generated) {
    console.error('LICENSES/THIRD_PARTY_NOTICES.md is stale; run pnpm licenses:generate.');
    process.exit(1);
  }
  console.log('Third-party notice inventory is current.');
} else {
  writeFileSync(outputPath, generated, 'utf8');
  console.log('Generated LICENSES/THIRD_PARTY_NOTICES.md.');
}
