"""Native Herdr workspace projection; scratch notes remain authoritative."""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tomllib
import uuid
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from .agent_sessions import latest_session_command
from .config import config_path
from .errors import StreamError
from .herdr_binding import HerdrEndpoint, binding_path, read_optional_binding, write_binding
from .models import Stream
from .projection import stream_cwd, stream_for_hints
from .resolve import resolve_stream_ref
from .store import list_streams


@dataclass(frozen=True)
class HerdrWorkspace:
    id: str
    label: str
    cwd: str = ""

    @classmethod
    def parse(cls, value: object) -> HerdrWorkspace:
        data = _object(value, "workspace")
        label = data.get("label")
        if not isinstance(label, str):
            raise StreamError("Herdr returned an invalid workspace label")
        return cls(_id(data, "workspace_id"), label)


@dataclass(frozen=True)
class HerdrPane:
    id: str
    tab_id: str
    workspace_id: str
    cwd: str

    @classmethod
    def parse(cls, data: object) -> HerdrPane:
        pane = _object(data, "pane")
        foreground_cwd, shell_cwd = pane.get("foreground_cwd"), pane.get("cwd")
        if any(value is not None and not isinstance(value, str) for value in (foreground_cwd, shell_cwd)):
            raise StreamError("Herdr returned an invalid pane cwd")
        return cls(_id(pane, "pane_id"), _id(pane, "tab_id"), _id(pane, "workspace_id"), foreground_cwd or shell_cwd or "")


def _object(value: object, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise StreamError(f"Herdr returned an unexpected {name} shape")
    return value


def _id(data: dict[str, Any], key: str) -> str:
    value = data.get(key)
    if not isinstance(value, str) or not value.strip():
        raise StreamError(f"Herdr returned an invalid {key}")
    return value


class HerdrClient:
    def __init__(self) -> None:
        self.context = dict(os.environ)
        self.binary = self.context.get("HERDR_BIN_PATH") or "herdr"

    def run(self, args: list[str]) -> subprocess.CompletedProcess[str]:
        try:
            result = subprocess.run(
                [self.binary, *args], env=self.context,
                text=True, capture_output=True, check=True, timeout=30,
            )
        except FileNotFoundError as exc:
            raise StreamError(f"missing executable: {self.binary}") from exc
        except subprocess.TimeoutExpired as exc:
            raise StreamError("Herdr command timed out; it may have completed, so inspect native state before retrying") from exc
        except subprocess.CalledProcessError as exc:
            detail = (exc.stderr or exc.stdout or "").strip()[-2000:]
            raise StreamError(f"Herdr {' '.join(args[:2])} failed: {detail}") from exc
        except (OSError, ValueError) as exc:
            raise StreamError(f"could not execute Herdr: {exc}") from exc
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr)
        return result

    def json_from_command(self, args: list[str]) -> dict[str, Any]:
        try:
            data = json.loads(self.run(args).stdout)
        except ValueError as exc:
            raise StreamError("Herdr command did not return valid JSON") from exc
        data = _object(data, "response")
        if "error" in data:
            error = _object(data["error"], "error")
            raise StreamError(f"Herdr command failed: {str(error.get('message') or error.get('code') or 'unknown error')[-2000:]}")
        return data

    def command_result(self, args: list[str], expected: str) -> dict[str, Any]:
        result = _object(self.json_from_command(args).get("result"), "result")
        if result.get("type") != expected:
            raise StreamError(f"Herdr returned an unexpected result; expected {expected}")
        return result

    def endpoint(self) -> HerdrEndpoint:
        server = _object(self.json_from_command(["status", "--json"]).get("server"), "server status")
        if server.get("running") is not True:
            raise StreamError("Herdr server is not running")
        if server.get("compatible") is not True or server.get("endpoint_compatible") is not True:
            raise StreamError("Herdr server is not API-compatible; no automatic update or restart will be attempted")
        version = server.get("version")
        parsed = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:[-+][\w.-]+)?", version) if isinstance(version, str) else None
        if parsed is None or tuple(map(int, parsed.groups())) < (0, 9, 3):
            raise StreamError("streams Herdr projection requires server version 0.9.3 or newer")
        socket = _id(server, "socket")
        if not Path(socket).is_absolute():
            raise StreamError("Herdr returned an invalid server socket path")
        session = server.get("session")
        if session is not None and not isinstance(session, str):
            raise StreamError("Herdr returned an invalid server session")
        return HerdrEndpoint(socket, session)

    def current_pane(self) -> HerdrPane:
        if self.context.get("HERDR_ENV") != "1" or not self.context.get("HERDR_PANE_ID"):
            raise StreamError("current Herdr targeting requires caller context; pass --workspace outside Herdr")
        return HerdrPane.parse(self.command_result(["pane", "current", "--current"], "pane_current").get("pane"))

    def resolve_workspace(self, selector: str | None) -> HerdrWorkspace:
        if selector is None:
            pane = self.current_pane()
            workspace_id = pane.workspace_id
        else:
            if not selector.strip():
                raise StreamError("Herdr --workspace requires a nonempty workspace ID", kind="invalid-ref")
            pane = None
            workspace_id = selector
        workspace = HerdrWorkspace.parse(self.command_result(["workspace", "get", workspace_id], "workspace_info").get("workspace"))
        if workspace.id != workspace_id:
            raise StreamError("Herdr workspace ID mismatch in native response")
        return replace(workspace, cwd=pane.cwd) if pane is not None else workspace


