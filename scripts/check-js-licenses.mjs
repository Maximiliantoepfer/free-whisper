import { resolve } from 'node:path';

import { installedJavascriptPackages } from './javascript-packages.mjs';

const root = resolve(import.meta.dirname, '..');
const licenses = [...new Set(installedJavascriptPackages(root).map((pkg) => pkg.license))].sort();
const allowed = new Set([
  'MIT',
  'Apache-2.0',
  'Apache-2.0 OR MIT',
  'ISC',
  'BSD-3-Clause'
]);
const rejected = licenses.filter((license) => !allowed.has(license));

if (rejected.length > 0) {
  console.error(`Disallowed frontend dependency licenses: ${rejected.join(', ')}`);
  process.exit(1);
}

console.log(`Frontend dependency license policy passed: ${licenses.join(', ')}`);
