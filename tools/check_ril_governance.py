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
POLICIES = {"recovery", "public", "deny", "entry", "csrf_only", "session", "scoped", "owner", "token", "operator", "administrator", "admin_reader", "admin_mail", "admin_refund", "admin_refund_approve", "admin_share_reports", "support_requester", "support_admin"}
WRITES = {"POST", "PUT", "PATCH", "DELETE"}
PROTECTED = {"session", "scoped", "owner", "administrator", "admin_reader", "admin_mail", "admin_refund", "admin_refund_approve", "admin_share_reports", "support_admin"}
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
    "archived_free_state", "save_archived_free",
    "purchased_state", "purchased_body_for_validation", "save_purchased",
    "own_ratings", "set_rating", "visible_free_summary", "thank_visible_free",
    "visible_summary", "set_like",
    "purchased_s3_completion_state", "complete_purchased_s3",
    "saved_place", "saved_state", "save_position", "add_book", "remove_book",
    "pinned_free_chapter", "pinned_free_chapters", "saved_archive_chapters", "saved_archive_toc_window", "purchased_only_books",
    "purchased_s3_completion_state", "complete_purchased_s3",
    "set_ril_behavior_consent", "readable_purchases", "mixed_book_toc_window", "layer_toc_window",
    "record_verified_continuation", "follow", "unfollow", "set_read_state",
    "save_rating", "add_comment", "delete_comment",
    "delivery_preference", "set_delivery_preference",
    "active_role", "create_free", "create_paid",
    "manageable_related_books", "find_related_candidates", "add_related_book", "remove_related_book",
    "manageable_project_status", "update_project_status",
    "owned_book_titles", "update_book_titles",
    "manageable_origin_metadata", "update_origin_metadata",
    "manageable_book_description", "update_book_description",
    "manageable_book_source_links", "update_book_source_links",
    "manageable_lifecycle", "update_lifecycle",
    "manageable_access_level", "verified_upgrade_book_access_level",
    "moderation_books", "moderation_overdue_inbox",
    "moderate_book_hold", "moderate_book_visibility",
    "moderation_pending_deletions", "moderation_pending_deletions_filtered",
    "moderation_pending_deletions_with_access_level",
    "moderation_pending_deletions_with_wait_age", "moderate_ordinary_deletion",
    "pending_count", "pending", "pending_filtered", "pending_with_access_level",
    "pending_with_wait_age",
    "preview", "decide", "private_history",
    "moderation_registration_pending_count", "moderation_registration_queue",
    "moderation_registration_queue_filtered", "moderation_registration_queue_with_access_level",
    "moderation_registration_queue_with_wait_age",
    "moderate_book_registration", "private_book_registration_approver",
    "mailbox", "messages", "send", "send_html", "send_to_peer", "start", "inbox", "receiver_action",
    "send_unkeyed", "set_block", "request", "remove", "request_once",
}
OWNER_CALLS = {
    "recommendation_settings", "update_recommendation_settings",
    "list_owned_books", "create_owned_book", "owned_book", "owner_registration_status",
    "ordinary_deletion_view", "advance_ordinary_deletion",
    "owned_book_is_public", "list_owned_chapters",
    "owned_unpublished_free_chapter", "pending_free_chapter",
    "edit_unpublished_free_chapter", "release_pending_free_chapter",
    "schedule_pending_free_chapter", "cancel_scheduled_free_chapter",
    "release_book",
    "list_books", "book", "list_chapters", "chapter", "create_book", "update_book", "create_chapter", "update_chapter",

    "status_for_owner", "latest_active_for_owner", "cancel_source_for_owner",
    "owned_layers", "addable_books", "current_books", "owner_offer_state",
    "add_book", "remove_book_if_current", "create_once",
    "update_metadata_if_current", "offer_if_current", "delete_owned_collection",
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
    # BR-060: only the two reviewed archive progress handlers may delegate
    # from the private archive router to its sibling sync_actions module.
    # Retain fail-closed resolution for all other nested handlers.
    if (source == root / "crates/web/src/auth/saved_free_chapters.rs"
            and handler in {
                "super::sync_actions::get_archived_free",
                "super::sync_actions::put_archived_free",
            }):
        return source.parent / "sync_actions.rs"
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
    if source == root / "crates/web/src/auth/book_shares.rs" and module == "admin_reports":
        return source.parent / "book_share_reports.rs"
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
    for helper in ("write", "csrf", "change_choice", "resume", "actor", "csrf_valid", "owner", "load", "administrator", "form_allowed"):
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
    # C-12 private history handlers delegate session, role and CSRF to a
    # narrowly named helper in the same Rust file. Only exact reviewed
    # admin-history bindings can use its evidence.
    history_handlers = {
        ("POST", "/api/v1/reader/chapters/{chapter_id}/admin-history", "admin_history::list"),
        ("POST", "/api/v1/reader/chapters/{chapter_id}/admin-history/{version_id}", "admin_history::read"),
    }
    history_binding = (entry["method"], entry["template"], entry["handler"]) in history_handlers
    sealed_handlers = {
        ("POST", "/api/v1/reader/chapters/{chapter_id}/admin-sealed-history", "admin_sealed_history::list"),
        ("POST", "/api/v1/reader/chapters/{chapter_id}/admin-sealed-history/{version_id}/delivery-grants", "admin_sealed_history::grant"),
        ("POST", "/api/v1/reader/chapters/{chapter_id}/admin-sealed-history/{version_id}/assets/{object_id}/delivery-grants", "admin_sealed_history::asset"),
    }
    sealed_binding = (entry["method"], entry["template"], entry["handler"]) in sealed_handlers

    if history_binding and entry["source"] == "crates/web/src/paid_grants_http.rs":
        if not re.search(r"\bauthorize\s*\(\s*&state\s*,\s*&headers\s*\)", body):
            errors.append(f"{label}: historical reader must invoke the verified helper")
        else:
            evidence += "\n" + functions.get("authorize", "")
    if sealed_binding and entry["source"] == "crates/web/src/paid_grants_http.rs":
        if not re.search(r"\bauthenticate_sealed_admin\s*\(\s*&state\s*,\s*&headers\s*\)", body):
            errors.append(f"{label}: sealed history must invoke A/CSRF helper")
        else:
            evidence += "\n" + functions.get("authenticate_sealed_admin", "")
    # S011: new exact-version paid comment handlers share a single A+D guard
    # helper. Inspect THAT helper and the explicit call; never accept an
    # arbitrary comment route or trust a descriptive "scoped" policy alone.
    paid_comment_binding = (
        entry["source"] == "crates/web/src/paid_grants_http.rs"
        and (entry["method"], entry["template"], entry["handler"]) in {
            ("GET", "/api/v1/reader/chapters/{chapter_id}/paid-comments", "paid_comments::list"),
            ("POST", "/api/v1/reader/chapters/{chapter_id}/paid-comments", "paid_comments::submit"),
        }
    )
    if paid_comment_binding:
        auth = functions.get("authorized", "")
        helper_called = re.search(
            r"\bauthorized\s*\(\s*&state,\s*&headers,\s*chapter,\s*purchase,\s*version\s*\)",
            body,
        )
        if not (
            helper_called
            and ".session_token(headers)" in auth
            and "active_session_user(&digest)" in auth
            and "StatusCode::UNAUTHORIZED" in auth
            and "resolve_entitlement(" in auth
            and "EntitlementDecision::Allowed" in auth
            and "pinned.chapter_id == chapter && pinned.version_id == version" in auth
            and "PgPaidChapterComments::new" in body
        ):
            errors.append(f"{label}: S011 paid discussion missing reviewed live A/D purchase+version guard")
        else:
            evidence += "\n" + auth
    # Logout wrapper delegates directly to its reviewed sibling handler.
    # Check the live callee: comments or policy labels cannot replace CSRF.
    if (entry["source"] == "crates/web/src/auth.rs"
            and (entry["method"], entry["template"], entry["handler"])
            == ("POST", "/logout", "logout")
            and "session_security::logout(State(state), headers, Form(form)).await" in body):
        sibling = ROOT / "crates/web/src/auth/session_security.rs"
        caller = _functions(sibling.read_text(encoding="utf-8")).get("logout", "")
        if ("state.cookies.same_origin(&headers)" in caller
                and "valid_csrf(&headers" in caller
                and "StatusCode::FORBIDDEN" in caller
                and ".revoke_session(&digest)" in caller):
            evidence += "\n" + caller
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
    s049_report_ops = {
        ("GET", "/admin/recommendation-reports", "admin_reports::queue"): "pending",
        ("POST", "/admin/recommendation-reports/{share_id}/decision", "admin_reports::decide"): "decide",
    }
    audited_s049 = (
        policy == "admin_share_reports"
        and entry["source"] == "crates/web/src/auth/book_shares.rs"
        and admin_key in s049_report_ops
    )
    if audited_s049:
        # Only this reviewed sibling router may delegate the live cookie to
        # a PG transaction that verifies an administrator role under lock.
        evidence += "\n" + functions.get("session_digest", "")
    if entry["template"].startswith(("/admin", "/internal")) and policy != "deny" and not (audited_admin or audited_support or audited_refund or approved_refund or audited_s049):
        errors.append(f"{label}: admin/internal route requires separately reviewed role policy")
    if policy == "admin_share_reports":
        repo = _production((ROOT / "crates/social/src/repositories/book_share_reports.rs")
                           .read_text(encoding="utf-8"))
        operation = s049_report_ops.get(admin_key) if audited_s049 else None
        required_repo = (
            "r.role='administrator' AND r.is_active",
            "s.revoked_at IS NULL",
            "s.expires_at>now()",
            "u.account_state='active'",
            "FOR SHARE OF s,u,r",
            "administrator(&mut tx, session_digest).await?",
        )
        guarded = bool(
            operation
            and "PgBookShareReports::new" in evidence
            and re.search(rf"\.{operation}\s*\(\s*&digest", evidence)
            and "session_digest(&s, &headers)" in evidence
            and "session_token_digest(&token)" in evidence
            and all(marker in repo for marker in required_repo)
        )
        if operation == "decide":
            guarded = guarded and all(marker in evidence for marker in (
                "s.cookies.same_origin(&headers)",
                "s.cookies.valid_csrf(&headers, &form.csrf_token)",
            )) and all(marker in repo for marker in (
                "FOR UPDATE", "INSERT INTO social_book_share_report_reviews",
                "UPDATE social_book_share_reports SET state=$2",
            ))
        if not guarded:
            errors.append(f"{label}: S049 administrator report review must retain locked role, session, CSRF and audited decision")
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
    if entry["template"].startswith("/moderation/"):
        allowed_moderation = {
            ("GET", "/moderation/books", "home"): "crates/web/src/auth/moderation_books.rs",
            ("GET", "/moderation/books/control", "page"): "crates/web/src/auth/moderation_books.rs",
            ("POST", "/moderation/books/{id}/hold", "submit"): "crates/web/src/auth/moderation_books.rs",
            ("POST", "/moderation/books/{id}/visibility", "submit_visibility"): "crates/web/src/auth/moderation_books.rs",
            ("GET", "/moderation/books/deletions", "deletion_queue"): "crates/web/src/auth/moderation_books.rs",
            ("POST", "/moderation/books/{id}/deletion", "review_deletion"): "crates/web/src/auth/moderation_books.rs",
            ("GET", "/moderation/covers", "page"): "crates/web/src/auth/moderation_covers.rs",
            ("GET", "/moderation/covers/{id}/preview", "preview"): "crates/web/src/auth/moderation_covers.rs",
            ("POST", "/moderation/covers/{id}/decision", "decide"): "crates/web/src/auth/moderation_covers.rs",
            ("GET", "/moderation/books/{id}/cover-history", "history"): "crates/web/src/auth/moderation_covers.rs",
            ("GET", "/moderation/books/registrations", "queue"): "crates/web/src/auth/moderation_registration.rs",
            ("POST", "/moderation/books/{id}/registration", "decide"): "crates/web/src/auth/moderation_registration.rs",
            ("GET", "/moderation/books/{id}/registration-history", "history"): "crates/web/src/auth/moderation_registration.rs",
            ("GET", "/moderation/books/{id}/history", "book_history"): "crates/web/src/auth/moderation_book_history.rs",
            ("GET", "/moderation/books/reviews", "reviewed_index"): "crates/web/src/auth/moderation_book_history.rs",
            ("GET", "/moderation/books/alerts", "page"): "crates/web/src/auth/moderation_overdue_inbox.rs",
        }
        if (allowed_moderation.get(admin_key) != entry["source"]
                or policy != "scoped"):
            errors.append(f"{label}: unreviewed moderator capability surface")
        if not ("session_actor(&digest)" in evidence
                and "actor.moderator || actor.administrator" in evidence
                and "StatusCode::FORBIDDEN" in evidence
                and "StatusCode::UNAUTHORIZED" in evidence):
            errors.append(f"{label}: moderator route lacks live A role guard")
    if entry["template"].startswith("/ops/") and policy not in {"deny", "operator"}:
        errors.append(f"{label}: operator route requires independently reviewed credential")


    # S009 public collection detail also serves an owner's private collection.
    # Guest access is legitimate, but the E repository must constrain every
    # private read by the identity returned from a live A session.
    collection_detail = (
        entry["source"] == "crates/web/src/auth/collections.rs"
        and (entry["method"], entry["template"], entry["handler"])
            == ("GET", "/collections/{id}", "detail")
        and policy == "scoped"
    )
    collection_detail_guarded = False
    if collection_detail:
        reader = functions.get("viewer", "")
        collection_repo = _production((ROOT / "crates/social/src/repositories/book_collections.rs")
                                      .read_text(encoding="utf-8"))
        collection_detail_guarded = bool(
            re.search(r"\bviewer\s*\(\s*&s\s*,\s*&h\s*\)", body)
            and ".session_token(h)" in reader
            and "active_session_user(&session_token_digest(&raw))" in reader
            and re.search(r"\.detail\s*\(\s*viewer\s*,\s*id\s*\)", body)
            and "WHERE id=$1 AND deleted_at IS NULL" in collection_repo
            and "(visibility='public' OR owner_user_id=$2)" in collection_repo
            and ".bind(viewer)" in collection_repo
            and "ok_or(CollectionError::NotFound)" in collection_repo
        )
        if not collection_detail_guarded:
            errors.append(f"{label}: collection private read lacks live session-bound owner gate")
        else:
            evidence += "\n" + reader
    if policy in PROTECTED:
        if not (re.search(r"\b(?:session_user|session_actor|active_session_user)\s*\(", evidence)
                or (audited_s049 and "session_digest(&s, &headers)" in evidence
                    and "session_token_digest(&token)" in evidence)):
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
        # The private reader needs its fixed chapter-local login redirect.
        # An unrelated unauthorized branch must not mask removal of that guard.
        if entry["template"] == "/read/{id}" and entry["handler"] == "private_reader::page":
            if not fixed_reader_return:
                errors.append(f"{label}: session failure does not explicitly deny/redirect")
        elif not (
            "StatusCode::UNAUTHORIZED" in evidence
            or 'Redirect::to("/login")' in evidence
            or collection_detail_guarded
        ):
            errors.append(f"{label}: session failure does not explicitly deny/redirect")
    if policy in {"scoped", "owner"}:
        proof = SCOPE_CALLS if policy == "scoped" else OWNER_CALLS
        # Shared write helpers contain both calls; each route must prove its own.
        if collection_detail:
            proof = {"detail"}
        elif policy == "scoped" and entry["template"] == "/me/books/{id}/follow":
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
        if collection_detail:
            scoped = collection_detail_guarded
        elif layer_pattern is not None:
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
        elif entry["source"] == "crates/web/src/auth/moderation_covers.rs":
            cover_ops = {"page": "pending_with_wait_age", "preview": "preview",
                         "decide": "decide", "history": "private_history"}
            operation = cover_ops.get(entry["handler"])
            scoped = bool(
                operation
                and re.search(rf"\.{operation}\s*\(\s*&digest", evidence)
                and "session(&state, &headers)" in evidence
                and "session_actor(&digest)" in evidence
                and "actor.moderator || actor.administrator" in evidence
            )
        elif entry["source"] == "crates/web/src/auth/moderation_registration.rs":
            registration_ops = {
                "queue": "moderation_registration_queue_with_wait_age",
                "decide": "moderate_book_registration",
                "history": "private_book_registration_approver",
            }
            operation = registration_ops.get(entry["handler"])
            scoped = bool(
                operation
                and re.search(rf"\b{operation}\s*\(\s*&digest", evidence)
                and "session_token_digest(&token)" in evidence
                and "session_actor(&digest)" in evidence
                and "actor.moderator || actor.administrator" in evidence
            )
        elif entry["source"] == "crates/web/src/auth/moderation_book_history.rs":
            history_ops = {
                "book_history": "private_book_moderation_history",
                "reviewed_index": "private_reviewed_moderation_decisions",
            }
            operation = history_ops.get(entry["handler"])
            scoped = bool(
                operation
                and re.search(rf"\b{operation}\s*\(\s*&digest", evidence)
                and "session_actor(&digest)" in evidence
                and "actor.moderator || actor.administrator" in evidence
                and "session_token_digest(&token)" in evidence
            )
        elif entry["source"] == "crates/web/src/auth/moderation_overdue_inbox.rs":
            scoped = bool(
                re.search(r"\bmoderation_overdue_inbox\s*\(\s*&digest", evidence)
                and "session_token_digest(&token)" in evidence
                and "session_actor(&digest)" in evidence
                and "actor.moderator || actor.administrator" in evidence
                and "StatusCode::FORBIDDEN" in evidence
                and "StatusCode::UNAUTHORIZED" in evidence
            )
        elif entry["source"] == "crates/web/src/auth/moderation_books.rs":
            # Book approval is the only reviewed moderation capability.
            # The live session/role are locked again in the PG mutation.
            operation = {
                "home": "moderation_registration_pending_count",
                "page": "moderation_books",
                "submit": "moderate_book_hold",
                "submit_visibility": "moderate_book_visibility",
                "deletion_queue": "moderation_pending_deletions_with_wait_age",
                "review_deletion": "moderate_ordinary_deletion",

            }.get(entry["handler"], "__unreviewed_moderation_operation__")
            scoped = (
                bool(re.search(rf"\b{operation}\s*\(\s*&digest\s*(?:,|\))", evidence))
                and "session_token_digest(&token)" in evidence
                and "session_actor(&digest)" in evidence
            )
        elif entry["source"] == "crates/web/src/auth/teams.rs":
            # A-04: the verified session actor is bound to one Team service.
            # Check the reviewed service source as well as each HTTP caller:
            # calling an unbound generic method cannot satisfy owner proof.
            required = {"index": "list", "create": "create", "detail": "get"}
            operation = required.get(entry["handler"])
            identity = functions.get("actor", "")
            service = _production((ROOT / "crates/books/src/services/translation_teams.rs")
                                  .read_text(encoding="utf-8"))
            scoped = bool(
                operation
                and re.search(r"\bactor\s*\(\s*&s\s*,\s*&headers\s*\)", body)
                and re.search(r"\.for_owner\s*\(\s*actor\s*\)", body)
                and re.search(rf"\bowner\s*\.\s*{operation}\s*\(", body)
                and ".active_session_user(&session_token_digest(&token))" in identity
                and "StatusCode::UNAUTHORIZED" in identity
                and "check_actor(actor)?" in service
                and "repository: &self.repository" in service
                and "actor," in service
                and ".list_owned(self.actor)" in service
                and re.search(r"\.create\s*\(\s*self\.actor\s*,\s*&name\s*,\s*&description\s*\)", service)
                and ".owned(self.actor, id)" in service
            )
        elif (entry["source"] == "crates/web/src/auth.rs"
              and (entry["method"], entry["template"], entry["handler"]) in {
                  ("GET", "/me/books/{id}/rate", "book_stars::page"),
                  ("POST", "/me/books/{id}/rate", "book_stars::submit"),
              }):
            # F-05: an actor-bound service delegates to the same live PG checks.
            # Validate the concrete service implementation, not its name alone.
            required = {"book_stars::page": "page", "book_stars::submit": "save"}
            operation = required.get(entry["handler"])
            service = _production((ROOT / "crates/recommendations/src/services/book_ratings.rs")
                                  .read_text(encoding="utf-8"))
            scoped = bool(
                operation
                and re.search(r"\bsession_user\s*\(\s*&state\s*,\s*&headers\s*\)", body)
                and re.search(r"\.for_actor\s*\(\s*actor\s*\)", body)
                and re.search(rf"\bratings\.{operation}\s*\(\s*book_id", body)
                and "actor," in service
                and "visible_summary_for_actor(book_id, Some(self.actor))" in service
                and ".own_ratings(self.actor, book_id)" in service
                and ".eligible(self.actor, book_id)" in service
                and ".set_rating(self.actor, book_id, rating.kind, rating.score)" in service
            )
        elif paid_comment_binding:
            # Exact canonical E repository operation with caller-bound actor,
            # not just a generic method called "list" or "create".
            operation = "list" if entry["method"] == "GET" else "create"
            scoped = bool(re.search(rf"\.{operation}\s*\(\s*actor\s*,", body)
                and re.search(r"\bauthorized\s*\(\s*&state,", body))
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
        allowed_history = history_binding and entry["source"] == "crates/web/src/paid_grants_http.rs"
        allowed_sealed = sealed_binding and entry["source"] == "crates/web/src/paid_grants_http.rs"
        if not (allowed_shell or allowed_unsealed or allowed_history or allowed_sealed):
            errors.append(f"{label}: unreviewed site administrator reader route")
        if allowed_sealed and not (
            ("sealed_history(" in evidence or "issue_paid_chapter_grant(" in evidence
             or "issue_paid_admin_asset_grant(" in evidence)
            and "authenticate_sealed_admin(" in evidence
            and "state.cookies.valid_csrf(headers" in evidence
            and "state.cookies.same_origin(headers)" in evidence
            and "StatusCode::FORBIDDEN" in evidence
            and "StatusCode::UNAUTHORIZED" in evidence
        ):
            errors.append(f"{label}: sealed history lacks live A/CSRF/D proof")
        if allowed_sealed and entry["handler"] != "admin_sealed_history::list" and not (
            "state.admin_audit.record(" in evidence
            or ".admin_audit" in evidence and ".record(" in evidence
        ):
            errors.append(f"{label}: sealed history must audit signed delivery")
        if allowed_history and not (
            "unsealed_historical_version(" in evidence
            or "unsealed_history(" in evidence
        ):
            errors.append(f"{label}: history must use a live chapter-version entitlement")
        if allowed_history and not (
            "state.cookies.same_origin(headers)" in evidence
            and "state.cookies.valid_csrf(headers" in evidence
            and "StatusCode::UNAUTHORIZED" in evidence
            and "StatusCode::FORBIDDEN" in evidence
        ):
            errors.append(f"{label}: history lacks required A/CSRF denial")
        common = (
            "session_token_digest(&session)",
            "session_actor(&digest)",
            "if actor.administrator",
            "PgAdministratorReaderEntitlements::new",
            "StatusCode::FORBIDDEN",
            "StatusCode::UNAUTHORIZED",
        )
        if not all(marker in evidence for marker in common):
            if not (allowed_sealed and all(marker in evidence for marker in (
                "session_token_digest(&session)",
                "session_actor(&digest)",
                "if actor.administrator",
                "StatusCode::FORBIDDEN",
                "StatusCode::UNAUTHORIZED",
            )) and (
                "PgAdministratorReaderEntitlements::new" in evidence
                or "PgAdministratorHistoricalEditionEntitlements::new" in evidence
            )):
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
