"""`scripts/check-api-types.py`, the field names both sides of a payload declare."""

import pathlib
import tempfile
import unittest

from _scripts import load

check_api_types = load("check-api-types")


def structs(*files: str):
    """The structs the check reads from Rust files holding `files`."""
    with tempfile.TemporaryDirectory() as folder:
        for index, source in enumerate(files):
            (pathlib.Path(folder) / f"m{index}.rs").write_text(source)
        return check_api_types.rust_structs(pathlib.Path(folder))


def interfaces(source: str):
    with tempfile.TemporaryDirectory() as folder:
        path = pathlib.Path(folder) / "types.ts"
        path.write_text(source)
        return check_api_types.ts_interfaces(path)


class RustSide(unittest.TestCase):
    def test_serde_decides_the_names_on_the_wire(self):
        found, _ = structs(
            """
            #[derive(Debug, Serialize)]
            pub struct Payload {
                pub id: i64,
                #[serde(rename = "type")]
                pub kind: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub note: Option<String>,
                #[serde(skip_serializing)]
                pub secret: String,
                #[sqlx(skip)]
                pub joined: String,
            }
            """
        )
        self.assertEqual(found["Payload"], {"id", "type", "note", "joined"})

    def test_a_flattened_struct_arrives_flat(self):
        found, _ = structs(
            """
            #[derive(Serialize)]
            pub struct Outer {
                pub id: i64,
                #[serde(flatten)]
                pub inner: Inner,
            }
            """,
            """
            #[derive(Serialize)]
            pub struct Inner {
                pub count: u32,
            }
            """,
        )
        self.assertEqual(found["Outer"], {"id", "count"})

    def test_a_struct_without_serialize_is_not_a_payload(self):
        found, _ = structs("#[derive(Debug)]\npub struct Internal {\n    pub id: i64,\n}\n")
        self.assertNotIn("Internal", found)

    def test_two_structs_of_one_name_are_both_reported(self):
        payload = "#[derive(Serialize)]\npub struct Category {\n    pub name: String,\n}\n"
        _, duplicates = structs(payload, payload.replace("name", "label"))
        self.assertEqual(sorted(duplicates), ["Category"])
        self.assertEqual(len(duplicates["Category"]), 2)


class TypeScriptSide(unittest.TestCase):
    def test_only_the_top_level_fields_count(self):
        found = interfaces(
            "export interface Health {\n"
            "  status: string;\n"
            "  instances: {\n"
            "    id: string;\n"
            "  }[];\n"
            "}\n"
        )
        self.assertEqual(found["Health"], {"status", "instances"})

    def test_an_interface_carries_what_it_extends(self):
        found = interfaces(
            "export interface Base {\n  id: string;\n}\n"
            "export interface Child extends Base {\n  name?: string;\n}\n"
        )
        self.assertEqual(found["Child"], {"id", "name"})

    def test_an_object_alias_is_read_as_an_interface(self):
        found = interfaces("export type Summary = {\n  moved: number;\n  failed: number;\n};\n")
        self.assertEqual(found["Summary"], {"moved", "failed"})

    def test_a_mapped_type_declares_no_payload(self):
        found = interfaces(
            "export type Axis = {\n  [K in keyof Facets]: Facets[K] extends Facet[] ? K : never;\n}[keyof Facets];\n"
        )
        self.assertNotIn("Axis", found)


if __name__ == "__main__":
    unittest.main()
