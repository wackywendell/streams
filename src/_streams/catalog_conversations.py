"""Bounded native conversation identity checks; filenames are discovery hints only."""
from __future__ import annotations

import json
import os
from pathlib import Path

from .errors import StreamError


LIMIT = 65536


def _pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate JSON key {key!r}')
        result[key] = value
    return result


def file_identity(harness: str, path: Path) -> str:
    if not path.is_absolute():
        raise StreamError('saved conversation path must be absolute')
    try:
        with path.open('rb') as handle:
            prefix = handle.read(LIMIT + 1)
        lines = prefix.splitlines()
        if not lines or (len(prefix) > LIMIT and b'\n' not in prefix[:LIMIT]):
            raise ValueError('missing or oversized conversation header')
        if harness in ('pi', 'codex'):
            header = json.loads(lines[0], object_pairs_hook=_pairs)
            if not isinstance(header, dict):
                raise ValueError('invalid conversation header')
            expected = 'session' if harness == 'pi' else 'session_meta'
            if header.get('type') != expected:
                raise ValueError('wrong conversation header type')
            value = header.get('id') if harness == 'pi' else header.get('payload', {}).get('id')
        elif harness == 'claude':
            # A truncated final line is not a complete record. Every complete
            # identity-bearing record in the bounded prefix must agree.
            if len(prefix) > LIMIT or not prefix.endswith(b'\n'):
                lines = lines[:-1]
            identities = set()
            for line in lines:
                record = json.loads(line, object_pairs_hook=_pairs)
                if not isinstance(record, dict):
                    raise ValueError('invalid conversation record')
                if 'sessionId' in record:
                    identity = record['sessionId']
                    if not isinstance(identity, str) or not identity.strip():
                        raise ValueError('invalid Claude sessionId')
                    identities.add(identity)
            if len(identities) != 1:
                raise ValueError('missing or ambiguous Claude sessionId')
            value = identities.pop()
        else:
            raise ValueError(f'unsupported session harness {harness!r}')
        if not isinstance(value, str) or not value.strip() or any(ord(c) < 32 or ord(c) == 127 for c in value):
            raise ValueError('invalid native conversation identity')
        return value
    except (OSError, ValueError, TypeError, AttributeError) as exc:
        raise StreamError(f'unavailable saved conversation {path}: {exc}') from exc


def native_identity(metadata: object, harness: str) -> tuple[str, str | None]:
    if not isinstance(metadata, dict) or metadata.get('agent') != harness:
        raise StreamError('invalid native agent_session harness')
    value = metadata.get('value')
    if not isinstance(value, str) or not value.strip() or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise StreamError('invalid native agent_session value')
    if metadata.get('kind') == 'id':
        return value, None
    if metadata.get('kind') == 'path':
        path = Path(value)
        return file_identity(harness, path), str(path.resolve())
    raise StreamError('invalid native agent_session kind')


def conversation_file(harness: str, identity: str, locator: str | None = None) -> Path:
    if locator is not None:
        path = Path(locator)
        if file_identity(harness, path) != identity:
            raise StreamError('saved conversation identity/path mismatch')
        return path.resolve()
    home = Path.home()
    roots = {
        'pi': [Path(os.environ.get('PI_CODING_AGENT_DIR', str(home / '.pi/agent'))) / 'sessions'],
        'codex': [Path(os.environ.get('CODEX_HOME', str(home / '.codex'))) / 'sessions',
                  Path(os.environ.get('CODEX_HOME', str(home / '.codex'))) / 'archived_sessions'],
        'claude': [Path(os.environ.get('CLAUDE_CONFIG_DIR', str(home / '.claude'))) / 'projects'],
    }
    if harness not in roots:
        raise StreamError(f'unsupported session harness {harness!r}')
    matches = set()
    count = 0
    try:
        for root in roots[harness]:
            for path in root.rglob('*.jsonl'):
                count += 1
                if count > 10000:
                    raise StreamError('conversation discovery limit exceeded; capture an exact saved locator')
                if identity not in path.name:
                    continue
                if file_identity(harness, path) == identity:
                    matches.add(path.resolve())
    except OSError as exc:
        raise StreamError(f'could not discover saved conversation: {exc}') from exc
    if len(matches) != 1:
        raise StreamError(f'saved conversation {identity!r} unavailable or ambiguous ({len(matches)} verified files)')
    return matches.pop()
