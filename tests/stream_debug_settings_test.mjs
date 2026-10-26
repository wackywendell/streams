import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createJiti } from 'jiti';
import { stripVTControlCharacters } from 'node:util';
import { initTheme } from '@earendil-works/pi-coding-agent';
const jiti = createJiti(import.meta.url);
const { defaults, loadSettings, saveSettings, validateSettings, settingsDialog: showSettingsDialog } = await jiti.import(new URL('../extensions/stream-checkpoint/settings.ts', import.meta.url).pathname);
const settingsDialog=(ctx,settings,save)=>showSettingsDialog(ctx,settings,save,{defaults:{stream:'automatic',session:'automatic'},save:assert.fail});
initTheme('dark');
const theme = {fg: (_color, text) => text, bold: text => text};
const down = '\x1b[B', enter = '\r', escape = '\x1b';

function temporarySettings(run) {
 const root = mkdtempSync(join(tmpdir(), 'stream-debug-settings-'));
 try {run(root, join(root, 'stream-reporting.json'));} finally {rmSync(root, {recursive:true, force:true});}
}

test('debug commands default off without changing legacy helper logging opt-in', () => {
 temporarySettings((root, path) => {
  assert.equal(defaults.debug_commands, false);assert.equal(defaults.session_logging, false);
  assert.deepEqual(loadSettings(root), defaults);
  for (const legacy of [{reporting:'manual'}, {session_logging:false}, {session_logging:true}]) {
   writeFileSync(path, JSON.stringify(legacy));
   const loaded = loadSettings(root);
   assert.equal(loaded.debug_commands, false);assert.equal(loaded.session_logging, legacy.session_logging ?? false);
   assert.equal(readFileSync(path, 'utf8'), JSON.stringify(legacy), 'loading must not rewrite legacy files');
  }
 });
});

test('debug commands require a boolean on disk and before saving', () => {
 temporarySettings((root, path) => {
  for (const debug_commands of [null, 'enabled', 0, 1, {}, []]) {
   const bytes = JSON.stringify({...defaults, debug_commands});writeFileSync(path, bytes);
   assert.throws(() => loadSettings(root), /Debug commands must be a boolean/);
   assert.throws(() => validateSettings({...defaults, debug_commands}), /Debug commands must be a boolean/);
   assert.throws(() => saveSettings(root, {...defaults, debug_commands}), /Debug commands must be a boolean/);
   assert.equal(readFileSync(path, 'utf8'), bytes);
  }
 });
});

test('debug and logging round-trip independently as top-level booleans preserving unknown disk keys', () => {
 temporarySettings((root, path) => {
  writeFileSync(path, JSON.stringify({future:{version:1}}));
  for (const debug_commands of [true, false]) for (const session_logging of [false, true]) {
   const loaded = loadSettings(root);
   writeFileSync(path, JSON.stringify({...loaded, future:{version:2}, concurrent:'keep'}));
   saveSettings(root, {...loaded, debug_commands, session_logging});
   const disk = JSON.parse(readFileSync(path, 'utf8'));
   assert.equal(disk.debug_commands, debug_commands);assert.equal(disk.session_logging, session_logging);
   assert.equal(disk.debug, undefined);assert.deepEqual(disk.future, {version:2});assert.equal(disk.concurrent, 'keep');
   assert.deepEqual(loadSettings(root), disk);
  }
 });
});

test('fallback Debug subsection saves independent toggles and returns to main settings', async () => {
 const saved = [], steps = [
  ['Work card & Herdr settings', 'Debug'], ['Debug', 'Debug commands: disabled'],
  ['Debug', 'Helper session logging: disabled'], ['Debug', 'Debug commands: enabled'],
  ['Debug', 'Back'], ['Work card & Herdr settings', 'Debug'],
  ['Debug', undefined], ['Work card & Herdr settings', 'Close'],
 ];
 await settingsDialog({ui:{notify:text => assert.fail(text), select:async (title, rows) => {
  const [expectedTitle, choice] = steps.shift();assert.equal(title, expectedTitle);
  if (title === 'Debug') assert.deepEqual(rows, [
   `Debug commands: ${saved.at(-1)?.debug_commands ? 'enabled' : 'disabled'}`,
   `Helper session logging: ${saved.at(-1)?.session_logging ? 'enabled' : 'disabled'}`, 'Back',
  ]);
  else {assert.ok(rows.includes('Debug'));assert.ok(rows.every(row => !/^(Debug commands|Helper session logging):/.test(row)));}
  if (choice !== undefined) assert.ok(rows.includes(choice));return choice;
 }}}, {...defaults}, value => saved.push(structuredClone(value)));
 assert.equal(steps.length, 0);
 assert.deepEqual(saved.map(value => [value.debug_commands, value.session_logging]), [[true,false], [true,true], [false,true]]);
});

