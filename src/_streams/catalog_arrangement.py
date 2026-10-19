"""Host-local restoration intent and verified Herdr inventory.

Plans contain only observed catalog conversations, never catalog history inferred
as launch intent. Native IDs are observations, not portable catalog identity.
"""
from __future__ import annotations

from .catalog_conversations import native_identity
from .errors import StreamError


def _id(record, field):
    value = record.get(field) if isinstance(record, dict) else None
    if not isinstance(value, str) or not value.strip() or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise StreamError(f'invalid Herdr snapshot {field}')
    return value


def structural_inventory(client) -> dict:
    """Read complete host topology without opening conversation files."""
    result = client.command_result(['api', 'snapshot'], 'session_snapshot')
    return parse_topology(result.get('snapshot') if isinstance(result, dict) else None)


def parse_topology(snapshot: object) -> dict:
    """Validate all native ownership/layout relationships, not conversation identity."""
    if not isinstance(snapshot, dict):
        raise StreamError('invalid Herdr snapshot; complete positive inventory required')
    for field in ('workspaces', 'tabs', 'panes', 'layouts', 'agents'):
        if not isinstance(snapshot.get(field), list) or len(snapshot[field]) > 10000:
            raise StreamError(f'invalid Herdr snapshot {field}; complete positive inventory required')
    indexed = {}
    for collection, key in (('workspaces', 'workspace_id'), ('tabs', 'tab_id'), ('panes', 'pane_id'), ('layouts', 'tab_id')):
        records = snapshot[collection]
        mapping = {_id(record, key): record for record in records}
        if len(mapping) != len(records):
            raise StreamError(f'ambiguous Herdr snapshot {collection}')
        indexed[collection] = mapping
    for tab in snapshot['tabs']:
        if _id(tab, 'workspace_id') not in indexed['workspaces'] or not isinstance(tab.get('label'), str):
            raise StreamError('invalid Herdr snapshot tab workspace/label')
    terminals = set()
    for pane in snapshot['panes']:
        tab = indexed['tabs'].get(_id(pane, 'tab_id'))
        terminal = _id(pane, 'terminal_id')
        if tab is None or tab['workspace_id'] != _id(pane, 'workspace_id') or terminal in terminals:
            raise StreamError('invalid Herdr snapshot pane ownership')
        terminals.add(terminal)
    if set(indexed['layouts']) != set(indexed['tabs']):
        raise StreamError('incomplete Herdr snapshot layouts')
    layout_panes = []
    for layout in snapshot['layouts']:
        tab = indexed['tabs'][layout['tab_id']]
        if _id(layout, 'workspace_id') != tab['workspace_id'] or not isinstance(layout.get('panes'), list):
            raise StreamError('invalid Herdr snapshot layout')
        for entry in layout['panes']:
            pane_id = _id(entry, 'pane_id')
            pane = indexed['panes'].get(pane_id)
            if pane is None or pane['tab_id'] != tab['tab_id']:
                raise StreamError('invalid Herdr snapshot layout pane')
            layout_panes.append(pane_id)
    if len(set(layout_panes)) != len(layout_panes) or set(layout_panes) != set(indexed['panes']):
        raise StreamError('incomplete Herdr snapshot pane inventory')
    for agent in snapshot['agents']:
        pane_id = _id(agent, 'pane_id')
        pane = indexed['panes'].get(pane_id)
        if pane is None or any(_id(agent, key) != pane[key] for key in ('workspace_id', 'tab_id', 'terminal_id')):
            raise StreamError('invalid Herdr snapshot agent ownership')
    return snapshot


def _verified_agents(agents) -> list[dict]:
    verified = []
    seen = set()
    for agent in agents:
        harness = agent.get('agent')
        if harness not in ('pi', 'codex', 'claude'):
            continue
        identity, locator = native_identity(agent.get('agent_session'), harness)
        if (harness, identity) in seen:
            raise StreamError('ambiguous live native conversation identity')
        seen.add((harness, identity))
        verified.append({**agent, 'native_id': identity, 'locator': locator})
    return verified


def verified_inventory(topology: dict) -> dict:
    """Enrich every supported host conversation; unresolved or duplicate IDs fail closed."""
    return {**topology, 'agents': _verified_agents(topology['agents'])}


def workspace_inventory(topology: dict, workspace_id: str) -> dict:
    """Enrich all target-workspace conversations or fail, never return a partial subset."""
    agents = [a for a in topology['agents'] if a['workspace_id'] == workspace_id]
    return {**topology, 'agents': _verified_agents(agents)}


def inventory(client) -> dict:
    """Read structurally complete, identity-verified host inventory for duplicate guards."""
    return verified_inventory(structural_inventory(client))


def capture(snapshot: dict, workspace_id: str, stream, sessions, host_id: str) -> dict:
    if not any(w['workspace_id'] == workspace_id for w in snapshot['workspaces']):
        raise StreamError('bound workspace absent from verified snapshot')
    known = {}
    for session in sessions:
        if session.stream_id == stream.id and session.host_id == host_id:
            key = (session.harness, session.harness_session_id)
            if key in known:
                raise StreamError('ambiguous catalog native conversation identity')
            known[key] = session
    tabs = []
    for tab in snapshot['tabs']:
        if tab['workspace_id'] != workspace_id:
            continue
        entries = []
        layout = next(item for item in snapshot['layouts'] if item['tab_id'] == tab['tab_id'])
        for pane in layout['panes']:
            for agent in snapshot['agents']:
                if agent['pane_id'] != pane['pane_id']:
                    continue
                session = known.get((agent['agent'], agent['native_id']))
                if session is None:
                    continue
                entries.append({'session_id': session.id, 'harness': session.harness,
                                'native_id': session.harness_session_id, 'locator': agent['locator'],
                                'pane_id': agent['pane_id'], 'terminal_id': agent['terminal_id']})
        if entries:
            # No provenance can be inferred from a label's words. Preserve every
            # non-default label as manual, including ones matching agent titles.
            label = tab['label']
            default = not label or label == str(tab.get('number'))
            tabs.append({'tab_id': tab['tab_id'], 'label': None if default else label,
                         'label_mode': 'automatic' if default else 'manual', 'sessions': entries})
    return {'stream_id': stream.id, 'workspace_id': workspace_id, 'tabs': tabs}


def validate_plan(plan: dict, stream_id: str) -> dict:
    if plan.get('stream_id') != stream_id or not isinstance(plan.get('tabs'), list):
        raise StreamError('invalid saved arrangement identity')
    _id(plan, 'workspace_id')
    seen = set()
    for tab in plan['tabs']:
        if (not isinstance(tab, dict) or not isinstance(tab.get('sessions'), list)
                or tab.get('label_mode') not in ('manual', 'automatic')
                or (tab.get('label') is not None and not isinstance(tab['label'], str))):
            raise StreamError('invalid saved arrangement tab')
        _id(tab, 'tab_id')
        if tab.get('label') is not None and any(ord(c) < 32 or ord(c) == 127 for c in tab['label']):
            raise StreamError('invalid saved arrangement tab label')
        for entry in tab['sessions']:
            for field in ('session_id', 'harness', 'native_id'):
                _id(entry, field)
            if entry['session_id'] in seen or (entry.get('locator') is not None and not isinstance(entry['locator'], str)):
                raise StreamError('invalid saved arrangement conversation')
            if entry.get('locator') is not None:
                _id(entry, 'locator')
            seen.add(entry['session_id'])
    return plan
