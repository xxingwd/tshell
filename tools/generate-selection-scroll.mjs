// Execute the pinned upstream method; JavaScript is only a fixture generator.
import { writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { stripTypeScriptTypes } from 'node:module';
const url = 'https://raw.githubusercontent.com/xtermjs/xterm.js/6.0.0/src/browser/services/SelectionService.ts';
const response = await fetch(url);
if (!response.ok) throw new Error(`HTTP ${response.status}`);
const source = await response.text();
const method = source.match(/private _getMouseEventScrollAmount\(event: MouseEvent\): number \{([\s\S]*?)\n  \}/)[1];
const constants = source.match(/const DRAG_SCROLL_\w+ = \d+;/g).join('\n');
const script = `${constants}
const getCoordsRelativeToElement = (_, event) => [0, event.y];
export function amount(event) { ${method} }`;
const { amount } = await import(`data:text/javascript;base64,${Buffer.from(stripTypeScriptTypes(script)).toString('base64')}`);
const context = { _coreBrowserService: { window: {} }, _screenElement: {}, _renderService: { dimensions: { css: { canvas: { height: 100 } } } } };
const cases = [-100, -50, -25, -12.5, -2, -0.1, 0, 50, 100, 100.1, 102, 112.5, 125, 150, 200].map(y => ({ y, amount: amount.call(context, { y }) }));
await writeFile(new URL('../tests/fixtures/xterm-selection-scroll.json', import.meta.url), JSON.stringify({ url, sha256: createHash('sha256').update(source).digest('hex'), interval: 50, cases }, null, 2) + '\n');
