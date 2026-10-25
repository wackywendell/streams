import assert from 'node:assert/strict';
import test from 'node:test';
import { isSession, parseLocalContext } from '../dist/streams-context.js';

const stream = {
  id:'effort', title:'Streams', title_mode:'inherit', goal:null, goal_short:null, goal_mode:'automatic',
  status:'active', archived:false, owner:{host_id:'host'}, repositories:{}, documents:[], issues:[], prs:[],
};
const session = {
  id:'ab441ac4-d600-5fd6-babf-fc43ab65623d', host_id:'host', stream_id:'effort', harness:'pi', harness_session_id:'native',
  previous_stream_ids:[], title:'Catalog', title_mode:'inherit', status:'active', state:'', state_short:null, remaining:'', checkout:null,
};
const context = () => structuredClone({
  catalog_source:'source', host_id:'host', title_defaults:{stream:'automatic', session:'manual'}, stream, session, sessions:[session],
});

test('record-local context requires fresh title defaults and rejects Authority-bearing packets', () => {
  const current = context();
  assert.deepEqual(parseLocalContext(current), current);
  for (const change of [value => delete value.title_defaults, value => value.title_defaults.stream = 'inherit', value => value.authority = null]) {
    const invalid = context(); change(invalid);
    assert.throws(() => parseLocalContext(invalid), /Invalid local catalog context/);
  }
});

test('addressed context rejects a roster and inconsistent host, membership or native identity', () => {
  for (const change of [
    value => value.stream.owner.host_id = 'foreign',
    value => value.session.stream_id = 'other',
    value => value.sessions.push({...value.session, id:'sibling'}),
    value => value.sessions = [],
    value => value.sessions[0] = {...value.session, harness_session_id:'other-native'},
  ]) {
    const invalid = context(); change(invalid);
    assert.throws(() => parseLocalContext(invalid), /Inconsistent addressed catalog context/);
  }
  assert.doesNotThrow(() => parseLocalContext({...context(), session:null, sessions:[]}));
  const foreign = context();
  foreign.stream.owner.host_id = foreign.session.host_id = 'foreign';
  assert.doesNotThrow(() => parseLocalContext(foreign));
});

test('checkout wire requires its independent repository URL', () => {
  const checkout = {repository:'repo', repository_url:'https://example.test/repo', ref:'main', directory_within_checkout:'.'};
  assert.equal(isSession({...session, checkout}), true);
  for (const repository_url of [undefined, null, '', '  ']) {
    assert.equal(isSession({...session, checkout:{...checkout, repository_url}}), false);
  }
});
