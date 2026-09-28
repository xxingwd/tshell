// Execute the pinned xterm clipboard addon with its real Base64 dependency.
import { stripTypeScriptTypes } from 'node:module';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
const urls = [
  'https://raw.githubusercontent.com/xtermjs/xterm.js/6.0.0/addons/addon-clipboard/src/ClipboardAddon.ts',
  'https://cdn.jsdelivr.net/npm/js-base64@3.7.7/base64.mjs'
];
const sources = await Promise.all(urls.map(async url => {
  // An optional source directory supports hosts that require a system proxy.
  const text = process.argv[2]
    ? await readFile(join(process.argv[2], new URL(url).pathname.split('/').at(-1)), 'utf8')
    : await fetch(url, { signal: AbortSignal.timeout(30000) }).then(response => {
      if (!response.ok) throw new Error(`HTTP ${response.status}: ${url}`);
      return response.text();
    });
  return { url, text, sha256: createHash('sha256').update(text).digest('hex') };
}));
const moduleUrl = text => `data:text/javascript;base64,${Buffer.from(text).toString('base64')}`;
const source = sources[0].text.replace(/^import .*;\r?\n/gm, '');
const { ClipboardAddon } = await import(moduleUrl(stripTypeScriptTypes(
  `import { Base64 as JSBase64 } from '${moduleUrl(sources[1].text)}';\n${source}`,
  { mode: 'transform' }
)));
const cases = [];
for (const payload of [
  'c;aGk=', 'c;' + Buffer.from('中文🙂\nline\t2').toString('base64'),
  'c;', 'c;!!!', 'c;aGk', 'c;aGk= ', 'c;/w==', 'c;aGl=',
  'c;aGk=;ignored', 'p;aGk=', 's;aGk=', 'cp;aGk=', 'c'
]) {
  const writes = [];
  const addon = new ClipboardAddon(undefined, {
    writeText: (selection, text) => { if (selection === 'c') writes.push(text); }
  });
  let handler;
  addon.activate({ parser: { registerOscHandler: (_, callback) => { handler = callback; } } });
  await handler(payload);
  cases.push({ payload, writes });
}
await writeFile(new URL('../tests/fixtures/xterm-clipboard.json', import.meta.url), JSON.stringify({
  sources: sources.map(({ text, ...metadata }) => metadata), cases
}, null, 2) + '\n');
