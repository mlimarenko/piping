"""Safety controls for the NetBox snapshot producer."""

import importlib.util
import copy
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).parents[1] / "scripts" / "netbox_snapshot_export.py"
SPEC = importlib.util.spec_from_file_location("netbox_snapshot_export", SCRIPT)
exporter = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(exporter)


class SafetyControls(unittest.TestCase):
    def test_source_settings_and_script_serializers_cannot_execute_during_bootstrap(self):
        guard = exporter.CoreImportGuard()
        for module in ("netbox.settings", "netbox.configuration", "configuration",
                       "extras.api.serializers.scripts", "extras.api.serializers_.scripts"):
            with self.assertRaises(ImportError):
                guard.find_spec(module)
        self.assertIsNone(guard.find_spec("dcim.models"))

    def test_external_settings_are_rejected_before_django_import(self):
        with patch.dict(os.environ, {"DJANGO_SETTINGS_MODULE": "source.settings"}):
            with self.assertRaisesRegex(ValueError, "external Django settings"):
                exporter.bootstrap({})

    def test_no_export_without_identity(self):
        result = subprocess.run([sys.executable, str(SCRIPT)], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("invalid command-line arguments", result.stderr)

    def test_unknown_argument_never_echoes_value(self):
        result = subprocess.run([sys.executable, str(SCRIPT), "--db-password", "CANARY"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("CANARY", result.stdout + result.stderr)

    def test_contract_tampering_and_schema_drift_are_rejected(self):
        raw = (SCRIPT.parents[1] / "contracts/netbox-4.7.1-source.json").read_bytes()
        contract = exporter.load_contract(raw)
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            exporter.load_contract(raw + b" ")
        schema = contract["schema"]
        observed = {"netbox_version": "4.7.1", "models": []}
        for model in schema["models"]:
            item = {key: copy.deepcopy(model[key]) for key in ("model", "table", "auto_created", "fields")}
            item["fields"] = [{k: v for k, v in field.items() if k not in ("disposition", "reason", "output_name", "value_shape")} for field in item["fields"]]
            observed["models"].append(item)
        self.assertEqual(exporter.validate_census(schema, observed), len(observed["models"]))
        observed["models"][0]["fields"][0]["type"] = "UnexpectedField"
        with self.assertRaisesRegex(ValueError, "census differs"):
            exporter.validate_census(schema, observed)

    def test_custom_fields_are_selected_by_name_and_declared_type(self):
        raw = (SCRIPT.parents[1] / "contracts/netbox-4.7.1-source.json").read_bytes()
        policy = exporter.load_contract(raw)["schema"]["custom_field_policy"]
        def definition(name, kind):
            return {"name": name, "type": kind, "status": "active",
                    "object_types__app_label": "dcim", "object_types__model": "device"}
        selected, excluded = exporter.custom_field_plan([
            definition("asset_label", "text"), definition("api_token", "text"),
            definition("blob", "json"), definition("parent_device", "object"),
        ], policy)
        self.assertEqual(selected, {"dcim.device": {"asset_label": "text", "parent_device": "object"}})
        self.assertEqual(excluded, {"credential_name": 1, "untyped": 1})
        self.assertEqual(exporter.approved_definition_names(selected), ["asset_label", "parent_device"])
        self.assertEqual(exporter.custom_field_value("safe", "text", policy), "safe")
        self.assertEqual(exporter.custom_field_value(42, "object", policy), 42)
        with self.assertRaises(ValueError):
            exporter.custom_field_value({"secret": "unsafe"}, "text", policy)
        with self.assertRaises(ValueError):
            exporter.custom_field_plan([definition("asset", "unknown")], policy)

    def test_custom_field_defaults_preserve_typed_values_and_refuse_references(self):
        raw = (SCRIPT.parents[1] / "contracts/netbox-4.7.1-source.json").read_bytes()
        policy = exporter.load_contract(raw)["schema"]["custom_field_policy"]
        for value, kind in (("rack-a", "text"), (17, "integer"), (None, "text"),
                            (None, "object"), (None, "multiobject"),
                            (["a", "b"], "multiselect"), ([], "multiobject")):
            self.assertEqual(exporter.custom_field_default(value, kind, policy), value)
        for value, kind in ((42, "object"), ([42], "multiobject"),
                            ("17", "integer"), ({"secret": "x"}, "text"),
                            (["wrong"], "multiobject")):
            with self.assertRaises(ValueError):
                exporter.custom_field_default(value, kind, policy)

    def test_content_type_relations_use_model_tokens(self):
        self.assertEqual(exporter.content_type_references([3, 2], {2: "dcim.site", 3: "ipam.vlan"}),
                         ["dcim.site", "ipam.vlan"])
        with self.assertRaisesRegex(ValueError, "unknown content type"):
            exporter.content_type_references([4], {2: "dcim.site"})

    def test_generic_ids_are_projected_as_canonical_edges(self):
        raw = (SCRIPT.parents[1] / "contracts/netbox-4.7.1-source.json").read_bytes()
        models = {m["model"]: m for m in exporter.load_contract(raw)["schema"]["models"]}
        pairs = (("ipam.ipaddress", "assigned_object_id"), ("ipam.prefix", "scope_id"),
                 ("ipam.service", "parent_object_id"), ("dcim.inventoryitem", "component_id"))
        specs = [next(f for f in models[model]["fields"] if f["name"] == field) for model, field in pairs]
        row = dict(exporter.output_field(spec, 99) for spec in specs)
        self.assertEqual(row, {"assigned_object": 99, "scope": 99, "parent": 99, "component": 99})
        self.assertTrue(all(spec["name"] not in row for spec in specs))

    def test_template_component_keeps_typed_identity_and_requires_evidence(self):
        raw = (SCRIPT.parents[1] / "contracts/netbox-4.7.1-source.json").read_bytes()
        schema = exporter.load_contract(raw)["schema"]
        spec = next(m for m in schema["models"] if m["model"] == "dcim.inventoryitemtemplate")
        fields = {f["name"]: f for f in spec["fields"]}
        row = dict([exporter.output_field(fields["component_id"], 99),
                    exporter.output_field(fields["component_type"], "dcim.frontporttemplate")])
        self.assertEqual(row, {"component": 99, "component_type": "dcim.frontporttemplate"})
        allowed = {m["model"] for m in schema["models"] if m["disposition"] == "export"}
        exporter.validate_generic_pairs(row, spec, allowed)
        exporter.validate_generic_pairs({"component": None, "component_type": None}, spec, allowed)
        for invalid in ({"component": 99}, {"component": 99, "component_type": None},
                        {"component": None, "component_type": "dcim.frontporttemplate"},
                        {"component": 99, "component_type": "plugin.widget"}):
            with self.assertRaises(ValueError):
                exporter.validate_generic_pairs(invalid, spec, allowed)

    def test_service_port_mappings_keep_values_and_reject_malformed(self):
        spec = {"name": "port_mappings", "value_shape": "service_port_mappings"}
        values = ["TCP/080", "udp/53", "sctp/65535"]
        self.assertEqual(exporter.output_field(spec, values), ("port_mappings", values))
        for bad in (None, [], "tcp/80", ["tcp/0"], ["icmp/80"], ["tcp/80", "TCP/080"], [42]):
            with self.assertRaises(ValueError):
                exporter.output_field(spec, bad)

    def test_cable_termination_projection_preserves_positions_and_edges(self):
        raw = (SCRIPT.parents[1] / "contracts/netbox-4.7.1-source.json").read_bytes()
        schema = exporter.load_contract(raw)["schema"]
        model = next(m for m in schema["models"] if m["model"] == "dcim.cabletermination")
        fields = {f["name"]: f for f in model["fields"]}
        self.assertEqual(fields["positions"]["disposition"], "export")
        for positions in (None, [], [1024, 1, 1]):
            row = dict(exporter.output_field(fields[name], value) for name, value in (
                ("cable", 17), ("cable_end", "A"), ("connector", 2),
                ("termination_id", 99), ("termination_type", "dcim.interface"),
                ("positions", positions)))
            self.assertEqual(row, {"cable": 17, "cable_end": "A", "connector": 2,
                                   "termination": 99, "termination_type": "dcim.interface",
                                   "positions": positions})
            exporter.validate_generic_pairs(row, model, {"dcim.interface"})
        for invalid in (0, "1", (1,), [0], [1025], [True], [None], [1.0]):
            with self.assertRaises(ValueError):
                exporter.output_field(fields["positions"], invalid)
        with self.assertRaises(ValueError):
            exporter.validate_generic_pairs({"termination": 99, "termination_type": "plugin.widget"}, model, {"dcim.interface"})

    def test_positions_array_requires_approved_model_and_base_field(self):
        base = type("PositiveSmallIntegerField", (), {})()
        field = type("ArrayField", (), {"name": "positions", "base_field": base})()
        model = SimpleNamespace(_meta=SimpleNamespace(label_lower="dcim.cabletermination"))
        spec = {"value_shape": "cable_termination_positions"}
        exporter.validate_export_array(model, field, spec)
        for bad_model, bad_field in (
            (SimpleNamespace(_meta=SimpleNamespace(label_lower="dcim.interface")), field),
            (model, type("ArrayField", (), {"name": "positions", "base_field": object()})()),
            (model, type("ArrayField", (), {"name": "other", "base_field": base})()),
        ):
            with self.assertRaises(ValueError):
                exporter.validate_export_array(bad_model, bad_field, spec)


if __name__ == "__main__":
    unittest.main()
