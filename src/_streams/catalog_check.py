"""Validate durable catalog, host-local runtime, and native Herdr work-card invariants."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any

from .catalog.model import Catalog, RecordKey, Session, Stream, record_key
from .catalog.sync.local import LocalCatalog
from .catalog.yaml_store import _directory as _store_directory, _path as _record_path
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
                elif fix and session_id is not None:
                    local.locations.remember(stream_id, cwd, session_id=session_id)
                    fixed.append({
                        "kind": "legacy-session-location-upgraded",
                        "stream_id": stream_id,
                        "session_id": session_id,
                        "checkout_id": registered.id,
                        "directory": str(found[0]),
                    })
                    return local.locations._selection(stream_id, session_id)
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


def _reconcile_pending_sidecar(
    pending_path: Path,
    live_by_pane: dict[str, dict] | None,
    *,
    fix: bool,
    fixed: list[dict[str, Any]],
    issues: list[dict[str, Any]],
    extra: dict[str, Any] | None = None,
) -> None:
    json_path = pending_path.with_suffix("")
    record = None
    if json_path.exists():
        try:
            record = json.loads(json_path.read_text())
        except (OSError, ValueError):
            record = None
    if fix and live_by_pane is not None and not pending_path.is_symlink() and not json_path.is_symlink():
        rec_pane = record.get("paneId") if isinstance(record, dict) else None
        rec_term = record.get("terminalId") if isinstance(record, dict) else None
        rec_tab = record.get("tabId") if isinstance(record, dict) else None
        rec_nonce = record.get("nonce") if isinstance(record, dict) else None
        live_card = live_by_pane.get(rec_pane) if isinstance(rec_pane, str) else None
        try:
            if live_card and live_card.get("terminal_id") == rec_term and live_card.get("tab_id") == rec_tab:
                card_tokens = live_card.get("tokens") if isinstance(live_card.get("tokens"), dict) else {}
                owner_term = card_tokens.get("strm_pi")
                owner_pane = next(
                    (p for p in live_by_pane.values() if p.get("terminal_id") == owner_term),
                    None,
                ) if isinstance(owner_term, str) else None
                owner_tokens = owner_pane.get("tokens") if isinstance(owner_pane, dict) and isinstance(owner_pane.get("tokens"), dict) else {}
                properly_stamped = (
                    card_tokens.get("strm_role") == "card"
                    and (not isinstance(rec_nonce, str) or card_tokens.get("strm_nonce") == rec_nonce)
                    and owner_pane is not None
                    and owner_tokens.get("strm_role") == "agent"
                    and owner_tokens.get("strm_card") == rec_term
                    and owner_tokens.get("strm_nonce") == card_tokens.get("strm_nonce")
                )
                if properly_stamped:
                    record["phase"] = "ready"
                    tmp_path = json_path.with_name(f".{json_path.name}.tmp-{os.getpid()}")
                    tmp_path.write_text(json.dumps(record) + "\n")
                    os.replace(tmp_path, json_path)
                    pending_path.unlink(missing_ok=True)
                    fixed.append({"kind": "pending-work-card-promoted-ready", "path": str(pending_path), "pane_id": rec_pane})
                    return
            if isinstance(rec_pane, str) and live_card is None and isinstance(record, dict):
                record["phase"] = "closed"
                tmp_path = json_path.with_name(f".{json_path.name}.tmp-{os.getpid()}")
                tmp_path.write_text(json.dumps(record) + "\n")
                os.replace(tmp_path, json_path)
                pending_path.unlink(missing_ok=True)
                fixed.append({"kind": "pending-work-card-marked-closed", "path": str(pending_path), "pane_id": rec_pane})
                return
            if rec_pane is None:
                pending_path.unlink(missing_ok=True)
                json_path.unlink(missing_ok=True)
                fixed.append({"kind": "unlaunched-pending-work-card-removed", "path": str(pending_path)})
                return
        except OSError:
            pass
    issue = {
        "kind": "pending-work-card-launch",
        "path": str(pending_path),
        "key": record.get("key") if isinstance(record, dict) else pending_path.name.removesuffix(".json.pending"),
        "pane_id": record.get("paneId") if isinstance(record, dict) else None,
        "terminal_id": record.get("terminalId") if isinstance(record, dict) else None,
        "tab_id": record.get("tabId") if isinstance(record, dict) else None,
        "message": f"pending work-card launch sidecar {pending_path} blocks automatic card open",
    }
    if extra:
        issue.update(extra)
    issues.append(issue)


def _check_herdr_locks_and_state(
    herdr_dir: Path,
    *,
    live_panes: list[dict[str, Any]] | None = None,
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
    live_by_pane = {p.get("pane_id") or p.get("id"): p for p in (live_panes or []) if isinstance(p, dict) and isinstance(p.get("pane_id") or p.get("id"), str)} if live_panes is not None else None
    for pending_path in sorted(state_dir.glob("*.json.pending")):
        _reconcile_pending_sidecar(pending_path, live_by_pane, fix=fix, fixed=fixed, issues=issues)


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
    workspaces = snapshot.get("workspaces") if isinstance(snapshot, dict) else None
    panes = snapshot.get("panes") if isinstance(snapshot, dict) else None
    _check_herdr_locks_and_state(
        herdr_dir,
        live_panes=panes if isinstance(panes, list) else None,
        unscoped=target_stream is None and pane_id is None,
        fix=fix,
        issues=issues,
        fixed=fixed,
    )
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
    if state_dir.exists() and target_stream is None and pane_id is None:
        live_by_pane = {p.get("pane_id"): p for p in panes if isinstance(p, dict) and isinstance(p.get("pane_id"), str)}
        for rec_path in sorted(state_dir.glob("*.json")):
            if rec_path.is_symlink() or rec_path.with_name(f"{rec_path.name}.pending").exists():
                continue
            try:
                rec = json.loads(rec_path.read_text())
            except (OSError, ValueError):
                continue
            if not isinstance(rec, dict) or rec.get("phase") != "ready":
                continue
            rec_pane = rec.get("paneId")
            rec_term = rec.get("terminalId")
            rec_tab = rec.get("tabId")
            op = live_by_pane.get(rec_pane) if isinstance(rec_pane, str) else None
            if (
                isinstance(op, dict)
                and op.get("tab_id") == rec_tab
                and op.get("terminal_id") != rec_term
                and not op.get("agent")
                and not op.get("agent_session")
                and not (op.get("tokens") or {})
                and str(op.get("cwd") or "").endswith("integrations/herdr-work-cards")
            ):
                if fix:
                    try:
                        client.run(["pane", "close", rec_pane])
                        rec["phase"] = "closed"
                        tmp_path = rec_path.with_name(f".{rec_path.name}.tmp-{os.getpid()}")
                        tmp_path.write_text(json.dumps(rec) + "\n")
                        os.replace(tmp_path, rec_path)
                        fixed.append({
                            "kind": "orphaned-restored-card-pane-closed",
                            "pane_id": rec_pane,
                            "tab_id": rec_tab,
                            "old_terminal_id": rec_term,
                            "restored_terminal_id": op.get("terminal_id"),
                        })
                    except StreamError:
                        pass
                else:
                    issues.append({
                        "kind": "orphaned-restored-card-pane",
                        "pane_id": rec_pane,
                        "tab_id": rec_tab,
                        "old_terminal_id": rec_term,
                        "restored_terminal_id": op.get("terminal_id"),
                        "message": f"pane {rec_pane} in tab {rec_tab} is an inert shell restored from previous card terminal {rec_term}",
                    })

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

        agent_session = p.get("agent_session")
        is_pi_agent = (
            p.get("agent") == "pi"
            or (isinstance(agent_session, dict) and (agent_session.get("agent") == "pi" or agent_session.get("agent_type") == "pi"))
            or role == "agent"
            or p_id == pane_id
        )
        if not is_pi_agent:
            continue

        bound_stream_id = associations.get(ws_id)
        if bound_stream_id:
            checked_streams.add(bound_stream_id)

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
            stream_sessions = tuple(s for s in catalog.sessions if s.stream_id == stream_record.id)
            doc_locs = local.locations.document_context(stream_record, session, sessions=stream_sessions)["document_locations"]
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
                                doc_locs = local.locations.document_context(stream_record, session, sessions=stream_sessions)["document_locations"]
                except StreamError:
                    pass
            for loc in doc_locs:
                if loc["error"] is not None:
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
        expected_key = _js_digest([endpoint.socket, scope_obj, [p_id, term_id, ws_id, tab_id]])

        if state_dir.exists():
            state_path = state_dir / f"{expected_key}.json"
            pending_path = state_dir / f"{expected_key}.json.pending"
            state_rec = None
            if state_path.exists() and not state_path.is_symlink():
                try:
                    state_rec = json.loads(state_path.read_text())
                except (OSError, ValueError):
                    state_rec = None
            if isinstance(state_rec, dict):
                rec_pane = state_rec.get("paneId")
                rec_term = state_rec.get("terminalId")
                rec_tab = state_rec.get("tabId")
                rec_nonce = state_rec.get("nonce")
                live_card = next(
                    (cp for cp in panes if isinstance(cp, dict) and cp.get("pane_id") == rec_pane and cp.get("terminal_id") == rec_term),
                    None,
                )
                if (
                    live_card is not None
                    and rec_tab == tab_id
                    and live_card.get("tab_id") == tab_id
                    and live_card.get("workspace_id") == ws_id
                    and isinstance(rec_nonce, str)
                    and rec_nonce
                ):
                    ctokens = live_card.get("tokens") if isinstance(live_card.get("tokens"), dict) else {}
                    expected_pair = {
                        "strm_agent": expected_identity,
                        "strm_nonce": rec_nonce,
                        "strm_pi": term_id,
                        "strm_card": rec_term,
                        "strm_native": native_session_id,
                    }
                    owner_ok = tokens.get("strm_role") == "agent" and all(tokens.get(k) == v for k, v in expected_pair.items())
                    card_ok = ctokens.get("strm_role") == "card" and all(ctokens.get(k) == v for k, v in expected_pair.items())
                    owner_foreign = any(k in tokens and tokens.get(k) != v for k, v in expected_pair.items())
                    card_foreign = any(k in ctokens and ctokens.get(k) != v for k, v in expected_pair.items())
                    if (not owner_ok or not card_ok) and not owner_foreign and not card_foreign:
                        if fix:
                            card_token_args = [arg for k, v in {**expected_pair, "strm_role": "card"}.items() for arg in ("--token", f"{k}={v}")]
                            owner_token_args = [arg for k, v in {**expected_pair, "strm_role": "agent"}.items() for arg in ("--token", f"{k}={v}")]
                            if not card_ok:
                                client.run(["pane", "report-metadata", rec_pane, "--source", "streams-work-cards", *card_token_args])
                                live_card["tokens"] = {**ctokens, **expected_pair, "strm_role": "card"}
                            if not owner_ok:
                                client.run(["pane", "report-metadata", p_id, "--source", "streams-work-cards", *owner_token_args])
                                tokens = {**tokens, **expected_pair, "strm_role": "agent"}
                                p["tokens"] = tokens
                            if pending_path.exists() or state_rec.get("phase") != "ready":
                                state_rec["phase"] = "ready"
                                tmp_path = state_path.with_name(f".{state_path.name}.tmp-{os.getpid()}")
                                tmp_path.write_text(json.dumps(state_rec) + "\n")
                                os.replace(tmp_path, state_path)
                                pending_path.unlink(missing_ok=True)
                            fixed.append({
                                "kind": "unstamped-work-card-pair-stamped",
                                "pane_id": p_id,
                                "card_pane_id": rec_pane,
                                "workspace_id": ws_id,
                                "tab_id": tab_id,
                                "stream_id": session.stream_id,
                                "session_id": session.id,
                                "nonce": rec_nonce,
                            })
                        else:
                            issues.append({
                                "kind": "unstamped-work-card-pair",
                                "pane_id": p_id,
                                "card_pane_id": rec_pane,
                                "workspace_id": ws_id,
                                "tab_id": tab_id,
                                "stream_id": session.stream_id,
                                "session_id": session.id,
                                "key": expected_key,
                                "message": (
                                    f"recorded live work card {rec_pane} for owner pane {p_id} is missing strm_* pair markers "
                                    f"(causes 'Caller is not the exact owned Streams card' / 'Recorded card is still live without verified ownership')"
                                ),
                            })
                            continue

            if (target_stream is not None or pane_id is not None) and pending_path.exists():
                live_by_pane = {p.get("pane_id") or p.get("id"): p for p in panes if isinstance(p, dict) and isinstance(p.get("pane_id") or p.get("id"), str)}
                _reconcile_pending_sidecar(
                    pending_path,
                    live_by_pane,
                    fix=fix,
                    fixed=fixed,
                    issues=issues,
                    extra={
                        "pane_id": p_id,
                        "workspace_id": ws_id,
                        "tab_id": tab_id,
                        "stream_id": session.stream_id,
                        "session_id": session.id,
                        "key": expected_key,
                        "message": f"pending work-card launch sidecar {pending_path} blocks automatic card open for pane {p_id}",
                    },
                )

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

    if stream is None and pane_id is None:
        if isinstance(backend, LocalCatalog):
            for sess in catalog.sessions:
                expected_path = _record_path(backend.store.root, record_key(sess))
                legacy_path = _record_path(backend.store.root, RecordKey("session", sess.id, sess.host_id))
                if legacy_path.exists() and not expected_path.exists():
                    if fix:
                        try:
                            with backend.store.transaction():
                                _store_directory(expected_path.parent.parent.parent, create=True)
                                _store_directory(expected_path.parent.parent, create=True)
                                _store_directory(expected_path.parent, create=True)
                                os.replace(legacy_path, expected_path)
                                try:
                                    legacy_path.parent.rmdir()
                                except OSError:
                                    pass
                            fixed.append({
                                "kind": "legacy-session-path-migrated",
                                "session_id": sess.id,
                                "stream_id": sess.stream_id,
                                "from": str(legacy_path.relative_to(backend.store.root)),
                                "to": str(expected_path.relative_to(backend.store.root)),
                            })
                            continue
                        except (OSError, Exception):
                            pass
                    issues.append({
                        "kind": "unpartitioned-session-record",
                        "session_id": sess.id,
                        "stream_id": sess.stream_id,
                        "path": str(legacy_path.relative_to(backend.store.root)),
                        "message": f"session {sess.id} is stored at legacy unpartitioned path {legacy_path.relative_to(backend.store.root)}; run streams check --fix to migrate to {expected_path.relative_to(backend.store.root)}",
                    })
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

    if pane_id is None:
        pre_streams = (stream,) if stream is not None else catalog.streams
        for s in pre_streams:
            if s.owner_host_id == backend.host_id and backend.authority(s.id) is None:
                issues.append({
                    "kind": "missing-stream-authority",
                    "stream_id": s.id,
                    "message": f"local owner stream {s.id!r} is missing its authority token in local.json",
                })
            s_sessions = tuple(item for item in catalog.sessions if item.stream_id == s.id)
            _check_stream_locations(s, s_sessions, local, fix=fix, issues=issues, fixed=fixed)

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
        target_stream_ids: set[str] | None = {stream.id}
    elif pane_id is not None and herdr_streams is not None:
        streams_to_check = tuple(s for s in catalog.streams if s.id in herdr_streams)
        target_stream_ids = set(herdr_streams)
        for s in streams_to_check:
            if s.owner_host_id == backend.host_id and backend.authority(s.id) is None:
                issues.append({
                    "kind": "missing-stream-authority",
                    "stream_id": s.id,
                    "message": f"local owner stream {s.id!r} is missing its authority token in local.json",
                })
    else:
        target_stream_ids = None

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
