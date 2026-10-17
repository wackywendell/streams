"""Host-local catalog directories and explicit native workspace associations.

Only native transports are reused from the scratch frontend integrations. Catalog
records never pass through scratch resolution, projection, or storage.
"""

from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
import tempfile
import uuid
from pathlib import Path

from .agent_sessions import Agent, AgentSessionCommand
from .catalog.model import Session, Stream
from .catalog.operations import Authority
from .cmux import CmuxClient
from .errors import StreamError
from .frontend import Frontend, FrontendMode, resolve_frontend
from .herdr import HerdrClient, HerdrPane, HerdrWorkspace


def _key(*parts: str | None) -> str:
    return hashlib.sha256(json.dumps(parts, ensure_ascii=True).encode()).hexdigest()


def _text(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip()) and '\x00' not in value


def _object_pairs(pairs: list[tuple[str, object]]) -> dict:
    data = {}
    for key, value in pairs:
        if key in data:
            raise ValueError(f'duplicate JSON key {key!r}')
        data[key] = value
    return data


class Runtime:
    def __init__(self, state_dir: Path, host_id: str, frontend_mode: FrontendMode) -> None:
        if not _text(host_id):
            raise StreamError('runtime requires a nonempty host ID')
        self.state_dir = Path(state_dir) / _key(host_id)
        self.host_id = host_id
        self.frontend_mode = frontend_mode

    def _read(self, path: Path, fields: set[str]) -> dict | None:
        try:
            if path.is_symlink():
                raise ValueError('state file must not be a symlink')
            data = json.loads(path.read_text(), object_pairs_hook=_object_pairs)
        except FileNotFoundError:
            return None
        except (OSError, ValueError) as exc:
            raise StreamError(f'could not read runtime state {path}: {exc}') from exc
        if not isinstance(data, dict) or set(data) != fields | {'version'} or type(data['version']) is not int or data['version'] != 1:
            raise StreamError(f'invalid runtime state schema: {path}')
        return data

    def _write(self, path: Path, data: dict) -> None:
        temporary = None
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            fd, temporary = tempfile.mkstemp(prefix='.runtime-', dir=path.parent)
            with os.fdopen(fd, 'w') as handle:
                json.dump({'version': 1, **data}, handle)
                handle.write('\n')
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, path)
            self._sync_directory(path.parent)
        except OSError as exc:
            raise StreamError(f'could not write runtime state {path}: {exc}') from exc
        finally:
            if temporary is not None:
                Path(temporary).unlink(missing_ok=True)

    @staticmethod
    def _sync_directory(path: Path) -> None:
        descriptor = os.open(path, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    @contextmanager
    def _open_lock(self, scope: str, stream_id: str):
        path = self.state_dir / 'attempts' / scope / _key(stream_id)
        path.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(path.with_suffix('.lock'), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            attempt = path.with_suffix('.json')
            if self._read(attempt, {'stream_id', 'session_id', 'command', 'cwd', 'workspace_id'}) is not None:
                raise StreamError(f'pending or partial open at {attempt}; inspect native state before retrying; no automatic relaunch')
            yield attempt
        finally:
            os.close(descriptor)

    def _directory_path(self, stream_id: str, session_id: str | None) -> Path:
        return self.state_dir / 'directories' / (_key(stream_id, session_id) + '.json')

    def _directory_record(self, stream_id: str, session_id: str | None) -> dict | None:
        path = self._directory_path(stream_id, session_id)
        data = self._read(path, {'stream_id', 'session_id', 'cwd'})
        if data is not None and (data['stream_id'] != stream_id or data['session_id'] != session_id or not _text(data['cwd']) or not Path(data['cwd']).is_absolute()):
            raise StreamError(f'invalid directory identity or cwd in runtime state: {path}')
        return data

    @staticmethod
    def _checked_directory(cwd: Path) -> Path:
        try:
            resolved = Path(cwd).expanduser().resolve(strict=True)
            if not resolved.is_dir():
                raise ValueError('not a directory')
            return resolved
        except (OSError, ValueError, RuntimeError) as exc:
            raise StreamError(f'cwd must be an existing directory: {cwd}: {exc}; pass --cwd') from exc

    def remember_directory(self, stream_id: str, cwd: Path, *, session_id: str | None = None) -> None:
        if not _text(stream_id) or (session_id is not None and not _text(session_id)):
            raise StreamError('directory mapping requires nonempty stream/session IDs')
        directory = self._checked_directory(cwd)
        self._directory_record(stream_id, session_id)  # Never overwrite a corrupt mapping.
        self._write(self._directory_path(stream_id, session_id), {
            'stream_id': stream_id, 'session_id': session_id, 'cwd': str(directory),
        })

    def _cwd(self, stream: Stream, session: Session | None, cwd: Path | None) -> Path:
        if cwd is not None:
            return self._checked_directory(cwd)
        data = self._directory_record(stream.id, session.id) if session is not None else None
        if data is None:
            data = self._directory_record(stream.id, None)
        if data is None:
            raise StreamError(f'no local directory for stream {stream.id!r}; pass --cwd PATH')
        return self._checked_directory(Path(data['cwd']))

    def _authorize(self, authority: Authority | None) -> None:
        if not isinstance(authority, Authority) or authority.host_id != self.host_id:
            raise StreamError('effective owner authority for this host is required; pending owner fields do not grant runtime authority')

    def _select_session(self, stream: Stream, sessions: tuple[Session, ...], ref: str | None) -> Session | None:
        eligible = [session for session in sessions if session.host_id == self.host_id and session.stream_id == stream.id]
        if ref is None:
            matches = eligible
        else:
            matches = ([s for s in eligible if s.id == ref]
                       or [s for s in eligible if s.title.casefold() == ref.casefold()]
                       or [s for s in eligible if ref.strip() and ref.casefold() in s.title.casefold()])
            if not matches:
                raise StreamError(f'no eligible local session matches {ref!r}', kind='not-found')
        if len(matches) > 1:
            raise StreamError('ambiguous local sessions; choose --session: ' + ', '.join(f'{s.id} ({s.title})' for s in matches),
                              kind='ambiguous', candidates=[s.id for s in matches])
        return matches[0] if matches else None

    @staticmethod
    def _command(session: Session | None) -> AgentSessionCommand | None:
        if session is None:
            return None
        try:
            command = AgentSessionCommand(Agent(session.harness), session.harness_session_id)
        except ValueError as exc:
            raise StreamError(f'unsupported session harness {session.harness!r}') from exc
        if any(any(ord(char) < 32 or ord(char) == 127 for char in arg) for arg in command.argv):
            raise StreamError('session arguments must not contain control characters')
        return command

    def _frontend(self, frontend: str | None) -> Frontend:
        selected = resolve_frontend(self.frontend_mode, frontend, os.environ).resolved
        if selected is None:
            raise StreamError('no native frontend selected; pass --frontend herdr or --frontend cmux (or use resume)')
        return selected

    @staticmethod
    def _transport(frontend: Frontend):
        if frontend == Frontend.HERDR:
            client = HerdrClient()
            endpoint = client.endpoint()
            scope = _key('herdr', endpoint.socket, endpoint.session)
        else:
            client = CmuxClient()
            scope = _key('cmux', os.environ.get('CMUX_SOCKET_PATH') or str(Path.home() / '.local/state/cmux/cmux.sock'))
        return client, scope

    def _association_path(self, scope: str, workspace_id: str) -> Path:
        return self.state_dir / 'associations' / scope / (_key(workspace_id) + '.json')

    def _association(self, path: Path) -> dict | None:
        data = self._read(path, {'stream_id', 'workspace_id', 'session_id', 'command', 'ready'})
        if data is not None and (not _text(data['stream_id']) or not _text(data['workspace_id'])
                                 or (data['session_id'] is not None and not _text(data['session_id']))
                                 or (data['command'] is not None and not _text(data['command']))
                                 or (data['session_id'] is None) != (data['command'] is None)
                                 or type(data['ready']) is not bool
                                 or path.name != _key(data['workspace_id']) + '.json'):
            raise StreamError(f'invalid association identity in runtime state: {path}')
        return data

    def current_stream_id(self, *, frontend: str | None = None) -> str:
        if 'STREAMS_STREAM_ID' in os.environ:
            explicit = os.environ['STREAMS_STREAM_ID']
            if not _text(explicit):
                raise StreamError('STREAMS_STREAM_ID must be a nonempty stream ID')
            return explicit
        selected = self._frontend(frontend)
        client, scope = self._transport(selected)
        if selected == Frontend.HERDR:
            workspace_id = client.current_pane().workspace_id
            client.resolve_workspace(workspace_id)
        else:
            workspace_id = os.environ.get('CMUX_WORKSPACE_ID')
            if not workspace_id:
                raise StreamError('current CMUX targeting requires exact caller workspace context')
            matches = [w for w in client.workspaces() if w.id == workspace_id]
            if len(matches) != 1:
                raise StreamError('could not confirm exact CMUX caller workspace ID; no focused-workspace fallback')
        binding = self._association(self._association_path(scope, workspace_id))
        if binding is None:
            raise StreamError(f'workspace {workspace_id} has no catalog association; open an explicit stream first')
        return binding['stream_id']

    def open(self, stream: Stream, sessions: tuple[Session, ...], *, authority: Authority | None,
             frontend: str | None = None, session_ref: str | None = None, cwd: Path | None = None) -> dict:
        self._authorize(authority)
        session = self._select_session(stream, sessions, session_ref)
        command = self._command(session)
        directory = self._cwd(stream, session, cwd)
        selected = self._frontend(frontend)
        client, scope = self._transport(selected)
        with self._open_lock(scope, stream.id) as attempt_path:
            bindings = []
            for path in (self.state_dir / 'associations' / scope).glob('*.json'):
                binding = self._association(path)
                if binding is not None and binding['stream_id'] == stream.id:
                    bindings.append(binding)
            if len(bindings) > 1:
                raise StreamError('multiple native workspace associations for this stream; inspect runtime state before retrying')
            if bindings:
                binding = bindings[0]
                workspace_id = binding['workspace_id']
                try:
                    if selected == Frontend.HERDR:
                        workspace = client.resolve_workspace(workspace_id)
                        if workspace.id != workspace_id:
                            raise StreamError('workspace ID mismatch')
                    elif len([w for w in client.workspaces() if w.id == workspace_id]) != 1:
                        raise StreamError('exact workspace ID not found')
                except StreamError as exc:
                    raise StreamError(f'stale recorded workspace {workspace_id}: {exc}. Inspect native state and the local association; no automatic relaunch') from exc
                if not binding['ready']:
                    raise StreamError(f'workspace {workspace_id} has a partial open; inspect native state before retrying; no automatic relaunch')
                if binding['session_id'] != (session.id if session else None) or binding['command'] != (command.command_text if command else None):
                    raise StreamError(f'workspace {workspace_id} has a different session; refusing to duplicate or replace it; inspect native state')
                if cwd is not None:
                    self.remember_directory(stream.id, directory, session_id=session.id if session else None)
                if selected == Frontend.HERDR:
                    client.run(['workspace', 'focus', workspace_id])
                else:
                    client.run(['cmux', 'select-workspace', '--workspace', workspace_id])
                return {'stream_id': stream.id, 'session_id': binding['session_id'], 'frontend': selected.value,
                        'workspace_id': workspace_id, 'reused': True}

            if any(ord(char) < 32 or ord(char) == 127 for char in stream.title):
                raise StreamError('workspace title must not contain control characters')
            self.remember_directory(stream.id, directory, session_id=session.id if session else None)
            attempt = {'stream_id': stream.id, 'session_id': session.id if session else None,
                       'command': command.command_text if command else None, 'cwd': str(directory), 'workspace_id': None}
            self._write(attempt_path, attempt)
            resources = ['workspace creation attempted']
            try:
                if selected == Frontend.HERDR:
                    created = client.command_result(['workspace', 'create', '--cwd', str(directory), '--label', stream.title,
                                                     '--env', f'STREAMS_STREAM_ID={stream.id}', '--focus'], 'workspace_created')
                    for field, key, label in (('workspace', 'workspace_id', 'workspace'), ('tab', 'tab_id', 'tab'), ('root_pane', 'pane_id', 'pane')):
                        resource = created.get(field)
                        if isinstance(resource, dict) and _text(resource.get(key)):
                            resources.append(f'{label} {resource[key]}')
                    workspace_id = HerdrWorkspace.parse(created.get('workspace')).id
                else:
                    args = ['cmux', '--json', '--id-format', 'uuids', 'new-workspace', '--name', stream.title,
                            '--cwd', str(directory), '--env', f'STREAMS_STREAM_ID={stream.id}', '--focus', 'true']
                    if command:
                        args.extend(['--command', command.command_text])
                    created = client.json_from_command(args)
                    workspace_id = created.get('workspace_id') if isinstance(created, dict) else None
                    if not _text(workspace_id):
                        raise StreamError(f'CMUX creation returned no workspace ID: {created!r}')
                    resources.append(f'workspace {workspace_id}')
                    for key, value in created.items():
                        if key != 'workspace_id' and key.endswith('_id') and _text(value):
                            resources.append(f'{key} {value}')
                attempt['workspace_id'] = workspace_id
                self._write(attempt_path, attempt)
                path = self._association_path(scope, workspace_id)
                previous = self._association(path)
                if previous is not None:
                    raise StreamError(f'workspace {workspace_id} already has an association; refusing to overwrite it')
                binding = {'stream_id': stream.id, 'workspace_id': workspace_id, 'session_id': session.id if session else None,
                           'command': command.command_text if command else None, 'ready': False}
                self._write(path, binding)
                if selected == Frontend.HERDR:
                    tab = created.get('tab')
                    pane = HerdrPane.parse(created.get('root_pane'))
                    if not isinstance(tab, dict) or tab.get('workspace_id') != workspace_id or pane.workspace_id != workspace_id or pane.tab_id != tab.get('tab_id'):
                        raise StreamError('created Herdr pane/tab workspace identity mismatch')
                    if command:
                        name = 'streams-' + uuid.uuid4().hex[:24]
                        resources.append(f'agent {name} start attempted')
                        started = client.command_result(['agent', 'start', name, '--kind', command.agent.value, '--pane', pane.id,
                                                         '--timeout', '20000', '--', *command.argv[1:]], 'agent_started').get('agent')
                        if (not isinstance(started, dict) or started.get('pane_id') != pane.id or started.get('workspace_id') != workspace_id
                                or started.get('tab_id') != pane.tab_id or started.get('name') != name
                                or started.get('agent') != command.agent.value or started.get('interactive_ready') is not True):
                            raise StreamError('Herdr agent identity/readiness mismatch')
                binding['ready'] = True
                self._write(path, binding)
                attempt_path.unlink()
                self._sync_directory(attempt_path.parent)
            except (StreamError, OSError, ValueError) as exc:
                raise StreamError(f'native open may be partial; {"; ".join(resources)}; {exc}. Inspect native state before retrying; no rollback or automatic relaunch') from exc
            return {'stream_id': stream.id, 'session_id': session.id if session else None, 'frontend': selected.value,
                    'workspace_id': workspace_id, 'reused': False}

    def resume(self, stream: Stream, sessions: tuple[Session, ...], *, authority: Authority | None,
               session_ref: str | None = None, cwd: Path | None = None) -> None:
        self._authorize(authority)
        session = self._select_session(stream, sessions, session_ref)
        command = self._command(session)
        if command is None:
            raise StreamError(f'stream {stream.id!r} has no eligible local session to resume')
        directory = self._cwd(stream, session, cwd)
        if cwd is not None:
            self.remember_directory(stream.id, directory, session_id=session.id)
        try:
            os.chdir(directory)
            os.environ['STREAMS_STREAM_ID'] = stream.id
            os.execvp(command.argv[0], list(command.argv))
        except OSError as exc:
            raise StreamError(f'could not resume {session.id!r}: {exc}') from exc
