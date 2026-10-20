"""Host-local catalog directories and explicit native workspace associations."""

from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
import tempfile
import uuid
from pathlib import Path
from typing import Iterator

from .agent_sessions import Agent, AgentSessionCommand
from .catalog.model import Catalog, Session, Stream
from .catalog.operations import Authority
from .catalog_checkouts import CheckoutRegistry
from .catalog_locations import DirectoryBinding, DirectoryBindings
from . import catalog_arrangement as arrangement
from .catalog_conversations import conversation_file, native_identity
from .cmux import CmuxClient
from .errors import StreamError
from .frontend import Frontend, FrontendMode, resolve_frontend
from .herdr import HerdrClient, HerdrPane, HerdrWorkspace


def _key(*parts: str | None) -> str:
    return hashlib.sha256(json.dumps(parts, ensure_ascii=True).encode()).hexdigest()


def _text(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip()) and '\x00' not in value


def _native_environment(stream_id: str) -> list[str]:
    # GUI-created terminals inherit the server environment, not this CLI's.
    # Pin selection paths (including defaults), never credentials or caller panes.
    home = Path.home()
    values = {
        'STREAMS_STREAM_ID': stream_id,
        'HOME': str(home),
        'XDG_CONFIG_HOME': os.environ.get('XDG_CONFIG_HOME') or str(home / '.config'),
        'XDG_DATA_HOME': os.environ.get('XDG_DATA_HOME') or str(home / '.local/share'),
        'XDG_STATE_HOME': os.environ.get('XDG_STATE_HOME') or str(home / '.local/state'),
        'PI_CODING_AGENT_DIR': os.environ.get('PI_CODING_AGENT_DIR') or str(home / '.pi/agent'),
        'CODEX_HOME': os.environ.get('CODEX_HOME', str(home / '.codex')),
        'CLAUDE_CONFIG_DIR': os.environ.get('CLAUDE_CONFIG_DIR', str(home / '.claude')),
        'PATH': os.environ.get('PATH', os.defpath),
        'HERDR_BIN_PATH': os.environ.get('HERDR_BIN_PATH') or 'herdr',
    }
    for key in ('HERDR_SOCKET_PATH', 'PI_OFFLINE'):
        if key in os.environ:
            values[key] = os.environ[key]
    return [argument for key, value in values.items() for argument in ('--env', f'{key}={value}')]


def _object_pairs(pairs: list[tuple[str, object]]) -> dict:
    data = {}
    for key, value in pairs:
        if key in data:
            raise ValueError(f'duplicate JSON key {key!r}')
        data[key] = value
    return data


class Enrollment:
    """One caller-workspace enrollment, usable only while its scope is open."""

    def __init__(self, runtime: Runtime, stream_id: str | None, workspace_id: str | None,
                 claim_path: Path | None, association_path: Path | None, binding: dict | None,
                 scope: str | None = None) -> None:
        self._runtime = runtime
        self._scope = scope
        self._stream_id = stream_id
        self._workspace_id = workspace_id
        self._claim_path = claim_path
        self._association_path = association_path
        self._binding = binding
        self._active = True

    @property
    def stream_id(self) -> str | None:
        return self._stream_id

    def claim(self, stream_id: str) -> None:
        """Durably reserve the chosen identity before saving catalog records."""
        if not self._active:
            raise StreamError('enrollment scope is closed')
        if not _text(stream_id):
            raise StreamError('enrollment claim requires a nonempty stream ID')
        if self.stream_id is not None and self.stream_id != stream_id:
            raise StreamError(f'enrollment has a different stream identity {self.stream_id!r}; refusing replacement')
        if self._claim_path is None:
            with self._runtime._open_lock(None, stream_id):
                self._stream_id = stream_id
            return
        # The workspace lock is already held. Never acquire it from open(),
        # which takes only this per-stream lock and must see claims before launch.
        with self._runtime._open_lock(self._scope, stream_id):
            bindings = self._runtime._associations(self._scope)
            claims = self._runtime._enrollment_claims(self._scope)
            if any(record['stream_id'] == stream_id and record['workspace_id'] != self._workspace_id
                   for record in (*bindings, *claims)):
                raise StreamError(f'stream {stream_id!r} already has an association or claim in another workspace; inspect runtime state')
            self._stream_id = stream_id
            # An open-created binding is not evidence of automatic enrollment.
            # Do not retroactively bypass its exact-session reuse guard.
            if self._binding is not None and not any(record['workspace_id'] == self._workspace_id for record in claims):
                return
            self._runtime._write(self._claim_path, {'stream_id': stream_id, 'workspace_id': self._workspace_id})

    def bind(self, stream: Stream, *, authority: Authority | None) -> None:
        """Bind the claimed stream without starting, focusing or renaming native resources."""
        if not self._active:
            raise StreamError('enrollment scope is closed')
        self._runtime._authorize(authority)
        self.claim(stream.id)
        if self._association_path is None or self._binding is not None:
            return
        binding = {'stream_id': stream.id, 'workspace_id': self._workspace_id,
                   'session_id': None, 'command': None, 'ready': True}
        try:
            self._runtime._write(self._association_path, binding)
        except StreamError as exc:
            raise StreamError(f'enrollment bind failed; claim for {stream.id!r} retained at {self._claim_path}; '
                              f'inspect state and retry this identity; no rollback: {exc}') from exc
        self._binding = binding