def stream_for_workspace(workspace: HerdrWorkspace, herdr: HerdrClient, *, root: Path | None = None) -> Stream:
    path = binding_path(herdr.endpoint(), workspace.id)
    root_args = {"root": root} if root is not None else {}
    try:
        ref = read_optional_binding(path)
        if ref is not None:
            return resolve_stream_ref(ref, exact=True, **root_args)
    except StreamError as exc:
        raise StreamError(
            f"Herdr workspace {workspace.id} association is unavailable: {exc}; pass an explicit stream to rebind",
            kind=exc.kind, candidates=exc.candidates,
        ) from exc
    streams = list_streams(**root_args)
    return stream_for_hints(
        streams, title=workspace.label, cwd=workspace.cwd,
        source=f"Herdr workspace {workspace.id}",
    )


def load_stream_for_update(
    slug: str | None,
    workspace_selector: str | None,
    current: bool,
    herdr: HerdrClient,
    *,
    root: Path | None = None,
) -> tuple[Stream, HerdrWorkspace]:
    if current and workspace_selector is not None:
        raise StreamError("use either --current or --workspace, not both")
    workspace = herdr.resolve_workspace(workspace_selector)
    stream = (resolve_stream_ref(slug, **({"root": root} if root is not None else {}))
              if slug is not None else stream_for_workspace(workspace, herdr, root=root))
    return stream, workspace


def update_workspace(stream: Stream, workspace: HerdrWorkspace, herdr: HerdrClient | None = None) -> Path:
    herdr = herdr or HerdrClient()
    path = binding_path(herdr.endpoint(), workspace.id)
    _project_workspace(stream, workspace, herdr, path)
    return path


def _project_workspace(stream: Stream, workspace: HerdrWorkspace, herdr: HerdrClient, path: Path) -> None:
    completed: list[str] = []
    try:
        # Commit the association before deriving native display from it.
        write_binding(path, stream.slug)
        completed.append("association")
        renamed = HerdrWorkspace.parse(herdr.command_result(["workspace", "rename", workspace.id, stream.title], "workspace_info").get("workspace"))
        if renamed.id != workspace.id:
            raise StreamError("Herdr renamed workspace ID mismatch in native response")
        completed.append("label")
        # report-metadata deliberately exits silently on success.
        herdr.run([
            "workspace", "report-metadata", workspace.id, "--source", "streams",
            "--token", f"stream_slug={stream.slug}", "--token", f"stream_status={stream.status}",
            "--token", f"stream_state={stream.state}",
        ])
        completed.append("metadata")
    except StreamError as exc:
        if not completed:
            raise StreamError(
                f"Herdr association for {workspace.id} was not changed; no native display update attempted; "
                f"binding: {path}; {exc}"
            ) from exc
        raise StreamError(
            f"Herdr refresh may be partial for {workspace.id}; association {stream.slug} recorded; "
            f"display completed: {', '.join(completed[1:]) or 'none confirmed'}; "
            f"binding: {path}; {exc}. Inspect state before retrying"
        ) from exc


def plugin_root() -> Path:
    return Path(__file__).resolve().with_name("herdr_plugin")


def _check_plugin(value: object, manifest: dict[str, Any]) -> None:
    plugin = _object(value, "plugin registration")
    if plugin.get("enabled") is not True:
        raise StreamError("Herdr plugin streams.links is disabled; enable it explicitly before opening")
    root, path = plugin.get("plugin_root"), plugin.get("manifest_path")
    source = plugin.get("source")
    matches = (
        plugin.get("plugin_id") == manifest["id"]
        and isinstance(root, str) and Path(root).resolve() == plugin_root()
        and isinstance(path, str) and Path(path).resolve() == plugin_root() / "herdr-plugin.toml"
        and isinstance(source, dict) and source.get("kind") == "local"
        and all(plugin.get(key) == manifest[key] for key in ("version", "min_herdr_version", "platforms", "panes"))
    )
    if not matches:
        raise StreamError("Herdr plugin streams.links has a conflicting source or manifest; no registration was replaced")


def _ensure_plugin(herdr: HerdrClient) -> bool:
    try:
        manifest = tomllib.loads((plugin_root() / "herdr-plugin.toml").read_text())
    except (OSError, ValueError) as exc:
        raise StreamError(f"could not read bundled Herdr plugin: {exc}") from exc
    plugins = herdr.command_result(["plugin", "list", "--plugin", "streams.links", "--json"], "plugin_list").get("plugins")
    if not isinstance(plugins, list) or len(plugins) > 1:
        raise StreamError("Herdr returned an unexpected streams.links registration list")
    if plugins:
        _check_plugin(plugins[0], manifest)
        return False
    try:
        registered = herdr.command_result(["plugin", "link", str(plugin_root())], "plugin_linked")
        _check_plugin(registered.get("plugin"), manifest)
    except StreamError as exc:
        raise StreamError(f"Herdr plugin registration may be partial: {exc}; inspect the global registry before retrying") from exc
    return True


