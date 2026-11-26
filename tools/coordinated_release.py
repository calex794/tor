#!/usr/bin/env python3
"""DEPLOY-05: durable coordination of an already adopted edge and web service.

Library for the trusted VPS release executor. No standalone activation CLI.
The caller holds the existing overall release lock and the edge-store lock,
and supplies the existing trusted, synchronous PG/application activation.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
import json
import os
from pathlib import Path
import re
import stat
import tempfile

import managed_edge_release as edge
import managed_systemd_release as unit
import release_bundle_v2 as bundle

JOURNAL_DIR = Path("/var/lib/tor-release/transactions")
PHASES = ("edge", "unit", "app", "verify")
STATES = ("active", "rollback_pending", "committed", "rolled_back", "outage_stopped")
TERMINAL = ("committed", "rolled_back", "outage_stopped")
MAX_JOURNAL = 65536


class ReleaseError(ValueError):
    pass


@dataclass(frozen=True)
class Record:
    candidate: str
    previous_app: str
    previous_edge: str
    receipt: unit.Receipt
    phase: str = "edge"
    state: str = "active"
    mode: str = "normal"

    def encode(self):
        data = {
            "version": 1,
            "candidate": self.candidate,
            "previous_app": self.previous_app,
            "previous_edge": self.previous_edge,
            "phase": self.phase,
            "state": self.state,
            "unit_previous": (None if self.receipt.previous is None
                              else self.receipt.previous.decode("ascii")),
            "unit_candidate": self.receipt.candidate.decode("ascii"),
            "unit_baseline": self.receipt.baseline,
        }
        if self.mode == "outage":
            data.update(version=2, mode="outage")
        elif self.mode != "normal":
            raise ReleaseError("unsupported release mode")
        return (json.dumps(data, sort_keys=True, separators=(",", ":")) + "\n").encode()

    @classmethod
    def decode(cls, raw):
        def unique(pairs):
            result = dict(pairs)
            if len(result) != len(pairs):
                raise ReleaseError("duplicate journal field")
            return result

        try:
            data = json.loads(raw, object_pairs_hook=unique)
            fields = {
                "version", "candidate", "previous_app", "previous_edge",
                "phase", "state", "unit_previous", "unit_candidate", "unit_baseline",
            }
            if type(data) is not dict or type(data.get("version")) is not int:
                raise ReleaseError("unsupported journal format")
            if data["version"] == 1 and set(data) == fields:
                mode = "normal"
            elif (data["version"] == 2 and set(data) == fields | {"mode"}
                  and data["mode"] == "outage"):
                mode = "outage"
            else:
                raise ReleaseError("unsupported journal format")
            for field in ("candidate", "previous_app", "previous_edge"):
                if type(data[field]) is not str:
                    raise ReleaseError("invalid journal release identity")
                bundle.validate_sha(data[field])
            if data["phase"] not in PHASES or data["state"] not in STATES:
                raise ReleaseError("invalid journal phase")
            if data["state"] == "committed" and data["phase"] != "verify":
                raise ReleaseError("invalid committed journal")
            if ((mode == "normal" and data["state"] == "outage_stopped")
                    or (mode == "outage" and (data["state"] == "rolled_back"
                                              or data["candidate"] == data["previous_app"]))):
                raise ReleaseError("invalid release mode or terminal state")
            candidate = data["unit_candidate"].encode("ascii")
            previous = data["unit_previous"]
            previous = None if previous is None else previous.encode("ascii")
            unit.policy(candidate)
            if previous is not None:
                unit.policy(previous)
            baseline = data["unit_baseline"]
            if type(baseline) is not dict or set(baseline) != set(unit.PROPERTIES):
                raise ReleaseError("invalid journal unit receipt")
            if any(type(value) is not str or len(value) > 4096 or
                   any(ord(c) < 32 for c in value) for value in baseline.values()):
                raise ReleaseError("invalid journal property")
            unit.check_identity(baseline)
            return cls(data["candidate"], data["previous_app"], data["previous_edge"],
                       unit.Receipt(previous, candidate, baseline),
                       data["phase"], data["state"], mode)
        except (ValueError, TypeError, AttributeError, UnicodeError) as exc:
            raise ReleaseError("journal invalid; operator recovery required") from exc


class Journal:
    """Single durable transaction. Unfinished releases block all new releases."""
    def __init__(self, directory: Path = JOURNAL_DIR):
        if not directory.is_absolute() or directory.resolve() != directory:
            raise ReleaseError("journal directory must be canonical")
        self.directory = directory
        self.path = directory / "release.json"

    def ensure(self):
        edge.trusted(self.directory.parent, directory=True)
        try:
            self.directory.mkdir(mode=0o700)
        except FileExistsError:
            pass
        else:
            self.directory.chmod(0o700)
            edge.sync_dir(self.directory.parent)
        if stat.S_IMODE(edge.trusted(self.directory, directory=True).st_mode) != 0o700:
            raise ReleaseError("journal must be private")

    def load(self):
        self.ensure()
        if not os.path.lexists(self.path):
            return None
        info = edge.trusted(self.path)
        if stat.S_IMODE(info.st_mode) != 0o600 or info.st_nlink != 1:
            raise ReleaseError("unsafe journal permissions")
        return Record.decode(edge.read_small(self.path, MAX_JOURNAL))

    def save(self, record: Record):
        self.ensure()
        raw = record.encode()
        if len(raw) > MAX_JOURNAL:
            raise ReleaseError("journal too large")
        Record.decode(raw)
        fd, name = tempfile.mkstemp(prefix=".release-", dir=self.directory)
        temporary = Path(name)
        try:
            with os.fdopen(fd, "wb") as stream:
                os.fchmod(stream.fileno(), 0o600)
                stream.write(raw)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, self.path)
            edge.sync_dir(self.directory)
        finally:
            temporary.unlink(missing_ok=True)

    def begin(self, record):
        existing = self.load()
        if existing is not None and existing.state not in TERMINAL:
            raise ReleaseError("unfinished release; recover before another update")
        if record.phase != "edge" or record.state != "active":
            raise ReleaseError("invalid initial journal")
        self.save(record)


class Coordinator:
    """App boundary: current(), preflight(sha), activate(sha), verify(sha),
    restore(previous, candidate). The trusted executor supplies that boundary
    and holds the release lock across its synchronous activation (including
    backup and additive SQLx migrations). Never undo database migrations.
    """
    def __init__(self, journal, store, nginx, policy, systemd, app):
        self.journal = journal
        self.store = store
        self.nginx = nginx
        self.policy = policy
        self.systemd = systemd
        self.app = app

    def verify(self, record, *, previous=False):
        app_sha = record.previous_app if previous else record.candidate
        edge_sha = record.previous_edge if previous else record.candidate
        if self.store.active_sha() != edge_sha:
            raise ReleaseError("unexpected edge release")
        self.nginx.test()
        self.nginx.probe(edge_sha, self.store.releases / edge_sha)
        if previous:
            if (unit.read_current(self.policy.directory) != record.receipt.previous
                    or self.systemd.snapshot() != record.receipt.baseline):
                raise ReleaseError("previous unit policy not restored")
        else:
            if unit.read_current(self.policy.directory) != record.receipt.candidate:
                raise ReleaseError("candidate unit file differs")
            unit.check_effective(record.receipt.baseline, self.systemd.snapshot(),
                                 unit.policy(record.receipt.candidate))
        if previous and record.mode == "outage":
            self.app.verify_outage(app_sha)
        else:
            self.app.verify(app_sha)

    def recover(self):
        """Explicit resume after failure/crash; never silently start a new update."""
        record = self.journal.load()
        if record is None or record.state in TERMINAL:
            return record
        record = replace(record, state="rollback_pending")
        self.journal.save(record)
        failures = []
        outage = record.mode == "outage"
        if outage:
            # Stop any failed candidate, restore the pointer without starting
            # the previously unavailable binary. Independent edge/unit repair
            # still runs if this operation fails.
            try:
                self.app.restore_outage(record.previous_app, record.candidate)
            except Exception:
                failures.append("app")
        # Restore old unit policy before restarting the old binary. Attempt
        # independent edge recovery even if another component cannot recover.
        try:
            self.store.recover(record.previous_edge, record.candidate, self.nginx)
        except Exception:
            failures.append("edge")
        unit_ok = True
        try:
            if PHASES.index(record.phase) >= PHASES.index("unit"):
                self.policy.restore(record.receipt, self.systemd)
        except Exception:
            failures.append("systemd")
            unit_ok = False
        if not outage:
            try:
                if PHASES.index(record.phase) >= PHASES.index("app"):
                    if not unit_ok:
                        raise ReleaseError("unit recovery required before app restart")
                    self.app.restore(record.previous_app, record.candidate)
                else:
                    self.app.verify(record.previous_app)
            except Exception:
                failures.append("app")
        if failures:
            raise ReleaseError("TOR_ROLLBACK_UNVERIFIED: " + ",".join(failures))
        try:
            self.verify(record, previous=True)
        except Exception as exc:
            raise ReleaseError("TOR_ROLLBACK_UNVERIFIED: final verification") from exc
        record = replace(record, state="outage_stopped" if outage else "rolled_back")
        self.journal.save(record)
        return record

    def run(self, source: Path, sha: str, signers: Path):
        return self._run(source, sha, signers)

    def run_outage(self, source: Path, sha: str, signers: Path, *, consume_approval):
        """Internal library boundary only; the future root executor owns approval.

        Requires a stopped, identified previous app and a distinct candidate.
        The callback validates/consumes exact approval under the release lock.
        Never expose this route through the ordinary sudo wrapper.
        """
        if not callable(consume_approval):
            raise ReleaseError("explicit outage approval required")
        return self._run(source, sha, signers, outage_approval=consume_approval)

    def _run(self, source: Path, sha: str, signers: Path, *, outage_approval=None):
        bundle.validate_sha(sha)
        outage = outage_approval is not None
        existing = self.journal.load()
        if existing is not None and existing.state not in TERMINAL:
            raise ReleaseError("unfinished release; recover before another update")
        previous_app = self.app.current()
        previous_edge = self.store.active_sha()
        bundle.validate_sha(previous_app)
        if outage and sha == previous_app:
            raise ReleaseError("outage requires a distinct forward release")
        self.app.preflight(sha)
        if outage:
            self.app.verify_outage(previous_app)
        else:
            self.app.verify(previous_app)
        self.nginx.test()
        self.nginx.probe(previous_edge, self.store.releases / previous_edge)
        self.store.prepare(source, sha, signers)
        receipt = self.policy.plan(source, sha, signers, self.systemd)
        record = Record(sha, previous_app, previous_edge, receipt,
                        mode="outage" if outage else "normal")
        if outage:
            # Preparation is inert. Recheck and consume one-shot approval
            # before writing active intent or mutating live components.
            self.app.verify_outage(previous_app)
            outage_approval(previous_app, sha)
        self.journal.begin(record)  # durable intent BEFORE edge link changes
        try:
            self.store.activate(sha, self.nginx)
            record = replace(record, phase="unit")
            self.journal.save(record)
            self.policy.apply_receipt(receipt, self.systemd)
            record = replace(record, phase="app")
            self.journal.save(record)
            if outage:
                self.app.activate_outage(sha)
            else:
                self.app.activate(sha)
            record = replace(record, phase="verify")
            self.journal.save(record)
            self.verify(record)
        except (Exception, KeyboardInterrupt) as failure:
            self.recover()
            if outage:
                raise ReleaseError("TOR_OUTAGE_RESTORED_STOPPED: candidate refused") from failure
            raise ReleaseError("TOR_ROLLBACK_VERIFIED: candidate refused") from failure
        # If the final durable write fails, recover() handles the last durable
        # state. Do not claim commit/rollback until its journal has been synced.
        record = replace(record, state="committed")
        self.journal.save(record)
        return record
