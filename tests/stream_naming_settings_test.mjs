import assert from 'node:assert/strict';
import test from 'node:test';
import {mkdtempSync, readFileSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {stripVTControlCharacters} from 'node:util';
import {createJiti} from 'jiti';
import {initTheme} from '@earendil-works/pi-coding-agent';
const jiti=createJiti(import.meta.url);
const {defaults, settingsDialog, saveSettings}=await jiti.import('../extensions/stream-checkpoint/settings.ts');
const {StreamCli}=await jiti.import('../extensions/stream-checkpoint/streams.ts');
initTheme('dark');
const theme={fg:(_color,text)=>text,bold:text=>text};
const down='\x1b[B',enter='\r',escape='\x1b';
const turn=()=>new Promise(resolve=>setImmediate(resolve));
function fixture(t,run) {
 const root=mkdtempSync(join(tmpdir(),'streams-naming-settings-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const native={document_command:'vim',workspace_naming:false,tab_naming:false,session_naming:false};
 writeFileSync(join(root,'work-cards.json'),JSON.stringify(native));
 const titleDefaults={stream:'manual',session:'automatic'},captured=structuredClone(titleDefaults),requests=[],saved=[],notices=[];
 const client=new StreamCli('/fixture',async request=>{
  requests.push(structuredClone(request));assert.equal(request.kind,'set_title_default');
  if(run)return run(request);
  titleDefaults[request.target]=request.mode;return {status:'success',result:{title_defaults:{...titleDefaults}}};
 });
 return {root,native,titleDefaults,captured,requests,saved,notices,
  ui:{notify:(text,level)=>notices.push({text,level})},
  naming:{defaults:captured,save:(target,mode)=>client.setTitleDefault(target,mode)},
  save:value=>{saveSettings(root,value);saved.push(structuredClone(value));},
 };
}
for(const custom of [false,true])test(`${custom?'custom':'fallback'} Naming keeps generation, import and outgoing synchronization independent`,async t=>{
 const f=fixture(t);
 if(custom) f.ui.custom=async factory=>{
  let closed=false;
  const component=factory({terminal:{rows:30},requestRender(){}},theme,{},()=>{closed=true;});
  for(let i=0;i<6;i++)component.handleInput(down);component.handleInput(enter);
  const text=()=>stripVTControlCharacters(component.render(110).join('\n'));
  assert.match(text(),/Stream title generation\s+manual/);assert.match(text(),/Session title generation\s+automatic/);
  for(let i=0;i<6;i++) {component.handleInput(enter);await turn();if(i<5)component.handleInput(down);}
  component.handleInput(escape);assert.equal(closed,false);component.handleInput(escape);assert.equal(closed,true);
 };
 else {
  const choices=['Naming','Stream title generation: manual','Session title generation: automatic','Pi name edits: enabled','Herdr workspace naming: disabled','Herdr tab naming: disabled','Pi session naming: disabled','Back','Close'];
  f.ui.select=async(title,rows)=>{const choice=choices.shift();assert.ok(rows.includes(choice),`${title}: ${rows}`);return choice;};
 }
 await settingsDialog({ui:f.ui},{...defaults},f.save,f.naming,f.root);
 assert.deepEqual(f.requests,[{kind:'set_title_default',target:'stream',mode:'automatic'},{kind:'set_title_default',target:'session',mode:'manual'}]);
 assert.deepEqual(f.captured,{stream:'manual',session:'automatic'},'settings must not mutate the captured defaults');
 assert.equal(f.saved.length,1);assert.equal(f.saved[0].pi_name_edits,false);
 const reporting=JSON.parse(readFileSync(join(f.root,'stream-reporting.json'),'utf8'));
 assert.equal(reporting.title_defaults,undefined);assert.equal(reporting.naming,undefined);
 assert.equal(reporting.reporting,'automatic');assert.equal(reporting.enrollment,'automatic');
 assert.deepEqual(JSON.parse(readFileSync(join(f.root,'work-cards.json'),'utf8')),{...f.native,workspace_naming:true,tab_naming:true,session_naming:true});
 assert.deepEqual(f.notices,[]);
});
for(const custom of [false,true])test(`${custom?'custom':'fallback'} Naming restores failed generation edits and uses returned defaults`,async t=>{
 let failing=true;
 const f=fixture(t,async request=>{
  if(failing)throw Error('settings backend unavailable');
  return {status:'success',result:{title_defaults:{stream:request.mode,session:'manual'}}};
 });
 if(custom)f.ui.custom=async factory=>{
  const component=factory({terminal:{rows:30},requestRender(){}},theme,{},()=>{});
  for(let i=0;i<6;i++)component.handleInput(down);component.handleInput(enter);
  const text=()=>stripVTControlCharacters(component.render(110).join('\n'));
  component.handleInput(enter);await turn();assert.match(text(),/Stream title generation\s+manual/);
  failing=false;component.handleInput(enter);await turn();
  assert.match(text(),/Stream title generation\s+automatic/);assert.match(text(),/Session title generation\s+manual/);
  component.handleInput(escape);component.handleInput(escape);
 };
 else {
  const choices=['Naming','Stream title generation: manual','Stream title generation: manual','Back','Close'];let calls=0;
  f.ui.select=async(_title,rows)=>{if(++calls===3)failing=false;if(calls===4)assert.ok(rows.includes('Session title generation: manual'));const choice=choices.shift();assert.ok(rows.includes(choice));return choice;};
 }
 await settingsDialog({ui:f.ui},{...defaults},f.save,f.naming,f.root);
 assert.equal(f.notices.length,1);assert.match(f.notices[0].text,/settings backend unavailable/);assert.equal(f.notices[0].level,'error');
 assert.deepEqual(f.requests,[{kind:'set_title_default',target:'stream',mode:'automatic'},{kind:'set_title_default',target:'stream',mode:'automatic'}]);
 assert.deepEqual(f.saved,[]);assert.deepEqual(JSON.parse(readFileSync(join(f.root,'work-cards.json'),'utf8')),f.native);
});
test('custom Naming displays an in-flight save and admits one action at a time',async t=>{
 let release;
 const f=fixture(t,request=>new Promise(resolve=>{release=()=>resolve({status:'success',result:{title_defaults:{stream:request.mode,session:'automatic'}}});}));
 f.ui.custom=async factory=>{
  let closed=false;
  const component=factory({terminal:{rows:30},requestRender(){}},theme,{},()=>{closed=true;});
  for(let i=0;i<6;i++)component.handleInput(down);component.handleInput(enter);component.handleInput(enter);
  assert.match(stripVTControlCharacters(component.render(110).join('\n')),/saving…/);
  component.handleInput(down);component.handleInput(enter);component.handleInput(escape);
  assert.equal(f.requests.length,1);assert.equal(closed,false);
  release();await turn();assert.doesNotMatch(stripVTControlCharacters(component.render(110).join('\n')),/saving…/);
  component.handleInput(escape);component.handleInput(escape);assert.equal(closed,true);
 };
 await settingsDialog({ui:f.ui},{...defaults},f.save,f.naming,f.root);
 assert.deepEqual(f.notices,[]);
});
test('Naming descriptions explain effective policy, initial titles and both Pi name directions on short terminals',async t=>{
 const f=fixture(t);
 f.ui.custom=async factory=>{
  const component=factory({terminal:{rows:16},requestRender(){}},theme,{},()=>{});
  for(let i=0;i<6;i++)component.handleInput(down);component.handleInput(enter);
  for(const expected of [/Default for Stream titles in inherit mode/,/Default for Session titles in inherit mode/,/Save changed Pi \/name edits as manual Session titles/,/Mirror the saved Stream title/,/Mirror saved Session titles/,/Mirror the saved Session title in Pi/]) {
   const lines=component.render(100),text=stripVTControlCharacters(lines.join('\n'));assert.ok(lines.length<=16,text);assert.match(text,expected);
   if(expected.source.startsWith('Default')) {assert.match(text,/Does not change explicit/);assert.match(text,/Enrollment may still create an initial title/);}
   component.handleInput(down);
  }
  component.handleInput(escape);component.handleInput(escape);
 };
 await settingsDialog({ui:f.ui},{...defaults},f.save,f.naming,f.root);
 assert.deepEqual(f.requests,[]);assert.deepEqual(f.saved,[]);
});
