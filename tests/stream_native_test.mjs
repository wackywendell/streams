import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { createJiti } from 'jiti';
const jiti = createJiti(import.meta.url);
const { NativeController } = await jiti.import(new URL('../extensions/stream-checkpoint/native.ts', import.meta.url).pathname);
const { default: register } = await jiti.import(new URL('../extensions/stream-checkpoint/index.ts', import.meta.url).pathname);
const { StreamsCardSource, parseStreamsCardContext } = await jiti.import(new URL('../integrations/herdr-work-cards/src/herdr/streams-source.ts', import.meta.url).pathname);
const naming = await jiti.import(new URL('../integrations/herdr-work-cards/src/pi/session-naming.ts', import.meta.url).pathname);
const { defaults } = await jiti.import(new URL('../extensions/stream-checkpoint/settings.ts', import.meta.url).pathname);
const turn = () => new Promise(resolve => setImmediate(resolve));
async function until(check) {for (let i = 0; i < 100 && !check(); i++) await turn();assert.ok(check());}
function environment(t, changes = {}) {
 const values = {HERDR_ENV:'1', HERDR_PANE_ID:'pane', HERDR_TAB_ID:'tab', HERDR_WORKSPACE_ID:'workspace', HERDR_SOCKET_PATH:'/socket', ...changes};
 const previous = Object.fromEntries(Object.keys(values).map(key => [key, process.env[key]]));
 for (const [key, value] of Object.entries(values)) {if (value === undefined) delete process.env[key];else process.env[key] = value;}
 t.after(() => {for (const [key, value] of Object.entries(previous)) {if (value === undefined) delete process.env[key];else process.env[key] = value;}});
}
function fixture(t, options = {}) {
 const id = randomUUID(), events = [], notices = [], entries = [];
 let name, count = 0, closed = false;
 const session = {id:'session', host_id:'host', stream_id:'stream', harness:'pi', harness_session_id:id, title:'Saved title', title_mode:'automatic', status:'active', state:'Saved state', state_short:null, remaining:'', checkout:null};
 const saved = {catalog_source:'catalog', host_id:'host', authority:{host_id:'host', token:'owner'}, stream:{id:'stream', title:'Effort', title_mode:'automatic', goal:null, goal_short:null, goal_mode:'automatic', owner:{host_id:'host'}, status:'active', archived:false, repositories:{}, documents:[], issues:[], prs:[]}, session, sessions:[session], native:{frontend:'herdr', pane_id:'pane', tab_id:'tab', workspace_id:'workspace'}, checkouts:[], document_locations:[]};
 const ctx = {hasUI:true, mode:'tui', cwd:'/repo', sessionManager:{getSessionId:() => id, getEntries:() => entries, getLeafId:() => 'leaf', getBranch:() => [{id:'leaf'}], buildSessionProjection:() => ({messages:[]})}, ui:{notify:text => notices.push(text), setStatus:() => {}, select:async () => 'Close'}};
 const pi = {getSessionName:() => name, setSessionName:value => {name = value;entries.push({id:`name-${++count}`, type:'session_info', name:value});}, appendEntry:(customType, data) => entries.push({type:'custom', customType, data})};
 const source = new StreamsCardSource({nativeSessionId:id, readSnapshot:async () => {events.push('read');if (options.read) await options.read();return parseStreamsCardContext(saved, id);}});
 const adapter = {
  automatic:options.automatic ?? true, sessionNaming:options.sessionNaming ?? true,
  read:async signal => {if (options.readSignal) await options.readSignal(signal);return source.read();}, bootstrap:async signal => {events.push('bootstrap');if (options.bootstrap) await options.bootstrap(signal);return {socket:'/socket', paneId:'pane', terminalId:'terminal', tabId:'tab', workspaceId:'workspace'};},
  open:async (_context, signal, mode) => {events.push(`open:${mode}`);if (options.open) await options.open(signal);if (mode === 'explicit') closed = false;return {kind:closed ? 'suppressed' : 'paired'};},
  close:async () => {events.push('close');closed = true;},
  summaries:async () => {events.push('summaries');}, names:async (_context, _signal, intent) => {events.push('names');options.names?.(intent);},
  sessionName:(snapshot, check, intent) => {events.push('sessionName');naming.projectSessionName(pi, ctx.sessionManager, id, snapshot.session.title, check, intent);},
 };
 let loads = 0, captured;
 const factory = async input => {loads++;captured = input;return adapter;};
 const capture = async (streamId, signal) => {events.push(`capture:${streamId}`);if (options.capture) return options.capture(streamId, signal);};
 const controller = new NativeController({pi, ctx, factory, capture, warning:text => notices.push(text)});
 t.after(() => controller.stop());
 return {id, ctx, pi, saved, entries, adapter, controller, events, notices, factory, capture, loads:() => loads, captured:() => captured, name:() => name};
}

