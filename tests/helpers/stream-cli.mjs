import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url);
const {StreamsChild}=await jiti.import('../../extensions/stream-checkpoint/child.ts');
const {StreamCli}=await jiti.import('../../extensions/stream-checkpoint/streams.ts');
const root=fileURLToPath(new URL('../../',import.meta.url));

/** Real v2 catalog transport, always isolated from the user's catalog and frontend. */
export function childCli(cwd,sessionId='fixture',environment) {
 const env={...process.env,...environment,HOME:environment?.HOME??cwd};
 for(const key of Object.keys(env))if(/^(HERDR_|CMUX_|STREAMS_|PI_CODING_AGENT_|WORK_CARDS_)/.test(key))delete env[key];
 Object.assign(env,{XDG_CONFIG_HOME:environment?.XDG_CONFIG_HOME??resolve(cwd,'config'),XDG_DATA_HOME:environment?.XDG_DATA_HOME??resolve(cwd,'data'),XDG_STATE_HOME:environment?.XDG_STATE_HOME??resolve(cwd,'state'),STREAMS_BIN_PATH:resolve(root,process.env.CARGO_TARGET_DIR??'target','debug','streams')});
 const child=new StreamsChild({sessionId,cwd,env,warning:message=>{throw Error(message);}});
 const cli=new StreamCli(cwd,child.run);
 return Object.assign(cli,{stop:()=>child.stop()});
}
