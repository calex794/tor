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
POLICIES = {"recovery", "public", "deny", "entry", "csrf_only", "session", "scoped", "owner", "token", "operator", "administrator", "admin_reader", "admin_mail", "admin_refund", "admin_refund_approve", "support_requester", "support_admin"}
WRITES = {"POST", "PUT", "PATCH", "DELETE"}
PROTECTED = {"session", "scoped", "owner", "administrator", "admin_reader", "admin_mail", "admin_refund", "admin_refund_approve", "support_admin"}
ADMIN_MAIL_HANDLERS = {
    ("GET", "/admin/mail", "admin_mail::index"): "users",
    ("GET", "/admin/mail/users/{user_id}", "admin_mail::user"): "mailbox",
    ("GET", "/admin/mail/{id}", "admin_mail::conversation"): "messages",
    ("GET", "/admin/mail/groups", "admin_mail::groups"): "groups",
    ("GET", "/admin/mail/groups/{id}", "admin_mail::group"): "group",
    ("GET", "/admin/mail/book-rooms", "admin_mail::book_rooms"): "book_rooms",
    ("GET", "/admin/mail/book-rooms/{id}", "admin_mail::book_room"): "book_room",
}
SUPPORT_ADMIN_HANDLERS = {
    ("GET", "/admin/support", "index"),
    ("GET", "/admin/support/{id}", "page"),
    ("GET", "/admin/support/{id}/messages", "messages"),
    ("POST", "/admin/support/{id}/messages", "submit"),
    ("POST", "/admin/support/{id}/read", "mark_read"),
    ("POST", "/admin/support/{id}/close", "close"),
    ("POST", "/admin/support/{id}/reopen", "reopen"),
}
SUPPORT_REQUESTER_HANDLERS = {
    ("GET", "/support", "page"),
    ("GET", "/support/widget", "widget"),
    ("GET", "/support/messages", "messages"),
    ("POST", "/support/messages", "submit"),
    ("GET", "/api/v1/support/tickets", "widget"),
    ("POST", "/api/v1/support/tickets", "json_submit"),
}
SCOPE_CALLS = {
    "consent", "set_consent", "set_feedback", "hidden_books", "impressions", "activate",
    "activate_organic",
    "list_books", "continue_reading", "free_state", "save_free",
    "purchased_state", "purchased_body_for_validation", "save_purchased",
    "own_ratings", "set_rating", "visible_free_summary", "thank_visible_free",
    "visible_summary", "set_like",
    "purchased_s3_completion_state", "complete_purchased_s3",
    "saved_place", "saved_state", "save_position", "add_book", "remove_book",
    "purchased_s3_completion_state", "complete_purchased_s3",
    "set_ril_behavior_consent", "readable_purchases",
    "record_verified_continuation", "follow", "unfollow", "set_read_state",
    "delivery_preference", "set_delivery_preference",
    "active_role", "create_free", "create_paid",
    "mailbox", "messages", "send", "send_html", "send_to_peer", "start",
    "send_unkeyed", "set_block", "request", "remove", "request_once",
}
OWNER_CALLS = {
    "recommendation_settings", "update_recommendation_settings",
    "list_owned_books", "create_owned_book", "owned_book",
    "owned_book_is_public", "list_owned_chapters",
    "owned_unpublished_free_chapter", "pending_free_chapter",
    "edit_unpublished_free_chapter", "release_pending_free_chapter",
    "schedule_pending_free_chapter", "cancel_scheduled_free_chapter",
    "release_book", "owned_book_description", "update_book_description",
    "owned_book_source_links", "update_book_source_links",
    "owned_project_status", "update_project_status",
    "owned_book_titles", "update_book_titles",
    "status_for_owner", "latest_active_for_owner", "cancel_source_for_owner",
    "owned_layers", "addable_books", "current_books", "owner_offer_state",
    "add_book", "remove_book_if_current", "create_once",
    "update_metadata_if_current", "offer_if_current",
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
    if source == root / "crates/web/src/auth/settings_routes.rs":
        # This router explicitly imports sibling account handlers with super::.
        allowed = {"profile_http", "notification_settings", "password_reset", "password_change", "session_security"}
        if module not in allowed:
            return root / "__unsupported_account_handler__"
        return source.parent / f"{module}.rs"
    if source == root / "crates/web/src/auth/layer_storefront.rs":
        return (source.parent / "layer_checkout_api.rs"
                if module == "layer_checkout_api"
                else root / "__unsupported_layer_handler__")
    if source == root / "crates/web/src/auth/admin_mail_routes.rs":
        return (source.parent / "admin_mail.rs"
                if module == "admin_mail"
                else root / "__unsupported_admin_mail_handler__")
    if source == root / "crates/web/src/auth/referral_routes.rs":
        # Only these three sibling handlers may be bound through the F-09 router.
        allowed = {"referral_alias", "referral_capture", "referral_dashboard"}
        if module not in allowed:
            return root / "__unsupported_referral_handler__"
        return source.parent / f"{module}.rs"
    if source.name in {"lib.rs", "mod.rs"}:
        return source.parent / f"{module}.rs"
    return source.with_suffix("") / f"{module}.rs"


def _with_known_helpers(body: str, functions: dict[str, str]) -> str:
    # Explicit delegation, not arbitrary source file scanning: proof disappears
    # when the handler stops invoking its trusted shared helper.
    # Private ranking handlers delegate trusted session and CSRF to explicit
    # local helpers; never trust a descriptive policy row alone.
    for helper in ("write", "csrf", "change_choice", "resume", "actor", "csrf_valid", "owner", "load", "administrator"):
        if re.search(rf"(?<!\w){helper}\s*\(", body) and helper in functions:
            body += "\n" + functions[helper]
    return body


def _contains_scoped_call(body: str, names: set[str]) -> bool:
    for name in names:
        if re.search(rf"\b{name}\s*\(\s*(?:actor_id|actor|user_id)\b", body):
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
    admin_key = (entry["method"], entry["template"], entry["handler"])
    audited_admin = (policy == "admin_mail"
                     and entry["source"] == "crates/web/src/auth/admin_mail_routes.rs"
                     and admin_key in ADMIN_MAIL_HANDLERS)
    audited_support = (
        policy == "support_admin"
        and entry["source"] == "crates/web/src/auth/support_admin.rs"
        and admin_key in SUPPORT_ADMIN_HANDLERS
    )
    audited_refund = (policy == "admin_refund"
                      and entry["source"] == "crates/web/src/auth/admin_refund_review.rs"
                      and admin_key == ("GET", "/admin/finance/refunds", "page"))
    approved_refund = (
        policy == "admin_refund_approve"
        and entry["source"] == "crates/web/src/auth/admin_refund_review.rs"
        and admin_key == ("POST", "/admin/finance/refunds/approve", "approval::submit")
    )
    if entry["template"].startswith(("/admin", "/internal")) and policy != "deny" and not (audited_admin or audited_support or audited_refund or approved_refund):
        errors.append(f"{label}: admin/internal route requires separately reviewed role policy")
    if policy == "admin_refund" and not (
            audited_refund
            and "PgRefundAdminReview::new" in evidence
            and ".recent(&digest, query.before)" in evidence
            and "session_token_digest" in evidence
            and "session_user(&state, &headers)" in evidence):
        errors.append(f"{label}: S036 administrator refund view must bind active session and audited read")
    if policy == "admin_refund_approve":
        required = (
            "!enabled(&state)",
            "session_user(&state, &headers)",
            "state.cookies.same_origin(&headers)",
            "state.cookies.valid_csrf(&headers",
            "permit_auth_attempt(&attempt_key, 6, 600)",
            "verify_password_for_step_up(&password, &password_hash)",
            "approval_step_up_payload_digest(actor, request, revision, reason)",
            ".issue_administrator_refund_step_up(",
        )
        approval_call = re.search(
            r"\.approve_once\s*\(\s*actor,\s*&session_digest,\s*&proof_token,\s*request,\s*revision,\s*reason,?\s*\)",
            evidence,
        )
        if not approved_refund or not all(marker in evidence for marker in required) or not approval_call:
            errors.append(f"{label}: S036 approval must retain live administrator session, fresh password, one-use proof, CSRF, opt-in and transactional approval")
    if policy == "admin_mail":
        method = ADMIN_MAIL_HANDLERS.get(admin_key) if audited_admin else None
        if not method:
            errors.append(f"{label}: unreviewed admin mail action")
        elif not (
            re.search(r"\bactor\s*\(\s*&state\s*,\s*&headers\s*\)", body)
            and re.search(rf"\.{method}\s*\(\s*&digest\b", body)
            and "PgAdminMail::new" in evidence
            and "session_token_digest" in evidence
        ):
            errors.append(f"{label}: missing session-bound audited admin mail repository call")
    if policy == "support_admin":
        if not audited_support:
            errors.append(f"{label}: unreviewed support administrator route")
        if not ("session_actor(" in evidence
                and "if !actor.administrator" in evidence
                and "StatusCode::FORBIDDEN" in evidence):
            errors.append(f"{label}: missing live support administrator role guard")
        if not _contains_scoped_call(evidence, {"admin_chats", "admin_messages", "admin_send", "admin_mark_read", "admin_set_closed"}):
            errors.append(f"{label}: missing actor-bound support administrator operation")
    if policy == "support_requester":
        allowed = (entry["source"] == "crates/web/src/auth/support.rs"
                   and admin_key in SUPPORT_REQUESTER_HANDLERS)
        identity = functions.get("__support_identity", "")
        if not (allowed
                and "support_identity::resolve(&state, &headers)" in evidence
                and re.search(r"\b(?:messages|send|json_send)\s*\(\s*&who", evidence)
                and "session_user(state, headers)" in identity
                and "valid_token(t)" in identity
                and '"tor-support:{token}"' in identity):
            errors.append(f"{label}: missing support requester capability boundary")
    if entry["template"].startswith("/ops/") and policy not in {"deny", "operator"}:
        errors.append(f"{label}: operator route requires independently reviewed credential")

    if policy in PROTECTED:
        if not re.search(r"\b(?:session_user|session_actor|active_session_user)\s*\(", evidence):
            errors.append(f"{label}: missing verified PG session guard")
        # Only the typed private reader may redirect to a specific local chapter.
        # Never allow an arbitrary dynamic redirect to satisfy a session guard.
        fixed_reader_return = (
            entry["template"] == "/read/{id}"
            and entry["handler"] == "private_reader::page"
            and "Path(id): Path<u64>" in evidence
            and "Ok(None) =>" in evidence
            and 'format!("/login?next=/read/{id}")' in evidence
            and "return private_response(Redirect::to(&destination), &[])" in evidence
        )
        if not (
            "StatusCode::UNAUTHORIZED" in evidence
            or 'Redirect::to("/login")' in evidence
            or fixed_reader_return
        ):
            errors.append(f"{label}: session failure does not explicitly deny/redirect")
    if policy in {"scoped", "owner"}:
        proof = SCOPE_CALLS if policy == "scoped" else OWNER_CALLS
        # Shared write helpers contain both calls; each route must prove its own.
        if policy == "scoped" and entry["template"] == "/me/books/{id}/follow":
            proof = {"follow"}
        elif policy == "scoped" and entry["template"] == "/me/books/{id}/unfollow":
            proof = {"unfollow"}
        mail_calls = {
            "mail::index": {"mailbox"}, "mail::start": {"start"},
            "mail::conversation": {"messages"}, "mail::poll": {"messages"}, "mail::send": {"send"},
            "mail::block": {"set_block"},
            "mail_api::get": {"messages", "mailbox"}, "mail_api::post": {"send_to_peer"},
            "friendship::request": {"request"}, "friendship::remove": {"remove"},
            "friendship::block": {"set_block"},
            "group_mail::index": {"list"},
            "group_mail::create": {"create"},
            "group_mail::detail": {"get", "messages"},
            "group_mail::poll": {"messages", "messages_after"},
            "group_mail::send": {"send"},
            "group_mail::leave": {"leave"},
            "group_mail::transfer": {"propose_transfer"},
            "group_mail::disband": {"disband"},
            "mail_book_previews::page": {"private_mail_title_for_member"},
            "book_rooms::page": {"overview"},
            "book_rooms::join": {"join"},
            "book_rooms::send": {"send"},
            "book_rooms::leave": {"leave"},
        }
        proof = mail_calls.get(entry["handler"], proof)
        # E-12 is book-team triage, not owner-only chapter publishing.
        if policy == "scoped" and entry["handler"] == "text_issues::author_page":
            proof = {"author_page"}
        elif policy == "scoped" and entry["handler"] == "text_issues::change_status":
            proof = {"update_status"}
        if entry["source"] == "crates/web/src/auth/group_mail/transfers.rs":
            proof = {
                "inbox": {"pending_transfer_incoming", "pending_transfer_outgoing"},
                "accept": {"accept_transfer"}, "decline": {"decline_transfer"},
                "revoke": {"revoke_transfer"},
            }.get(entry["handler"], proof)
        if entry["source"] == "crates/web/src/auth/group_mail/invitations.rs":
            proof = {
                "inbox": {"pending_for"}, "invite_page": {"pending_sent"},
                "invite": {"invite"}, "accept": {"accept_invite"},
                "decline": {"decline_invite"}, "revoke": {"revoke_invite"},
            }.get(entry["handler"], proof)
        if entry["source"] == "crates/web/src/auth/notifications.rs":
            proof = {
                "page": {"list_section"}, "mark_one": {"mark_read"},
                "mark_all": {"mark_all_read"}, "save_preferences": {"set_preferences"},
            }.get(entry["handler"], proof)
        # The Layer form/JSON route delegates a *session-derived* actor into
        # one shared financial command. Do not call that delegation scoped
        # without proving the wallet, command and authoritative PG buy binding.
        layer_delegates = {
            ("POST", "/layers/{layer_id}/purchase", "buy_form"):
                r"layer_checkout_api::charge\s*\(\s*&state\s*,\s*actor\s*,",
            ("POST", "/api/v1/layers/{layer_id}/purchase", "layer_checkout_api::buy"):
                r"\bcharge\s*\(\s*&state\s*,\s*actor\s*,",
        }
        layer_pattern = layer_delegates.get(admin_key)
        if layer_pattern is not None:
            charged = functions.get("__layer_charge", "")
            scoped = (
                entry["source"] == "crates/web/src/auth/layer_storefront.rs"
                and bool(re.search(layer_pattern, evidence))
                and bool(re.search(r"\bsession_user\s*\(\s*&state\s*,\s*&headers\s*\)", evidence))
                and "if !state.layer_checkout_enabled" in charged
                and "PositiveId::new(actor)" in charged
                and "WalletId::new(actor.get())" in charged
                and "LayerCheckoutCommand {" in charged
                and bool(re.search(r"(?m)^\s*actor,\s*$", charged))
                and "PgLayers::new" in charged
                and ".buy(&command)" in charged
            )
        else:
            scoped = _contains_scoped_call(evidence, proof)
        if not scoped:
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
    rec_admin = {
        ("GET", "/api/v1/admin/recommendations/quality", "admin::quality"),
        ("POST", "/api/v1/admin/recommendations/control", "admin::control"),
        ("GET", "/me/recommendations/manage", "admin::page"),
        ("POST", "/me/recommendations/manage", "admin::form_control"),
    }
    if (entry["template"] == "/me/refunds"
            and entry["source"] == "crates/web/src/auth/refund_browser.rs"):
        if entry["method"] == "POST":
            checks = ("!enabled(&state)", "session_user(&state, &headers)",
                      "same_origin(&headers)", "valid_csrf(&headers",
                      "IdempotencyKey::parse(&form.command_key)",
                      ".request_once(actor, purchase, &key, &form.reason)")
            if not all(check in evidence for check in checks):
                errors.append(f"{label}: S036 buyer refund cannot bypass session/CSRF/policy")
        elif entry["method"] == "GET" and not (
                "!enabled(&state)" in evidence
                and "recent_for_buyer(actor)" in evidence
                and "session_user(&state, &headers)" in evidence):
            errors.append(f"{label}: S036 refund history must remain buyer-scoped")
    route_key = (entry["method"], entry["template"], entry["handler"])
    e12_team = {
        ("GET", "/me/books/{id}/issues", "text_issues::author_page"),
        ("POST", "/me/books/{id}/issues/{issue_id}/status", "text_issues::change_status"),
    }
    if route_key == (
        "POST", "/api/v1/reader/chapters/{chapter_id}/text-issues",
        "text_issues::submit",
    ) and policy != "scoped":
        errors.append(f"{label}: E-12 paid correction must remain actor-scoped")
    if route_key in e12_team and policy != "scoped":
        errors.append(f"{label}: E-12 team triage must remain actor-scoped")
    if route_key in rec_admin and policy != "administrator":
        errors.append(f"{label}: recommendation administration policy downgraded")
    if policy == "admin_reader":
        # Only the exact read-only shell and one session-bound CSRF POST for
        # current unsealed bodies. Never permit a generic paid grant, wallet
        # command, export or arbitrary privileged route through this policy.
        allowed_shell = (
            entry["method"] == "GET"
            and entry["template"] == "/read/admin/{chapter_id}"
            and entry["source"] == "crates/web/src/paid_grants_http.rs"
            and entry["handler"] == "admin_reader::admin_reader"
        )
        allowed_unsealed = (
            entry["method"] == "POST"
            and entry["template"] == "/api/v1/reader/chapters/{chapter_id}/admin-unsealed"
            and entry["source"] == "crates/web/src/paid_grants_http.rs"
            and entry["handler"] == "admin_unsealed::read"
        )
        if not (allowed_shell or allowed_unsealed):
            errors.append(f"{label}: unreviewed site administrator reader route")
        common = (
            "session_token_digest(&session)",
            "session_actor(&digest)",
            "if actor.administrator",
            "PgAdministratorReaderEntitlements::new",
            "StatusCode::FORBIDDEN",
            "StatusCode::UNAUTHORIZED",
        )
        if not all(marker in evidence for marker in common):
            errors.append(f"{label}: missing live administrator read-entitlement proof")
        if allowed_shell and ".resolve_entitlement(" not in evidence:
            errors.append(f"{label}: admin shell lacks sealed D entitlement")
        if allowed_unsealed and not (
            evidence.count(".unsealed_version(") >= 2
            and "state.admin_unsealed.read(pinned)" in evidence
            and "state.cookies.same_origin(&headers)" in evidence
            and "state.cookies.valid_csrf(&headers" in evidence
            and "StatusCode::CONFLICT" in evidence
        ):
            errors.append(f"{label}: unsealed admin text lacks double D check and CSRF")
    if policy == "administrator":
        if route_key not in rec_admin or entry["source"] != "crates/web/src/recommendation_http/mod.rs":
            errors.append(f"{label}: administrator exception is not allowed here")
        if not ("session_actor(" in evidence and "!verified.administrator" in evidence
                and "StatusCode::FORBIDDEN" in evidence):
            errors.append(f"{label}: missing independent administrator role guard")
    if policy == "recovery":
        allowed = {("POST", "/forgot-password", "password_reset::request"),
                   ("POST", "/reset-password", "password_reset::confirm")}
        if (entry["method"], entry["template"], entry["handler"]) not in allowed:
            errors.append(f"{label}: recovery exception is not allowed here")
        if ("login_limiter::limit(" not in evidence
                or len(re.findall(r"\bpermit_auth_attempt\s*\(", limiter)) < 2):
            errors.append(f"{label}: missing distributed recovery rate limit")
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
        if (entry.get("method"), entry.get("template")) in {
            ("POST", "/layers/{layer_id}/purchase"),
            ("POST", "/api/v1/layers/{layer_id}/purchase"),
        }:
            helper_path = root / "crates/web/src/auth/layer_checkout_api.rs"
            try:
                source_cache[path]["__layer_charge"] = _functions(
                    helper_path.read_text(encoding="utf-8")
                ).get("charge", "")
            except OSError:
                source_cache[path]["__layer_charge"] = ""
        if entry.get("policy") == "support_requester":
            identity_file = root / "crates/web/src/auth/support_identity.rs"
            source_cache[path]["__support_identity"] = (
                _strip_comments(identity_file.read_text(encoding="utf-8"))
                if identity_file.exists() else ""
            )
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
