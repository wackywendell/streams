"""CLI for durable work records. Runtime effects and publication are separate outcomes."""

from __future__ import annotations

import argparse
from collections.abc import Mapping
from dataclasses import fields, is_dataclass, replace
from enum import Enum
import json
import os
from pathlib import Path
import sys
from uuid import uuid4

import yaml

from . import catalog_config as config
from .catalog.codec import record_to_data
from .catalog.model import Checkout, Document, Link, Repository, Session, STATUSES, Stream, ValidationError
from .catalog.operations import Operation, normalize_changes, update
from .catalog.storage import StorageError
from .catalog.sync import Result, SyncCoordinator
from .errors import StreamError
from .frontend import Frontend, resolve_frontend


def output_data(value):
    if isinstance(value, Stream):
        return record_to_data(value)
    if isinstance(value, Enum):
        return value.value
    if is_dataclass(value):
        return {field.name: output_data(getattr(value, field.name)) for field in fields(value)}
    if isinstance(value, Mapping):
        return {key: output_data(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [output_data(item) for item in value]
    return value


def emit(value: object, args: argparse.Namespace) -> None:
    value = output_data(value)
    if args.json:
        print(json.dumps(value, ensure_ascii=False, indent=2))
    else:
        print(yaml.safe_dump(value, allow_unicode=True, sort_keys=False).rstrip())


def runtime():
    from .catalog_runtime import Runtime
    return Runtime(config.runtime_state_dir(), config.host_id(), config.get_frontend_mode())


def resolve(records, ref: str):
    """Resolve identity before names; an ambiguous name is never a selection heuristic."""
    if not ref or not ref.strip():
        raise StreamError("reference must be nonempty", kind="invalid-ref")
    exact = [item for item in records if ref == item.id or
             isinstance(item, Session) and ref == f"{item.host_id}/{item.id}"]
    matches = exact or [item for item in records if item.title.casefold() == ref.casefold()]
    matches = matches or [item for item in records if ref.casefold() in item.title.casefold()]
    if not matches:
        raise StreamError(f"no record matches {ref!r}", kind="not-found")
    if len(matches) != 1:
        candidates = [f"{item.host_id}/{item.id}" if isinstance(item, Session) else item.id for item in matches]
        raise StreamError(f"ambiguous reference {ref!r}: {', '.join(candidates)}", kind="ambiguous", candidates=candidates)
    return matches[0]


def stream_ref(args) -> str:
    if args.current:
        if args.ref is not None:
            raise StreamError("use a reference or --current, not both")
        return runtime().current_stream_id(frontend=args.frontend)
    if args.ref is None:
        raise StreamError("supply a stream reference or --current", kind="invalid-ref")
    return args.ref


def target_stream(catalog, args, *, local=False):
    ref = stream_ref(args)
    try:
        if args.current:
            stream = catalog.stream(ref)
            if stream is None:
                raise StreamError(f"associated stream {ref!r} no longer exists", kind="not-found")
            return stream
        return resolve(catalog.streams, ref)
    except StreamError as error:
        if local and error.kind == "not-found":
            raise StreamError(f"{error}; run streams sync to adopt records visible only in refreshed reads", kind=error.kind) from error
        raise


def catalog_data(catalog):
    return {"streams": [record_to_data(item) for item in catalog.streams],
            "sessions": [record_to_data(item) for item in catalog.sessions]}


def read_catalog(backend, args):
    result = SyncCoordinator(backend).read(refresh=args.refresh)
    sharing = {"freshness": result.freshness, "fetched_at": result.fetched_at,
               "view": "local" if result.catalog == result.local.catalog else "projected",
               "conflicts": list(result.conflicts), "detail": result.detail}
    if result.conflicts and result.confirmed is not None:
        sharing["confirmed"] = catalog_data(result.confirmed.catalog)
    return result.catalog, sharing


def maybe_sync(backend, event):
    return SyncCoordinator(backend).maybe_sync(event)


def save(backend, operation):
    return backend.save(operation, origin={"command": "streams"})


def result_data(result):
    # Materialize immutable mappings without deep-copying record/history objects.
    data = output_data(replace(result, domain=None))
    if result.domain is not None:
        domain = output_data(replace(result.domain, record=None))
        domain.pop("record")
        data["domain"] = domain
    return data


def finish(backend, operation, saved, *, event, args, extra=None):
    payload = {"operation_id": operation.id, "save": result_data(saved)}
    kind = "session" if isinstance(operation.after, Session) else "stream"
    accepted = saved.domain.record if saved.domain and saved.domain.record else operation.after
    payload[kind] = record_to_data(accepted)
    ok = saved.accepted
    if ok:
        try:
            publication = maybe_sync(backend, event)
            payload["publication"] = result_data(publication)
            ok = not publication.requires_repair
            if ok:
                current = backend.snapshot().catalog.get(operation.target)
                payload[kind] = record_to_data(current)
        except (StreamError, StorageError, OSError, ValueError) as error:
            payload["completion_error"] = {"message": str(error), "detail": "local save succeeded; inspect before retrying"}
            ok = False
    payload.update(extra or {})
    emit(payload, args)
    return 0 if ok else 2


def text_value(args, name):
    value = getattr(args, name, None)
    path = getattr(args, name + "_file", None)
    if path is not None:
        value = sys.stdin.read() if path == "-" else Path(path).expanduser().read_text(encoding="utf-8")
    return value


def scalar_changes(args, fields):
    changes = {}
    for name in fields:
        value = text_value(args, name)
        if value is not None:
            changes[name] = value
    for name in ("title", "goal"):
        mode = getattr(args, name + "_mode", None)
        if mode is not None:
            changes[name + "_mode"] = mode
    if getattr(args, "clear_goal", False):
        changes["goal"] = None
    return changes


def collection_changes(args, before):
    changes = {}
    if args.add_repository or args.remove_repository:
        repositories = dict(before.repositories)
        for alias in args.remove_repository:
            repositories.pop(alias, None)
        for alias, url in args.add_repository:
            repository = Repository(url)
            if alias in repositories and repositories[alias] != repository:
                raise StreamError(f"repository {alias!r} exists; remove and add it explicitly to replace it")
            repositories[alias] = repository
        changes["repositories"] = repositories
    for field, cls in (("documents", Document), ("issues", Link), ("prs", Link)):
        singular = {"documents": "document", "issues": "issue", "prs": "pr"}[field]
        additions, removals = getattr(args, "add_" + singular), getattr(args, "remove_" + singular)
        if not additions and not removals:
            continue
        def identity(item):
            return item.identity if isinstance(item, Document) else item.url
        removed = {tuple(item) for item in removals} if field == "documents" else set(removals)
        values = [item for item in getattr(before, field) if identity(item) not in removed]
        for arguments in additions:
            item = cls(*arguments)
            old = next((old for old in values if identity(old) == identity(item)), None)
            if old is not None and old != item:
                raise StreamError(f"{singular} exists with different contents; remove and add it explicitly")
            if old is None:
                values.append(item)
        changes[field] = tuple(values)
    documents = list(changes.get("documents", before.documents))
    for repository, path, commit in args.document_added_at:
        matches = [index for index, item in enumerate(documents) if item.identity == (repository, path)]
        if not matches:
            raise StreamError(f"no document matches repository/path: {repository}:{path}")
        index = matches[0]
        documents[index] = replace(documents[index], added_at=commit)
        changes["documents"] = tuple(documents)
    return changes


def command_init(args):
    backend = config.initialize(args.sync, remote=args.remote, branch=args.branch)
    emit({"catalog": str(config.catalog_root()), "sync": args.sync, "host_id": backend.host_id}, args)
    return 0


def command_config(args):
    if args.action == "set":
        config.set_config(args.key, args.value)
    values = config.load_config()
    if args.action:
        if args.key not in config.CONFIG_KEYS:
            raise StreamError(f"unknown configuration key: {args.key}")
        section, name = args.key.split(".")
        value = values[section][name]
        print(json.dumps(value) if args.json else value)
    elif args.json:
        emit(values, args)
    else:
        print(f"Config: {config.config_path()}")
        overrides = config.read_overrides()
        for key in config.CONFIG_KEYS:
            section, name = key.split(".")
            source = "configured" if name in overrides.get(section, {}) else "default"
            print(f"{key}: {json.dumps(values[section][name])} ({source})")
    return 0


def command_frontend(args):
    emit(resolve_frontend(config.get_frontend_mode(), args.frontend, dict(os.environ)).as_json(), args)
    return 0


def command_list(args):
    backend = config.open_catalog()
    catalog, sharing = read_catalog(backend, args)
    query = (args.query or "").casefold()
    streams = [item for item in catalog.streams if
               (args.all or item.archived == args.archived) and
               (not args.status or item.status in args.status) and
               (query in item.title.casefold() or query in (item.goal or "").casefold())]
    streams.sort(key=lambda item: (item.title.casefold(), item.id))
    if args.json:
        emit({"streams": [record_to_data(item) for item in streams], "sharing": sharing}, args)
    else:
        print("ID  STATUS  TITLE")
        for item in streams:
            title = "".join(char if char.isprintable() else " " for char in item.title)
            print(f"{item.id}  {item.status}  {title}")
        if sharing.get("detail"):
            print(sharing["detail"], file=sys.stderr)
        if sharing["conflicts"]:
            print("catalog conflicts: showing local records; use list --json or show for confirmed alternatives", file=sys.stderr)
    return 0


def command_show(args):
    backend = config.open_catalog()
    catalog, sharing = read_catalog(backend, args)
    stream = target_stream(catalog, args)
    emit({"stream": record_to_data(stream), "sessions": [record_to_data(item) for item in catalog.sessions if item.stream_id == stream.id], "sharing": sharing}, args)
    return 0


def command_create(args):
    backend = config.open_catalog()
    changes = scalar_changes(args, ("goal", "status"))
    stream = Stream(id=str(uuid4()), title=args.title, goal=None, owner_host_id=backend.host_id, title_mode="manual")
    changes.update(collection_changes(args, stream))
    if changes:
        stream = replace(stream, **normalize_changes(stream, changes, actor="user"))
    operation = Operation(str(uuid4()), None, stream, "user")
    saved = save(backend, operation)
    extra = {}
    code = 0
    if saved.accepted:
        try:
            local = runtime()
            local.remember_directory(stream.id, Path(args.cwd or Path.cwd()))
            if args.command == "start":
                extra["runtime"] = local.open(stream, (), authority=backend.authority(stream.id), frontend=args.frontend, cwd=Path(args.cwd) if args.cwd else None)
        except (StreamError, OSError, ValueError) as error:
            failure = error if isinstance(error, StreamError) else StreamError(str(error))
            extra["runtime"] = {"error": failure.as_json(), "detail": "stream saved; retry open with its ID"}
            code = 2
    result = finish(backend, operation, saved, event="start" if args.command == "start" else "stream", args=args, extra=extra)
    return code or result


def command_update(args):
    backend = config.open_catalog()
    before = target_stream(backend.snapshot().catalog, args, local=True)
    changes = scalar_changes(args, ("title", "goal", "status"))
    changes.update(collection_changes(args, before))
    if not changes:
        raise StreamError("update requires at least one field change")
    operation = update(str(uuid4()), before, changes)
    return finish(backend, operation, save(backend, operation), event="stream", args=args)


def command_archive(args):
    backend = config.open_catalog()
    before = target_stream(backend.snapshot().catalog, args, local=True)
    operation = update(str(uuid4()), before, {"archived": args.command == "archive"})
    return finish(backend, operation, save(backend, operation), event="stream", args=args)


def command_check(args):
    backend = config.open_catalog()
    catalog = backend.snapshot().catalog
    stream = resolve(catalog.streams, args.ref) if args.ref is not None else None
    emit({"valid": True, "streams": 1 if stream else len(catalog.streams), "sessions": sum(item.stream_id == stream.id for item in catalog.sessions) if stream else len(catalog.sessions)}, args)
    return 0


def command_sync(args):
    result = maybe_sync(config.open_catalog(), args.event)
    emit(result_data(result), args)
    return 2 if result.needs_attention else 0


def command_runtime(args):
    backend = config.open_catalog()
    catalog = backend.snapshot().catalog
    stream = target_stream(catalog, args, local=True)
    local = runtime()
    authority = backend.authority(stream.id)
    sessions = tuple(item for item in catalog.sessions if item.stream_id == stream.id)
    kwargs = {"authority": authority, "session_ref": args.session, "cwd": Path(args.cwd) if args.cwd else None}
    if args.command == "open":
        result = local.open(stream, sessions, frontend=args.frontend, **kwargs)
        publication = maybe_sync(backend, "start")
        emit({"stream_id": stream.id, "runtime": result, "publication": result_data(publication)}, args)
    else:
        # Resume replaces this process, so perform the event check before exec.
        maybe_sync(backend, "start")
        local.resume(stream, sessions, **kwargs)
    return 0


def command_session(args):
    backend = config.open_catalog()
    catalog = backend.snapshot().catalog
    if args.session_command == "list":
        stream = resolve(catalog.streams, args.stream) if args.stream else None
        emit({"sessions": [record_to_data(item) for item in catalog.sessions if stream is None or item.stream_id == stream.id]}, args)
        return 0
    if args.session_command == "show":
        emit({"session": record_to_data(resolve(catalog.sessions, args.ref))}, args)
        return 0
    if args.session_command == "attach":
        stream = target_stream(catalog, args, local=True)
        authority = backend.authority(stream.id)
        checkout = Checkout(args.repository, args.vcs_ref, args.directory_within_checkout) if args.repository else None
        if args.repository is None and (args.vcs_ref is not None or args.directory_within_checkout != "."):
            raise StreamError("checkout --ref/--directory-within-checkout requires --repository")
        if args.repository is not None and args.vcs_ref is None:
            raise StreamError("checkout --repository requires --ref")
        session = Session(str(uuid4()), backend.host_id, stream.id, args.harness, args.session_id, args.title, title_mode="manual", checkout=checkout)
        operation = Operation(str(uuid4()), None, session, "user", ownership_token=authority.token if authority else None)
    else:
        before = resolve(catalog.sessions, args.ref)
        authority = backend.authority(before.stream_id)
        changes = scalar_changes(args, ("title", "status", "state", "remaining"))
        if not changes:
            raise StreamError("session update requires at least one field change")
        operation = update(str(uuid4()), before, changes, ownership_token=authority.token if authority else None)
    saved = save(backend, operation)
    extra = None
    if args.session_command == "attach" and saved.accepted:
        try:
            runtime().remember_directory(stream.id, Path(args.cwd or Path.cwd()), session_id=session.id)
        except (StreamError, OSError) as error:
            extra = {"runtime": {"error": str(error), "detail": "session saved; pass --cwd when resuming"}}
    return finish(backend, operation, saved, event="attach" if args.session_command == "attach" else "session", args=args, extra=extra)


def command_agent(args):
    from . import catalog_agent
    backend = config.open_catalog()
    if args.agent_command == "context":
        emit(catalog_agent.context(backend, harness=args.harness, native_id=args.session_id, stream_id=args.stream_id,
                                   workspace_stream_id=runtime().context_stream_id()), args)
        return 0
    source = sys.stdin.read()
    if args.agent_command == "apply":
        operation = catalog_agent.parse_update(source, catalog_source=catalog_agent.source_id(backend))
        return finish(backend, operation, save(backend, operation),
                      event="session" if isinstance(operation.after, Session) else "stream", args=args)
    payload, code = catalog_agent.enroll(backend, runtime(), source)
    if code == 0 or any(item["save"].accepted for item in payload["saves"]):
        try:
            publication = maybe_sync(backend, "attach")
            payload["publication"] = result_data(publication)
            if publication.requires_repair:
                code = 2
        except (StreamError, StorageError, OSError, ValueError) as error:
            payload["completion_error"] = {"message": str(error), "detail": "local save succeeded; inspect before retrying"}
            code = 2
    emit(payload, args)
    return code


def add_output(parser):
    parser.add_argument("--json", action="store_true", help="print structured JSON instead of the default output")


def add_target(parser):
    parser.add_argument("ref", nargs="?", help="stable ID, exact title or unique title substring")
    parser.add_argument("-c", "--current", action="store_true", help="use the calling session/workspace's explicit association")
    parser.add_argument("-f", "--frontend", choices=("auto", *Frontend), help="select a runtime backend independently of --current")


def add_text(parser, name, *, clear=False):
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--" + name, help=f"set {name}")
    group.add_argument("--" + name + "-file", help="read UTF-8 text from a file; - reads stdin")
    if clear:
        group.add_argument("--clear-" + name, action="store_true", help=f"make {name} unspecified")


def add_stream_fields(parser):
    add_text(parser, "goal", clear=True)
    parser.add_argument("--status", choices=STATUSES)
    for field in ("title", "goal"):
        parser.add_argument("--" + field + "-mode", choices=("automatic", "manual"))
    parser.add_argument("--add-repository", nargs=2, metavar=("ALIAS", "URL"), action="append", default=[])
    parser.add_argument("--remove-repository", metavar="ALIAS", action="append", default=[])
    parser.add_argument("--add-document", nargs=4, metavar=("REPOSITORY", "REF", "PATH", "DESCRIPTION"), action="append", default=[])
    parser.add_argument("--remove-document", nargs=2, metavar=("REPOSITORY", "PATH"), action="append", default=[])
    parser.add_argument("--document-added-at", nargs=3, metavar=("REPOSITORY", "PATH", "COMMIT"), action="append", default=[],
                        help="record a known commit for document recovery, not revision-pinned opening")
    for field in ("issue", "pr"):
        parser.add_argument("--add-" + field, nargs=2, metavar=("URL", "DESCRIPTION"), action="append", default=[])
        parser.add_argument("--remove-" + field, metavar="URL", action="append", default=[])


def build_parser():
    parser = argparse.ArgumentParser(description="Manage durable work streams.", allow_abbrev=False)
    sub = parser.add_subparsers(dest="command", required=True, parser_class=lambda **kwargs: argparse.ArgumentParser(allow_abbrev=False, **kwargs))
    init = sub.add_parser("init", help="explicitly initialize a new local or shared catalog")
    init.add_argument("--sync", choices=("none", "git"), required=True)
    init.add_argument("--remote")
    init.add_argument("--branch", default="main")
    init.set_defaults(func=command_init)
    settings = sub.add_parser("config", help="read or change catalog location and frontend settings")
    settings.set_defaults(func=command_config)
    actions = settings.add_subparsers(dest="action")
    for name in ("get", "set"):
        action = actions.add_parser(name, allow_abbrev=False)
        action.add_argument("key")
        if name == "set":
            action.add_argument("value")
        add_output(action)
    frontend = sub.add_parser("frontend", help="inspect frontend selection without native calls")
    frontend_sub = frontend.add_subparsers(required=True)
    frontend_get = frontend_sub.add_parser("get", allow_abbrev=False)
    frontend_get.add_argument("-f", "--frontend", choices=("auto", *Frontend))
    add_output(frontend_get)
    frontend_get.set_defaults(func=command_frontend)
    listing = sub.add_parser("list", help="list streams with literal title/goal search")
    listing.add_argument("query", nargs="?")
    listing.add_argument("--status", choices=STATUSES, action="append")
    visibility = listing.add_mutually_exclusive_group()
    visibility.add_argument("--archived", action="store_true")
    visibility.add_argument("--all", action="store_true")
    listing.add_argument("--refresh", action="store_true", help="fetch shared state now without pushing")
    listing.set_defaults(func=command_list)
    show = sub.add_parser("show", help="show the complete stream and its sessions as YAML")
    add_target(show)
    show.add_argument("--refresh", action="store_true", help="fetch shared state now without pushing")
    show.set_defaults(func=command_show)
    for name in ("create", "start"):
        create = sub.add_parser(name, help="create a stream" + (" and open its workspace" if name == "start" else " without launching anything"))
        create.add_argument("title")
        add_stream_fields(create)
        create.add_argument("--cwd", help="record a host-local working directory (default: caller's cwd)")
        if name == "start":
            create.add_argument("-f", "--frontend", choices=("auto", *Frontend))
        create.set_defaults(func=command_create)
    editing = sub.add_parser("update", help="edit explicit stream fields without changing its identity")
    add_target(editing)
    editing.add_argument("--title")
    add_stream_fields(editing)
    editing.set_defaults(func=command_update)
    for name in ("archive", "restore"):
        lifecycle = sub.add_parser(name, help="change visibility without changing work status or closing runtimes")
        add_target(lifecycle)
        lifecycle.set_defaults(func=command_archive)
    check = sub.add_parser("check", help="validate saved catalog records and relationships")
    check.add_argument("ref", nargs="?")
    check.set_defaults(func=command_check)
    sync = sub.add_parser("sync", help="attempt explicit publication and reconciliation now")
    sync.add_argument("--event", choices=("explicit", "start", "shutdown"), default="explicit",
                      help="lifecycle trigger; start respects the stream interval, shutdown bypasses timing")
    sync.set_defaults(func=command_sync)
    for name in ("open", "resume"):
        native = sub.add_parser(name, help="open a workspace" if name == "open" else "resume one recorded conversation in this terminal")
        add_target(native)
        native.add_argument("--session", help="select a session by ID/title when more than one is eligible")
        native.add_argument("--cwd", help="use and remember an explicit local directory")
        native.set_defaults(func=command_runtime)
    sessions = sub.add_parser("session", help="inspect, attach and report individual contributions")
    session_sub = sessions.add_subparsers(dest="session_command", required=True)
    session_list = session_sub.add_parser("list", allow_abbrev=False)
    session_list.add_argument("--stream", help="limit sessions to this stream")
    session_show = session_sub.add_parser("show", allow_abbrev=False)
    session_show.add_argument("ref")
    attach = session_sub.add_parser("attach", allow_abbrev=False)
    add_target(attach)
    attach.add_argument("--harness", choices=("pi", "codex", "claude"), required=True)
    attach.add_argument("--session-id", required=True, help="existing native conversation identity; does not start it")
    attach.add_argument("--title", required=True)
    attach.add_argument("--cwd")
    attach.add_argument("--repository", help="stream repository alias for this checkout")
    attach.add_argument("--ref", dest="vcs_ref", help="portable VCS branch/bookmark reference")
    attach.add_argument("--directory-within-checkout", default=".")
    session_update = session_sub.add_parser("update", allow_abbrev=False)
    session_update.add_argument("ref")
    session_update.add_argument("--title")
    session_update.add_argument("--title-mode", choices=("automatic", "manual"))
    session_update.add_argument("--status", choices=STATUSES)
    add_text(session_update, "state")
    add_text(session_update, "remaining")
    for child in (session_list, session_show, attach, session_update):
        child.set_defaults(func=command_session)
        add_output(child)
    agents = sub.add_parser("agent", help="local captured-record integration for agent harnesses")
    agent_sub = agents.add_subparsers(dest="agent_command", required=True)
    context = agent_sub.add_parser("context", allow_abbrev=False, help="read local records without fetching or attaching")
    context.add_argument("--harness", choices=("pi", "codex", "claude"), required=True)
    context.add_argument("--session-id", required=True)
    context.add_argument("--stream-id", help="exact stream identity; never switches an attached conversation")
    enroll = agent_sub.add_parser("enroll", allow_abbrev=False, help="enroll the caller from a structured stdin request")
    apply = agent_sub.add_parser("apply", allow_abbrev=False, help="apply a captured-record update from stdin")
    for child in (context, enroll, apply):
        child.set_defaults(func=command_agent)
        add_output(child)
    for name, child in sub.choices.items():
        if name not in ("frontend", "session", "agent"):
            add_output(child)
    return parser


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if sum(value == "-" for key, value in vars(args).items() if key.endswith("_file")) > 1:
            raise StreamError("only one field can read stdin in a command")
        return args.func(args)
    except (StreamError, ValidationError, StorageError, OSError, ValueError) as error:
        failure = error if isinstance(error, StreamError) else StreamError(str(error))
        if getattr(args, "json", False):
            print(json.dumps({"error": failure.as_json()}, ensure_ascii=False, indent=2))
        else:
            print(f"streams: {failure}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