class Runtime:
    def __init__(self, state_dir: Path, host_id: str, frontend_mode: FrontendMode, *, checkout_dir: Path | None = None) -> None:
        if not _text(host_id):
            raise StreamError('runtime requires a nonempty host ID')
        self.state_dir = Path(state_dir) / _key(host_id)
        self.host_id = host_id
        self.frontend_mode = frontend_mode
        self.checkouts = CheckoutRegistry(checkout_dir or Path(state_dir) / 'checkouts', host_id)
        self.locations = DirectoryBindings(self.state_dir, self.checkouts)

    def check_state(self, catalog: Catalog, target_stream_ids: set[str] | None) -> list[dict]:
        """Report durable runtime issues without probing native clients or changing state."""
        issues = []
        for path in sorted((self.state_dir / "attempts").glob("*/*.json")):
            try:
                raw = self._attempt(path)
            except StreamError as exc:
                issues.append({
                    "kind": "invalid-open-attempt",
                    "path": str(path),
                    "message": str(exc),
                })
                continue
            stream_id = raw["stream_id"]
            if target_stream_ids is None or stream_id in target_stream_ids:
                issues.append({
                    "kind": "pending-open-attempt",
                    "stream_id": stream_id,
                    "workspace_id": raw["workspace_id"],
                    "path": str(path),
                    "message": f"pending or partial open attempt at {path} blocks workspace open and lock acquisition",
                })

        for path in sorted((self.state_dir / "enrollments").glob("*/*.json")):
            try:
                claim = self._enrollment_claim(path)
            except StreamError as exc:
                issues.append({
                    "kind": "invalid-enrollment-claim",
                    "path": str(path),
                    "message": str(exc),
                })
                continue
            if claim is None or (target_stream_ids is not None and claim["stream_id"] not in target_stream_ids):
                continue
            assoc_path = self.state_dir / "associations" / path.parent.name / path.name
            try:
                assoc = self._association(assoc_path)
            except StreamError:
                assoc = None
            if assoc is None or not assoc.get("ready") or assoc.get("stream_id") != claim["stream_id"] or catalog.stream(claim["stream_id"]) is None:
                issues.append({
                    "kind": "pending-enrollment-claim",
                    "stream_id": claim["stream_id"],
                    "workspace_id": claim["workspace_id"],
                    "path": str(path),
                    "message": f"unfinished or mismatched enrollment claim marker at {path}",
                })

        for path in sorted((self.state_dir / "associations").glob("*/*.json")):
            try:
                assoc = self._association(path)
            except StreamError as exc:
                issues.append({
                    "kind": "invalid-workspace-association",
                    "path": str(path),
                    "message": str(exc),
                })
                continue
            if assoc is None:
                continue
            if target_stream_ids is not None and assoc["stream_id"] not in target_stream_ids:
                continue
            if catalog.stream(assoc["stream_id"]) is None:
                issues.append({
                    "kind": "dangling-workspace-association",
                    "stream_id": assoc["stream_id"],
                    "workspace_id": assoc["workspace_id"],
                    "path": str(path),
                    "message": f"workspace {assoc['workspace_id']!r} is associated with missing stream {assoc['stream_id']!r}",
                })
        for path in sorted((self.state_dir / "arrangements").glob("*/*.json")):
            try:
                plan = self._arrangement(path)
            except StreamError as exc:
                issues.append({
                    "kind": "invalid-saved-arrangement", "path": str(path), "message": str(exc),
                })
                continue
            if plan is None or (target_stream_ids is not None and plan["stream_id"] not in target_stream_ids):
                continue
            if catalog.stream(plan["stream_id"]) is None:
                issues.append({
                    "kind": "dangling-saved-arrangement", "path": str(path),
                    "stream_id": plan["stream_id"], "workspace_id": plan["workspace_id"],
                    "message": f"saved arrangement at {path} references missing stream {plan['stream_id']!r}",
                })
        return issues

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
    def _open_lock(self, scope: str | None, stream_id: str):
        # Endpoint/frontend scopes select journals, never the serialization boundary.
        lock = self.state_dir / 'stream-locks' / (_key(stream_id) + '.lock')
        lock.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            self._check_stream_scopes(scope, stream_id)
            yield self.state_dir / 'attempts' / scope / (_key(stream_id) + '.json') if scope is not None else None
        finally:
            os.close(descriptor)

    def _check_stream_scopes(self, scope: str | None, stream_id: str) -> None:
        for collection, reader in (('associations', self._association), ('enrollments', self._enrollment_claim)):
            for path in (self.state_dir / collection).glob('*/*.json'):
                record = reader(path)
                if record is not None and record['stream_id'] == stream_id and path.parent.name != scope:
                    raise StreamError(f'known workspace association or enrollment claim in another scope at {path}; '
                                      'cannot verify native state; inspect runtime state before retrying')
        for path in (self.state_dir / 'attempts').glob('*/*.json'):
            attempt = self._attempt(path)
            if attempt['stream_id'] == stream_id:
                raise StreamError(f'pending or partial open at {path}; inspect native state before retrying; no automatic relaunch')

    def _attempt(self, path: Path) -> dict:
        try:
            attempt = self._read(path, {'stream_id', 'session_id', 'command', 'cwd', 'workspace_id', 'effects', 'launches'})
        except StreamError as exc:
            raise StreamError(f'invalid open attempt at {path}: {exc}') from exc
        if (attempt is None or not _text(attempt['stream_id']) or not _text(attempt['cwd'])
                or path.name != _key(attempt['stream_id']) + '.json'
                or any(attempt[key] is not None and not _text(attempt[key])
                       for key in ('session_id', 'command', 'workspace_id'))
                or (attempt['session_id'] is None) != (attempt['command'] is None)
                or not isinstance(attempt['effects'], list) or not isinstance(attempt['launches'], list)):
            raise StreamError(f'invalid open attempt identity at {path}')
        return attempt

    @contextmanager
    def _enrollment_lock(self, path: Path):
        descriptor = None
        try:
            try:
                path.parent.mkdir(parents=True, exist_ok=True)
                descriptor = os.open(path.with_suffix('.lock'), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
                fcntl.flock(descriptor, fcntl.LOCK_EX)
            except OSError as exc:
                raise StreamError(f'could not lock enrollment state {path}: {exc}') from exc
            yield
        finally:
            if descriptor is not None:
                os.close(descriptor)

    def remember_directory(self, stream_id: str, cwd: Path, *, session_id: str | None = None, if_missing: bool = False,
                           repository_url: str | None = None, checkout_id: str | None = None) -> DirectoryBinding:
        return self.locations.remember(stream_id, cwd, session_id=session_id, if_missing=if_missing,
                                       repository_url=repository_url, checkout_id=checkout_id)

    def _cwd(self, stream: Stream, session: Session | None, cwd: Path | None) -> Path:
        if cwd is not None:
            return self.locations.checked(cwd)
        binding = self.locations.read(stream.id, session.id) if session is not None else None
        if binding is None:
            binding = self.locations.read(stream.id, None)
        if binding is None:
            raise StreamError(f'no local directory for stream {stream.id!r}; pass --cwd PATH')
        return self.locations.checked(binding.cwd)

    def _authorize(self, authority: Authority | None) -> None:
        if not isinstance(authority, Authority) or authority.host_id != self.host_id:
            raise StreamError('effective owner authority for this host is required; pending owner fields do not grant runtime authority')

    def _select_session(self, stream: Stream, sessions: tuple[Session, ...], ref: str | None) -> Session | None:
        eligible = [session for session in sessions if session.host_id == self.host_id and session.stream_id == stream.id]
        if ref is None:
            matches = [s for s in eligible if s.status not in ('done', 'parked')]
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
    def _command(session: Session | None, locator: str | None = None) -> AgentSessionCommand | None:
        if session is None:
            return None
        try:
            agent = Agent(session.harness)
            saved = conversation_file(session.harness, session.harness_session_id, locator)
            command = AgentSessionCommand(agent, str(saved) if agent == Agent.PI else session.harness_session_id)
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

    def _associations(self, scope: str) -> list[dict]:
        return [binding for path in (self.state_dir / 'associations' / scope).glob('*.json')
                if (binding := self._association(path)) is not None]

    def association_inventory(self, scope: str) -> list[dict]:
        """Read exact workspace bindings for one transport scope without native effects."""
        return self._associations(scope)

    def _enrollment_path(self, scope: str, workspace_id: str) -> Path:
        return self.state_dir / 'enrollments' / scope / (_key(workspace_id) + '.json')

    def _enrollment_claim(self, path: Path, workspace_id: str | None = None) -> dict | None:
        claim = self._read(path, {'stream_id', 'workspace_id'})
        if claim is not None and (not _text(claim['stream_id']) or not _text(claim['workspace_id'])
                                  or path.name != _key(claim['workspace_id']) + '.json'
                                  or (workspace_id is not None and claim['workspace_id'] != workspace_id)):
            raise StreamError(f'invalid enrollment claim identity: {path}')
        return claim

    def _enrollment_claims(self, scope: str) -> list[dict]:
        return [claim for path in (self.state_dir / 'enrollments' / scope).glob('*.json')
                if (claim := self._enrollment_claim(path)) is not None]

    def _caller_workspace(self, selected: Frontend, client) -> str:
        if selected == Frontend.HERDR:
            workspace_id = client.current_pane().workspace_id
            if not _text(workspace_id) or client.resolve_workspace(workspace_id).id != workspace_id:
                raise StreamError('could not confirm exact Herdr caller workspace ID; workspace mismatch')
        else:
            workspace_id = os.environ.get('CMUX_WORKSPACE_ID')
            if not _text(workspace_id):
                raise StreamError('current CMUX targeting requires exact caller workspace context')
            matches = [w for w in client.workspaces() if w.id == workspace_id]
            if len(matches) != 1:
                raise StreamError('could not confirm exact CMUX caller workspace ID; no focused-workspace fallback')
        return workspace_id

    def _verify_herdr_agent_pane(self, harness: str, native_id: str) -> tuple[HerdrClient, str, dict] | None:
        if harness != 'pi' or resolve_frontend(self.frontend_mode, None, os.environ).resolved != Frontend.HERDR:
            return None
        try:
            client, scope = self._transport(Frontend.HERDR)
            pane = client.current_pane()
            if (not _text(pane.id) or pane.id != os.environ.get('HERDR_PANE_ID')
                    or not _text(pane.workspace_id)):
                raise ValueError('could not confirm exact Herdr caller pane')
            result = client.command_result(['agent', 'get', pane.id], 'agent_info')
            agent = result.get('agent') if isinstance(result, dict) else None
            if (not isinstance(agent, dict) or agent.get('pane_id') != pane.id
                    or agent.get('workspace_id') != pane.workspace_id or agent.get('agent') != 'pi'):
                raise ValueError('Herdr agent pane, workspace or harness mismatch')
            actual_id, _ = native_identity(agent.get('agent_session'), 'pi')
            if actual_id != native_id:
                raise ValueError('requested Pi session differs from the native pane session')
            return client, scope, {'frontend': 'herdr', 'pane_id': pane.id, 'tab_id': pane.tab_id, 'workspace_id': pane.workspace_id}
        except (StreamError, OSError, ValueError) as exc:
            raise StreamError(f'could not confirm automatic Pi enrollment: {exc}; '
                              "a background helper cannot automatically enroll its parent's workspace") from exc

    def verify_automatic_enrollment(self, harness: str, native_id: str) -> dict | None:
        """Fence automatic Pi enrollment to the conversation registered in its pane."""
        verified = self._verify_herdr_agent_pane(harness, native_id)
        return verified[2] if verified is not None else None

    def attest_context(self, stream: Stream, session: Session, *, authority: Authority | None) -> dict:
        """Read-only native proof for a projection, including a ready Stream association."""
        self._authorize(authority)
        if session.host_id != self.host_id or session.stream_id != stream.id or session.harness != 'pi':
            raise StreamError('native projection requires the exact local Pi Session')
        verified = self._verify_herdr_agent_pane(session.harness, session.harness_session_id)
        if verified is None:
            raise StreamError('native projection attestation requires an exact Herdr caller')
        client, scope, native = verified
        workspace_id = native['workspace_id']
        if not _text(workspace_id) or client.resolve_workspace(workspace_id).id != workspace_id:
            raise StreamError('could not confirm exact Herdr caller workspace ID; workspace mismatch')
        binding = self._association(self._association_path(scope, workspace_id))
        if binding is None or not binding['ready'] or binding['stream_id'] != stream.id:
            raise StreamError('native projection requires a ready workspace association for this Stream')
        return native

    def native_inventory(self) -> dict:
        """Read-only conversation identities, never a missing-file inference of death.

        The native owner requests this only for handover and must match every tuple
        to its own fresh snapshot under the workspace lock before any effect.
        """
        client, _ = self._transport(Frontend.HERDR)
        snapshot = client.command_result(['api', 'snapshot'], 'session_snapshot').get('snapshot')
        if not isinstance(snapshot, dict) or not isinstance(snapshot.get('panes'), list):
            raise StreamError('native conversation inventory unavailable')
        inventory = []
        for pane in snapshot['panes']:
            if not isinstance(pane, dict) or not all(_text(pane.get(k)) for k in ('pane_id', 'terminal_id', 'workspace_id', 'tab_id')):
                raise StreamError('invalid native conversation inventory tuple')
            metadata = pane.get('agent_session')
            tokens = pane.get('tokens') or {}
            is_pi = (pane.get('agent') == 'pi' or isinstance(metadata, dict) and metadata.get('agent') == 'pi'
                     or isinstance(tokens, dict) and tokens.get('strm_role') == 'agent')
            identity = native_identity(metadata, 'pi')[0] if is_pi else None
            inventory.append({k: pane[k] for k in ('pane_id', 'terminal_id', 'workspace_id', 'tab_id')}
                             | {'native_session_id': identity})
        return {'socket': client.endpoint().socket, 'panes': inventory}

    def _caller_context(self, frontend: str | None, *, explicit_precedence: bool = False) -> tuple[str | None, str | None, str | None]:
        explicit = os.environ.get('STREAMS_STREAM_ID')
        if explicit is not None and not _text(explicit):
            raise StreamError('STREAMS_STREAM_ID must be a nonempty stream ID')
        try:
            selected = resolve_frontend(self.frontend_mode, frontend, os.environ).resolved
        except StreamError:
            # --current historically permits an explicit identity without native
            # probes when AUTO cannot choose one exact frontend.
            if explicit_precedence and explicit is not None:
                return explicit, None, None
            raise
        if selected is None:
            return explicit, None, None
        client, scope = self._transport(selected)
        return explicit, scope, self._caller_workspace(selected, client)

    def _enrollment_identity(self, explicit: str | None, scope: str, workspace_id: str) -> tuple[str | None, dict | None]:
        binding = self._association(self._association_path(scope, workspace_id))
        if binding is not None and not binding['ready']:
            raise StreamError(f'calling workspace {workspace_id} has a partial open; inspect runtime state before enrollment or reporting')
        claim = self._enrollment_claim(self._enrollment_path(scope, workspace_id), workspace_id)
        identities = {value for value in (explicit, binding['stream_id'] if binding else None,
                                          claim['stream_id'] if claim else None) if value is not None}
        if len(identities) > 1:
            raise StreamError(f'enrollment identity mismatch for workspace {workspace_id}; '
                              'calling workspace association, enrollment claim and explicit identity disagree; '
                              'inspect runtime state; refusing replacement')
        return next(iter(identities), None), binding

    @contextmanager
    def enrollment(self, frontend: str | None = None) -> Iterator[Enrollment]:
        """Serialize enrollment and catalog saves for the exact calling native workspace.

        Call claim(chosen_id) before creating catalog records, then bind() after
        saves. A surviving claim is returned on retry even if no Stream was saved.
        """
        explicit, scope, workspace_id = self._caller_context(frontend)
        if scope is None:
            enrollment = Enrollment(self, explicit, None, None, None, None)
            try:
                yield enrollment
            finally:
                enrollment._active = False
            return
        path = self._enrollment_path(scope, workspace_id)
        with self._enrollment_lock(path):
            stream_id, binding = self._enrollment_identity(explicit, scope, workspace_id)
            association_path = self._association_path(scope, workspace_id)
            enrollment = Enrollment(self, stream_id, workspace_id, path, association_path, binding, scope)
            try:
                yield enrollment
            finally:
                enrollment._active = False

    def context_stream_id(self, *, frontend: str | None = None) -> str | None:
        """Read exact caller associations/claims without creating locks or bindings."""
        explicit, scope, workspace_id = self._caller_context(frontend)
        if scope is None:
            return explicit
        return self._enrollment_identity(explicit, scope, workspace_id)[0]

    def current_stream_id(self, *, frontend: str | None = None) -> str:
        explicit, scope, workspace_id = self._caller_context(frontend, explicit_precedence=True)
        if scope is None:
            if explicit is not None:
                return explicit
            raise StreamError('no native frontend selected; pass --frontend herdr or --frontend cmux (or use resume)')
        binding = self._association(self._association_path(scope, workspace_id))
        if explicit is not None:
            if binding is not None and binding['ready'] and binding['stream_id'] != explicit:
                raise StreamError('explicit STREAMS_STREAM_ID and ready workspace association disagree; inspect runtime state')
            return explicit
        if binding is None:
            raise StreamError(f'workspace {workspace_id} has no catalog association; open an explicit stream first')
        # Unlike enrollment/reporting, --current can identify a partial binding
        # for inspection. Claims alone are not a current workspace association.
        return binding['stream_id']

    def _arrangement_path(self, scope: str, stream_id: str) -> Path:
        return self.state_dir / 'arrangements' / scope / (_key(stream_id) + '.json')

    def _arrangement(self, path: Path, stream_id: str | None = None) -> dict | None:
        data = self._read(path, {'stream_id', 'workspace_id', 'tabs'})
        if data is None:
            return None
        if not _text(data['stream_id']) or path.name != _key(data['stream_id']) + '.json':
            raise StreamError(f'invalid saved arrangement identity: {path}')
        return arrangement.validate_plan(data, stream_id if stream_id is not None else data['stream_id'])

    def _saved_arrangement(self, scope: str, stream_id: str) -> dict | None:
        return self._arrangement(self._arrangement_path(scope, stream_id), stream_id)

    def _capture_arrangement(self, stream, sessions, scope, binding, snapshot) -> dict:
        if not binding['ready'] or binding['stream_id'] != stream.id:
            raise StreamError('capture requires a ready workspace association for this Stream')
        verified = arrangement.workspace_inventory(snapshot, binding['workspace_id'])
        plan = arrangement.capture(verified, binding['workspace_id'], stream, sessions, self.host_id)
        if plan['tabs']:
            self._write(self._arrangement_path(scope, stream.id), plan)
        return plan

    def capture_arrangement(self, stream: Stream, sessions: tuple[Session, ...], *, authority: Authority | None,
                            frontend: str | None = None) -> dict:
        """Capture exact live catalog membership, never infer enrollment or launch intent."""
        self._authorize(authority)
        selected = self._frontend(frontend)
        if selected != Frontend.HERDR:
            raise StreamError('multi-tab arrangement capture is unavailable for CMUX')
        client, scope = self._transport(selected)
        with self._open_lock(scope, stream.id):
            bindings = [b for b in self._associations(scope) if b['stream_id'] == stream.id]
            if len(bindings) != 1:
                raise StreamError('capture requires one ready bound workspace')
            return self._capture_arrangement(stream, sessions, scope, bindings[0], arrangement.structural_inventory(client))

    def _archive_closed(self, scope, binding, claim) -> None:
        # Persist the complete evidence first. An interruption can leave both
        # copies, but cannot discard the only evidence of a surviving claim.
        workspace_id = binding['workspace_id']
        archive = self.state_dir / 'closed' / scope / (uuid.uuid4().hex + '.json')
        self._write(archive, {'binding': binding, 'claim': claim})
        if claim is not None:
            path = self._enrollment_path(scope, workspace_id)
            path.unlink()
            self._sync_directory(path.parent)
        path = self._association_path(scope, workspace_id)
        path.unlink()
        self._sync_directory(path.parent)

    @staticmethod
    def _live_session(snapshot, session):
        return next((a for a in snapshot['agents'] if a['agent'] == session.harness
                     and a['native_id'] == session.harness_session_id), None)

    def _restore_entries(self, stream, sessions, plan, session_ref, cwd):
        skipped, attention = [], []
        known = {s.id: s for s in sessions if s.stream_id == stream.id and s.host_id == self.host_id}
        if session_ref is not None:
            session = self._select_session(stream, sessions, session_ref)
            saved = next((entry for tab in (plan or {}).get('tabs', []) for entry in tab['sessions']
                          if entry['session_id'] == session.id), {})
            groups = [{'label': None, 'sessions': [{'session_id': session.id, **saved}]}]
        elif plan is not None and plan['tabs']:
            groups = plan['tabs']
        else:
            candidates = [s for s in known.values() if s.status not in ('done', 'parked')]
            groups = [{'label': None, 'sessions': [{'session_id': candidates[0].id}]}] if len(candidates) == 1 else []
            if len(candidates) > 1:
                attention.append('multiple local Sessions; empty workspace opened; choose --session: ' + ', '.join(s.id for s in candidates))
        requested = [(tab, entry) for tab in groups for entry in tab['sessions']
                     if entry['session_id'] in known and (session_ref is not None or known[entry['session_id']].status not in ('done', 'parked'))]
        if cwd is not None and len(requested) > 1:
            raise StreamError('--cwd is ambiguous across a multi-Session arrangement; choose --session')
        prepared = []
        for tab in groups:
            entries = []
            for entry in tab['sessions']:
                session = known.get(entry['session_id'])
                reason = None
                if session is None:
                    reason = 'Session is not available on this host for this Stream'
                elif session_ref is None and session.status in ('done', 'parked'):
                    reason = 'Session is done or parked; choose --session to restore explicitly'
                elif ('native_id' in entry and (entry['native_id'] != session.harness_session_id or entry['harness'] != session.harness)):
                    reason = 'saved arrangement native identity differs from catalog Session'
                if reason is not None:
                    skipped.append({'session_id': entry['session_id'], 'reason': reason})
                    continue
                try:
                    directory = self._cwd(stream, session, cwd)
                    command = self._command(session, entry.get('locator'))
                except StreamError as exc:
                    if 'unsupported session harness' in str(exc):
                        raise
                    skipped.append({'session_id': session.id, 'reason': str(exc)})
                    continue
                self.remember_directory(stream.id, directory, session_id=session.id)
                entries.append({'session': session, 'command': command, 'cwd': directory,
                                'locator': str(conversation_file(session.harness, session.harness_session_id, entry.get('locator')))})
            if entries:
                prepared.append({'label': tab.get('label'), 'entries': entries})
        return prepared, skipped, attention

    def open(self, stream: Stream, sessions: tuple[Session, ...], *, authority: Authority | None,
             frontend: str | None = None, session_ref: str | None = None, cwd: Path | None = None) -> dict:
        self._authorize(authority)
        selected = self._frontend(frontend)
        if selected == Frontend.CMUX:
            result = self._open_cmux(stream, sessions, session_ref=session_ref, cwd=cwd)
            return {**result, 'restored': [] if result['reused'] or result['session_id'] is None else [result['session_id']],
                    'skipped': [], 'unavailable': [], 'needs_attention': [], 'arrangement_support': 'single-session-only'}
        client, scope = self._transport(selected)
        with self._open_lock(scope, stream.id) as attempt_path:
            bindings = [b for b in self._associations(scope) if b['stream_id'] == stream.id]
            claims = [c for c in self._enrollment_claims(scope) if c['stream_id'] == stream.id]
            if len(bindings) > 1:
                raise StreamError('multiple native workspace associations for this stream; inspect runtime state')
            if any(not any(b['workspace_id'] == c['workspace_id'] and b['ready'] for b in bindings) for c in claims):
                raise StreamError('pending or unmatched enrollment claim; inspect runtime state; no automatic relaunch')
            binding = bindings[0] if bindings else None
            if binding is not None and not binding['ready']:
                raise StreamError('workspace has a partial open; inspect native state before retrying; no automatic relaunch')
            snapshot = None
            if binding is not None:
                # Focusing a bound workspace needs complete topology, not host-wide identity.
                snapshot = arrangement.structural_inventory(client)
                claim = self._enrollment_claim(self._enrollment_path(scope, binding['workspace_id']), binding['workspace_id'])
                if claim is not None and claim['stream_id'] != stream.id:
                    raise StreamError('enrollment claim and workspace association identity mismatch')
                if not any(w['workspace_id'] == binding['workspace_id'] for w in snapshot['workspaces']):
                    snapshot = arrangement.verified_inventory(snapshot)
                    self._archive_closed(scope, binding, claim)
                    binding = None
            plan = self._saved_arrangement(scope, stream.id)
            attention = []
            if binding is not None:
                workspace_id = binding['workspace_id']
                target = self._select_session(stream, sessions, session_ref) if session_ref is not None else None
                if target is not None:
                    snapshot = arrangement.verified_inventory(snapshot)
                if cwd is not None:
                    # Remember a choice, but never claim to chdir a live terminal.
                    remembered = target
                    if remembered is None:
                        local = [s for s in sessions if s.stream_id == stream.id and s.host_id == self.host_id]
                        if len(local) > 1:
                            raise StreamError('--cwd is ambiguous across multiple Sessions; choose --session')
                        remembered = local[0] if local else None
                    self.remember_directory(stream.id, self.locations.checked(cwd), session_id=remembered.id if remembered else None)
                live = self._live_session(snapshot, target) if target is not None else None
                if live is not None and live['workspace_id'] != workspace_id:
                    raise StreamError('selected conversation is already running in another native workspace; refusing duplicate launch')
                if target is None or live is not None:
                    try:
                        self._capture_arrangement(stream, sessions, scope, binding, snapshot)
                    except StreamError as exc:
                        attention.append(f'arrangement capture unavailable; last good layout retained: {exc}')
                    client.run(['workspace', 'focus', workspace_id])
                    if live is not None:
                        client.run(['agent', 'focus', live['pane_id']])
                    return {'stream_id': stream.id, 'session_id': target.id if target else binding['session_id'],
                            'frontend': selected.value, 'workspace_id': workspace_id, 'reused': True,
                            'restored': [], 'skipped': [], 'unavailable': [], 'needs_attention': attention}
            prepared, skipped, attention = self._restore_entries(stream, sessions, plan, session_ref, cwd)
            if binding is not None and not prepared:
                client.run(['workspace', 'focus', workspace_id])
                return {'stream_id': stream.id, 'session_id': None, 'frontend': selected.value, 'workspace_id': workspace_id,
                        'reused': True, 'restored': [], 'skipped': skipped, 'unavailable': skipped,
                        'needs_attention': attention + ['selected saved conversation unavailable; existing tabs left untouched']}
            prior = arrangement.capture(snapshot, workspace_id, stream, sessions, self.host_id)['tabs'] if binding is not None else []
            if prepared and snapshot is None:
                snapshot = arrangement.inventory(client)
            native_ids = [(e['session'].harness, e['session'].harness_session_id) for t in prepared for e in t['entries']]
            if len(set(native_ids)) != len(native_ids):
                raise StreamError('ambiguous saved arrangement native conversation identity')
            for tab in prepared:
                for entry in tab['entries']:
                    if self._live_session(snapshot, entry['session']) is not None:
                        raise StreamError(f'conversation {entry["session"].id!r} is already running; refusing duplicate launch')
            first = prepared[0]['entries'][0] if prepared else None
            directory = first['cwd'] if first else self._cwd(stream, None, cwd)
            if any(ord(c) < 32 or ord(c) == 127 for c in stream.title):
                raise StreamError('workspace title must not contain control characters')
            workspace_id = binding['workspace_id'] if binding else None
            attempt = {'stream_id': stream.id, 'session_id': first['session'].id if first else None,
                       'command': first['command'].command_text if first else None, 'cwd': str(directory),
                       'workspace_id': workspace_id, 'effects': [], 'launches': []}
            self._write(attempt_path, attempt)
            restored, observed_tabs, resources = [], [], []
            try:
                root_pane = None
                if binding is None:
                    self.remember_directory(stream.id, directory)
                    attempt['effects'].append({'kind': 'workspace_create'})
                    self._write(attempt_path, attempt)
                    created = client.command_result(['workspace', 'create', '--cwd', str(directory), '--label', stream.title,
                                                     *_native_environment(stream.id), '--focus'], 'workspace_created')
                    for field, key in (('workspace', 'workspace_id'), ('tab', 'tab_id'), ('root_pane', 'pane_id')):
                        if isinstance(created.get(field), dict) and _text(created[field].get(key)):
                            resources.append(created[field][key])
                    attempt['effects'][-1]['returned'] = created
                    self._write(attempt_path, attempt)
                    workspace_id = HerdrWorkspace.parse(created.get('workspace')).id
                    attempt['workspace_id'] = workspace_id
                    self._write(attempt_path, attempt)
                    path = self._association_path(scope, workspace_id)
                    if self._association(path) is not None:
                        raise StreamError('created workspace already has an association; refusing overwrite')
                    binding = {'stream_id': stream.id, 'workspace_id': workspace_id, 'session_id': first['session'].id if first else None,
                               'command': first['command'].command_text if first else None, 'ready': False}
                    self._write(path, binding)
                    root_pane = self._created_pane(created, workspace_id)
                else:
                    path = self._association_path(scope, workspace_id)
                    binding = {k: v for k, v in binding.items() if k != 'version'}
                    binding['ready'] = False
                    self._write(path, binding)
                for group in prepared:
                    entries = group['entries']
                    if root_pane is not None:
                        pane = root_pane
                        root_pane = None
                        if group['label'] is not None:
                            attempt['effects'].append({'kind': 'tab_rename', 'tab_id': pane.tab_id, 'label': group['label']})
                            self._write(attempt_path, attempt)
                            client.run(['tab', 'rename', pane.tab_id, group['label']])
                    else:
                        args = ['tab', 'create', '--workspace', workspace_id, '--cwd', str(entries[0]['cwd']),
                                *_native_environment(stream.id), '--no-focus']
                        if group['label'] is not None:
                            args.extend(['--label', group['label']])
                        attempt['effects'].append({'kind': 'tab_create'})
                        self._write(attempt_path, attempt)
                        created = client.command_result(args, 'tab_created')
                        attempt['effects'][-1]['returned'] = created
                        self._write(attempt_path, attempt)
                        pane = self._created_pane(created, workspace_id)
                    observed = {'tab_id': pane.tab_id, 'label': group['label'],
                                'label_mode': 'manual' if group['label'] is not None else 'automatic', 'sessions': []}
                    observed_tabs.append(observed)
                    for index, entry in enumerate(entries):
                        if index:
                            attempt['effects'].append({'kind': 'pane_split', 'pane_id': pane.id})
                            self._write(attempt_path, attempt)
                            split = client.command_result(['pane', 'split', '--pane', pane.id, '--direction', 'right',
                                                           '--cwd', str(entry['cwd']), *_native_environment(stream.id), '--no-focus'], 'pane_split')
                            attempt['effects'][-1]['returned'] = split
                            self._write(attempt_path, attempt)
                            new_pane = HerdrPane.parse(split.get('pane'))
                            if new_pane.workspace_id != workspace_id or new_pane.tab_id != pane.tab_id:
                                raise StreamError('split pane ownership mismatch')
                            pane = new_pane
                        session, command = entry['session'], entry['command']
                        launch = {'session_id': session.id, 'pane_id': pane.id, 'tab_id': pane.tab_id,
                                  'name': 'streams-' + uuid.uuid4().hex[:24], 'ready': False}
                        attempt['launches'].append(launch)
                        self._write(attempt_path, attempt)
                        started = client.command_result(['agent', 'start', launch['name'], '--kind', command.agent.value, '--pane', pane.id,
                                                         '--timeout', '20000', '--', *command.argv[1:]], 'agent_started').get('agent')
                        launch['returned'] = started
                        if (not isinstance(started, dict) or started.get('pane_id') != pane.id or started.get('tab_id') != pane.tab_id
                                or started.get('workspace_id') != workspace_id or started.get('name') != launch['name']
                                or started.get('agent') != command.agent.value or started.get('interactive_ready') is not True):
                            self._write(attempt_path, attempt)
                            raise StreamError('Herdr agent identity/readiness mismatch')
                        launch['ready'] = True
                        restored.append(session.id)
                        self._write(attempt_path, attempt)
                        observed['sessions'].append({'session_id': session.id, 'harness': session.harness,
                                                     'native_id': session.harness_session_id, 'locator': entry['locator'],
                                                     'pane_id': pane.id, 'terminal_id': started.get('terminal_id')})
                # Existing tabs remain untouched. Merge newly launched tabs with
                # the verified live arrangement, rather than stale saved intent.
                if prior or observed_tabs:
                    self._write(self._arrangement_path(scope, stream.id), {'stream_id': stream.id, 'workspace_id': workspace_id, 'tabs': prior + observed_tabs})
                binding['ready'] = True
                self._write(path, binding)
                attempt_path.unlink()
                self._sync_directory(attempt_path.parent)
            except (StreamError, OSError, ValueError) as exc:
                raise StreamError(f'native open may be partial; resources {"; ".join(resources)}; confirmed launches: {restored}; {exc}. '
                                  'Inspect native state before retrying; no rollback or automatic relaunch') from exc
            client.run(['workspace', 'focus', workspace_id])
            if skipped:
                attention.append('some saved Sessions were unavailable or excluded; inspect skipped diagnostics')
            return {'stream_id': stream.id, 'session_id': first['session'].id if first else None, 'frontend': selected.value,
                    'workspace_id': workspace_id, 'reused': bool(bindings and bindings[0]['workspace_id'] == workspace_id),
                    'restored': restored, 'skipped': skipped, 'unavailable': skipped, 'needs_attention': attention}

    @staticmethod
    def _created_pane(created, workspace_id):
        tab = created.get('tab')
        pane = HerdrPane.parse(created.get('root_pane'))
        if not isinstance(tab, dict) or tab.get('workspace_id') != workspace_id or pane.workspace_id != workspace_id or pane.tab_id != tab.get('tab_id'):
            raise StreamError('created Herdr pane/tab workspace identity mismatch')
        return pane

    def _open_cmux(self, stream: Stream, sessions: tuple[Session, ...], *,
                   session_ref: str | None = None, cwd: Path | None = None) -> dict:
        session = self._select_session(stream, sessions, session_ref)
        command = self._command(session)
        directory = self._cwd(stream, session, cwd)
        client, scope = self._transport(Frontend.CMUX)
        with self._open_lock(scope, stream.id) as attempt_path:
            bindings = [binding for binding in self._associations(scope) if binding['stream_id'] == stream.id]
            for claim in self._enrollment_claims(scope):
                if claim['stream_id'] == stream.id and not any(binding['workspace_id'] == claim['workspace_id'] and binding['ready']
                                                               for binding in bindings):
                    raise StreamError(f'pending or unmatched enrollment claim for {stream.id!r} in workspace {claim["workspace_id"]}; '
                                      'inspect runtime state and reconcile this identity; no automatic relaunch')
            if len(bindings) > 1:
                raise StreamError('multiple native workspace associations for this stream; inspect runtime state before retrying')
            if bindings:
                binding = bindings[0]
                workspace_id = binding['workspace_id']
                try:
                    if len([w for w in client.workspaces() if w.id == workspace_id]) != 1:
                        raise StreamError('exact workspace ID not found')
                except StreamError as exc:
                    raise StreamError(f'stale recorded workspace {workspace_id}: {exc}. Inspect native state and the local association; no automatic relaunch') from exc
                if not binding['ready']:
                    raise StreamError(f'workspace {workspace_id} has a partial open; inspect native state before retrying; no automatic relaunch')
                claim = self._enrollment_claim(self._enrollment_path(scope, workspace_id), workspace_id)
                if claim is not None and claim['stream_id'] != stream.id:
                    raise StreamError('enrollment claim and workspace association identity mismatch; inspect runtime state')
                enrolled_sessionless = binding['session_id'] is None and claim is not None
                if not enrolled_sessionless and (binding['session_id'] != (session.id if session else None) or binding['command'] != (command.command_text if command else None)):
                    raise StreamError(f'workspace {workspace_id} has a different session; refusing to duplicate or replace it; inspect native state')
                if cwd is not None:
                    self.remember_directory(stream.id, directory, session_id=session.id if session else None)
                client.run(['cmux', 'select-workspace', '--workspace', workspace_id])
                return {'stream_id': stream.id, 'session_id': binding['session_id'], 'frontend': Frontend.CMUX.value,
                        'workspace_id': workspace_id, 'reused': True}

            if any(ord(char) < 32 or ord(char) == 127 for char in stream.title):
                raise StreamError('workspace title must not contain control characters')
            self.remember_directory(stream.id, directory, session_id=session.id if session else None)
            attempt = {'stream_id': stream.id, 'session_id': session.id if session else None,
                       'command': command.command_text if command else None, 'cwd': str(directory), 'workspace_id': None,
                       'effects': [{'kind': 'workspace_create'}],
                       'launches': [{'session_id': session.id, 'command': command.command_text}] if command else []}
            self._write(attempt_path, attempt)
            resources = ['workspace creation attempted']
            try:
                args = ['cmux', '--json', '--id-format', 'uuids', 'new-workspace', '--name', stream.title,
                        '--cwd', str(directory), *_native_environment(stream.id), '--focus', 'true']
                if command:
                    args.extend(['--command', command.command_text])
                created = client.json_from_command(args)
                attempt['effects'][0]['returned'] = created
                workspace_id = created.get('workspace_id') if isinstance(created, dict) else None
                if not _text(workspace_id):
                    self._write(attempt_path, attempt)
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
                binding['ready'] = True
                self._write(path, binding)
                attempt_path.unlink()
                self._sync_directory(attempt_path.parent)
            except (StreamError, OSError, ValueError) as exc:
                raise StreamError(f'native open may be partial; {"; ".join(resources)}; {exc}. Inspect native state before retrying; no rollback or automatic relaunch') from exc
            return {'stream_id': stream.id, 'session_id': session.id if session else None, 'frontend': Frontend.CMUX.value,
                    'workspace_id': workspace_id, 'reused': False}

    def resume(self, stream: Stream, sessions: tuple[Session, ...], *, authority: Authority | None,
               session_ref: str | None = None, cwd: Path | None = None) -> None:
        self._authorize(authority)
        session = self._select_session(stream, sessions, session_ref)
        if session is None:
            raise StreamError(f'stream {stream.id!r} has no eligible local session to resume')
        # Resume never chooses a launch frontend. When Herdr is available, use
        # its read-only inventory even if another frontend's environment is set.
        selected = Frontend.HERDR if self.frontend_mode == FrontendMode.HERDR or os.environ.get('HERDR_ENV') == '1' else None
        client, scope = self._transport(selected) if selected == Frontend.HERDR else (None, None)
        with self._open_lock(scope, stream.id):
            if client is not None:
                bindings = [b for b in self._associations(scope) if b['stream_id'] == stream.id]
                claims = [c for c in self._enrollment_claims(scope) if c['stream_id'] == stream.id]
                if any(not b['ready'] for b in bindings):
                    raise StreamError('known workspace association is partial; resume refused')
                if len(bindings) > 1 or any(not any(b['workspace_id'] == c['workspace_id'] and b['ready']
                                                    for b in bindings) for c in claims):
                    raise StreamError('multiple associations or pending enrollment claim; inspect runtime state; resume refused')
                snapshot = arrangement.inventory(client)
                if self._live_session(snapshot, session) is not None:
                    raise StreamError('conversation is already running in a native pane; refusing duplicate resume')
            plan = self._resume_arrangement(scope, stream.id)
            locator = next((e.get('locator') for t in (plan or {}).get('tabs', []) for e in t['sessions']
                            if e['session_id'] == session.id and e['native_id'] == session.harness_session_id
                            and e['harness'] == session.harness), None)
            self._exec_session(stream, session, cwd, locator)

    def _resume_arrangement(self, scope: str | None, stream_id: str) -> dict | None:
        plan = self._saved_arrangement(scope, stream_id) if scope is not None else None
        if plan is not None:
            return plan
        plans = []
        for path in (self.state_dir / 'arrangements').glob('*/*.json'):
            checked = self._arrangement(path)
            if checked is None:
                raise StreamError(f'invalid saved arrangement identity: {path}')
            if checked['stream_id'] == stream_id:
                plans.append(checked)
        if len(plans) > 1:
            raise StreamError('multiple saved arrangements across scopes; inspect runtime state before resume')
        return plans[0] if plans else None

    def _exec_session(self, stream, session, cwd, locator):
        command = self._command(session, locator)
        directory = self._cwd(stream, session, cwd)
        if cwd is not None:
            self.remember_directory(stream.id, directory, session_id=session.id)
        try:
            os.chdir(directory)
            os.environ['STREAMS_STREAM_ID'] = stream.id
            os.execvp(command.argv[0], list(command.argv))
        except OSError as exc:
            raise StreamError(f'could not resume {session.id!r}: {exc}') from exc