@dataclass(frozen=True)
class HerdrOpenedWorkspace:
    workspace: HerdrWorkspace
    root_pane: HerdrPane
    links_pane: HerdrPane
    binding: Path
    agent_name: str | None


def _check_pane_location(pane: HerdrPane, workspace: HerdrWorkspace, tab_id: str) -> None:
    if pane.workspace_id != workspace.id or pane.tab_id != tab_id:
        raise StreamError("Herdr pane workspace/tab ID mismatch in native response")


def open_workspace(stream: Stream, herdr: HerdrClient | None = None) -> HerdrOpenedWorkspace:
    if sys.platform != "darwin":
        raise StreamError("this Herdr links-pane prototype supports macOS only")
    cwd = stream_cwd(stream)
    session = latest_session_command(stream)
    arguments = (stream.title, *session.argv) if session is not None else (stream.title,)
    if any(any(ord(char) < 32 for char in argument) for argument in arguments):
        raise StreamError("Herdr opening requires title/session arguments without control characters")
    herdr = herdr or HerdrClient()
    endpoint = herdr.endpoint()
    registered = _ensure_plugin(herdr)
    resources = [f"plugin streams.links registered at {plugin_root()}"] if registered else []
    agent_name = None
    try:
        resources.append("workspace creation attempted")
        created = herdr.command_result(["workspace", "create", "--cwd", str(cwd), "--label", stream.title, "--focus"], "workspace_created")
        workspace = HerdrWorkspace.parse(created.get("workspace"))
        resources.append(f"workspace {workspace.id}")
        tab = _object(created.get("tab"), "created tab")
        tab_id = _id(tab, "tab_id")
        resources.append(f"tab {tab_id}")
        if _id(tab, "workspace_id") != workspace.id:
            raise StreamError("Herdr created tab workspace ID mismatch")
        root_pane = HerdrPane.parse(created.get("root_pane"))
        resources.append(f"root pane {root_pane.id}")
        _check_pane_location(root_pane, workspace, tab_id)
        binding = binding_path(endpoint, workspace.id)
        resources.append(f"binding {binding}")
        _project_workspace(stream, workspace, herdr, binding)
        links = herdr.command_result([
            "plugin", "pane", "open", "--plugin", "streams.links", "--entrypoint", "links", "--placement", "split",
            # Static manifest argv resolves launch.sh from the plugin directory.
            "--target-pane", root_pane.id, "--direction", "down", "--cwd", str(plugin_root()), "--no-focus",
            "--env", f"STREAMS_PYTHON={sys.executable}",
            "--env", f"STREAMS_IMPORT_ROOT={Path(__file__).resolve().parents[1]}",
            "--env", f"STREAMS_BINDING={binding}",
            "--env", f"HOME={Path.home()}",
            "--env", f"XDG_CONFIG_HOME={config_path().parent.parent.expanduser().resolve()}",
        ], "plugin_pane_opened")
        plugin_pane = _object(links.get("plugin_pane"), "opened plugin pane")
        links_pane = HerdrPane.parse(plugin_pane.get("pane"))
        resources.append(f"links pane {links_pane.id}")
        if plugin_pane.get("plugin_id") != "streams.links" or plugin_pane.get("entrypoint") != "links":
            raise StreamError("Herdr opened plugin identity mismatch")
        _check_pane_location(links_pane, workspace, tab_id)
        # Plugin split starts balanced. Grow the root so links use about 20%.
        resized = herdr.command_result([
            "pane", "resize", "--pane", root_pane.id, "--direction", "down", "--amount", "0.3",
        ], "pane_resize")
        resize = _object(resized.get("resize"), "pane resize")
        if resize.get("pane_id") != root_pane.id or resize.get("focused_pane_id") != root_pane.id or resize.get("changed") is not True:
            raise StreamError("Herdr could not size the links pane while preserving root focus")
        if session is not None:
            agent_name = f"streams-{uuid.uuid4().hex[:24]}"
            resources.append(f"agent start attempted: {agent_name}")
            started = herdr.command_result([
                "agent", "start", agent_name, "--kind", session.agent.value, "--pane", root_pane.id,
                "--timeout", "20000", "--", *session.argv[1:],
            ], "agent_started")
            agent = _object(started.get("agent"), "started agent")
            _check_pane_location(HerdrPane.parse(agent), workspace, tab_id)
            if agent.get("pane_id") != root_pane.id or agent.get("name") != agent_name or agent.get("agent") != session.agent.value or agent.get("interactive_ready") is not True:
                raise StreamError("Herdr returned an unexpected started agent identity/readiness")
    except StreamError as exc:
        raise StreamError(
            f"Herdr open may be partial; resources: {'; '.join(resources)}; {exc}. "
            "Inspect native state before retrying; no resources were closed or relaunched"
        ) from exc
    return HerdrOpenedWorkspace(workspace, root_pane, links_pane, binding, agent_name)
