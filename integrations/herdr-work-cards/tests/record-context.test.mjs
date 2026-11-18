import assert from 'node:assert/strict';
import test from 'node:test';
import {
  isReferenceTarget, isReferenceText, isReferenceValue, isRepositoryDocument,
  isResourceKey, isSession, isStream, parseLocalContext, referenceUrl, resourceKey,
} from '../dist/streams-context.js';

const stream = {
  id:'effort', title:'Streams', title_mode:'inherit', goal:null, goal_short:null, goal_mode:'automatic',
  status:'active', owner:{host_id:'host'}, repositories:{}, references:[],
};
const session = {
  id:'ab441ac4-d600-5fd6-babf-fc43ab65623d', host_id:'host', stream_id:'effort', harness:'pi', harness_session_id:'native',
  membership_epoch:0, title:'Catalog', title_mode:'inherit', status:'active', state:'', state_short:null, remaining:'', checkout:null,
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


const reference = () => ({
  name:'Reference', purpose:'Define acceptance', role:'brief', state:'active',
  target:{kind:'link', url:'https://EXAMPLE.test/brief'},
});

test('standalone references preserve exact values, roles, states and resource keys', () => {
  for (const kind of ['pr', 'issue', 'link', 'document']) {
    const value = reference();
    value.target = kind === 'document'
      ? {kind, repository_url:'git@example.test:tools', path:'docs/design.md'}
      : {kind, url:'https://EXAMPLE.test/item'};
    const original = structuredClone(value);
    for (const role of ['deliverable', 'dependency', 'brief', 'background']) {
      for (const state of ['active', 'fulfilled', 'dropped']) {
        assert.equal(isReferenceValue({...value, role, state}), true);
      }
    }
    assert.equal(isReferenceValue(value), true);
    assert.deepEqual(value, original);
    const key = resourceKey(value.target);
    assert.equal(isResourceKey(key), true);
    if (kind === 'document') {
      assert.deepEqual(resourceKey({...value.target, ref:'topic', commit:'new', source_session_id:'source'}), key);
    } else {
      for (const other of ['pr', 'issue', 'link']) {
        assert.deepEqual(resourceKey({...value.target, kind:other}), key);
      }
      assert.notDeepEqual(resourceKey({...value.target, url:'https://example.test/item'}), key);
    }
    assert.equal(isReferenceValue({...value, revision:'old'}), false);
    assert.equal(isReferenceValue({...value, extra:true}), false);
    assert.equal(isResourceKey({...key, extra:true}), false);
    assert.equal(isReferenceValue({...value, target:{...value.target, extra:true}}), false);
    for (const field of ['name', 'purpose', 'role', 'state', 'target']) {
      const invalid = structuredClone(value);
      delete invalid[field];
      assert.equal(isReferenceValue(invalid), false, field);
    }
    assert.equal(isReferenceValue({...value, role:'Brief'}), false);
    assert.equal(isReferenceValue({...value, state:'unknown'}), false);
    assert.equal(isReferenceTarget({...value.target, kind:'unknown'}), false);
  }
});

test('reference text matches Rust whitespace and Unicode scalar validity without trimming', () => {
  for (const text of ['\ufeff', '😀', 'leading\nnewline', "--value='exact'", '\u001b']) {
    assert.equal(isReferenceText(text), true, JSON.stringify(text));
    for (const field of ['name', 'purpose']) {
      const value = {...reference(), [field]:text};
      assert.equal(isReferenceValue(value), true, field);
      assert.equal(value[field], text);
    }
  }
  for (const text of ['', ' \n', '\u0085', '\u00a0\u3000', 'bad\0text', '\ud800', '\udfff', 'bad\ud800text']) {
    assert.equal(isReferenceText(text), false, JSON.stringify(text));
    for (const field of ['name', 'purpose']) {
      assert.equal(isReferenceValue({...reference(), [field]:text}), false, field);
    }
  }
  for (const value of [null, undefined, 1, true, [], {}]) assert.equal(isReferenceText(value), false);
});

test('repository documents have portable paths and optional nullable recovery context', () => {
  const document = {repository_url:'git@example.test:tools', path:'docs/design.md'};
  assert.equal(isRepositoryDocument(document), true);
  assert.equal(isRepositoryDocument({...document, description:'old relationship'}), false);
  for (const field of ['ref', 'commit', 'source_session_id']) {
    for (const value of [undefined, null, 'established', '\ufeff', '😀']) {
      assert.equal(isRepositoryDocument({...document, [field]:value}), true);
    }
    for (const value of ['', ' \n', '\u0085', 'bad\0text', '\ud800', '\udfff', 42]) {
      assert.equal(isRepositoryDocument({...document, [field]:value}), false, field);
    }
  }
  for (const path of ['/absolute', 'C:drive', 'docs\\file', '../parent', 'docs/../file', '', '\u0085']) {
    assert.equal(isRepositoryDocument({...document, path}), false, path);
    assert.equal(isResourceKey({kind:'document', ...document, path}), false, path);
  }
  for (const field of ['repository_url', 'path']) {
    for (const text of ['\ufeff', '😀', 'exact\ntext']) {
      assert.equal(isRepositoryDocument({...document, [field]:text}), true);
    }
    for (const text of ['', '\u0085', '\0', '\ud800', '\udfff']) {
      assert.equal(isRepositoryDocument({...document, [field]:text}), false);
    }
  }
});

test('reference web targets share raw and decoded safety without normalising keys', () => {
  for (const url of [
    'https://EXAMPLE.test/path', 'http://localhost:8080/a%20b', 'https://[::1]/',
    'https://example.test/%F0%9F%98%80',
  ]) {
    assert.ok(referenceUrl(url), url);
    assert.equal(isReferenceValue({...reference(), target:{kind:'link', url}}), true);
    assert.deepEqual(resourceKey({kind:'link', url}), {kind:'url', url});
  }
  for (const url of [
    'file:///tmp/doc', 'javascript:alert(1)', 'https://', 'https://user@example.test',
    'https://user:secret@example.test', 'https://:secret@example.test', 'https://example.test/a b',
    'https://example.test/\\path', 'https://example.test/\u0085', 'https://example.test/\ufeff',
    'https://example.test/%', 'https://example.test/%0', 'https://example.test/%gg',
    'https://example.test/%FF', 'https://example.test/%ED%A0%80', 'https://example.test/%00',
    'https://example.test/%1b', 'https://example.test/%E2%80%AE',
  ]) {
    assert.equal(referenceUrl(url), undefined, url);
    assert.equal(isResourceKey({kind:'url', url}), false, url);
    for (const kind of ['pr', 'issue', 'link']) assert.equal(isReferenceTarget({kind, url}), false, url);
  }
});

test('Stream requires references and rejects old fields and duplicate exact keys', () => {
  assert.equal(isStream(stream), true);
  const {references, ...missing} = stream;
  assert.equal(isStream(missing), false);
  for (const field of ['documents', 'issues', 'prs']) assert.equal(isStream({...stream, [field]:[]}), false);
  const a = reference();
  assert.equal(isStream({...stream, references:[a, {...a, target:{kind:'pr', url:a.target.url}}]}), false);
  assert.equal(isStream({...stream, references:[a, {...a, target:{kind:'pr', url:'https://example.test/brief'}}]}), true);
  const doc = {...a, target:{kind:'document', repository_url:'repo', path:'plan', ref:'one'}};
  assert.equal(isStream({...stream, references:[doc, {...doc, target:{...doc.target, ref:'two'}}]}), false);
});

test('checkout wire permits absent repository URL without guessing it', () => {
  const checkout = {repository:'repo', repository_url:'https://example.test/repo', ref:'main', directory_within_checkout:'.'};
  assert.equal(isSession({...session, checkout}), true);
  for (const repository_url of [undefined, null]) assert.equal(isSession({...session, checkout:{...checkout, repository_url}}), true);
  for (const repository_url of ['', '  ']) assert.equal(isSession({...session, checkout:{...checkout, repository_url}}), false);
});
