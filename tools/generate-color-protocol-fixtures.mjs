// Run pinned xterm.js colour handlers; no JavaScript is used by the application.
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { stripTypeScriptTypes } from 'node:module';

const paths = ['src/common/InputHandler.ts', 'src/browser/CoreBrowserTerminal.ts',
  'src/common/input/XParseColor.ts', 'src/common/Types.ts'];
const sources = await Promise.all(paths.map(async path => {
  const url = `https://raw.githubusercontent.com/xtermjs/xterm.js/6.0.0/${path}`;
  const text = process.argv[2]
    ? await readFile(join(process.argv[2], path.split('/').at(-1)), 'utf8')
    : await fetch(url, { signal: AbortSignal.timeout(30000) }).then(r => {
      if (!r.ok) throw new Error(`HTTP ${r.status}: ${url}`);
      return r.text();
    });
  return { url, text, sha256: createHash('sha256').update(text).digest('hex') };
}));
const method = (source, name) => {
  const match = source.match(new RegExp(`  (?:public|private) ${name}\\([^]*?\\n  \\}`));
  if (!match) throw new Error(`Missing method: ${name}`);
  return match[0];
};
const enums = ['ColorRequestType', 'SpecialColorIndex'].map(name =>
  sources[3].text.match(new RegExp(`export const enum ${name} \\{[^]*?\\n\\}`))[0]).join('\n');
const names = ['setOrReportIndexedColor', '_setOrReportSpecialColor', 'setOrReportFgColor',
  'setOrReportBgColor', 'setOrReportCursorColor', 'restoreIndexedColor',
  'restoreFgColor', 'restoreBgColor', 'restoreCursorColor'];
const script = `${enums}\n${sources[2].text}
const color = { toColorRGB: value => value };
const channels = { toColor: (...value) => value };
const C0 = { ESC: '\\x1b' }, C1_ESCAPED = { ST: '\\x1b\\\\' };
${sources[0].text.match(/export function isValidColorIndex[^]*?\n\}/)[0]}
export class Handler {
  _specialColors = [256, 257, 258];
  ${names.map(name => method(sources[0].text, name)).join('\n')}
  ${method(sources[1].text, '_handleColorEvent')}
}`;
const { Handler } = await import(`data:text/javascript;base64,${Buffer.from(
  stripTypeScriptTypes(script, { mode: 'transform' })).toString('base64')}`);

const ansi = Array.from({ length: 16 }, (_, i) => (i * 0x0b0d0f) & 0xffffff);
const rgb = value => [value >>> 16, value >>> 8 & 255, value & 255];
const indexed = i => {
  if (i < 16) return rgb(ansi[i]);
  if (i >= 232) return [8 + (i - 232) * 10].flatMap(v => [v, v, v]);
  const n = i - 16, level = v => v ? 55 + v * 40 : 0;
  return [level(Math.floor(n / 36)), level(Math.floor(n / 6) % 6), level(n % 6)];
};
const defaults = { foreground: rgb(0x102030), background: rgb(0xfafafa),
  cursor: rgb(0xff00ff), ansi: Array.from({ length: 256 }, (_, i) => indexed(i)) };
const inputs = [
  [[10, '?'], [11, '?'], [12, '?'], [4, '0;?;15;?;16;?;231;?;232;?;255;?']],
  [[4, '1;#112233;1;?;1;#abcdef;1;?']],
  [[10, '#123;?;?'], [10, '?;?;?']],
  [[11, '#111222333;#444455556666'], [11, '?;?']],
  [[4, '2;#112233;3;#abcdef'], [104, '2'], [4, '2;?;3;?'], [104, ''], [4, '3;?']],
  [[10, '#aabbcc'], [11, '#112233'], [12, '#778899'], [110, ''], [111, ''], [112, ''], [10, '?;?;?']],
  [[4, '256;#112233;999;?;-1;?;bad;?;1;?;orphan']],
  [[104, 'bad;256;-1;'], [10, '?;?;?']],
];
for (const value of ['rgb:f/e/d', 'rgb:abc/def/123', 'rgb:1234/5678/9abc',
  'RGB:ABC/DEF/123', '#aBcDeF', 'rgb:1/22/333', '#zzzzzz', '#1234', 'red']) {
  inputs.push([[10, value], [10, '?']]);
}
const methods = { 4: 'setOrReportIndexedColor', 10: 'setOrReportFgColor',
  11: 'setOrReportBgColor', 12: 'setOrReportCursorColor', 104: 'restoreIndexedColor',
  110: 'restoreFgColor', 111: 'restoreBgColor', 112: 'restoreCursorColor' };
const cases = inputs.map(commands => {
  const replies = [], handler = new Handler();
  handler._themeService = {
    colors: structuredClone(defaults),
    modifyColors: callback => callback(handler._themeService.colors),
    restoreColor: index => {
      const colors = handler._themeService.colors;
      if (index === undefined) colors.ansi = structuredClone(defaults.ansi);
      else if (index < 256) colors.ansi[index] = [...defaults.ansi[index]];
      else colors[['foreground', 'background', 'cursor'][index - 256]] =
        [...defaults[['foreground', 'background', 'cursor'][index - 256]]];
    }
  };
  handler.coreService = { triggerDataEvent: value => replies.push(value) };
  handler._onColor = { fire: event => handler._handleColorEvent(event) };
  for (const [code, value] of commands) handler[methods[code]](value);
  return { commands, replies };
});
await writeFile(new URL('../tests/fixtures/xterm-color-protocol.json', import.meta.url),
  JSON.stringify({ sources: sources.map(({ text, ...metadata }) => metadata),
    theme: { foreground: 0x102030, background: 0xfafafa, cursor: 0xff00ff, selection: 0x445566, ansi },
    cases }, null, 2) + '\n');
console.log(`Generated ${cases.length} colour protocol cases`);
