"""Read-only inventory of Pi replay inputs; never enroll or repair native state."""
from __future__ import annotations

from . import catalog_arrangement as arrangement
from .catalog.codec import record_to_data
from .catalog_agent import source_id
from .catalog_conversations import conversation_file, native_identity
from .errors import StreamError
from .frontend import Frontend


def capture_inputs(backend, runtime, *, stream_id=None, workspace_id=None):
    if stream_id is not None and workspace_id is not None:
        raise StreamError('select a stream or a workspace, not both')
    snapshot, authorities = backend.inspect()
    catalog = snapshot.catalog
    source = source_id(backend)
    selected = catalog.stream(stream_id) if stream_id is not None else None
    if stream_id is not None and selected is None:
        raise StreamError(f'no stream with exact ID {stream_id!r}', kind='not-found')

    topology, bindings, errors = None, {}, []
    try:
        client, scope = runtime._transport(Frontend.HERDR)
        for binding in runtime.association_inventory(scope):
            if not binding['ready']:
                continue
            identity = binding['workspace_id']
            if identity in bindings or catalog.stream(binding['stream_id']) is None:
                raise StreamError('ambiguous or dangling ready workspace association')
            bindings[identity] = binding['stream_id']
        topology = arrangement.structural_inventory(client)
        for workspace in topology['workspaces']:
            if not isinstance(workspace.get('label'), str):
                raise StreamError('invalid Herdr snapshot workspace label')
    except (StreamError, OSError, ValueError) as exc:
        topology = None
        errors.append(str(exc))

    workspaces = [
        {'id': w['workspace_id'], 'label': w['label'], 'stream_id': bindings.get(w['workspace_id'])}
        for w in topology['workspaces']
    ] if topology is not None else []
    result = {'version': 1, 'host_id': backend.host_id,
              'streams': [record_to_data(s) for s in catalog.streams],
              'workspaces': workspaces, 'topology_error': '; '.join(errors) or None}
    if stream_id is None and workspace_id is None:
        return result
    if workspace_id is not None:
        matches = [w for w in workspaces if w['id'] == workspace_id]
        if not matches:
            raise StreamError(f'workspace {workspace_id!r} not observed in valid Herdr topology', kind='not-found')
        workspace = matches[0]
        selected = catalog.stream(workspace['stream_id']) if workspace['stream_id'] else None
        target = {'kind': 'workspace', 'id': workspace_id, 'title': workspace['label']}
        workspace_ids = [workspace_id]
    else:
        target = {'kind': 'stream', 'id': selected.id, 'title': selected.title}
        workspace_ids = [w['id'] for w in workspaces if w['stream_id'] == selected.id]

    live, unsupported, unresolved = {}, [], []

    def unresolved_agent(agent, error):
        unresolved.append({key: agent[key] for key in ('workspace_id', 'tab_id', 'pane_id', 'terminal_id')}
                          | {'harness': 'pi', 'error': str(error)})
        errors.append(f"workspace {agent['workspace_id']!r}, pane {agent['pane_id']!r}: {error}")

    for identity in workspace_ids:
        agents = [a for a in topology['agents'] if a['workspace_id'] == identity]
        # Unsupported panes are observations, not identities to resolve from disk.
        unsupported.extend({'harness': a.get('agent'), 'agent_session': a.get('agent_session'),
                            'workspace_id': identity, 'pane_id': a['pane_id']}
                           for a in agents if a.get('agent') != 'pi')
        # Capture may retain partial observations; live attachment/restore keeps
        # its strict all-or-nothing workspace inventory and duplicate guards.
        candidates = {}
        for agent in agents:
            if agent.get('agent') != 'pi':
                continue
            try:
                native_id, locator = native_identity(agent.get('agent_session'), 'pi')
                candidates.setdefault(native_id, []).append({**agent, 'native_id': native_id, 'locator': locator})
            except (StreamError, OSError, ValueError) as exc:
                unresolved_agent(agent, exc)
        for native_id, matches in candidates.items():
            if len(matches) > 1:
                for agent in matches:
                    unresolved_agent(agent, 'ambiguous live native conversation identity')
                continue
            live.setdefault(native_id, []).extend(matches)

    known = {}
    for session in catalog.sessions:
        key = (session.host_id, session.harness, session.harness_session_id)
        if key in known:
            raise StreamError('ambiguous catalog native conversation identity')
        known[key] = session
    samples = {}
    if selected is not None:
        for session in catalog.sessions:
            if session.stream_id != selected.id:
                continue
            if session.harness != 'pi':
                unsupported.append({'host_id': session.host_id, 'harness': session.harness,
                                    'native_session_id': session.harness_session_id})
                continue
            samples[(session.host_id, session.harness_session_id)] = session
    for native_id in live:
        samples[(backend.host_id, native_id)] = known.get((backend.host_id, 'pi', native_id))

    # Do not resolve unrelated workspace transcripts just to establish liveness.
    # Their unverified identities could include any historical local conversation.
    if (topology is not None
            and any(a.get('agent') == 'pi' and a['workspace_id'] not in workspace_ids for a in topology['agents'])
            and any(host == backend.host_id and native_id not in live for host, native_id in samples)):
        errors.append('Pi identities outside selected workspaces were not verified; historical open state unknown')

    def context(session, fallback):
        stream = catalog.stream(session.stream_id) if session else fallback
        authority = None
        if stream is not None:
            value = authorities.get(stream.id)
            authority = {'host_id': value.host_id, 'token': '[redacted]'} if value is not None else None
        return {'catalog_source': source, 'host_id': backend.host_id,
                'stream': record_to_data(stream) if stream else None,
                'session': record_to_data(session) if session else None,
                'sessions': [record_to_data(s) for s in catalog.sessions if stream and s.stream_id == stream.id],
                'authority': authority}

    captured = []
    for (host, native_id), session in sorted(samples.items()):
        local = host == backend.host_id
        agents = live.get(native_id, []) if local else []
        locations = [{key: a[key] for key in ('workspace_id', 'tab_id', 'pane_id', 'terminal_id')} for a in agents]
        fallback_ids = {bindings.get(a['workspace_id']) for a in agents}
        if session is None and len(fallback_ids) > 1:
            raise StreamError('ambiguous workspace stream context for uncarded conversation')
        fallback = catalog.stream(next(iter(fallback_ids))) if fallback_ids else None
        transcript, unavailable = None, None
        if not local:
            unavailable = 'foreign host'
        else:
            try:
                locators = {a['locator'] for a in agents if a['locator'] is not None}
                if len(locators) > 1:
                    raise StreamError('ambiguous saved conversation locators')
                transcript = str(conversation_file('pi', native_id, next(iter(locators), None)))
            except (StreamError, OSError, ValueError) as exc:
                unavailable = str(exc)
        captured.append({'host_id': host, 'harness': 'pi', 'native_session_id': native_id,
                         'open': True if agents else None if errors or not local else False,
                         'locations': locations, 'catalog': context(session, fallback),
                         'transcript_file': transcript, 'unavailable': unavailable})
    result.update(target=target, samples=captured, unsupported=unsupported, unresolved=unresolved,
                  topology_error='; '.join(errors) or None)
    return result