test('explicit naming reclaim shares the effect lane without reopening the display', async t => {
 environment(t);const intents=[],f=fixture(t,{sessionNaming:false,names:intent=>intents.push(intent)});
 f.controller.start();await f.controller.idle();f.pi.setSessionName('Manual name');
 const before=f.events.length;
 await f.controller.reclaimNames();
 assert.equal(intents.at(-1),'explicit');assert.equal(f.name(),'Saved title');
 assert.ok(!f.events.slice(before).some(event=>event.startsWith('open:')));
 assert.equal(f.entries.filter(entry=>entry.customType===naming.sessionNamingEntry).at(-1).data.mode,'auto');
});
test('discovery and controller construction have no effects; only gated session_start loads', async t => {
 environment(t);const f = fixture(t);await turn();assert.equal(f.loads(), 0);assert.deepEqual(f.events, []);
 f.controller.start();await f.controller.idle();assert.equal(f.loads(), 1);
 assert.equal(f.captured().sessionId, f.id);assert.equal(f.captured().cwd, '/repo');assert.notEqual(f.captured().env, process.env);
 assert.ok(f.events.indexOf('read') < f.events.indexOf('bootstrap'));assert.ok(f.events.includes('open:automatic'));
 assert.equal(f.name(), 'Saved title');assert.equal(f.events.at(-1), 'capture:stream');
 assert.equal(f.entries.filter(entry => entry.customType === naming.sessionNamingEntry).length, 1);
 f.controller.update();await f.controller.idle();
 assert.equal(f.entries.filter(entry => entry.customType === naming.sessionNamingEntry).length, 1);
});
for (const [label, env, ctx] of [
 ['headless', {}, {hasUI:false}], ['rpc', {}, {mode:'rpc'}], ['outside Herdr', {HERDR_ENV:undefined}, {}], ['no exact pane', {HERDR_PANE_ID:undefined}, {}],
]) test(`${label} never loads native dependencies or effects`, async t => {
 environment(t, env);const f = fixture(t);Object.assign(f.ctx, ctx);f.controller.start();f.controller.update();await f.controller.idle();
 assert.equal(f.loads(), 0);await assert.rejects(f.controller.open(), /interactive/);assert.deepEqual(f.events, []);
});
test('non-UUID helper identity is ineligible', async t => {
 environment(t);const f = fixture(t);f.ctx.sessionManager.getSessionId = () => 'helper';
 const controller = new NativeController({pi:f.pi, ctx:f.ctx, factory:f.factory, warning:assert.fail});t.after(() => controller.stop());
 controller.start();await controller.idle();assert.equal(f.loads(), 0);
});
test('unbound and inherited helper attestation failures stay quiet, with actionable explicit errors', async t => {
 environment(t);const f = fixture(t), session = f.saved.session;f.saved.session = null;f.saved.sessions = [];
 f.controller.start();await f.controller.idle();assert.deepEqual(f.events, ['read']);assert.deepEqual(f.notices, []);
 await assert.rejects(f.controller.open(), /unbound/);assert.equal(f.loads(), 1);assert.ok(!f.events.some(event => event.startsWith('capture:')));
 f.saved.session = {...session, harness_session_id:randomUUID()};f.saved.sessions = [f.saved.session];
 await assert.rejects(f.controller.open(), /exact host\/pi\/native/);assert.deepEqual(f.notices, []);
});
test('transient native startup reference mismatch waits for fresh attestation without replacing the real conversation', async t => {
 environment(t);let stale = true;
 const f = fixture(t, {read:async () => {if (stale) throw Error('Streams context read failed: requested Pi session differs from the native pane session');}});
 const before = structuredClone(f.saved);
 f.controller.start();await f.controller.idle();
 assert.equal(f.ctx.sessionManager.getSessionId(), f.id);assert.deepEqual(f.events, ['read']);assert.deepEqual(f.notices, []);
 stale = false;f.controller.update();await f.controller.idle();
 assert.equal(f.loads(), 1);assert.equal(f.events.filter(e => e === 'open:automatic').length, 1);
 assert.deepEqual(f.saved, before);assert.equal(f.events.at(-1), 'capture:stream');
});
test('missed native reference refresh never authorizes effects even when the real conversation is unchanged', async t => {
 environment(t);t.mock.timers.enable({apis:['setTimeout']});
 const f = fixture(t, {read:async () => {throw Error('Streams context read failed: requested Pi session differs from the native pane session');}});
 f.controller.start();await f.controller.idle();
 for (let i = 0; i < 6; i++) {t.mock.timers.tick(15000);await f.controller.idle();}
 assert.equal(f.events.filter(e => e === 'read').length, 5);assert.equal(f.loads(), 1);
 assert.equal(f.ctx.sessionManager.getSessionId(), f.id);assert.ok(f.events.every(e => e === 'read'));
 await assert.rejects(f.controller.open(), /native pane session/);
 assert.ok(f.events.every(e => e === 'read'));assert.deepEqual(f.notices, []);
});
test('ready enrollment recovers on update using the same source without model work', async t => {
 environment(t);const f = fixture(t), session = f.saved.session;f.saved.session = null;
 f.controller.start();await f.controller.idle();f.saved.session = session;f.controller.update();await f.controller.idle();
 assert.equal(f.loads(), 1);assert.ok(f.events.includes('open:automatic'));assert.equal(f.events.at(-1), 'capture:stream');
});
test('pinned source replacement is terminal across updates, tree invalidation, and explicit open', async t => {
 environment(t);const f = fixture(t);f.controller.start();await f.controller.idle();const before = f.events.filter(event => event === 'summaries').length;
 f.saved.catalog_source = 'replacement';f.controller.update();await f.controller.idle();
 f.saved.catalog_source = 'catalog';f.controller.invalidate();await f.controller.idle();await assert.rejects(f.controller.open(), /scope changed/);
 assert.equal(f.events.filter(event => event === 'summaries').length, before);assert.equal(f.loads(), 1);assert.equal(f.notices.length, 1);
});
test('refreshes coalesce and explicit open/close share one serial lane', async t => {
 environment(t);let release, entered = false;
 const f = fixture(t, {open:async () => {if (!entered) {entered = true;await new Promise(resolve => {release = resolve;});}}});
 f.controller.start();await until(() => entered);for (let i = 0; i < 10; i++) f.controller.update();
 const close = f.controller.close(), open = f.controller.open();assert.equal(f.events.includes('close'), false);
 release();await Promise.all([close, open]);await f.controller.idle();
 assert.deepEqual(f.events.filter(event => event.startsWith('open:') || event === 'close'), ['open:automatic', 'close', 'open:explicit', 'open:automatic']);
});
test('tree invalidation aborts stale work without recreating source or reopening suppressed cards', async t => {
 environment(t);let release, signal;
 const f = fixture(t, {open:async value => {if (!signal) {signal = value;await new Promise(resolve => {release = resolve;});}}});
 f.controller.start();await until(() => !!release);f.controller.invalidate();assert.equal(signal.aborted, true);release();await f.controller.idle();
 assert.equal(f.loads(), 1);assert.equal(f.events.filter(event => event === 'capture:stream').length, 1);
 await f.controller.close();f.controller.invalidate();await f.controller.idle();
 assert.equal(f.events.at(-1), 'capture:stream');assert.equal(f.events.filter(event => event === 'open:explicit').length, 0);
});
test('shutdown aborts stale responses, never closes surfaces or captures final progress', async t => {
 environment(t);let release, signal;const f = fixture(t, {open:async value => {signal = value;await new Promise(resolve => {release = resolve;});}});
 f.controller.start();await until(() => !!release);f.controller.stop();f.controller.stop();assert.equal(signal.aborted, true);release();await f.controller.idle();
 assert.equal(f.events.includes('close'), false);assert.equal(f.events.includes('summaries'), false);assert.equal(f.events.some(event => event.startsWith('capture:')), false);
});
for (const change of ['pane', 'endpoint', 'cwd', 'session', 'attested-tab']) test(`changed ${change} fences pending native effects`, async t => {
 environment(t);let release;const f = fixture(t, {read:async () => {if (!release) await new Promise(resolve => {release = resolve;});}});
 f.controller.start();await until(() => !!release);
 if (change === 'pane') process.env.HERDR_PANE_ID = 'other';
 if (change === 'endpoint') process.env.HERDR_SOCKET_PATH = '/other';
 if (change === 'cwd') f.ctx.cwd = '/other';
 if (change === 'session') f.ctx.sessionManager.getSessionId = () => randomUUID();
 if (change === 'attested-tab') f.saved.native.tab_id = 'other';
 release();await f.controller.idle();assert.equal(f.events.includes('bootstrap'), false);assert.equal(f.events.includes('sessionName'), false);assert.equal(f.events.some(event => event.startsWith('capture:')), false);
});
test('manual startup keeps independently enabled naming and summaries, without opening a pane', async t => {
 environment(t);const f = fixture(t, {automatic:false});f.controller.start();await f.controller.idle();
 assert.equal(f.events.some(event => event.startsWith('open:')), false);assert.ok(f.events.includes('names'));assert.equal(f.name(), 'Saved title');
 await f.controller.open();assert.ok(f.events.includes('open:explicit'));
});
test('catalog title modes both project into Pi while the session naming switch remains independent', async t => {
 environment(t);for (const mode of ['disabled', 'manual', 'automatic']) {
  const f = fixture(t, {sessionNaming:mode !== 'disabled'});if (mode === 'manual') f.saved.session.title_mode = 'manual';
  f.controller.start();await f.controller.idle();
  assert.equal(f.name(), mode === 'disabled' ? undefined : 'Saved title');
  assert.equal(f.events.includes('sessionName'), mode !== 'disabled');
 }
});
test('Pi naming provenance survives reload and permanently preserves manual overrides, even equal labels', async t => {
 environment(t);const f = fixture(t);f.controller.start();await f.controller.idle();assert.equal(f.name(), 'Saved title');
 f.saved.session.title = 'Updated';f.controller.update();await f.controller.idle();assert.equal(f.name(), 'Updated');
 f.pi.setSessionName('Updated');f.saved.session.title = 'Next';f.controller.update();await f.controller.idle();assert.equal(f.name(), 'Updated');
 f.controller.stop();const reload = new NativeController({pi:f.pi, ctx:f.ctx, factory:f.factory, warning:assert.fail});t.after(() => reload.stop());
 reload.start();await reload.idle();assert.equal(f.name(), 'Updated');
});
test('preexisting manual Pi names are not adopted by matching catalog title', async t => {
 environment(t);const f = fixture(t);f.pi.setSessionName('Saved title');f.controller.start();await f.controller.idle();
 f.saved.session.title = 'Updated';f.controller.update();await f.controller.idle();assert.equal(f.name(), 'Saved title');
});

