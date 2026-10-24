"""`scripts/contract-breaks.py`, what the contract promises besides its operations."""

import copy
import json
import unittest

from _scripts import ROOT, load

contract_breaks = load("contract-breaks")

CONTRACT = json.loads((ROOT / "backend/openapi/v1.json").read_text())


def notification(current: dict) -> dict:
    return current["components"]["schemas"]["Notification"]


class Payload(unittest.TestCase):
    def test_the_contract_against_itself_breaks_nothing(self):
        self.assertEqual(contract_breaks.payload_breaks(CONTRACT, CONTRACT, "Notification"), [])
        self.assertEqual(contract_breaks.scope_changes(CONTRACT, CONTRACT), [])

    def test_a_retyped_field_breaks_a_receiver(self):
        current = copy.deepcopy(CONTRACT)
        notification(current)["properties"]["data"]["type"] = "string"
        self.assertEqual(
            contract_breaks.payload_breaks(CONTRACT, current, "Notification"),
            ["Notification.data changes its type from object to string"],
        )

    def test_a_field_no_longer_required_breaks_a_receiver(self):
        current = copy.deepcopy(CONTRACT)
        notification(current)["required"].remove("event")
        self.assertEqual(
            contract_breaks.payload_breaks(CONTRACT, current, "Notification"),
            ["Notification.event is no longer required, so a receiver may miss it"],
        )

    def test_a_field_removed_from_a_named_schema_breaks_a_receiver(self):
        current = copy.deepcopy(CONTRACT)
        del current["components"]["schemas"]["AllowedMentions"]["properties"]["parse"]
        self.assertEqual(
            contract_breaks.payload_breaks(CONTRACT, current, "Notification"),
            ["Notification.allowed_mentions.parse is in the release's contract and no longer documented"],
        )

    def test_an_array_whose_items_change_type_breaks_a_receiver(self):
        current = copy.deepcopy(CONTRACT)
        mentions = current["components"]["schemas"]["AllowedMentions"]
        mentions["properties"]["parse"]["items"]["type"] = "integer"
        self.assertEqual(
            contract_breaks.payload_breaks(CONTRACT, current, "Notification"),
            ["Notification.allowed_mentions.parse[] changes its type from string to integer"],
        )

    def test_a_field_added_breaks_nothing(self):
        current = copy.deepcopy(CONTRACT)
        notification(current)["properties"]["extra"] = {"type": "string"}
        self.assertEqual(contract_breaks.payload_breaks(CONTRACT, current, "Notification"), [])


class Deprecations(unittest.TestCase):
    def test_a_deprecated_path_no_release_published_is_named(self):
        current = copy.deepcopy(CONTRACT)
        current["paths"]["/never/released"] = {"post": {"deprecated": True, "responses": {}}}
        self.assertEqual(
            contract_breaks.unreleased_deprecations(CONTRACT, current),
            ["POST /never/released is deprecated, and no release published it: remove it"],
        )

    def test_a_deprecated_path_of_the_release_is_not(self):
        self.assertEqual(contract_breaks.unreleased_deprecations(CONTRACT, CONTRACT), [])

    def test_an_owner_route_the_release_mounted_is_released(self):
        current = copy.deepcopy(CONTRACT)
        current["paths"]["/jobs/{id}/cancel"] = {"post": {"deprecated": True, "responses": {}}}
        main = '        .route("/jobs/{id}/cancel", post(api::jobs::cancel))\n'
        routes = contract_breaks.routed(main)
        self.assertEqual(routes, {"/jobs/{id}/cancel"})
        self.assertEqual(contract_breaks.unreleased_deprecations(CONTRACT, current, routes), [])


class Scopes(unittest.TestCase):
    def test_a_scope_changed_either_way_is_named(self):
        current = copy.deepcopy(CONTRACT)
        current["paths"]["/auth/me"]["get"]["x-routarr-scope"] = "operate"
        self.assertEqual(
            contract_breaks.scope_changes(CONTRACT, current),
            ["x-routarr-scope changed on GET /auth/me: read in the release, operate now"],
        )

    def test_a_change_the_release_accepts_whole_is_not_named(self):
        problem = "x-routarr-scope changed on GET /health: operate in the release, read now"
        accepted = f"# Accepted, and named in the notes.\n{problem}\n"
        self.assertEqual(contract_breaks.unaccepted([problem], accepted), [])
        self.assertEqual(contract_breaks.unaccepted([problem], problem[:-4]), [problem])


if __name__ == "__main__":
    unittest.main()
