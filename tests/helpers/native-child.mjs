// Typed protocol fake: one process, no native effects or request subprocesses.
import { appendFileSync, readFileSync, realpathSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { checkoutSource } from '../../extensions/stream-checkpoint/streams-bin.ts';

if (process.env.STREAMS_BIN_PATH !== realpathSync(process.argv[1])) throw Error('native launch binary was not canonicalized');
const file=process.env.STREAMS_NATIVE_FIXTURE,log=process.env.STREAMS_NATIVE_LOG;
const send=value=>process.stdout.write(JSON.stringify(value)+'\n');
const reply=(id,result)=>send({type:'response',id,outcome:{status:'success',result}});
send({type:'ready',protocol:2,source:checkoutSource()});
const input=createInterface({input:process.stdin});
input.on('close',()=>{const value=JSON.parse(readFileSync(file,'utf8'));if(value.shutdownWarning)send({type:'warning',message:value.shutdownWarning});});
input.on('line',line=>{
 const message=JSON.parse(line);
 if(message.type==='cancel') {send({type:'response',id:message.id,outcome:{status:'error',error:{kind:'cancelled',message:'Cancelled'}}});return;}
 const {id,request}=message,value=JSON.parse(readFileSync(file,'utf8'));appendFileSync(log,JSON.stringify(request)+'\n');
 if(value.wait===request.kind)return;
 if(value.fail===request.kind) {send({type:'response',id,outcome:{status:'error',error:{kind:value.failureKind??'runtime',message:value.failMessage??'uncertain native effect; pending retained'}}});return;}
 const result=value.invalid===request.kind?{}:request.kind==='session_started'?{native:'active',display:value.automatic?'opened':'suppressed'}
  :request.kind==='attach'?{native:'active'}:request.kind==='open_card'?{display:'opened'}:request.kind==='close_card'?{closed:true}:request.kind==='sync'?{local:'unchanged',sharing:'local-only',domain:null}:request.kind==='lifecycle'?{session_changed:false,stream_changed:false,saves:[]}:{};
 reply(id,result);send({type:'view',view:{desired_pi_name:value.sessionNaming?value.title:null}});
});
