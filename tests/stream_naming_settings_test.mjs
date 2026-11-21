import assert from 'node:assert/strict';
import test from 'node:test';
import { stripVTControlCharacters } from 'node:util';
import { createJiti } from 'jiti';
import { initTheme } from '@earendil-works/pi-coding-agent';
const jiti = createJiti(import.meta.url);
const { defaultConfigSnapshot, settingsDialog } = await jiti.import('../extensions/stream-checkpoint/settings.ts');
initTheme('dark');
const theme = {fg:(_color,text)=>text,bold:text=>text};
const down = '\x1b[B', enter = '\r', escape = '\x1b';
const turn = () => new Promise(resolve => setImmediate(resolve));

function fixture(run) {
 const snapshot = defaultConfigSnapshot();
 snapshot.naming = {stream: 'manual', session: 'automatic'};
 snapshot.pi.naming = {session: false, import_edits: true};
 const requests = [], notices = [];
 const actions = {
  setConfig: async (key, value) => {
   requests.push({kind: 'set_config', key, value});
   if (run) return run(key, value);
   if (key === 'naming.stream') snapshot.naming.stream = value;
   else if (key === 'naming.session') snapshot.naming.session = value;
   else if (key === 'pi.naming.import_edits') snapshot.pi.naming.import_edits = value === 'true';
   else if (key === 'pi.naming.session') snapshot.pi.naming.session = value === 'true';
   return structuredClone(snapshot);
  },
  unsetConfig: async () => structuredClone(snapshot),
 };
 return {snapshot, requests, notices, actions, ui: {notify: (text, level) => notices.push({text, level})}};
}

for (const custom of [false, true]) test(`${custom ? 'custom' : 'fallback'} Naming keeps generation, import and outgoing Pi sync independent`, async () => {
 const f = fixture();
 if (custom) f.ui.custom = async factory => {
  let closed = false;
  const component = factory({terminal:{rows:30}, requestRender(){}}, theme, {}, () => {closed = true;});
  for (let i=0; i<6; i++) component.handleInput(down); component.handleInput(enter);
  const text = () => stripVTControlCharacters(component.render(110).join('\n'));
  assert.match(text(), /naming\.stream\s+manual/); assert.match(text(), /naming\.session\s+automatic/);
  for (let i=0; i<4; i++) {component.handleInput(enter); await turn(); if (i < 3) component.handleInput(down);}
  component.handleInput(escape); assert.equal(closed, false); component.handleInput(escape); assert.equal(closed, true);
 };
 else {
  const choices = ['Naming', 'naming.stream: manual', 'naming.session: automatic', 'pi.naming.import_edits: true', 'pi.naming.session: false', 'Back', 'Close'];
  f.ui.select = async (title, rows) => {const choice = choices.shift(); assert.ok(rows.includes(choice), `${title}: ${rows}`); return choice;};
 }
 await settingsDialog({ui: f.ui}, f.snapshot, f.actions);
 assert.deepEqual(f.requests, [
  {kind: 'set_config', key: 'naming.stream', value: 'automatic'},
  {kind: 'set_config', key: 'naming.session', value: 'manual'},
  {kind: 'set_config', key: 'pi.naming.import_edits', value: 'false'},
  {kind: 'set_config', key: 'pi.naming.session', value: 'true'},
 ]);
 assert.deepEqual(f.notices, []);
});

for (const custom of [false, true]) test(`${custom ? 'custom' : 'fallback'} Naming restores failed edits and uses returned snapshot`, async () => {
 let failing = true;
 const f = fixture(async (key, value) => {
  if (failing) throw Error('settings backend unavailable');
  f.snapshot.naming.stream = value;
  f.snapshot.naming.session = 'manual';
  return structuredClone(f.snapshot);
 });
 if (custom) f.ui.custom = async factory => {
  const component = factory({terminal:{rows:30}, requestRender(){}}, theme, {}, () => {});
  for (let i=0; i<6; i++) component.handleInput(down); component.handleInput(enter);
  const text = () => stripVTControlCharacters(component.render(110).join('\n'));
  component.handleInput(enter); await turn(); assert.match(text(), /naming\.stream\s+manual/);
  failing = false; component.handleInput(enter); await turn();
  assert.match(text(), /naming\.stream\s+automatic/); assert.match(text(), /naming\.session\s+manual/);
  component.handleInput(escape); component.handleInput(escape);
 };
 else {
  const choices = ['Naming', 'naming.stream: manual', 'naming.stream: manual', 'Back', 'Close']; let calls = 0;
  f.ui.select = async (_title, rows) => {if (++calls === 3) failing = false; if (calls === 4) assert.ok(rows.includes('naming.session: manual')); const choice = choices.shift(); assert.ok(rows.includes(choice)); return choice;};
 }
 await settingsDialog({ui: f.ui}, f.snapshot, f.actions);
 assert.equal(f.notices.length, 1); assert.match(f.notices[0].text, /settings backend unavailable/); assert.equal(f.notices[0].level, 'error');
 assert.equal(f.requests.length, 2);
});

test('custom Naming displays an in-flight save and admits one action at a time', async () => {
 let release;
 const f = fixture(async (_key, value) => new Promise(resolve => {release = () => {f.snapshot.naming.stream = value; resolve(structuredClone(f.snapshot));};}));
 f.ui.custom = async factory => {
  let closed = false;
  const component = factory({terminal:{rows:30}, requestRender(){}}, theme, {}, () => {closed = true;});
  for (let i=0; i<6; i++) component.handleInput(down); component.handleInput(enter); component.handleInput(enter);
  assert.match(stripVTControlCharacters(component.render(110).join('\n')), /saving…/);
  component.handleInput(down); component.handleInput(enter); component.handleInput(escape);
  assert.equal(f.requests.length, 1); assert.equal(closed, false);
  release(); await turn();
  assert.match(stripVTControlCharacters(component.render(110).join('\n')), /naming\.stream\s+automatic/);
  component.handleInput(escape); component.handleInput(escape); assert.equal(closed, true);
 };
 await settingsDialog({ui: f.ui}, f.snapshot, f.actions);
});

test('Naming descriptions explain effective policy and Pi name directions without herdr rows', async () => {
 const f = fixture();
 await settingsDialog({ui:{custom:async factory => {
  const component = factory({terminal:{rows:16}, requestRender(){}}, theme, {}, () => {});
  for (let i=0; i<6; i++) component.handleInput(down); component.handleInput(enter);
  const renderedAll = [];
  for (const description of [
   'Whether automatic reports may propose Stream titles.',
   'Whether automatic reports may propose Session titles.',
   'Save changed Pi /name edits as manual Session titles.',
   'Mirror the saved Session title in Pi.',
  ]) {
   const lines = component.render(100); assert.ok(lines.length <= 16, lines.join('\n'));
   renderedAll.push(lines.join(' '));
   assert.ok(lines.join(' ').includes(description), lines.join('\n'));
   component.handleInput(down);
  }
  assert.doesNotMatch(renderedAll.join(' '), /herdr\./);
  component.handleInput(escape); component.handleInput(escape);
 }}}, f.snapshot, f.actions);
});
