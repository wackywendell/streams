import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';
import { stripVTControlCharacters } from 'node:util';
import { initTheme } from '@earendil-works/pi-coding-agent';
const jiti = createJiti(import.meta.url);
const { defaults, defaultConfigSnapshot, reportingSettingsFromSnapshot, validateSettings, settingsDialog } = await jiti.import(new URL('../extensions/stream-checkpoint/settings.ts', import.meta.url).pathname);
initTheme('dark');
const theme = {fg: (_color, text) => text, bold: text => text};
const down = '\x1b[B', enter = '\r', escape = '\x1b';

function snapshotActions(initial = defaultConfigSnapshot(), onWrite) {
 const current = structuredClone(initial);
 return {
  current,
  actions: {
   setConfig: async (key, value) => {
    if (onWrite) await onWrite(key, value);
    if (key === 'pi.reporting.debug_commands') current.pi.reporting.debug_commands = value === 'true';
    else if (key === 'pi.reporting.session_logging') current.pi.reporting.session_logging = value === 'true';
    return structuredClone(current);
   },
   unsetConfig: async () => structuredClone(current),
  },
 };
}

test('debug commands default off without changing legacy helper logging opt-in', () => {
 assert.equal(defaults.debug_commands, false);
 assert.equal(defaults.session_logging, false);
 const snap = defaultConfigSnapshot();
 assert.deepEqual(reportingSettingsFromSnapshot(snap), defaults);
});

test('validateSettings requires boolean debug_commands', () => {
 for (const debug_commands of [null, 'enabled', 0, 1, {}, []]) {
  assert.throws(() => validateSettings({...defaults, debug_commands}), /Debug commands must be a boolean/);
 }
});

test('fallback Debug subsection saves independent toggles via child protocol and returns to main settings', async () => {
 const saved = [], steps = [
  ['Streams Pi & naming settings', 'Debug'], ['Debug', 'pi.reporting.debug_commands: false'],
  ['Debug', 'pi.reporting.session_logging: false'], ['Debug', 'pi.reporting.debug_commands: true'],
  ['Debug', 'Back'], ['Streams Pi & naming settings', 'Debug'],
  ['Debug', undefined], ['Streams Pi & naming settings', 'Close'],
 ];
 const {current, actions} = snapshotActions(defaultConfigSnapshot(), (key, value) => saved.push([key, value]));
 await settingsDialog({ui:{notify:text => assert.fail(text), select:async (title, rows) => {
  const [expectedTitle, choice] = steps.shift(); assert.equal(title, expectedTitle);
  if (title === 'Debug') assert.deepEqual(rows, [
   `pi.reporting.debug_commands: ${current.pi.reporting.debug_commands}`,
   `pi.reporting.session_logging: ${current.pi.reporting.session_logging}`, 'Back',
  ]);
  else {assert.ok(rows.includes('Debug')); assert.ok(rows.every(row => !/^(pi\.reporting\.debug_commands|pi\.reporting\.session_logging):/.test(row)));}
  if (choice !== undefined) assert.ok(rows.includes(choice)); return choice;
 }}}, defaultConfigSnapshot(), actions);
 assert.equal(steps.length, 0);
 assert.deepEqual(saved, [['pi.reporting.debug_commands', 'true'], ['pi.reporting.session_logging', 'true'], ['pi.reporting.debug_commands', 'false']]);
});

test('custom Debug subsection keeps navigation in place and saves toggles separately', async () => {
 const saved = []; let closed = false;
 const {actions} = snapshotActions(defaultConfigSnapshot(), (key, value) => saved.push([key, value]));
 await settingsDialog({ui:{notify:text => assert.fail(text), custom:async factory => {
  const component = factory({terminal:{rows:16}, requestRender(){}}, theme, {}, () => {closed = true;});
  const press = data => component.handleInput(data), render = () => stripVTControlCharacters(component.render(100).join('\n'));
  for (let i=0; i<5; i++) press(down);
  assert.match(render(), /Configure debug commands/); assert.doesNotMatch(render(), /pi\.reporting\.debug_commands\s+false/);
  press(enter); assert.match(render(), /pi\.reporting\.debug_commands\s+false/); assert.match(render(), /pi\.reporting\.session_logging\s+false/);
  assert.ok(component.render(100).length <= 16); assert.match(render(), /Enable frozen capture and replay commands/);
  press(enter); await new Promise(r => setImmediate(r));
  assert.deepEqual(saved.at(-1), ['pi.reporting.debug_commands', 'true']);
  press(down); assert.match(render(), /Save complete helper inputs/); assert.ok(component.render(100).length <= 16);
  press(enter); await new Promise(r => setImmediate(r));
  assert.deepEqual(saved.at(-1), ['pi.reporting.session_logging', 'true']);
  press(escape); assert.equal(closed, false); assert.match(render(), /Configure debug commands/);
  press(enter); assert.match(render(), /pi\.reporting\.debug_commands\s+true/); assert.match(render(), /pi\.reporting\.session_logging\s+true/);
  press(enter); await new Promise(r => setImmediate(r));
  assert.deepEqual(saved.at(-1), ['pi.reporting.debug_commands', 'false']);
  press(escape); press(escape); assert.equal(closed, true);
 }}}, defaultConfigSnapshot(), actions);
 assert.equal(saved.length, 3);
});

for (const custom of [false, true]) test(`${custom ? 'custom' : 'fallback'} Debug toggles roll back failed saves`, async () => {
 const notices = [], saved = []; let fail = true;
 const {actions} = snapshotActions(defaultConfigSnapshot(), (key, value) => {
  if (fail) throw Error('settings backend unavailable');
  saved.push([key, value]);
 });
 const ui = {notify:(text, level) => {assert.equal(level, 'error'); notices.push(text);}};
 if (custom) ui.custom = async factory => {
  const component = factory({terminal:{rows:30}, requestRender(){}}, theme, {}, () => {});
  const press = data => component.handleInput(data), render = () => stripVTControlCharacters(component.render(100).join('\n'));
  for (let i=0; i<5; i++) press(down); press(enter);
  press(enter); await new Promise(r => setImmediate(r)); assert.match(render(), /pi\.reporting\.debug_commands\s+false/);
  press(down); press(enter); await new Promise(r => setImmediate(r)); assert.match(render(), /pi\.reporting\.session_logging\s+false/);
  fail = false; press(enter); await new Promise(r => setImmediate(r));
  assert.deepEqual(saved, [['pi.reporting.session_logging', 'true']]);
  press(escape); press(escape);
 };
 else {
  const choices = ['Debug', 'pi.reporting.debug_commands: false', 'pi.reporting.session_logging: false', 'pi.reporting.session_logging: false', 'Back', 'Close']; let calls = 0;
  ui.select = async (_title, rows) => {
   calls++; if (calls === 4) fail = false;
   const choice = choices.shift(); assert.ok(rows.includes(choice)); return choice;
  };
 }
 await settingsDialog({ui}, defaultConfigSnapshot(), actions);
 assert.equal(notices.length, 2); assert.ok(notices.every(text => text.includes('settings backend unavailable')));
 assert.equal(saved.length, 1); assert.deepEqual(saved, [['pi.reporting.session_logging', 'true']]);
});
