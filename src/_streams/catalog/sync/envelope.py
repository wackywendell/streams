"""Original intent in a versioned commit message, independent of commit hashes."""

import json

from ..codec import object_fields, record_from_data, record_to_data
from ..model import ValidationError
from ..operations import Operation

PREFIX = "Streams operation v1\n\n"
BOOTSTRAP = "Streams catalog v1\n"


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValidationError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def load_json(value: str):
    try:
        return json.loads(value, object_pairs_hook=_unique,
                          parse_constant=lambda value: (_ for _ in ()).throw(ValidationError(f"invalid JSON: {value}")))
    except (ValueError, TypeError) as error:
        raise ValidationError(f"invalid JSON: {error}") from error


def encode_operation(operation: Operation, *, origin: dict[str, str] | None = None) -> str:
    from ..model import record_key
    origin = dict(origin or {})
    if any(not isinstance(k, str) or not isinstance(v, str) for k, v in origin.items()):
        raise ValidationError("origin must map strings to strings")
    data = {"version": 1, "id": operation.id, "kind": record_key(operation.after).kind,
            "before": None if operation.before is None else record_to_data(operation.before),
            "after": record_to_data(operation.after), "actor": operation.actor,
            "ownership_token": operation.ownership_token, "origin": origin}
    return PREFIX + json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"


def decode_operation(message: str) -> tuple[Operation, dict[str, str]]:
    if not message.startswith(PREFIX):
        raise ValidationError("unsupported operation commit message")
    data = object_fields(load_json(message[len(PREFIX):]),
                         {"version", "id", "kind", "before", "after", "actor", "ownership_token", "origin"})
    try:
        if type(data["version"]) is not int or data["version"] != 1:
            raise ValidationError("unsupported operation envelope version")
        before = None if data["before"] is None else record_from_data(data["kind"], data["before"])
        operation = Operation(id=data["id"], before=before, after=record_from_data(data["kind"], data["after"]),
                              actor=data["actor"], ownership_token=data["ownership_token"])
        origin = data["origin"]
        if not isinstance(origin, dict) or any(not isinstance(k, str) or not isinstance(v, str) for k, v in origin.items()):
            raise ValidationError("origin must map strings to strings")
        return operation, origin
    except KeyError as error:
        raise ValidationError(f"missing operation field: {error}") from error
