import { build } from 'esbuild';
import { mkdir, writeFile, readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { dirname, resolve } from 'node:path';

const output = new URL('../../assets/preview/', import.meta.url);
const licenses = new Map();
await mkdir(output, { recursive: true });
for (const [name, entry] of [
  ['mermaid', 'import mermaid from "mermaid"; window.tshellMermaid = mermaid;'],
]) {
  const result = await build({
    stdin: { contents: entry, resolveDir: import.meta.dirname },
    bundle: true, minify: true, format: 'iife', platform: 'browser',
    target: 'chrome120', legalComments: 'linked', write: false,
    outfile: `${name}.js`,
    absWorkingDir: import.meta.dirname, metafile: true,
  });
  for (const file of result.outputFiles) {
    const filename = file.path.split(/[\\/]/).at(-1);
    const contents = filename.endsWith('.LEGAL.txt')
      ? Buffer.from(file.text.replace(/[ \t]+$/gm, '').trimEnd() + '\n')
      : file.contents;
    await writeFile(new URL(filename, output), contents);
    console.log(filename, contents.length, createHash('sha256').update(contents).digest('hex'));
  }
  for (const input of Object.keys(result.metafile.inputs)) {
    if (input === '<stdin>') continue;
    let directory = dirname(resolve(import.meta.dirname, input));
    while (directory !== dirname(directory)) {
      try {
        const pkg = JSON.parse(await readFile(resolve(directory, 'package.json'), 'utf8'));
        if (!pkg.name || !pkg.version) { directory = dirname(directory); continue; }
        const key = `${pkg.name}@${pkg.version}`;
        if (!licenses.has(key)) {
          for (const filename of ['LICENSE', 'LICENSE.md', 'LICENSE.txt', 'license', 'license.md', 'license.txt', 'License', 'LICENCE']) {
            try {
              licenses.set(key, await readFile(resolve(directory, filename), 'utf8'));
              break;
            } catch { /* Try the package's next license filename. */ }
          }
          if (!licenses.has(key)) throw new Error(`Missing license for ${key}`);
        }
        break;
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
        directory = dirname(directory);
      }
    }
  }
}
await writeFile(new URL('THIRD_PARTY_LICENSES.txt', output), [...licenses].sort(([a], [b]) => a.localeCompare(b)).map(([name, license]) => `${name}\n${'='.repeat(72)}\n${license}\n`).join('\n').trimEnd() + '\n');
for (const [pkg, name] of [['mermaid', 'mermaid']]) {
  await writeFile(new URL(`${name}.LICENSE`, output), await readFile(new URL(`node_modules/${pkg}/LICENSE`, import.meta.url)));
}
