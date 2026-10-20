"""Validate durable catalog, host-local runtime, and native Herdr work-card invariants."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any

from .catalog.model import Catalog, Session, Stream
from .catalog_agent import source_id
from .catalog_checkouts import infer_checkout_repository_urls, inspect_directory
from .catalog_conversations import native_identity
from .catalog_runtime import Runtime, _key
from .errors import StreamError
from .herdr import HerdrClient

PAIR_KEYS = ("strm_agent", "strm_nonce", "strm_pi", "strm_card", "strm_role", "strm_native")


def _js_digest(value: object) -> str:
    return hashlib.sha256(json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")).hexdigest()


def _should_check_herdr(local: Runtime, *, pane_id: str | None, frontend: str | None) -> bool:
    if pane_id is not None or frontend == "herdr":
        return True
    if frontend == "cmux":
        return False
    if os.environ.get("HERDR_ENV") == "1" or os.environ.get("HERDR_SOCKET_PATH"):
        return True
    config_home = Path(os.environ.get("XDG_CONFIG_HOME") or (Path.home() / ".config"))
    if (config_home / "herdr").exists():
        return True
    return any((local.state_dir / name).exists() for name in ("associations", "enrollments", "attempts"))


def _check_runtime_files(
    catalog: Catalog,
    local: Runtime,
    target_stream_ids: set[str] | None,
    issues: list[dict[str, Any]],
) -> None:
    for path in sorted((local.state_dir / "attempts").glob("*/*.json")):
        try:
            raw = json.loads(path.read_text())
        except (OSError, ValueError) as exc:
            issues.append({
                "kind": "invalid-open-attempt",
                "path": str(path),
                "message": f"invalid open attempt at {path}: {exc}",
            })
            continue
        stream_id = raw.get("stream_id") if isinstance(raw, dict) else None
        if target_stream_ids is None or stream_id in target_stream_ids:
            issues.append({
                "kind": "pending-open-attempt",
                "stream_id": stream_id,
                "workspace_id": raw.get("workspace_id") if isinstance(raw, dict) else None,
                "path": str(path),
                "message": f"pending or partial open attempt at {path} blocks workspace open and lock acquisition",
            })

    for path in sorted((local.state_dir / "enrollments").glob("*/*.json")):
        try:
            claim = local._enrollment_claim(path)
        except StreamError as exc:
            issues.append({
                "kind": "invalid-enrollment-claim",
                "path": str(path),
                "message": str(exc),
            })
            continue
        if claim is None or (target_stream_ids is not None and claim["stream_id"] not in target_stream_ids):
            continue
        assoc_path = local.state_dir / "associations" / path.parent.name / path.name
        try:
            assoc = local._association(assoc_path)
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

    for path in sorted((local.state_dir / "associations").glob("*/*.json")):
        try:
            assoc = local._association(path)
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


def _check_directory_selection(
    stream_id: str,
    session_id: str | None,
    local: Runtime,
    *,
    fix: bool,
    issues: list[dict[str, Any]],
    fixed: list[dict[str, Any]],
) -> dict[str, Any] | None:
    if not local.locations.path(stream_id, session_id).exists():
        return None
    try:
        selection = local.locations._selection(stream_id, session_id)
        if selection is None:
            return None
        if selection["version"] == 1:
            cwd = local.locations.checked(Path(selection["cwd"]))
            found = inspect_directory(cwd)
            if found is not None:
                registered = local.checkouts.find(found[0])
                if registered is None or not registered.repository_urls:
                    if fix:
                        urls = infer_checkout_repository_urls(found[0], local.checkouts)
                        if registered is None:
                            registered = local.checkouts.register(found[0])
                        for url in urls:
                            registered = local.checkouts.register(found[0], repository_url=url)
                        local.locations.remember(stream_id, cwd, session_id=session_id)
                        fixed.append({
                            "kind": "legacy-session-checkout-registered",
                            "stream_id": stream_id,
                            "session_id": session_id,
                            "checkout_id": registered.id,
                            "directory": str(found[0]),
                            "repository_urls": list(registered.repository_urls),
                        })
                        return local.locations._selection(stream_id, session_id)
                    issues.append({
                        "kind": "unregistered-session-checkout",
                        "stream_id": stream_id,
                        "session_id": session_id,
                        "cwd": str(cwd),
                        "message": f"session directory {cwd} is in a VCS checkout that is not registered with a repository URL",
                    })
        else:
            loc = selection["location"]
            if loc["kind"] == "directory":
                local.locations.checked(Path(loc["cwd"]))
            else:
                local.checkouts.directory(loc["id"], loc["within"])
                chk = local.checkouts.get(loc["id"])
                if not chk.repository_urls:
                    if fix:
                        urls = infer_checkout_repository_urls(Path(chk.directory), local.checkouts)
                        if urls:
                            for url in urls:
                                local.checkouts.register(Path(chk.directory), repository_url=url)
                            fixed.append({
                                "kind": "session-checkout-repository-bound",
                                "stream_id": stream_id,
                                "session_id": session_id,
                                "checkout_id": chk.id,
                                "directory": chk.directory,
                                "repository_urls": list(urls),
                            })
                            return selection
                    issues.append({
                        "kind": "unbound-session-checkout",
                        "stream_id": stream_id,
                        "session_id": session_id,
                        "checkout_id": chk.id,
                        "directory": chk.directory,
                        "message": f"checkout {chk.directory} has no repository URL binding",
                    })
        return selection
    except StreamError as exc:
        issues.append({
            "kind": "invalid-directory-binding",
            "stream_id": stream_id,
            "session_id": session_id,
            "message": str(exc),
        })
        return None


def _check_stream_locations(
    stream: Stream,
    sessions: tuple[Session, ...],
    local: Runtime,
    *,
    fix: bool,
    issues: list[dict[str, Any]],
    fixed: list[dict[str, Any]],
) -> None:
    _check_directory_selection(stream.id, None, local, fix=fix, issues=issues, fixed=fixed)
    for session in sessions:
        if session.host_id != local.host_id:
            continue
        _check_directory_selection(stream.id, session.id, local, fix=fix, issues=issues, fixed=fixed)


def _check_herdr_locks_and_state(
    herdr_dir: Path,
    *,
    unscoped: bool,
    fix: bool,
    issues: list[dict[str, Any]],
    fixed: list[dict[str, Any]],
) -> None:
    if not herdr_dir.exists():
        return
    for lock_path in sorted(herdr_dir.glob(".work-cards-*.lock")):
        try:
            if lock_path.is_symlink() or not lock_path.is_file():
                raise ValueError("lock path is not a regular file")
            content = lock_path.read_text(encoding="utf-8", errors="replace")
        except (OSError, ValueError) as exc:
            issues.append({
                "kind": "invalid-work-cards-lock",
                "path": str(lock_path),
                "message": f"invalid work-cards lock {lock_path}: {exc}",
            })
            continue
        if content != "work-cards workspace lock v1\n":
            if fix and not lock_path.is_symlink():
                lock_path.unlink(missing_ok=True)
                fixed.append({
                    "kind": "legacy-work-cards-lock-removed",
                    "path": str(lock_path),
                })
            else:
                issues.append({
                    "kind": "legacy-work-cards-lock",
                    "path": str(lock_path),
                    "content": content.strip(),
                    "message": f"legacy or malformed work-cards lock {lock_path} (expected 'work-cards workspace lock v1') blocks native workspace lock acquisition",
                })

    if not unscoped:
        return
    state_dir = herdr_dir / ".streams-work-cards"
    if not state_dir.exists():
        return
    for pending_path in sorted(state_dir.glob("*.json.pending")):
        json_path = pending_path.with_suffix("")
        record = None
        if json_path.exists():
            try:
                record = json.loads(json_path.read_text())
            except (OSError, ValueError):
                record = None
        issues.append({
            "kind": "pending-work-card-launch",
            "path": str(pending_path),
            "key": record.get("key") if isinstance(record, dict) else pending_path.name.removesuffix(".json.pending"),
            "pane_id": record.get("paneId") if isinstance(record, dict) else None,
            "terminal_id": record.get("terminalId") if isinstance(record, dict) else None,
            "tab_id": record.get("tabId") if isinstance(record, dict) else None,
            "message": f"pending work-card launch sidecar {pending_path} blocks automatic card open",
        })


def _check_herdr_live(
    backend,
    catalog: Catalog,
    local: Runtime,
    *,
    target_stream: Stream | None,
    pane_id: str | None,
    fix: bool,
    issues: list[dict[str, Any]],
    fixed: list[dict[str, Any]],
) -> set[str] | None:
    client = HerdrClient()
    try:
        endpoint = client.endpoint()
        snapshot = client.command_result(["api", "snapshot"], "session_snapshot")["snapshot"]
    except StreamError as exc:
        if pane_id is not None:
            issues.append({
                "kind": "herdr-unavailable",
                "pane_id": pane_id,
                "message": str(exc),
            })
        return None

    herdr_dir = Path(endpoint.socket).parent
    scope = _key("herdr", endpoint.socket, endpoint.session)
    _check_herdr_locks_and_state(
        herdr_dir,
        unscoped=target_stream is None and pane_id is None,
        fix=fix,
        issues=issues,
        fixed=fixed,
    )

    workspaces = snapshot.get("workspaces") if isinstance(snapshot, dict) else None
    panes = snapshot.get("panes") if isinstance(snapshot, dict) else None
    if not isinstance(workspaces, list) or not isinstance(panes, list):
        issues.append({
            "kind": "invalid-herdr-snapshot",
            "message": "Herdr snapshot is missing workspaces or panes",
        })
        return None

    associations = {}
    for path in sorted((local.state_dir / "associations" / scope).glob("*.json")):
        try:
            assoc = local._association(path)
        except StreamError:
            continue
        if assoc is not None:
            associations[assoc["workspace_id"]] = assoc["stream_id"]

    if pane_id is not None:
        target_pane = next((p for p in panes if isinstance(p, dict) and p.get("pane_id") == pane_id), None)
        if target_pane is None:
            issues.append({
                "kind": "pane-not-found",
                "pane_id": pane_id,
                "message": f"Herdr pane {pane_id!r} is not present in the live snapshot",
            })
            return set()
        candidate_panes = [target_pane]
        candidate_workspaces = [w for w in workspaces if isinstance(w, dict) and w.get("workspace_id") == target_pane.get("workspace_id")]
    elif target_stream is not None:
        target_ws_ids = {
            ws_id for ws_id, s_id in associations.items() if s_id == target_stream.id
        } | {
            w.get("workspace_id")
            for w in workspaces
            if isinstance(w, dict) and isinstance(w.get("tokens"), dict) and w["tokens"].get("strm_stream") == target_stream.id
        }
        candidate_workspaces = [w for w in workspaces if isinstance(w, dict) and w.get("workspace_id") in target_ws_ids]
        candidate_panes = [p for p in panes if isinstance(p, dict) and p.get("workspace_id") in target_ws_ids]
    else:
        candidate_workspaces = [w for w in workspaces if isinstance(w, dict)]
        candidate_panes = [p for p in panes if isinstance(p, dict)]

    cat_source = source_id(backend)
    cat_source_digest = _js_digest(cat_source)
    checked_streams: set[str] = set()

    for ws in candidate_workspaces:
        ws_id = ws.get("workspace_id")
        tokens = ws.get("tokens") if isinstance(ws.get("tokens"), dict) else {}
        bound_stream_id = associations.get(ws_id)
        if bound_stream_id:
            checked_streams.add(bound_stream_id)
        if any(k.startswith("strm_") for k in tokens):
            expected_stream_id = bound_stream_id or tokens.get("strm_stream")
            if (
                not tokens.get("strm_binding")
                or tokens.get("strm_host") != backend.host_id
                or tokens.get("strm_catalog") != cat_source_digest
                or (bound_stream_id is not None and tokens.get("strm_stream") != bound_stream_id)
            ):
                issues.append({
                    "kind": "conflicting-workspace-binding",
                    "workspace_id": ws_id,
                    "stream_id": expected_stream_id,
                    "tokens": {k: v for k, v in tokens.items() if k.startswith("strm_")},
                    "message": f"workspace {ws_id} has conflicting or incomplete strm_* binding tokens",
                })

    state_dir = herdr_dir / ".streams-work-cards"

    for p in candidate_panes:
        p_id = p.get("pane_id")
        term_id = p.get("terminal_id")
        tab_id = p.get("tab_id")
        ws_id = p.get("workspace_id")
        tokens = p.get("tokens") if isinstance(p.get("tokens"), dict) else {}
        role = tokens.get("strm_role")

        if role == "card":
            owner_term = tokens.get("strm_pi")
            owner_pane = next((op for op in panes if isinstance(op, dict) and op.get("terminal_id") == owner_term), None)
            if owner_pane is None:
                issues.append({
                    "kind": "orphaned-card-pane",
                    "pane_id": p_id,
                    "terminal_id": term_id,
                    "workspace_id": ws_id,
                    "tab_id": tab_id,
                    "owner_terminal_id": owner_term,
                    "message": f"card pane {p_id} references missing owner terminal {owner_term!r}",
                })
            elif owner_pane.get("tab_id") != tab_id or owner_pane.get("workspace_id") != ws_id:
                issues.append({
                    "kind": "separated-card-pane",
                    "pane_id": p_id,
                    "owner_pane_id": owner_pane.get("pane_id"),
                    "workspace_id": ws_id,
                    "tab_id": tab_id,
                    "message": f"card pane {p_id} is separated from owner pane {owner_pane.get('pane_id')}",
                })
            continue

        is_pi_agent = p.get("agent") == "pi" or role == "agent" or p_id == pane_id
        if not is_pi_agent:
            continue

        bound_stream_id = associations.get(ws_id)
        if bound_stream_id:
            checked_streams.add(bound_stream_id)

        agent_session = p.get("agent_session")
        locator = agent_session.get("value") if isinstance(agent_session, dict) and agent_session.get("kind") == "path" else None
        if not isinstance(locator, str) or not locator.strip():
            if p.get("agent") == "pi" or p_id == pane_id or any(k in tokens for k in PAIR_KEYS):
                issues.append({
                    "kind": "missing-agent-session-locator",
                    "pane_id": p_id,
                    "workspace_id": ws_id,
                    "tab_id": tab_id,
                    "stream_id": bound_stream_id,
                    "message": f"pane {p_id} has no Pi session transcript path",
                })
            continue

        try:
            native_session_id, _ = native_identity(agent_session, "pi")
        except StreamError as exc:
            issues.append({
                "kind": "unverified-agent-conversation",
                "pane_id": p_id,
                "workspace_id": ws_id,
                "tab_id": tab_id,
                "stream_id": bound_stream_id,
                "locator": locator,
                "message": str(exc),
            })
            continue

        matching_sessions = [
            s for s in catalog.sessions
            if s.host_id == backend.host_id and s.harness == "pi" and s.harness_session_id == native_session_id
        ]
        if not matching_sessions:
            issues.append({
                "kind": "unenrolled-agent-session",
                "pane_id": p_id,
                "workspace_id": ws_id,
                "tab_id": tab_id,
                "stream_id": bound_stream_id,
                "native_session_id": native_session_id,
                "message": f"Pi conversation {native_session_id} in pane {p_id} is not enrolled in the catalog",
            })
            continue

        session = matching_sessions[0]
        checked_streams.add(session.stream_id)
        if target_stream is not None and session.stream_id != target_stream.id:
            continue

        if bound_stream_id is None:
            issues.append({
                "kind": "unbound-agent-workspace",
                "pane_id": p_id,
                "workspace_id": ws_id,
                "tab_id": tab_id,
                "stream_id": session.stream_id,
                "session_id": session.id,
                "native_session_id": native_session_id,
                "message": f"workspace {ws_id} has no host-local association for Stream {session.stream_id}",
            })
        elif bound_stream_id != session.stream_id:
            issues.append({
                "kind": "session-workspace-stream-mismatch",
                "pane_id": p_id,
                "workspace_id": ws_id,
                "tab_id": tab_id,
                "workspace_stream_id": bound_stream_id,
                "session_stream_id": session.stream_id,
                "session_id": session.id,
                "native_session_id": native_session_id,
                "message": f"Session {session.id} in pane {p_id} belongs to Stream {session.stream_id}, but workspace {ws_id} is bound to Stream {bound_stream_id}",
            })

        stream_record = catalog.stream(session.stream_id)
        if stream_record is not None and stream_record.documents:
            doc_locs = local.locations.document_context(stream_record, session)["document_locations"]
            if fix and any(loc["error"] is not None and "no repository binding" in loc["error"] for loc in doc_locs):
                try:
                    selected = local.locations.read(stream_record.id, session.id)
                    if selected is not None and selected.checkout_id is not None:
                        chk = local.checkouts.get(selected.checkout_id)
                        if not chk.repository_urls:
                            inferred = infer_checkout_repository_urls(Path(chk.directory), local.checkouts)
                            if inferred:
                                for url in inferred:
                                    local.checkouts.register(Path(chk.directory), repository_url=url)
                                fixed.append({
                                    "kind": "session-checkout-repository-bound",
                                    "pane_id": p_id,
                                    "session_id": session.id,
                                    "checkout_id": chk.id,
                                    "directory": chk.directory,
                                    "repository_urls": list(inferred),
                                })
                                doc_locs = local.locations.document_context(stream_record, session)["document_locations"]
                except StreamError:
                    pass
            for loc in doc_locs:
                if loc["error"] is not None:
                    if pane_id is None and loc["error"].startswith("Document not present in this checkout:"):
                        repo_obj = stream_record.repositories.get(loc["repository"])
                        if repo_obj is not None and any(
                            repo_obj.url in chk.repository_urls and (Path(chk.directory) / loc["path"]).is_file()
                            for chk in local.checkouts.list()
                        ):
                            continue
                    issues.append({
                        "kind": "unresolved-session-document",
                        "pane_id": p_id,
                        "workspace_id": ws_id,
                        "tab_id": tab_id,
                        "stream_id": session.stream_id,
                        "session_id": session.id,
                        "repository": loc["repository"],
                        "path": loc["path"],
                        "message": loc["error"],
                    })

        scope_obj = {
            "catalogSource": cat_source,
            "hostId": backend.host_id,
            "streamId": session.stream_id,
            "sessionId": session.id,
            "nativeSessionId": native_session_id,
        }
        expected_identity = _js_digest(scope_obj)
        expected_key = _js_digest([endpoint.socket, scope_obj, [p_id, term_id, tab_id, ws_id]])

        if state_dir.exists() and (target_stream is not None or pane_id is not None):
            pending_path = state_dir / f"{expected_key}.json.pending"
            if pending_path.exists():
                issues.append({
                    "kind": "pending-work-card-launch",
                    "pane_id": p_id,
                    "workspace_id": ws_id,
                    "tab_id": tab_id,
                    "stream_id": session.stream_id,
                    "session_id": session.id,
                    "key": expected_key,
                    "path": str(pending_path),
                    "message": f"pending work-card launch sidecar {pending_path} blocks automatic card open for pane {p_id}",
                })

        present_pair_keys = [k for k in PAIR_KEYS if k in tokens]
        if present_pair_keys:
            if tokens.get("strm_agent") != expected_identity or tokens.get("strm_native") != native_session_id:
                old_card_term = tokens.get("strm_card")
                old_card_pane = next(
                    (cp for cp in panes if isinstance(cp, dict) and cp.get("terminal_id") == old_card_term),
                    None,
                )
                if fix and old_card_pane is None:
                    clear_args = [arg for k in PAIR_KEYS if k in tokens for arg in ("--clear-token", k)]
                    client.run(["pane", "report-metadata", p_id, "--source", "streams-work-cards", *clear_args])
                    fixed.append({
                        "kind": "stale-agent-owner-markers-cleared",
                        "pane_id": p_id,
                        "terminal_id": term_id,
                        "workspace_id": ws_id,
                        "tab_id": tab_id,
                        "stream_id": session.stream_id,
                        "session_id": session.id,
                        "native_session_id": native_session_id,
                        "cleared_native_session_id": tokens.get("strm_native"),
                    })
                else:
                    issues.append({
                        "kind": "stale-agent-owner-markers",
                        "pane_id": p_id,
                        "terminal_id": term_id,
                        "workspace_id": ws_id,
                        "tab_id": tab_id,
                        "stream_id": session.stream_id,
                        "session_id": session.id,
                        "native_session_id": native_session_id,
                        "marker_native_session_id": tokens.get("strm_native"),
                        "marker_agent": tokens.get("strm_agent"),
                        "marker_card": old_card_term,
                        "card_live": old_card_pane is not None,
                        "message": (
                            f"pane {p_id} has stale strm_* owner markers for Pi session "
                            f"{tokens.get('strm_native')!r} while running enrolled Pi session "
                            f"{native_session_id!r} (causes 'Foreign or incomplete agent owner markers')"
                        ),
                    })
                continue

            if not all(tokens.get(k) for k in PAIR_KEYS) or tokens.get("strm_role") != "agent" or tokens.get("strm_pi") != term_id:
                issues.append({
                    "kind": "incomplete-agent-owner-markers",
                    "pane_id": p_id,
                    "workspace_id": ws_id,
                    "tab_id": tab_id,
                    "stream_id": session.stream_id,
                    "session_id": session.id,
                    "tokens": {k: tokens.get(k) for k in present_pair_keys},
                    "message": f"pane {p_id} has incomplete or invalid strm_* agent owner markers",
                })
                continue

        marked_cards = [
            cp for cp in panes
            if isinstance(cp, dict)
            and cp.get("pane_id") != p_id
            and isinstance(cp.get("tokens"), dict)
            and (cp["tokens"].get("strm_agent") == expected_identity or cp["tokens"].get("strm_pi") == term_id)
        ]
        if len(marked_cards) > 1:
            issues.append({
                "kind": "duplicate-card-panes",
                "pane_id": p_id,
                "card_pane_ids": [cp.get("pane_id") for cp in marked_cards],
                "stream_id": session.stream_id,
                "session_id": session.id,
                "message": f"multiple card panes reference owner pane {p_id}",
            })
        elif len(marked_cards) == 1:
            cp = marked_cards[0]
            ctokens = cp.get("tokens") or {}
            if not present_pair_keys:
                issues.append({
                    "kind": "half-stamped-card-pair",
                    "pane_id": p_id,
                    "card_pane_id": cp.get("pane_id"),
                    "workspace_id": ws_id,
                    "tab_id": tab_id,
                    "stream_id": session.stream_id,
                    "session_id": session.id,
                    "message": f"card pane {cp.get('pane_id')} is stamped for {p_id}, but owner pane {p_id} lacks strm_* markers",
                })
            elif (
                ctokens.get("strm_role") != "card"
                or ctokens.get("strm_agent") != expected_identity
                or ctokens.get("strm_pi") != term_id
                or ctokens.get("strm_card") != cp.get("terminal_id")
                or tokens.get("strm_card") != cp.get("terminal_id")
                or ctokens.get("strm_nonce") != tokens.get("strm_nonce")
                or ctokens.get("strm_native") != native_session_id
            ):
                issues.append({
                    "kind": "contradictory-card-pair-markers",
                    "pane_id": p_id,
                    "card_pane_id": cp.get("pane_id"),
                    "workspace_id": ws_id,
                    "tab_id": tab_id,
                    "stream_id": session.stream_id,
                    "session_id": session.id,
                    "message": f"owner pane {p_id} and card pane {cp.get('pane_id')} have contradictory strm_* pair markers",
                })

    return checked_streams


def check_all(
    backend,
    local: Runtime,
    *,
    stream: Stream | None = None,
    pane_id: str | None = None,
    frontend: str | None = None,
    fix: bool = False,
) -> dict[str, Any]:
    catalog = backend.snapshot().catalog
    issues: list[dict[str, Any]] = []
    fixed: list[dict[str, Any]] = []

    save_marker = backend.store.root / ".catalog-sync" / "save.json"
    if save_marker.exists():
        issues.append({
            "kind": "interrupted-catalog-save",
            "path": str(save_marker),
            "message": f"catalog save repair marker at {save_marker} blocks catalog writes",
        })

    herdr_streams: set[str] | None = None
    if _should_check_herdr(local, pane_id=pane_id, frontend=frontend):
        herdr_streams = _check_herdr_live(
            backend,
            catalog,
            local,
            target_stream=stream,
            pane_id=pane_id,
            fix=fix,
            issues=issues,
            fixed=fixed,
        )

    if stream is not None:
        streams_to_check = (stream,)
        target_stream_ids: set[str] | None = {stream.id}
    elif pane_id is not None and herdr_streams is not None:
        streams_to_check = tuple(s for s in catalog.streams if s.id in herdr_streams)
        target_stream_ids = set(herdr_streams)
    else:
        streams_to_check = catalog.streams
        target_stream_ids = None

    for s in streams_to_check:
        if s.owner_host_id == backend.host_id and backend.authority(s.id) is None:
            issues.append({
                "kind": "missing-stream-authority",
                "stream_id": s.id,
                "message": f"local owner stream {s.id!r} is missing its authority token in local.json",
            })
        if pane_id is None:
            s_sessions = tuple(item for item in catalog.sessions if item.stream_id == s.id)
            _check_stream_locations(s, s_sessions, local, fix=fix, issues=issues, fixed=fixed)

    if stream is None and pane_id is None:
        for chk in local.checkouts.list():
            if not chk.repository_urls:
                if fix:
                    urls = infer_checkout_repository_urls(Path(chk.directory), local.checkouts)
                    if urls:
                        for url in urls:
                            local.checkouts.register(Path(chk.directory), repository_url=url)
                        fixed.append({
                            "kind": "checkout-repository-bound",
                            "checkout_id": chk.id,
                            "directory": chk.directory,
                            "repository_urls": list(urls),
                        })
                        continue
                issues.append({
                    "kind": "unbound-checkout",
                    "checkout_id": chk.id,
                    "directory": chk.directory,
                    "message": f"registered checkout {chk.directory} has no repository URL binding",
                })

    _check_runtime_files(catalog, local, target_stream_ids, issues)

    session_count = (
        sum(item.stream_id == stream.id for item in catalog.sessions)
        if stream is not None
        else len(catalog.sessions)
    )
    result: dict[str, Any] = {
        "valid": len(issues) == 0,
        "streams": 1 if stream is not None else len(catalog.streams),
        "sessions": session_count,
    }
    if issues:
        result["issues"] = issues
    if fixed:
        result["fixed"] = fixed
    return result
