// Development-only oracle. Executes upstream xterm.js 6.0.0 code, not a port of
// our Rust implementation. Node 24+; no package install and no JS app runtime.
import { stripTypeScriptTypes } from 'node:module';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
const version = '6.0.0';
const files = ['src/common/input/Keyboard.ts', 'src/browser/Clipboard.ts',
  'src/common/Color.ts', 'src/common/buffer/Constants.ts',
  'addons/addon-webgl/src/TextureAtlas.ts', 'addons/addon-webgl/src/Constants.ts'];
const sources = await Promise.all(files.map(async path => {
  const url = `https://raw.githubusercontent.com/xtermjs/xterm.js/${version}/${path}`;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${response.status}: ${url}`);
  const text = await response.text();
  return { path, url, text, sha256: createHash('sha256').update(text).digest('hex') };
}));
const text = path => sources.find(s=>s.path.endsWith(path)).text;
const withoutImports = source => source.replace(/^import .*?;\r?\n/gm,'');
async function compile(source) {
  const js = stripTypeScriptTypes(source,{mode:'transform'});
  return import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
}
const keyboard = await compile(`const KeyboardResultType={SEND_KEY:0,PAGE_UP:1,PAGE_DOWN:2,SELECT_ALL:3};
const C0={ESC:'\\x1b',DEL:'\\x7f',HT:'\\t',CR:'\\r',NUL:'\\0',FS:'\\x1c',GS:'\\x1d',US:'\\x1f'};
${withoutImports(text('Keyboard.ts'))}`);
const clipboard = await compile(withoutImports(text('Clipboard.ts')));
const keys = {backspace:8,tab:9,enter:13,escape:27,left:37,right:39,up:38,down:40,
  insert:45,delete:46,home:36,end:35,pageup:33,pagedown:34,
  ...Object.fromEntries(Array.from({length:12},(_,i)=>[`f${i+1}`,112+i]))};
const keyCases=[];
for(const app_cursor of [false,true]) for(const [key,keyCode] of Object.entries(keys)) for(let mask=0;mask<8;mask++) {
  const event={key,keyCode,shiftKey:!!(mask&1),altKey:!!(mask&2),ctrlKey:!!(mask&4),metaKey:false};
  const result=keyboard.evaluateKeyboardEvent(event,app_cursor,false,false);
  keyCases.push({key:[event.ctrlKey&&'ctrl',event.altKey&&'alt',event.shiftKey&&'shift',key].filter(Boolean).join('-'), app_cursor, bytes:result.key??null});
}
const paste=[];
for(const value of ['', 'a\nb\r\nc\rd', '中文🙂', '\x1b[31mred\x1b[0m', '\x1b[201~']) for(const bracketed of [false,true]) {
  paste.push({text:value,bracketed,bytes:clipboard.bracketTextForPaste(clipboard.prepareTextForTerminal(value),bracketed)});
}
// Run the original WebGL foreground resolver. DOM/canvas are not needed to
// resolve colour. Stub only the disabled minimum-contrast option (default=1).
const atlas=text('TextureAtlas.ts');
const start=atlas.indexOf('  private _getForegroundColor(');
const end=atlas.indexOf('\n  private ',start+10);
if(start<0||end<0) throw new Error('Upstream foreground resolver changed');
const opacity=text('addon-webgl/src/Constants.ts').match(/export const DIM_OPACITY = ([\d.]+);/)[1];
const colorCode=withoutImports(text('Color.ts'));
const constants=withoutImports(text('buffer/Constants.ts'));
const resolver=await compile(`${colorCode}\n${constants}
const DIM_OPACITY=${opacity};
const AttributeData={toColorRGB:(v:number)=>[v>>>16&255,v>>>8&255,v&255]};
export class Probe {
 constructor(public _config:any) {}
 _getMinimumContrastColor(){return undefined;}
 _getColorFromAnsiIndex(i:number){return this._config.colors.ansi[i];}
 ${atlas.slice(start,end)}
}`);
const colors=[];
const ansi=Array.from({length:256},(_,i)=> i<16 ? [0x182030,0x9a2030,0x208040,0x908020,0x3050a0,0x804090,0x208090,0xc0c8d0,0x586070,0xfa6070,0x60d080,0xe0d050,0x7090e0,0xc080d0,0x60c0d0,0xffffff][i] : i<232 ? (()=>{const n=i-16;const c=v=>v===0?0:55+40*v;return c(Math.floor(n/36))*65536+c(Math.floor(n/6)%6)*256+c(n%6);})() : (8+(i-232)*10)*0x010101);
const asColor=rgb=>resolver.channels.toColor(rgb>>>16&255,rgb>>>8&255,rgb&255);
const specs=[['default',0,0],['red',0x1000000,1],['indexed-red',0x2000000,1],['bright-red',0x1000000,9],['blue',0x1000000,4],['indexed-blue',0x2000000,4],['bright-blue',0x1000000,12],['cube',0x2000000,130],['rgb',0x3000000,0x204060]];
for(const background of [0xffffff,0x101622]) for(const [name,mode,value] of specs) for(const bold of [false,true]) for(const faint of [false,true]) for(const inverse of [false,true]) {
  const foreground=background===0xffffff?0x273244:0xdbe4f0;
  const probe=new resolver.Probe({allowTransparency:true,drawBoldTextInBrightColors:true,colors:{foreground:asColor(foreground),background:asColor(background),ansi:ansi.map(asColor)}});
  // TextureAtlas swaps foreground/background slots before calling the resolver.
  const resolved=probe._getForegroundColor(0,0,0,0,inverse?0:mode,inverse?0:value,inverse,faint,bold,false);
  colors.push({name,mode,value,bold,faint,inverse,foreground,background,rgb:resolved.rgba>>>8,alpha:resolved.rgba&255});
}
await mkdir('tests/fixtures',{recursive:true});
await writeFile('tests/fixtures/xterm-6.json',JSON.stringify({version,sources:sources.map(({text,...meta})=>meta),ansi,keys:keyCases,paste,colors},null,2)+'\n');
console.log(`Generated ${keyCases.length} key, ${paste.length} paste, ${colors.length} colour cases from xterm.js ${version}`);
