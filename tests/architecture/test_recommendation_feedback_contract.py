"""REC-13 design contract smoke checks; not a substitute for runtime PG/HTTP tests."""
from __future__ import annotations

import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / "docs/contracts/recommendation-feedback-v1.md"
SCHEMA = ROOT / "docs/contracts/recommendation-feedback-v1.schema.json"
DOC = ROOT / "docs/roadmap/recommendations-execution-contract.md"


class RecFeedbackContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
        self.text = SPEC.read_text(encoding="utf-8")
        self.defs = self.schema["$defs"]

    def test_machine_schema_has_all_protocol_edges(self) -> None:
        self.assertEqual(self.schema["$schema"], "https://json-schema.org/draft/2020-12/schema")
        for item in ("ImpressionBatchRequest", "ActivationCommand",
                     "ExplicitFeedbackCommand", "InternalAttributionFact",
                     "DailyQualityAggregate"):
            self.assertIn(item, self.defs)
        for schema in self.defs.values():
            self.assertFalse(schema.get("additionalProperties", True))
        self.assertEqual(self.defs["ImpressionBatchRequest"]["properties"]["items"]["maxItems"], 20)

    def test_client_cannot_claim_domain_trust_or_other_user(self) -> None:
        for item in ("ImpressionClaim", "ActivationCommand", "ExplicitFeedbackCommand"):
            body = self.defs[item]["properties"]
            for forbidden in ("user_id", "subject_token", "trusted", "trust_class",
                              "source_event_id", "chapter_completed"):
                self.assertNotIn(forbidden, body)
        facts = self.defs["InternalAttributionFact"]["properties"]
        self.assertEqual(facts["trust_class"]["enum"],
                         ["client_observed", "server_verified_navigation",
                          "domain_committed", "explicit_user_choice"])

    def test_no_click_is_not_a_forged_domain_event(self) -> None:
        events = self.defs["InternalAttributionFact"]["properties"]
        self.assertEqual(set(events["event_type"]["enum"]),
                         {"recommendation_impression", "recommendation_click",
                          "recommendation_action", "recommendation_feedback"})
        self.assertNotIn("no_click", events["action_type"]["enum"])
        self.assertIn("ChapterCompletedV1", self.text)
        self.assertIn("UNKNOWN/CENSORED", self.text)
        self.assertIn("не штраф", self.text.lower())

    def test_contract_links_acceptance_and_non_goals(self) -> None:
        for required in ("REC-FB-01", "REC-FB-20", "OpenAPI",
                         "30 дней", "180 дней", "no_verified_start",
                         "personalized_recommendations_enabled=false",
                         "No Volga", "не реализованный runtime"):
            self.assertIn(required, self.text)
        self.assertIn("recommendation-feedback-v1.md", DOC.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
