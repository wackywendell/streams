"""Event eligibility only: no clock, file format, agent or network dependencies."""

from dataclasses import dataclass
import math


@dataclass(frozen=True)
class Timing:
    last_sync: float | None = None
    last_push: float | None = None
    retry_after: float = 0

    def __post_init__(self):
        for value in (self.last_sync, self.last_push, self.retry_after):
            if value is not None and (type(value) not in (int, float) or not math.isfinite(value)):
                raise ValueError("sync timestamps must be finite numbers")


@dataclass(frozen=True)
class Policy:
    stream_seconds: float = 300
    session_seconds: float = 3600
    cooldown_seconds: float = 60
    timeout_seconds: float = 30
    attempts: int = 3

    def __post_init__(self):
        for value in (self.stream_seconds, self.session_seconds, self.cooldown_seconds, self.timeout_seconds):
            if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
                raise ValueError("sync intervals must be finite nonnegative numbers")
        if self.timeout_seconds == 0 or type(self.attempts) is not int or not 1 <= self.attempts <= 3:
            raise ValueError("sync needs a positive timeout and one to three attempts")

    def read_due(self, *, fetched_at: float | None, now: float, retry_after: float = 0, force: bool = False) -> bool:
        Timing(last_sync=fetched_at, retry_after=retry_after)
        if force:
            return True
        if now < retry_after:
            return False
        return fetched_at is None or now - fetched_at >= self.session_seconds

    def due(self, event: str, timing: Timing, *, now: float, pending_stream: bool = False) -> bool:
        if event in ("shutdown", "explicit", "force"):
            return True
        if event not in ("session", "host", "stream", "start", "attach"):
            raise ValueError(f"unsupported sync event: {event}")
        if now < timing.retry_after:
            return False
        short = pending_stream or event in ("stream", "start", "attach")
        last = timing.last_sync if short else timing.last_push
        interval = self.stream_seconds if short else self.session_seconds
        return last is None or now - last >= interval
