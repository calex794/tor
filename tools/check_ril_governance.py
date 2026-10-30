#!/usr/bin/env python3
"""RIL-08: fail-closed static mapping of every Axum route to its guard evidence.

This is a source-level regression alarm, NOT an authorization proof. It is
paired with real PostgreSQL/browser tests; it must never grant access.
The JSON inventory is bound to exact handler names and source locations.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = Path("docs/contracts/ril-route-guards-v1.json")
METHODS = r"get|post|put|patch|delete|head|options|trace"
POLICIES = {"public", "deny", "entry", "csrf_only", "session", "scoped", "owner", "token", "operator"}
WRITES = {"POST", "PUT", "PATCH", "DELETE"}
PROTECTED = {"session", "scoped", "owner"}
SCOPE_CALLS = {
    "list_books", "continue_reading", "free_state", "save_free",
    "purchased_state", "purchased_body_for_validation", "save_purchased",
    "own_ratings", "set_rating",
    "purchased_s3_completion_state", "complete_purchased_s3",
    "saved_place", "saved_state", "save_position", "add_book", "remove_book",
    "purchased_s3_completion_state", "complete_purchased_s3",
    "set_ril_behavior_consent", "readable_purchases", "purchased_terms",
    "record_verified_continuation", "follow", "unfollow",
}
OWNER_CALLS = {
    "list_owned_books", "create_owned_book", "owned_book",
    "owned_book_is_public", "list_owned_chapters",
    "owned_unpublished_free_chapter", "pending_free_chapter",
    "edit_unpublished_free_chapter", "release_pending_free_chapter",
    "release_book", "owned_book_description", "update_book_description",
    # Both D methods bind the verified session actor and check Layer owner.
    # The picker is advisory; add_book rechecks under the canonical Book lock.
    "addable_books", "add_book", "create_once", "offer_if_current",
    "owned_layers",
}
FUNC = re.compile(
    r"(?m)^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([a-zA-Z_]\w*)\s*\("
)
CALL = re.compile(rf"(?<!\w)(?:routing::)?({METHODS})\s*\(\s*([\w:]+)")
REGISTRATION = re.compile(r"\.route\s*\(")


def _production(source: str) -> str:
    return source.split("#[cfg(test)]", 1)[0]


def _route_calls(source: str, file: str) -> tuple[list[dict[str, str]], list[str]]:
    entries: list[dict[str, str]] = []
    issues = []
    for found in REGISTRATION.finditer(_production(source)):
        at = found.end()
        depth, quoted, escaped = 1, False, False
        i = at
        while i < len(source) and depth:
            c = source[i]
            i += 1
            if quoted:
                if escaped:
                    escaped = False
                elif c == "\\":
                    escaped = True
                elif c == '"':
                    quoted = False
            elif c == '"':
                quoted = True
            elif c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
        if depth:
            issues.append(f"{file}: unterminated route registration")
            continue
        args = re.match(r'\s*"([^"\n]+)"\s*,(.*)\Z', source[at:i - 1], re.S)
        if not args:
            issues.append(f"{file}: dynamic route registration needs explicit audit")
            continue
        template, declaration = args.groups()
        found_methods = list(CALL.finditer(declaration))
        if not found_methods:
            issues.append(f"{file}: no literal method/handler for {template}")
        for match in found_methods:
            entries.append({
                "source": file,
                "template": template,
                "method": match.group(1).upper(),
                "handler": match.group(2),
            })
    return entries, issues


def _strip_comments(code: str) -> str:
    code = re.sub(r"/\*[\s\S]*?\*/", "", code)
    return "\n".join(
        "" if line.lstrip().startswith("//") else line.split(" //", 1)[0]
        for line in code.splitlines()
    )


def _functions(code: str) -> dict[str, str]:
    source = _production(code)
    spans = list(FUNC.finditer(source))
    return {
        item.group(1): _strip_comments(source[item.start():spans[i + 1].start()
                         if i + 1 < len(spans) else len(source)])
        for i, item in enumerate(spans)
    }


def _handler_file(root: Path, entry: dict[str, str]) -> Path:
    source = root / entry["source"]
    handler = entry["handler"]
    if "::" not in handler:
        return source
    module, _ = handler.rsplit("::", 1)
    # All current registrations use one same-crate module prefix.
    if "::" in module:
        return root / "__unsupported_nested_handler__"
    if source.name in {"lib.rs", "mod.rs"}:
        return source.parent / f"{module}.rs"
    return source.with_suffix("") / f"{module}.rs"


def _with_known_helpers(body: str, functions: dict[str, str]) -> str:
    # Explicit delegation, not arbitrary source file scanning: proof disappears
    # when the handler stops invoking its trusted shared helper.
    # Private ranking handlers delegate trusted session and CSRF to explicit
    # local helpers; never trust a descriptive policy row alone.
    for helper in ("write", "csrf", "change_choice", "resume", "actor", "csrf_valid", "owner"):
        if re.search(rf"(?<!\w){helper}\s*\(", body) and helper in functions:
            body += "\n" + functions[helper]
    return body


def _contains_scoped_call(body: str, names: set[str]) -> bool:
    for name in names:
        if re.search(rf"\b{name}\s*\(\s*(?:actor|user_id)\b", body):
            return True
    return False


def _verify(entry: dict[str, str], functions: dict[str, str], limiter: str) -> list[str]:
    label = f'{entry["method"]} {entry["template"]} [{entry["handler"]}]'
    errors = []
    handler = entry["handler"].rsplit("::", 1)[-1]
    body = functions.get(handler)
    if body is None:
        return [f"{label}: handler function missing or renamed"]
    evidence = _with_known_helpers(body, functions)
    policy = entry["policy"]
    if policy not in POLICIES:
        return [f"{label}: invalid policy {policy!r}"]
    if entry["method"] in WRITES and policy == "public":
        errors.append(f"{label}: state-changing route cannot be unguarded public")
    if (entry["template"].startswith(("/me", "/read/", "/api/v1/me"))
            and policy in {"public", "entry", "csrf_only"}
            and entry["template"] != "/logout"):
        errors.append(f"{label}: protected route downgraded to public/entry")
    if entry["template"].startswith(("/admin", "/internal")) and policy != "deny":
        errors.append(f"{label}: admin/internal route requires separately reviewed role policy")
    if entry["template"].startswith("/ops/") and policy not in {"deny", "operator"}:
        errors.append(f"{label}: operator route requires independently reviewed credential")

    if policy in PROTECTED:
        if not re.search(r"\b(?:session_user|session_actor|active_session_user)\s*\(", evidence):
            errors.append(f"{label}: missing verified PG session guard")
        if not ("StatusCode::UNAUTHORIZED" in evidence or "Redirect::to(\"/login\")" in evidence):
            errors.append(f"{label}: session failure does not explicitly deny/redirect")
    if policy in {"scoped", "owner"}:
        proof = SCOPE_CALLS if policy == "scoped" else OWNER_CALLS
        # Shared write helpers contain both calls; each route must prove its own.
        if policy == "scoped" and entry["template"] == "/me/books/{id}/follow":
            proof = {"follow"}
        elif policy == "scoped" and entry["template"] == "/me/books/{id}/unfollow":
            proof = {"unfollow"}
        if not _contains_scoped_call(evidence, proof):
            errors.append(f"{label}: missing actor-bound {policy} repository call")
    if entry["method"] in WRITES:
        if policy in {"deny", "public"}:
            pass
        elif not (
            re.search(r"\bsame_origin\s*\(", evidence)
            and re.search(r"\bvalid_csrf\s*\(", evidence)
            and "StatusCode::FORBIDDEN" in evidence
        ):
            errors.append(f"{label}: missing Origin+CSRF rejection")
    if policy == "entry":
        if entry["method"] != "POST" or entry["template"] not in {"/login", "/register"}:
            errors.append(f"{label}: entry exception only applies to login/register POST")
        # Identity AND peer-address throttles must remain independent.
        if ("login_limiter::limit(" not in evidence
                or len(re.findall(r"\bpermit_auth_attempt\s*\(", limiter)) < 2):
            errors.append(f"{label}: missing distributed authentication rate limit")
    if policy == "csrf_only" and (entry["method"], entry["template"]) != ("POST", "/logout"):
        errors.append(f"{label}: CSRF-only exception only allowed for logout")
    if policy == "token":
        # The exception is reserved for this one protected service route.
        # Inspect actual handler and helper wiring, not a descriptive comment.
        endpoint = functions.get("endpoint", "")
        authorized = functions.get("authorized", "")
        if (entry["method"], entry["template"], entry["handler"]) != (
            "GET", "/metrics", "observability::metrics_endpoint"
        ):
            errors.append(f"{label}: token exception is not allowed here")
        if not (
            "TOR_METRICS_TOKEN" in body
            and re.search(r"\bendpoint\s*\(", body)
            and re.search(r"\bauthorized\s*\(", endpoint)
            and "StatusCode::NOT_FOUND" in endpoint
            and re.search(r"\bequal_secret\s*\(", authorized)
        ):
            errors.append(f"{label}: missing fail-closed token guard")

    if policy == "operator":
        # The only reviewed exception is the isolated read-only diagnostic page.
        # Verified independently from reader/author sessions and bearer metrics.
        operator = functions.get("authorized", "")
        gate = functions.get("respond", "")
        rejected = functions.get("denied", "")
        key = (entry["method"], entry["template"], entry["handler"])
        if key == ("GET", "/ops/traces", "operator_traces::page"):
            if not (
                "TOR_OBS_OPERATOR_PASSWORD" in body
                and "respond(" in body
                and "if !authorized(&headers, secret)" in gate
                and "return denied()" in gate
                and "StatusCode::UNAUTHORIZED" in rejected
                and "get_all(header::AUTHORIZATION)" in operator
                and "Basic " in operator
                and "equal_secret(" in operator
            ):
                errors.append(f"{label}: trace operator password boundary missing")
        elif key == ("GET", "/ops/overview", "operator_overview::page"):
            if not (
                "TOR_OBS_OPERATOR_PASSWORD" in body
                and "respond(headers, secret.as_deref(), state.postgres).await" in body
                and "if !operator_traces::authorized(&headers, secret)" in gate
                and "return operator_traces::denied()" in gate
                and "operator_traces::secure(render(&summary))" in gate
            ):
                errors.append(f"{label}: overview must re-use exact operator gate before database access")
        else:
            errors.append(f"{label}: operator privilege reserved for two reviewed read-only pages")

    if policy == "deny" and "StatusCode::NOT_FOUND" not in evidence:
        errors.append(f"{label}: disabled endpoint no longer denies")
    return errors


def check(root: Path = ROOT) -> list[str]:
    try:
        data = json.loads((root / INVENTORY).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return [f"RIL security inventory unavailable: {exc}"]
    if data.get("schema") != "tor_ril08_route_policies_v1":
        return ["wrong RIL-08 inventory version"]
    inventory = data.get("routes")
    if not isinstance(inventory, list):
        return ["RIL-08 inventory routes must be a list"]
    errors: list[str] = []
    actual: list[dict[str, str]] = []
    files = list((root / "crates").glob("*/src/**/*.rs"))
    for path in sorted(files):
        source = path.relative_to(root).as_posix()
        text = path.read_text(encoding="utf-8")
        discovered, failures = _route_calls(text, source)
        actual.extend(discovered)
        errors.extend(failures)
    def key(record: dict[str, str]) -> tuple[str, str, str, str]:
        return tuple(record.get(field, "") for field in ("method", "template", "source", "handler"))
    old = [key(r) for r in inventory]
    found = [key(r) for r in actual]
    for record in set(found) - set(old):
        errors.append(f"route without reviewed security policy: {record}")
    for record in set(old) - set(found):
        errors.append(f"stale security policy/handler mismatch: {record}")
    if len(old) != len(set(old)) or len(found) != len(set(found)):
        errors.append("duplicate route-policy binding or registration")
    if not actual:
        errors.append("no registered routes discovered")
    try:
        limiter = _strip_comments(
            (root / "crates/web/src/auth/login_limiter.rs").read_text(encoding="utf-8")
        )
    except OSError:
        limiter = ""
    source_cache: dict[Path, dict[str, str]] = {}
    for entry in inventory:
        if not isinstance(entry, dict):
            errors.append("route policy must be an object")
            continue
        path = _handler_file(root, entry)
        if path not in source_cache:
            try:
                source_cache[path] = _functions(path.read_text(encoding="utf-8"))
            except OSError:
                source_cache[path] = {}
        errors.extend(_verify(entry, source_cache[path], limiter))
    return errors


if __name__ == "__main__":
    findings = check()
    for finding in findings:
        print("FAIL:", finding, file=sys.stderr)
    if findings:
        raise SystemExit(1)
    print("RIL-08: all registered route+method+handler bindings have reviewed"
          " session/scope/owner/CSRF/rate guard evidence")