test('custom Debug subsection keeps navigation in place and saves toggles separately', async () => {
 const saved = [];let closed = false;
 await settingsDialog({ui:{notify:text => assert.fail(text), custom:async factory => {
  const component = factory({terminal:{rows:16}, requestRender(){}}, theme, {}, () => {closed = true;});
  const press = data => component.handleInput(data), render = () => stripVTControlCharacters(component.render(100).join('\n'));
  for (let i=0; i<5; i++) press(down);
  assert.match(render(), /Configure debug commands/);assert.doesNotMatch(render(), /Debug commands\s+disabled/);
  press(enter);assert.match(render(), /Debug commands\s+disabled/);assert.match(render(), /Helper session logging\s+disabled/);
  assert.ok(component.render(100).length <= 16);assert.match(render(), /Enable frozen capture and replay commands/);
  press(enter);assert.deepEqual([saved.at(-1).debug_commands, saved.at(-1).session_logging], [true,false]);
  press(down);assert.match(render(), /Save complete helper inputs/);assert.ok(component.render(100).length <= 16);
  press(enter);assert.deepEqual([saved.at(-1).debug_commands, saved.at(-1).session_logging], [true,true]);
  press(escape);assert.equal(closed, false);assert.match(render(), /Configure debug commands/);
  press(enter);assert.match(render(), /Debug commands\s+enabled/);assert.match(render(), /Helper session logging\s+enabled/);
  press(enter);assert.deepEqual([saved.at(-1).debug_commands, saved.at(-1).session_logging], [false,true]);
  press(escape);press(escape);assert.equal(closed, true);
 }}}, {...defaults}, value => saved.push(structuredClone(value)));
 assert.equal(saved.length, 3);
});

for (const custom of [false, true]) test(`${custom ? 'custom' : 'fallback'} Debug toggles roll back failed saves`, async () => {
 const notices = [], saved = [];let fail = true;
 const save = value => {if (fail) throw Error('settings disk unavailable');saved.push(structuredClone(value));};
 const ui = {notify:(text, level) => {assert.equal(level, 'error');notices.push(text);}};
 if (custom) ui.custom = async factory => {
  const component = factory({terminal:{rows:30}, requestRender(){}}, theme, {}, () => {});
  const press = data => component.handleInput(data), render = () => stripVTControlCharacters(component.render(100).join('\n'));
  for (let i=0; i<5; i++) press(down);press(enter);
  press(enter);assert.match(render(), /Debug commands\s+disabled/);
  press(down);press(enter);assert.match(render(), /Helper session logging\s+disabled/);
  fail = false;press(enter);assert.deepEqual([saved.at(-1).debug_commands, saved.at(-1).session_logging], [false,true]);
  press(escape);press(escape);
 };
 else {
  const choices = ['Debug', 'Debug commands: disabled', 'Helper session logging: disabled', 'Helper session logging: disabled', 'Back', 'Close'];let calls = 0;
  ui.select = async (_title, rows) => {
   calls++;if (calls === 4) fail = false;
   const choice = choices.shift();assert.ok(rows.includes(choice));return choice;
  };
 }
 await settingsDialog({ui}, {...defaults}, save);
 assert.equal(notices.length, 2);assert.ok(notices.every(text => text.includes('settings disk unavailable')));
 assert.equal(saved.length, 1);assert.deepEqual([saved[0].debug_commands, saved[0].session_logging], [false,true]);
});
