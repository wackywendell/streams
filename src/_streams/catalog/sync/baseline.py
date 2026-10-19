"""A validated snapshot root with retained creation evidence, not invented history."""

from dataclasses import dataclass
import json
from types import MappingProxyType
from typing import Mapping

from ..codec import object_fields
from ..model import Catalog, Stream, ValidationError, text
from ..operations import Authority
from ..yaml_store import YamlStore
from .envelope import decode_fingerprints, decode_operation, encode_operation, load_json, operation_digest

BASELINE = "Streams catalog baseline v1\n"


@dataclass(frozen=True)
class Baseline:
    host: str
    creations: Mapping[str, str]
    fingerprints: Mapping[str, str]

    def __post_init__(self) -> None:
        text(self.host, "baseline host")
        if not isinstance(self.creations, Mapping):
            raise ValidationError("baseline creations must be a mapping")
        creations = dict(self.creations)
        for id, envelope in creations.items():
            text(id, "stream ID")
            text(envelope, "creation envelope")
        if not isinstance(self.fingerprints, Mapping):
            raise ValidationError("baseline fingerprints must be a mapping")
        fingerprints = decode_fingerprints(dict(self.fingerprints))
        object.__setattr__(self, "creations", MappingProxyType(creations))
        object.__setattr__(self, "fingerprints", MappingProxyType(fingerprints))


def encode_baseline(baseline: Baseline) -> str:
    data = {"version": 1, "host": baseline.host, "creations": dict(baseline.creations),
            "fingerprints": dict(baseline.fingerprints)}
    return BASELINE + json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"


def decode_baseline(message: str, files: Mapping[str, bytes]) -> tuple[Baseline, Catalog, Mapping[str, Authority]]:
    if not message.startswith(BASELINE):
        raise ValidationError("unsupported catalog baseline message")
    data = object_fields(load_json(message[len(BASELINE):]), {"version", "host", "creations", "fingerprints"})
    try:
        if type(data["version"]) is not int or data["version"] != 1:
            raise ValidationError("unsupported catalog baseline version")
        fingerprints = decode_fingerprints(data["fingerprints"])
        baseline = Baseline(data["host"], data["creations"], fingerprints)
    except KeyError as error:
        raise ValidationError(f"missing baseline field: {error}") from error
    catalog = YamlStore.decode_snapshot(dict(files)).catalog
    if set(baseline.creations) != {stream.id for stream in catalog.streams}:
        raise ValidationError("baseline creations must exactly match snapshot streams")
    if any(stream.owner_host_id != baseline.host for stream in catalog.streams) or any(
            record.host_id != baseline.host for record in catalog.sessions) or any(
            record.id != baseline.host for record in catalog.hosts):
        raise ValidationError("baseline snapshot producers must belong to its host")
    authorities = {}
    creation_ids = set()
    for id, envelope in baseline.creations.items():
        operation, origin = decode_operation(envelope)
        if encode_operation(operation, origin=origin) != envelope:
            raise ValidationError("baseline creation envelope must be canonical")
        if operation.before is not None or not isinstance(operation.after, Stream) or operation.after.id != id:
            raise ValidationError("baseline creation must create its indexed stream")
        if operation.after.owner_host_id != baseline.host:
            raise ValidationError("baseline creation owner must match its host")
        if operation.id in creation_ids:
            raise ValidationError("duplicate baseline creation operation ID")
        creation_ids.add(operation.id)
        if baseline.fingerprints.get(operation.id) != operation_digest(operation):
            raise ValidationError("baseline creation fingerprint does not match original intent")
        authorities[id] = Authority(baseline.host, operation.id)
    return baseline, catalog, MappingProxyType(authorities)
