import assert from 'node:assert/strict';
import test from 'node:test';
import { getEventListeners } from 'node:events';
import { stripVTControlCharacters } from 'node:util';
import { createJiti } from 'jiti';
import { initTheme } from '@earendil-works/pi-coding-agent';
import { visibleWidth } from '@earendil-works/pi-tui';
const jiti=createJiti(import.meta.url);
const {confirmCapture}=await jiti.import(new URL('../extensions/stream-checkpoint/capture-dialog.ts',import.meta.url).pathname);
initTheme('dark');
const theme={fg:(color,text)=>`\x1b[${color==='accent'?35:90}m${text}\x1b[39m`,bold:text=>text};
const inventory=()=>({host_id:'host',topology_error:null,unsupported:[{}],samples:Array.from({length:80},(_,i)=>({host_id:'host',native_session_id:`native-${i}`,catalog:{session:{title:`Conversation ${i} 界🙂`}},open:i%3?true:null,unavailable:i===79?'Synthetic missing transcript':null}))});
const context=ui=>({mode:'tui',sessionManager:{getSessionId:()=> 'native-0'},ui});

for(const rows of [12,16,30])for(const width of [24,80])test(`capture confirmation stays compact at ${width} columns and ${rows} rows with 80 conversations`,async()=>{
 const controller=new AbortController();
 const accepted=await confirmCapture(context({custom:async factory=>new Promise(done=>{
  const component=factory({terminal:{rows},requestRender(){}},theme,{},done),lines=component.render(width);
  assert.ok(lines.length<=rows-4,lines.join('\n'));assert.ok(lines.every(line=>visibleWidth(line)<=width));
  assert.doesNotMatch(stripVTControlCharacters(lines.join('\n')),/Conversation 79/);
  assert.match(stripVTControlCharacters(lines[1]),/80 Pi/);assert.doesNotMatch(lines[1],/\x1b\[(35|90)m/,'summary uses normal body foreground');
  component.handleInput('\r');
 })}),inventory(),'/private/captures',controller.signal);
 assert.equal(accepted,true);assert.equal(getEventListeners(controller.signal,'abort').length,0);
});

test('review scrolls through all conversations with the bounded built-in SelectList, then returns without capturing',async()=>{
 let opened=0,reviewed=false;const data=inventory();data.topology_error='Synthetic topology warning';
 const controller=new AbortController();
 const accepted=await confirmCapture(context({
  custom:async factory=>new Promise(done=>{
   const terminal={rows:16},component=factory({terminal,requestRender(){}},theme,{},done);
   const render=width=>{const lines=component.render(width);assert.ok(lines.length<=terminal.rows-4);assert.ok(lines.every(line=>visibleWidth(line)<=width));return stripVTControlCharacters(lines.join('\n'));};
   render(80);
   if(opened++===0){component.handleInput('\x1b[B');terminal.rows=12;render(24);component.handleInput('\r');}
   else if(opened===2){
    assert.match(render(80),/Capture inventory/);assert.match(render(80),/Storage: \/private\/captures/);assert.match(render(80),/Synthetic topology warning/);
    const seen=new Set();
    for(let i=0;i<83;i++){
     for(const match of render(80).matchAll(/Conversation (\d+)/g))seen.add(Number(match[1]));
     if(i<82)component.handleInput('\x1b[B');
    }
    assert.equal(seen.size,80);assert.match(render(80),/Conversation 79/);assert.match(render(80),/unavailable/);
    terminal.rows=12;render(24);reviewed=true;component.handleInput('\x1b');
   }else component.handleInput('\x1b');
  }),
  select:()=>assert.fail('terminal review must not use the unbounded ui.select dialog'),
 }),data,'/private/captures',controller.signal);
 assert.equal(reviewed,true);assert.equal(accepted,false);assert.equal(opened,3);assert.equal(getEventListeners(controller.signal,'abort').length,0);
});

test('capture confirmation aborts and disposes its listener without accepting',async()=>{
 const controller=new AbortController();
 await assert.rejects(confirmCapture(context({custom:async factory=>new Promise(done=>{
  const component=factory({terminal:{rows:16},requestRender(){}},theme,{},done);component.render(80);controller.abort();
 })}),inventory(),'/private/captures',controller.signal),/abort/i);
 assert.equal(getEventListeners(controller.signal,'abort').length,0);
});

test('capture review explicitly lists unidentified Pi panes with their locations and reasons',async()=>{
 const data=inventory(),controller=new AbortController();let menus=0,reviewed=false;
 data.topology_error='Synthetic missing identity';
 data.unresolved=[{workspace_id:'w1',tab_id:'t1',pane_id:'p2',terminal_id:'term2',harness:'pi',error:'Identity not available'}];
 const accepted=await confirmCapture({...context({select:async(title,entries)=>{
  if(title.startsWith('Capture inventory')){
   assert.ok(entries.some(line=>line.includes('Unresolved Pi')&&line.includes('w1/p2')&&line.includes('Identity not available')));reviewed=true;return undefined;
  }
  assert.match(title,/1 unresolved/);assert.match(title,/incomplete visibility/);
  return menus++===0?entries[1]:undefined;
 }}),mode:'rpc'},data,'/private/captures',controller.signal);
 assert.equal(reviewed,true);assert.equal(accepted,false);
});

test('RPC confirmation uses built-in dialogs and cancelling review never authorizes a capture',async()=>{
 const controller=new AbortController(),data=inventory();let menus=0;
 const accepted=await confirmCapture({...context({select:async(title,entries,{signal})=>{
  assert.equal(signal,controller.signal);
  if(title.startsWith('Capture inventory')){assert.ok(entries.some(line=>line.includes('Conversation 79')));return undefined;}
  assert.ok(title.length<300);return menus++===0?entries[1]:undefined;
 }}),mode:'rpc'},data,'/private/captures',controller.signal);
 assert.equal(accepted,false);assert.equal(menus,2);
});
