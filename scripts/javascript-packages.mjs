import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

function packageDirectories(nodeModules) {
  if (!existsSync(nodeModules)) return [];
  const directories = [];
  for (const entry of readdirSync(nodeModules, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name === '.bin') continue;
    const path = join(nodeModules, entry.name);
    if (entry.name.startsWith('@')) {
      for (const scoped of readdirSync(path, { withFileTypes: true })) {
        if (scoped.isDirectory()) directories.push(join(path, scoped.name));
      }
    } else {
      directories.push(path);
    }
  }
  return directories;
}

/**
 * Reads the exact package manifests installed by the frozen pnpm workspace.
 * It deliberately does not call `pnpm licenses`: Corepack-only developer
 * setups have no global pnpm shim, while the installed dependency graph is
 * already the authoritative input for the Windows bundle.
 */
export function installedJavascriptPackages(root) {
  const store = resolve(root, 'node_modules', '.pnpm');
  if (!existsSync(store)) {
    throw new Error('pnpm dependencies are missing; run corepack pnpm install --frozen-lockfile');
  }
  const packages = new Map();
  for (const entry of readdirSync(store, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    for (const directory of packageDirectories(join(store, entry.name, 'node_modules'))) {
      const manifestPath = join(directory, 'package.json');
      if (!existsSync(manifestPath)) continue;
      const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
      if (typeof manifest.name !== 'string' || typeof manifest.version !== 'string') continue;
      const license = typeof manifest.license === 'string' ? manifest.license : 'NOASSERTION';
      const homepage = typeof manifest.homepage === 'string' ? manifest.homepage : null;
      packages.set(`${manifest.name}@${manifest.version}`, {
        name: manifest.name,
        version: manifest.version,
        license,
        homepage
      });
    }
  }
  return [...packages.values()].sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`)
  );
}
