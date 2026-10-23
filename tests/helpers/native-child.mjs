// Protocol fake for native adapter acknowledgement and fencing tests; no request subprocesses.
import { appendFileSync, readFileSync, realpathSync } from 'node:fs';
import { createInterface } from 'node:readline';

if (process.env.STREAMS_RUST_BIN !== realpathSync(process.argv[1])) throw Error('native launch binary was not canonicalized');
const file = process.env.STREAMS_NATIVE_FIXTURE, log = process.env.STREAMS_NATIVE_LOG;
const sessionId = process.argv[process.argv.indexOf('--session-id') + 1];
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const read = () => JSON.parse(readFileSync(file, 'utf8'));
const record = argv => appendFileSync(log, JSON.stringify(argv) + '\n');
const reply = (id, value, code=0) => send({type:'response', id, code, value, notices:[]});
let previous;
function scope(force=false) {
 const value = read();
 const event = value.fail === 'observe'
  ? {type:'event', event:'scope', value:null, reason:value.failMessage}
  : {type:'event', event:'scope', value:Object.fromEntries(['version','scope','native','context','session','config'].map(key => [key,value[key]]))};
 const serialized = JSON.stringify(event);
 if (force || serialized !== previous) {previous = serialized;send(event);}
}
send({type:'ready', protocol:1});
record(['native', 'observe', '--session-id', sessionId, '--json']);scope();
createInterface({input:process.stdin}).on('line', line => {
 const message = JSON.parse(line);
 if (message.type === 'cancel') {reply(message.id, {error:{kind:'cancelled', message:'Cancelled'}}, 1);return;}
 const {id, argv} = message, value = read();record(argv);
 const command = argv.find(arg => ['observe','open','close','project','capture'].includes(arg));
 if (value.wait === command) return;
 if (value.fail === command) {reply(id, {error:{kind:value.failureKind ?? 'error', message:value.failMessage ?? 'uncertain native effect; pending retained'}}, 1);return;}
 const native = {workspace_id:value.context.workspaceId, owner_pane_id:value.context.paneId, owner_terminal_id:value.context.terminalId,
  session_id:value.scope.sessionId, native_session_id:value.scope.nativeSessionId, binding:value.ackBinding ?? value.context.binding};
 reply(id, value.invalid === command ? {} : command === 'observe' ? value : command === 'open' ? {display:'opened',native}
  : command === 'close' ? {closed:true} : command === 'project' ? {names:{workspace:'disabled',tab:'disabled'},native} : {captured:true});
 scope(value.echoScope);
});