function registration(t, f, cli = {}) {
 const handlers = new Map(), commands = new Map(), tools = new Map(), shortcuts = new Map();
 const client = {sync:async () => ({local:'unchanged', sharing:'local-only', domain:null}), context:async () => structuredClone(f.saved), captureArrangement:f.capture, ...cli};
 register({...f.pi, on:(name, handler) => handlers.set(name, handler), registerCommand:(name, value) => commands.set(name, value), registerTool:value => tools.set(value.name, value), registerShortcut:(name, value) => shortcuts.set(name, value)}, {cli:client, native:f.factory, settings:{...defaults, reporting:'manual', enrollment:'manual'}, infer:async () => {throw Error('native lifecycle must not infer');}});
 t.after(async () => {await handlers.get('session_shutdown')({}, f.ctx);});
 return {handlers, commands, tools, shortcuts, run:args => commands.get('streams').handler(args, f.ctx)};
}
test('extension reload/start replaces stale lifetime and pane commands do not report or attach', async t => {
 environment(t);const f = fixture(t), r = registration(t, f);assert.equal(f.loads(), 0);
 await r.handlers.get('session_start')({}, f.ctx);await until(() => f.events.includes('capture:stream'));
 assert.deepEqual([...r.commands.keys()], ['streams']);assert.deepEqual([...r.shortcuts.keys()], []);
 await r.run('pane close');await r.run('pane');assert.ok(f.events.includes('close'));assert.ok(f.events.includes('open:explicit'));
 const before = f.events.length;await r.run('pane close extra');assert.equal(f.events.length, before);assert.match(f.notices.at(-1), /pane \[close\]/);
 await r.handlers.get('session_shutdown')({}, f.ctx);await r.handlers.get('session_start')({}, f.ctx);await until(() => f.loads() === 2);await turn();
 assert.equal(f.events.filter(event => event === 'close').length, 1);
});
test('successful guided edits refresh from persisted source immediately; existing receipt entry is unchanged', async t => {
 environment(t);const f = fixture(t);let menu = 0;
 const receipt = {operation_id:'edit', save:{local:'persisted', sharing:'local-only', domain:{outcome:'changes'}}};
 const r = registration(t, f, {apply:async () => {f.saved.session.state = 'Manually saved';return receipt;}});
 f.ctx.ui.select = async () => {assert.ok(menu < 4, f.notices.join('\n'));return ['This session — edit fields', 'state: "Saved state"', 'Save field', 'Close'][menu++];};
 f.ctx.ui.editor = async () => 'Manually saved';
 await r.handlers.get('session_start')({}, f.ctx);await until(() => f.events.includes('capture:stream'));const initial = f.events.filter(event => event === 'capture:stream').length;
 await r.run('card');await until(() => f.events.filter(event => event === 'capture:stream').length > initial);
 assert.deepEqual(f.entries.filter(entry => entry.customType === 'stream-catalog-receipt').map(entry => entry.data), [receipt]);assert.equal(f.loads(), 1);
});
test('tree changes reject queued explicit actions instead of replaying them on a later branch', async t => {
 environment(t);let release;const f = fixture(t, {open:async () => {if (!release) await new Promise(resolve => {release = resolve;});}});
 f.controller.start();await until(() => !!release);const close = f.controller.close();
 const rejected = assert.rejects(close, /tree changed/);f.controller.invalidate();await rejected;release();await f.controller.idle();
 assert.equal(f.events.includes('close'), false);
});
test('a reload retires pending old startup responses before they can project or capture', async t => {
 environment(t);let release;const f = fixture(t, {read:async () => {if (!release) await new Promise(resolve => {release = resolve;});}}), r = registration(t, f);
 await r.handlers.get('session_start')({}, f.ctx);await until(() => !!release);
 let settled = false;const replacement = r.handlers.get('session_start')({}, f.ctx).then(() => {settled = true;});
 await turn();assert.equal(settled, false);assert.equal(f.loads(), 1);
 release();await replacement;await until(() => f.events.includes('capture:stream'));
 assert.equal(f.loads(), 2);assert.equal(f.events.filter(event => event === 'bootstrap').length, 1);assert.equal(f.events.filter(event => event === 'capture:stream').length, 1);
});
test('normal attach receipts immediately recover native projection without another model response', async t => {
 environment(t);const f = fixture(t), session = f.saved.session;f.saved.session = null;f.saved.sessions = [];
 const receipt = {stream:f.saved.stream, session, authority:f.saved.authority, saves:[{operation_id:'enroll', save:{local:'persisted', sharing:'local-only', domain:{outcome:'changes'}}}]};
 const r = registration(t, f, {list:async () => [f.saved.stream], enroll:async () => {f.saved.session = session;f.saved.sessions = [session];return receipt;}});
 f.ctx.ui.select = async (_title, rows) => rows.find(row => row.startsWith('stream ·'));
 f.ctx.ui.editor = async () => 'Contribution';f.ctx.ui.confirm = async () => true;
 await r.handlers.get('session_start')({}, f.ctx);await until(() => f.events.includes('read'));assert.equal(f.events.includes('bootstrap'), false);
 await r.run('attach');await until(() => f.events.includes('capture:stream'));
 assert.equal(f.loads(), 1);assert.deepEqual(f.entries.filter(entry => entry.customType === 'stream-catalog-receipt').map(entry => entry.data), [receipt]);
});
test('retired explicit pane failures do not notify a replacement lifetime', async t => {
 environment(t);let release;const f = fixture(t, {automatic:false, open:async () => {await new Promise(resolve => {release = resolve;});}}), r = registration(t, f);
 await r.handlers.get('session_start')({}, f.ctx);await until(() => f.events.includes('capture:stream'));
 const command = r.run('pane');await until(() => !!release);
 const replacement = r.handlers.get('session_start')({}, f.ctx);release();await replacement;await command;await turn();
 assert.deepEqual(f.notices, []);
});
test('capture failure defers recovery to a real update without retrying native projection', async t => {
 environment(t);t.mock.timers.enable({apis:['setTimeout']});let fail = true;
 const f = fixture(t, {capture:async () => {if (fail) throw Error('workspace lock timed out');}});
 f.controller.start();await f.controller.idle();
 const before = [...f.events];assert.match(f.notices[0], /Arrangement capture deferred.*workspace lock timed out/);
 t.mock.timers.tick(90000);await f.controller.idle();assert.deepEqual(f.events, before);
 // Explicit pane success is not falsely reported as failure merely because capture deferred.
 await f.controller.open();assert.equal(f.events.filter(event => event === 'open:explicit').length, 1);
 assert.equal(f.notices.length, 1);
 fail = false;f.controller.update();await f.controller.idle();
 assert.equal(f.events.filter(event => event === 'capture:stream').length, 3);
 assert.equal(f.events.filter(event => event === 'open:automatic').length, 2);
 assert.equal(f.loads(), 1);assert.equal(f.events.filter(event => event === 'bootstrap').length, 1);
 const recovered = [...f.events];t.mock.timers.tick(90000);await f.controller.idle();assert.deepEqual(f.events, recovered);
});
test('successful projection resets the bounded native retry budget even when capture fails', async t => {
 environment(t);t.mock.timers.enable({apis:['setTimeout']});let fail = true;
 const f = fixture(t, {open:async () => {if (fail) throw Error('workspace lock timed out');}, capture:async () => {throw Error('capture unavailable');}});
 f.controller.start();await f.controller.idle();
 for (let i = 0; i < 4; i++) {t.mock.timers.tick(15000);await f.controller.idle();}
 assert.equal(f.events.filter(event => event === 'open:automatic').length, 5);
 t.mock.timers.tick(15000);await f.controller.idle();assert.equal(f.events.filter(event => event === 'open:automatic').length, 5);
 fail = false;f.controller.update();await f.controller.idle();
 fail = true;f.controller.update();await f.controller.idle();
 for (let i = 0; i < 4; i++) {t.mock.timers.tick(15000);await f.controller.idle();}
 assert.equal(f.events.filter(event => event === 'open:automatic').length, 11);
});
for (const stage of ['read', 'bootstrap', 'capture']) test(`shutdown awaits retired ${stage} while sync runs independently`, async t => {
 environment(t);let release, signal, syncFinished = false;t.after(() => release?.());
 const gate = async value => {signal = value;await new Promise(resolve => {release = resolve;});};
 const f = fixture(t, {[stage === 'read' ? 'readSignal' : stage]:stage === 'capture' ? (_streamId, signal) => gate(signal) : gate});
 const r = registration(t, f, {sync:async event => {if (event === 'shutdown') syncFinished = true;return {local:'unchanged', sharing:'local-only', domain:null};}});
 await r.handlers.get('session_start')({}, f.ctx);await until(() => !!release);
 let settled = false;const shutdown = r.handlers.get('session_shutdown')({}, f.ctx).then(() => {settled = true;});
 await turn();assert.equal(signal.aborted, true);assert.equal(syncFinished, true);assert.equal(settled, false);
 release();await shutdown;assert.equal(settled, true);assert.equal(f.events.includes('close'), false);
 if (stage !== 'capture') assert.equal(f.events.some(event => event.startsWith('capture:')), false);
 assert.deepEqual(f.notices, []);
});
test('pane effect errors propagate without misleading enrollment instructions', async t => {
 environment(t);const f = fixture(t, {automatic:false, open:async () => {throw Error('workspace lock timed out');}}), r = registration(t, f);
 await r.handlers.get('session_start')({}, f.ctx);await until(() => f.events.includes('capture:stream'));
 await r.run('pane');assert.match(f.notices.at(-1), /workspace lock timed out/);
 assert.doesNotMatch(f.notices.at(-1), /attach|enrollment|reload/);
 assert.equal(f.events.filter(event => event === 'capture:stream').length, 1);
});
test('old injected CLI without arrangement capture remains supported', async t => {
 environment(t);const f = fixture(t), r = registration(t, f, {captureArrangement:undefined});
 await r.handlers.get('session_start')({}, f.ctx);await until(() => f.events.includes('sessionName'));await r.run('pane');
 assert.equal(f.events.some(event => event.startsWith('capture:')), false);
});
