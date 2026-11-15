"""Exact S042/S043 route inventory and service/transaction guard regression alarms."""
import re
from pathlib import Path

BINDINGS = {('GET', '/api/v1/ads/chapter/campaigns/{id}', 'api::campaign'), ('POST', '/api/v1/ads/daily-campaigns/{id}/cancel', 'api::cancel_daily'), ('GET', '/api/v1/me/ads/chapter-campaigns', 'api::chapter_campaigns'), ('GET', '/api/v1/me/ads/daily-campaigns', 'api::daily_campaigns'), ('POST', '/api/v1/ads/chapter/impressions', 'api::impression'), ('POST', '/api/v1/ads/chapter/campaigns', 'api::chapter'), ('GET', '/me/ads', 'console::page'), ('POST', '/api/v1/ads/chapter/campaigns/{id}/cancel', 'api::cancel_chapter'), ('POST', '/me/ads/publisher/settings', 'console::publisher_settings'), ('POST', '/api/v1/me/chapter-ads/publisher-settings', 'api::publisher_settings'), ('POST', '/me/ads/publisher/credit', 'console::publisher_credit'), ('POST', '/admin/promotion/paid/daily-offer', 'admin::daily'), ('POST', '/api/v1/ads/chapter/click', 'api::click'), ('POST', '/api/v1/ads/daily-campaigns', 'api::daily'), ('POST', '/me/ads/chapter', 'console::chapter'), ('POST', '/admin/promotion/paid/chapter-package', 'admin::package'), ('GET', '/api/v1/ads/chapter/packages', 'api::packages'), ('POST', '/admin/promotion/paid/slot-policy', 'admin::slot_policy'), ('GET', '/admin/promotion/paid', 'admin::page'), ('POST', '/me/ads/daily', 'console::daily'), ('GET', '/api/v1/me/chapter-ads/publisher-ledger', 'api::publisher'), ('POST', '/api/v1/me/chapter-ads/publisher-credit', 'api::publisher_credit'), ('POST', '/admin/promotion/paid/{product}/{id}/cancel', 'admin::cancel'), ('GET', '/api/v1/ads/daily-offers', 'api::offers'), ('POST', '/me/ads/{product}/{id}/cancel', 'console::cancel')}
SOURCE = "crates/web/src/paid_ads_http/mod.rs"

def compact(text):
    return re.sub(r"\s+", "", text)

def verify(root, entry, parse_functions):
    label = f'{entry["method"]} {entry["template"]}'
    binding = (entry["method"], entry["template"], entry["handler"])
    if entry["source"] != SOURCE or binding not in BINDINGS:
        return [f"{label}: paid advertising binding is not reviewed"]
    base = root / "crates/web/src/paid_ads_http"
    common = parse_functions((base / "mod.rs").read_text())
    module, handler = entry["handler"].split("::")
    functions = dict(common)
    functions.update(parse_functions((base / f"{module}.rs").read_text()))
    if module == "admin" and handler == "cancel":
        functions["cancel_form"] = parse_functions((base / "console.rs").read_text())["cancel_form"]
    body = functions.get(handler, "")
    evidence = body
    added = set()
    for _ in range(6):
        for name in ("purchase_response", "cancel_response", "cancel_form", "campaigns", "actor", "csrf", "api_csrf", "command_key"):
            if name not in added and re.search(rf"(?<!\w){name}\s*\(", evidence):
                evidence += "\n" + functions.get(name, "")
                added.add(name)
    source = compact(evidence)
    errors = []
    public = entry["handler"] in {"api::offers", "api::packages"}
    if not public and not all(marker in source for marker in ("session_actor(&digest)", "StatusCode::UNAUTHORIZED", "session_digest:digest")):
        errors.append(f"{label}: missing live actor/session proof")
    if entry["template"].startswith("/admin/"):
        direct = "actor(&s,&h,true)" in compact(body)
        delegated = "cancel_form(&s,&h,&p,id,b,true)" in compact(body) and "actor(s,h,admin)" in source
        if not (direct or delegated) or "administrator&&!a.administrator" not in source:
            errors.append(f"{label}: missing explicit administrator role check")
    if entry["method"] == "POST":
        if not all(m in source for m in ("cookies.same_origin(h)", "cookies.valid_csrf(h,token)", "StatusCode::FORBIDDEN")):
            errors.append(f"{label}: missing Origin/CSRF rejection")
        operation = {
          "daily": ".purchase(", "chapter": ".purchase(", "cancel": ".cancel(",
          "cancel_daily": ".cancel(", "cancel_chapter": ".cancel(", "impression": ".impression(",
          "publisher_settings": ".publisher_settings(", "publisher_credit": ".release_publisher(",
          "click": ".click(", "package": ".publish_package(", "slot_policy": ".slot_policy(",
        }.get(handler)
        if module == "admin" and handler == "daily": operation = ".publish_daily("
        if not operation or operation not in source:
            errors.append(f"{label}: missing reviewed typed service operation")
        if (handler in {"daily", "chapter", "cancel", "cancel_daily", "cancel_chapter", "publisher_credit"}
                and not (module == "admin" and handler == "daily") and "command_key" not in source):
            errors.append(f"{label}: missing stable money command key")
    wiring = compact((base / "mod.rs").read_text())
    if 'std::env::var(name).as_deref()==Ok("1")' not in wiring:
        errors.append(f"{label}: advertising flags are not explicitly default off")
    if "DefaultBodyLimit::max(8192)" not in wiring or "Arc<dynAdFundingPort>" not in wiring:
        errors.append(f"{label}: missing bounded body or D posting port")
    return errors
