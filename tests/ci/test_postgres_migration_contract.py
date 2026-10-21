"""Preservation boundaries and opt-in, local PostgreSQL expression proofs."""

import hashlib
import json
import os
import pathlib
import subprocess
import unittest
from unittest.mock import Mock

from scripts.ci import postgres_migration_contract as contract


ROOT = pathlib.Path(__file__).resolve().parents[2]


def obj(kind, name, definition):
    return {"kind": kind, "name": name, "definition": definition}


def column(name="public.owned.value", position=1):
    return obj("column", name, {"position": position, "type": "text",
                               "not_null": False, "default": None,
                               "collation": "default", "identity": "", "generated": ""})


class PreservationContract(unittest.TestCase):
    def setUp(self):
        self.canonical = [obj("namespace", "public", "public"),
                          obj("relation", "public.owned", {"kind": "r"}), column(),
                          obj("index", "public.owned_idx", "CREATE INDEX owned_idx ON public.owned (value)"),
                          obj("function", "public.f()", "CREATE FUNCTION public.f() RETURNS trigger ..."),
                          obj("trigger", "public.owned.guard", {"enabled": "O", "internal": False}),
                          obj("trigger", '["public", "owned", "internal", "fk"]',
                              {"enabled": "O", "internal": True, "constraint": {"name": "fk"}})]

    def test_lossless_catalog_rejects_duplicate_identities(self):
        with self.assertRaises(ValueError):
            contract.catalog_objects(json.dumps([column(), column()]))

    def test_inventory_requires_exact_full_extra_set_and_definition(self):
        extras = [obj("relation", "public.retained", {"kind": "r"}),
                  column("public.retained.value")]
        approved = [{"kind": o["kind"], "name": o["name"],
                     "definition_sha256": contract.definition_hash(o["definition"])} for o in extras]
        contract.canonical_readiness(self.canonical, self.canonical + extras, trusted_inventory=approved)
        for installed, inventory in (
            (self.canonical + extras, ()),
            (self.canonical + extras[:1], approved),
            (self.canonical + extras + [column("public.retained.unlisted")], approved),
            (self.canonical + [extras[0], column("public.retained.value", 2)], approved),
            (self.canonical + extras, approved + [approved[0]]),
        ):
            with self.subTest(installed=installed), self.assertRaises(ValueError):
                contract.canonical_readiness(self.canonical, installed, trusted_inventory=inventory)
        contract.exact_target_preservation(self.canonical + extras, self.canonical + extras)

    def test_every_existing_field_remains_exact(self):
        for index, field, changed in ((2, "type", "varchar(32)"), (2, "not_null", True),
                                      (2, "default", "'other'::text"), (2, "collation", "C"),
                                      (5, "enabled", "D"), (6, "enabled", "R")):
            installed = json.loads(json.dumps(self.canonical))
            installed[index]["definition"][field] = changed
            with self.subTest(field=field), self.assertRaises(ValueError):
                contract.canonical_readiness(self.canonical, installed)
        for index in range(len(self.canonical)):
            installed = self.canonical[:index] + self.canonical[index + 1:]
            with self.subTest(missing=index), self.assertRaises(ValueError):
                contract.canonical_readiness(self.canonical, installed)
        for index in (3, 4):
            installed = json.loads(json.dumps(self.canonical))
            installed[index]["definition"] += " changed"
            with self.subTest(changed=index), self.assertRaises(ValueError):
                contract.canonical_readiness(self.canonical, installed)

    def test_layout_approval_is_bound_but_never_weakens_target_seal(self):
        installed = self.canonical.copy()
        installed[2] = column(position=7)
        with self.assertRaises(ValueError):
            contract.canonical_readiness(self.canonical, installed)
        contract.canonical_readiness(self.canonical, installed,
                                     position_pairs={"public.owned.value": (1, 7)})
        with self.assertRaises(ValueError):
            contract.canonical_readiness(self.canonical, installed,
                                         position_pairs={"public.owned.value": (1, 8)})
        with self.assertRaises(ValueError):
            contract.exact_target_preservation(self.canonical, installed)
        with self.assertRaises(ValueError):
            contract.canonical_readiness(self.canonical, self.canonical,
                                         position_pairs={"public.absent.value": (1, 2)})

    def test_new_slots_are_explicit_additions_not_replacements(self):
        addition = column("public.owned.new_value", position=8)
        contract.exact_target_preservation(self.canonical, self.canonical + [addition], additions=[addition])
        with self.assertRaises(ValueError):
            contract.exact_target_preservation(self.canonical, self.canonical + [addition])
        with self.assertRaises(ValueError):
            contract.exact_target_preservation(self.canonical, self.canonical, additions=[column()])

    def test_rendered_constraint_equality_is_not_a_native_proof(self):
        form = next(iter(contract.historical_check_forms().values()))
        canonical = [obj("constraint", form.name, form.canonical)]
        installed = [obj("constraint", form.name, form.historical)]
        with self.assertRaises(ValueError):
            contract.canonical_readiness(canonical, installed)


class CheckSourceRegistration(unittest.TestCase):
    def source_prefix(self, source):
        prefix = {}
        for path in subprocess.check_output(["git", "ls-tree", "-r", "--name-only",
                source, "apps/api/migrations"], cwd=ROOT, text=True).splitlines():
            if path.endswith(".sql") and int(pathlib.Path(path).name[:4]) <= 22:
                raw = subprocess.check_output(["git", "show", f"{source}:{path}"], cwd=ROOT)
                prefix[path] = hashlib.sha384(raw).hexdigest()
        self.assertEqual(len(prefix), 22)
        return prefix

    def test_only_exact_registered_source_prefix_and_postgres_are_eligible(self):
        prefix = self.source_prefix(contract.HISTORICAL_SOURCE_SHA)
        for source in contract.REGISTERED_CHECK_SOURCES:
            with self.subTest(source=source):
                actual = self.source_prefix(source)
                contract.validate_check_source(source, contract.definition_hash(actual), contract.HISTORICAL_POSTGRES_IMAGE)
        renamed = dict(prefix)
        renamed["renamed.sql"] = renamed.pop(next(iter(renamed)))
        missing = dict(prefix)
        missing.pop(next(iter(missing)))
        changed = dict(prefix)
        changed[next(iter(changed))] = "0" * 96
        form = next(iter(contract.historical_check_forms().values()))
        for source, actual, pg in [("f" * 40, prefix, contract.HISTORICAL_POSTGRES_IMAGE),
                ("8f812de3adc4b62abbfa3400d9539b25cdb7058a", prefix, contract.HISTORICAL_POSTGRES_IMAGE),
                ("75b9733bc7e31fdfc9a5f51e1f00b6b4fac761dc", prefix, contract.HISTORICAL_POSTGRES_IMAGE),
                ("9e457c484e4b032bd1df0c57b72e937fc8c29902", prefix, contract.HISTORICAL_POSTGRES_IMAGE),
                (None, prefix, contract.HISTORICAL_POSTGRES_IMAGE),
                *[(source, p, contract.HISTORICAL_POSTGRES_IMAGE) for source in contract.REGISTERED_CHECK_SOURCES
                  for p in (renamed, missing, changed)],
                *[(source, prefix, "sha256:" + "0" * 64) for source in contract.REGISTERED_CHECK_SOURCES]]:
            sql = Mock(side_effect=AssertionError("source rejection must precede SQL"))
            kwargs = dict(source_sha=source, prefix_sha384=actual, postgres_image=pg)
            with self.subTest(source=source), self.assertRaisesRegex(ValueError, "Unregistered source"):
                contract.prove_native_check(sql, form, {}, **kwargs)
            with self.assertRaisesRegex(ValueError, "Unregistered source"):
                contract.prove_original_native_checks(sql, b"{}", trusted_capture_sha256="0" * 64, **kwargs)
            sql.assert_not_called()
        # Isolate catalog handoff checks from the native compiler (covered natively).
        # Only this unit setup assigns the private issuer; JSON reconstruction cannot.
        source = "5e4b78bfe647ea55829f24992b944cd2a8392172"
        binding = {"name": form.name, "definition": form.historical}
        proof = contract.NativeCheckProof(form.name, form.canonical, form.historical,
            contract.definition_hash(binding), "unit compiler output", source,
            contract.definition_hash(prefix), contract.HISTORICAL_POSTGRES_IMAGE, "a" * 64, "b" * 64, form.historical)
        canonical = [obj("constraint", form.name, form.canonical)]
        installed = [obj("constraint", form.name, form.historical)]
        kwargs = dict(source_sha=source, prefix_sha384=prefix, postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                      installed_check_bindings=[binding])
        with self.assertRaisesRegex(ValueError, "Invalid or duplicate native CHECK proof"):
            contract.canonical_readiness(canonical, installed, check_proofs=[proof], **kwargs)
        object.__setattr__(proof, "_issuer", contract._NATIVE_PROOF_ISSUER)
        contract.canonical_readiness(canonical, installed, check_proofs=[proof], **kwargs)
        with self.assertRaisesRegex(ValueError, "Invalid or duplicate native CHECK proof"):
            contract.canonical_readiness(canonical, installed, check_proofs=[proof],
                **dict(kwargs, source_sha="ccb216481bcbc4d54315205269753e5d9d276d29"))
        with self.assertRaisesRegex(ValueError, "Invalid or duplicate native CHECK proof"):
            contract.canonical_readiness(canonical, installed, check_proofs=[proof],
                **dict(kwargs, source_sha="70254dc18cd36e8717de88284ba7456a12f50742"))
        with self.assertRaisesRegex(ValueError, "Invalid or duplicate native CHECK proof"):
            contract.canonical_readiness(canonical, installed, check_proofs=[proof],
                **dict(kwargs, source_sha="96c9833e6a4cfdee0eff9c206fcd2cbee9e8dd53"))
        with self.assertRaisesRegex(ValueError, "Invalid or duplicate native CHECK proof"):
            contract.canonical_readiness(canonical, installed, check_proofs=[proof],
                **dict(kwargs, source_sha=contract.HISTORICAL_SOURCE_SHA))
        import copy
        mixed = copy.copy(proof)
        object.__setattr__(mixed, "source_sha", contract.HISTORICAL_SOURCE_SHA)
        with self.assertRaisesRegex(ValueError, "Invalid or duplicate native CHECK proof"):
            contract.canonical_readiness(canonical, installed, check_proofs=[proof, mixed], **kwargs)
        drifted = dict(binding, definition=form.historical + " changed")
        with self.assertRaises(ValueError):
            contract.canonical_readiness(canonical, installed, check_proofs=[proof],
                **dict(kwargs, installed_check_bindings=[drifted]))


class NativeSession:
    """One psql session; no connection credentials or host network are needed."""

    def __init__(self, container, database="postgres"):
        self.process = subprocess.Popen(
            ["docker", "exec", "-i", container, "psql", "-U", "postgres", "-d", database, "-X", "-qAt",
             "-v", "ON_ERROR_STOP=1"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True,
        )
        self.sequence = 0

    def sql(self, statement):
        self.sequence += 1
        marker = f"pwa_native_statement_{self.sequence}_complete"
        self.process.stdin.write(statement.rstrip().rstrip(";") + ";\n"
                                 + f"SELECT '{marker}';\n")
        self.process.stdin.flush()
        lines = []
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError("Local PostgreSQL statement failed: " + self.process.stderr.read())
            if line.rstrip("\n") == marker:
                return "".join(lines).strip()
            lines.append(line)

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write("\\q\n")
            self.process.stdin.flush()
        self.process.wait(timeout=10)
        self.process.stdin.close()
        self.process.stdout.close()
        self.process.stderr.close()


@unittest.skipUnless(os.environ.get("PIPING_SCHEMA_PROOF_CONTAINER"),
                     "requires an owned, disposable, networkless PostgreSQL 18.3 fixture")
class NativeHistoricalChecks(unittest.TestCase):
    def readiness(self, canonical, installed, **kwargs):
        return contract.canonical_readiness(canonical, installed,
            source_sha=contract.HISTORICAL_SOURCE_SHA, prefix_sha384=self.prefix_checksums,
            postgres_image=contract.HISTORICAL_POSTGRES_IMAGE, **kwargs)

    @classmethod
    def setUpClass(cls):
        cls.container = os.environ["PIPING_SCHEMA_PROOF_CONTAINER"]
        image = subprocess.check_output(["docker", "inspect", "--format", "{{.Image}}", cls.container], text=True).strip()
        if image != contract.HISTORICAL_POSTGRES_IMAGE:
            raise ValueError("Fixture is not the bound immutable PostgreSQL image")
        cls.session = NativeSession(cls.container)
        cls.session.sql("CREATE DATABASE pwa118_contract")
        cls.session.close()
        cls.session = NativeSession(cls.container, "pwa118_contract")
        cls.sql = cls.session.sql
        cls.sql("SET search_path=pg_catalog,public")
        cls.forms = contract.historical_check_forms()
        cls.bindings = {}
        cls.proofs = []
        # Read immutable source bytes, never the mutable worktree migration files.
        paths = subprocess.check_output(["git", "ls-tree", "-r", "--name-only",
                                         contract.HISTORICAL_SOURCE_SHA, "apps/api/migrations"], cwd=ROOT, text=True).splitlines()
        cls.prefix_checksums = {}
        for path in paths:
            if path.endswith(".sql") and int(pathlib.Path(path).name[:4]) <= 22:
                extracted = os.environ.get("PIPING_SCHEMA_PROOF_MIGRATIONS_DIR")
                raw = ((pathlib.Path(extracted) / pathlib.Path(path).name).read_bytes() if extracted else
                       subprocess.check_output(["git", "show", f"{contract.HISTORICAL_SOURCE_SHA}:{path}"], cwd=ROOT))
                cls.prefix_checksums[path] = hashlib.sha384(raw).hexdigest()
                cls.sql("BEGIN")
                cls.sql(raw.decode())
                cls.sql("COMMIT")
        cls.sql("BEGIN")
        for table in {name.rsplit(".", 1)[0] for name in cls.forms}:
            for binding in json.loads(cls.sql(contract.CHECK_BINDINGS_SQL % ("'" + table + "'"))):
                if binding["name"] in cls.forms:
                    cls.bindings[binding["name"]] = binding

    @classmethod
    def tearDownClass(cls):
        try:
            cls.sql("ROLLBACK")
        finally:
            cls.session.close()
            owner = NativeSession(cls.container)
            try:
                owner.sql("DROP DATABASE pwa118_contract")
            finally:
                owner.close()

    def test_all_twelve_source_forms_have_native_typed_equivalence(self):
        self.assertEqual(len(self.prefix_checksums), 22)
        self.assertEqual(set(self.bindings), set(self.forms))
        for name, form in self.forms.items():
            with self.subTest(name=name):
                self.assertEqual(self.bindings[name]["definition"], form.canonical)
                proof = contract.prove_native_check(self.sql, form, self.bindings[name],
                                                    source_sha=contract.HISTORICAL_SOURCE_SHA,
                                                    postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                                                    prefix_sha384=self.prefix_checksums)
                self.proofs.append(proof)
                self.assertTrue(proof.planned_expression)
        output = os.environ.get("PIPING_SCHEMA_PROOF_OUTPUT")
        if output:
            pathlib.Path(output).write_text(json.dumps({
                "source_sha": contract.HISTORICAL_SOURCE_SHA,
                "postgres_image": contract.HISTORICAL_POSTGRES_IMAGE,
                "postgres_version": self.sql("SHOW server_version"),
                "prefix_sha384": self.prefix_checksums,
                "proofs": [proof.receipt() for proof in self.proofs],
                "bindings": self.bindings,
                "scope": "native source forms; original installed AST not captured",
            }, indent=2) + "\n")

    def test_unknown_bindings_and_constraint_states_fail_closed(self):
        form = next(iter(self.forms.values()))
        original = self.bindings[form.name]
        variants = []
        for field, value in (("validated", False), ("enforced", False), ("no_inherit", True),
                             ("definition", form.historical.replace("'global'", "'other'")),
                             ("definition", form.historical[:-1] + " OR true)"),
                             ("ast", original["ast"].replace(":opno 98", ":opno 99")),
                             ("dependencies", ["function public.custom(text)"])):
            variant = json.loads(json.dumps(original))
            variant[field] = value
            variants.append(variant)
        for field, value in (("type", "text"), ("collation", "public.custom"),
                             ("deterministic", False), ("position", 1601)):
            variant = json.loads(json.dumps(original))
            variant["columns"][0][field] = value
            variants.append(variant)
        for binding in variants:
            with self.subTest(binding=binding), self.assertRaises(ValueError):
                contract.prove_native_check(self.sql, form, binding,
                                            source_sha=contract.HISTORICAL_SOURCE_SHA,
                                            postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                                            prefix_sha384=self.prefix_checksums)

    def test_native_membership_array_nodes_and_bound_readiness(self):
        for form in self.forms.values():
            if len(form.columns) != 1:
                continue
            name, data_type = form.columns[0]
            self.sql(f"CREATE TEMP TABLE original_binding ({name} {data_type}, CONSTRAINT observed {form.historical_sql})")
            try:
                binding = json.loads(self.sql(contract.CHECK_BINDINGS_SQL % "'pg_temp.original_binding'"))[0]
                binding["name"] = form.name
                self.assertEqual(binding["definition"], form.historical)
                proof = contract.prove_native_check(self.sql, form, binding,
                                                    source_sha=contract.HISTORICAL_SOURCE_SHA,
                                                    postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                                                    prefix_sha384=self.prefix_checksums)
                canonical = [obj("constraint", form.name, form.canonical)]
                installed = [obj("constraint", form.name, form.historical)]
                self.readiness(canonical, installed, check_proofs=[proof],
                                             installed_check_bindings=[binding])
                stale = json.loads(json.dumps(binding))
                stale["enforced"] = False
                with self.assertRaises(ValueError):
                    self.readiness(canonical, installed, check_proofs=[proof],
                                                 installed_check_bindings=[stale])
            finally:
                self.sql("DROP TABLE pg_temp.original_binding")

    def test_original_nested_and_requires_an_original_native_ast(self):
        form = self.forms["public.playbook_artifact.chk_playbook_artifact_size"]
        # PG18's parser flattens this DDL. A rendering alone cannot supply the
        # missing original node tree: deliberately do not authorize that case.
        binding = json.loads(json.dumps(self.bindings[form.name]))
        binding["definition"] = form.historical
        with self.assertRaisesRegex(ValueError, "typed AST"):
            contract.prove_native_check(self.sql, form, binding,
                                        source_sha=contract.HISTORICAL_SOURCE_SHA,
                                        postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                                        prefix_sha384=self.prefix_checksums)

    def test_registered_native_nested_association_and_tampering(self):
        form = self.forms["public.playbook_artifact.chk_playbook_artifact_size"]
        declarations = ",".join(f"{name} {data_type}" for name, data_type in form.columns)
        constraints = ", CONSTRAINT canonical " + form.canonical_sql
        constraints += "".join(f", CONSTRAINT proof_atom_{index} CHECK ({atom})"
                               for index, atom in enumerate(form.atoms))
        constraints += ", CONSTRAINT proof_pair CHECK (" + " AND ".join(form.atoms[:2]) + ")"
        self.sql("CREATE TEMP TABLE association_fixture (" + declarations + constraints + ")")
        try:
            references = json.loads(self.sql(contract.CHECK_BINDINGS_SQL % "'pg_temp.association_fixture'"))
            binding = next(row for row in references if row["name"].endswith(".canonical"))
            nested_ast = contract._associated_source_ast(references, form)
            binding["name"] = form.name
            binding["definition"] = form.historical
            binding["ast"] = nested_ast
            proof = contract.prove_native_check(self.sql, form, binding,
                                                source_sha=contract.HISTORICAL_SOURCE_SHA,
                                                postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                                                prefix_sha384=self.prefix_checksums)
            self.assertEqual(proof.association_truth_rows, 81)
            canonical = [obj("constraint", form.name, form.canonical)]
            installed = [obj("constraint", form.name, form.historical)]
            self.readiness(canonical, installed, check_proofs=[proof], installed_check_bindings=[binding])
            reconstructed = contract.NativeCheckProof(**proof.receipt())
            with self.assertRaises(ValueError):
                self.readiness(canonical, installed, check_proofs=[reconstructed], installed_check_bindings=[binding])
            for old, new in ((":boolop and", ":boolop or"), (":opfuncid", ":unknownfuncid"),
                             (":constisnull false", ":constisnull true")):
                altered = dict(binding, ast=binding["ast"].replace(old, new, 1))
                with self.assertRaises(ValueError):
                    contract.prove_native_check(self.sql, form, altered,
                                                source_sha=contract.HISTORICAL_SOURCE_SHA,
                                                postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                                                prefix_sha384=self.prefix_checksums)
        finally:
            self.sql("DROP TABLE pg_temp.association_fixture")

    @unittest.skipUnless(os.environ.get("PIPING_ORIGINAL_CHECK_CAPTURE"),
                         "requires a source-authenticated private original CHECK capture")
    def test_original_capture_is_bound_to_actual_candidate(self):
        self.assertTrue(os.environ.get("PIPING_SCHEMA_PROOF_MIGRATIONS_DIR"), "requires image-extracted SQL")
        raw = pathlib.Path(os.environ["PIPING_ORIGINAL_CHECK_CAPTURE"]).read_bytes()
        trusted = os.environ["PIPING_ORIGINAL_CHECK_CAPTURE_SHA256"]
        source = "5e4b78bfe647ea55829f24992b944cd2a8392172"
        kwargs = dict(source_sha=source, prefix_sha384=self.prefix_checksums,
                      postgres_image=contract.HISTORICAL_POSTGRES_IMAGE)
        proofs = contract.prove_original_native_checks(self.sql, raw, trusted_capture_sha256=trusted, **kwargs)
        capture = json.loads(raw)
        self.assertEqual(len(proofs), 12)
        self.assertTrue(all(p.source_sha == source and p.capture_hash == trusted for p in proofs))
        canonical = [obj("constraint", name, form.canonical) for name, form in self.forms.items()]
        installed = [obj("constraint", binding["name"], binding["definition"]) for binding in capture["bindings"]]
        contract.canonical_readiness(canonical, installed, check_proofs=proofs,
                                    installed_check_bindings=capture["bindings"], **kwargs)
        for wrong in ({"source_sha": contract.HISTORICAL_SOURCE_SHA},
                      {"source_sha": "ccb216481bcbc4d54315205269753e5d9d276d29"},
                      {"source_sha": "70254dc18cd36e8717de88284ba7456a12f50742"},
                      {"source_sha": "96c9833e6a4cfdee0eff9c206fcd2cbee9e8dd53"}, {"source_sha": "f" * 40},
                      {"prefix_sha384": {}}, {"postgres_image": "sha256:" + "0" * 64}):
            with self.subTest(wrong=wrong), self.assertRaises(ValueError):
                contract.canonical_readiness(canonical, installed, check_proofs=proofs,
                    installed_check_bindings=capture["bindings"], **dict(kwargs, **wrong))
        reconstructed = contract.NativeCheckProof(**proofs[0].receipt())
        with self.assertRaises(ValueError):
            contract.canonical_readiness(canonical, installed, check_proofs=(reconstructed, *proofs[1:]),
                                        installed_check_bindings=capture["bindings"], **kwargs)
        import copy
        mixed = copy.copy(proofs[0])
        object.__setattr__(mixed, "source_sha", contract.HISTORICAL_SOURCE_SHA)
        with self.assertRaises(ValueError):
            contract.canonical_readiness(canonical, installed, check_proofs=(mixed, *proofs[1:]),
                                        installed_check_bindings=capture["bindings"], **kwargs)
        output = os.environ.get("PIPING_SCHEMA_PROOF_OUTPUT")
        if output:
            pathlib.Path(output).write_text(json.dumps({"source": source, "checks": len(proofs),
                "prefix_sha256": contract.definition_hash(self.prefix_checksums),
                "postgres_image": contract.HISTORICAL_POSTGRES_IMAGE, "capture_sha256": trusted,
                "actual_source_preserved": True, "wrong_plan_mixed_source_reconstructed_proof_denied": True,
                "native_original_capture": "PASS"}, indent=2))

    @unittest.skipUnless(os.environ.get("PIPING_ORIGINAL_CHECK_CAPTURE"),
                         "requires a source-authenticated private original CHECK capture")
    def test_original_target_twelve_native_bindings(self):
        raw = pathlib.Path(os.environ["PIPING_ORIGINAL_CHECK_CAPTURE"]).read_bytes()
        trusted_hash = os.environ["PIPING_ORIGINAL_CHECK_CAPTURE_SHA256"]
        kwargs = dict(source_sha=contract.HISTORICAL_SOURCE_SHA,
                      postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                      prefix_sha384=self.prefix_checksums)
        proofs = contract.prove_original_native_checks(self.sql, raw, trusted_capture_sha256=trusted_hash, **kwargs)
        capture = json.loads(raw)
        self.assertEqual({proof.name for proof in proofs}, set(self.forms))
        self.assertTrue(all(proof.capture_hash == trusted_hash for proof in proofs))
        canonical = [obj("constraint", name, form.canonical) for name, form in self.forms.items()]
        installed = [obj("constraint", binding["name"], binding["definition"]) for binding in capture["bindings"]]
        self.readiness(canonical, installed, check_proofs=proofs, installed_check_bindings=capture["bindings"])
        # Each mutation is hypothetically source-approved to exercise validation
        # beyond the digest boundary. No altered private fixture is written.
        mutations = (
            ("dependencies", 0, "refobjsubid", -1),
            ("dependencies", 0, "refobjid", "0"),
            ("bindings", 0, "validated", False),
            ("objects.functions", 0, "strict", False),
            ("objects.operators", 0, "function", "public.unknown(text,text)"),
            ("casts", 0, "method", "b"),
        )
        for path, index, field, value in mutations:
            changed = json.loads(raw)
            destination = changed
            for key in path.split("."):
                destination = destination[key]
            destination[index][field] = value
            altered = json.dumps(changed).encode()
            with self.subTest(path=path, field=field), self.assertRaises(ValueError):
                contract.prove_original_native_checks(self.sql, altered,
                    trusted_capture_sha256=hashlib.sha256(altered).hexdigest(), **kwargs)
        with self.assertRaises(ValueError):
            contract.prove_original_native_checks(self.sql, raw, trusted_capture_sha256="0" * 64, **kwargs)
        for old, new in ((":boolop and", ":boolop or"), (":constisnull false", ":constisnull true")):
            changed = json.loads(raw)
            name = "public.playbook_artifact.chk_playbook_artifact_size"
            original = next(row for row in changed["original"] if row["name"] == name)
            binding = next(row for row in changed["bindings"] if row["name"] == name)
            original["original_conbin"] = original["original_conbin"].replace(old, new, 1)
            binding["ast"] = self.sql("SELECT regexp_replace(" + contract._sql_literal(original["original_conbin"])
                + r", ':(location|list_start|list_end) -?[0-9]+', ':\1 -1', 'g')")
            altered = json.dumps(changed).encode()
            with self.subTest(changed_node=old), self.assertRaises(ValueError):
                contract.prove_original_native_checks(self.sql, altered,
                    trusted_capture_sha256=hashlib.sha256(altered).hexdigest(), **kwargs)
        output = os.environ.get("PIPING_ORIGINAL_CHECK_PROOF_OUTPUT")
        if output:
            pathlib.Path(output).write_text(json.dumps({
                "count": len(proofs), "source_sha": contract.HISTORICAL_SOURCE_SHA,
                "prefix_hash": contract.HISTORICAL_PREFIX_HASH,
                "postgres_image": contract.HISTORICAL_POSTGRES_IMAGE,
                "trusted_capture_sha256": trusted_hash,
                "proofs": [proof.receipt() for proof in proofs],
                "scope": "all original CHECK bindings; no helper wiring or target deployment",
            }, indent=2) + "\n")

    def test_native_unknown_operator_branch_and_check_flags(self):
        form = self.forms["public.secrets_variable.chk_secrets_variable_kind"]
        self.sql("CREATE FUNCTION public.untrusted_equal(text,text) RETURNS boolean "
                 "LANGUAGE sql IMMUTABLE AS 'SELECT true'")
        self.sql("CREATE OPERATOR public.= (FUNCTION=public.untrusted_equal, LEFTARG=text, RIGHTARG=text)")
        checks = (
            "CHECK ((kind)::text OPERATOR(public.=) ANY (ARRAY['field','named']))",
            "CHECK (kind IN ('field','named') OR kind IS NULL)",
            "CHECK (kind IN ('field','changed'))",
            form.historical_sql + " NOT ENFORCED",
            form.historical_sql + " NOT VALID",
        )
        for check in checks:
            self.sql("CREATE TEMP TABLE unsupported_binding (kind varchar(32))")
            try:
                self.sql("ALTER TABLE pg_temp.unsupported_binding ADD CONSTRAINT observed " + check)
                binding = json.loads(self.sql(contract.CHECK_BINDINGS_SQL % "'pg_temp.unsupported_binding'"))[0]
                binding["name"] = form.name
                with self.assertRaises(ValueError):
                    contract.prove_native_check(self.sql, form, binding,
                                                source_sha=contract.HISTORICAL_SOURCE_SHA,
                                                postgres_image=contract.HISTORICAL_POSTGRES_IMAGE,
                                                prefix_sha384=self.prefix_checksums)
            finally:
                self.sql("DROP TABLE pg_temp.unsupported_binding")
        self.sql("DROP OPERATOR public.= (text,text)")
        self.sql("DROP FUNCTION public.untrusted_equal(text,text)")

    def test_membership_three_valued_semantics_and_complement(self):
        for form in self.forms.values():
            if len(form.columns) != 1:
                continue
            column, data_type = form.columns[0]
            canonical = form.canonical[len("CHECK "):]
            historical = form.historical[len("CHECK "):]
            members = list(form.members)
            values = list(dict.fromkeys(members + ["", "outside", "global ", "GLOBAL", "pull,push"]
                                         + [value.upper() for value in members]))
            tuples = ",".join("('" + value + "')" for value in values) + ",(NULL)"
            result = self.sql(f"WITH inputs(v) AS (VALUES {tuples}), typed AS (SELECT v::{data_type} AS {column} FROM inputs) "
                              f"SELECT count(*) FROM typed WHERE ({canonical}) IS DISTINCT FROM ({historical})")
            self.assertEqual(result, "0", form.name)
            member_literals = ",".join("'" + value + "'" for value in members)
            expected = f"CASE WHEN {column} IS NULL THEN NULL WHEN {column} IN ({member_literals}) THEN true ELSE false END"
            self.assertEqual(self.sql(f"WITH inputs(v) AS (VALUES {tuples}), typed AS (SELECT v::{data_type} AS {column} FROM inputs) "
                                      f"SELECT count(*) FROM typed WHERE ({canonical}) IS DISTINCT FROM ({expected})"), "0")
            self.assertEqual(self.sql(f"SELECT ({canonical}) IS NULL FROM (SELECT NULL::{data_type} AS {column}) s"), "t")
        self.assertEqual(self.sql("SELECT jsonb_build_array(NULL::text = ANY(ARRAY[NULL::text]), "
                                  "'outside'::text = ANY(ARRAY[NULL::text]), NULL::text = ANY(ARRAY[]::text[]))"),
                         '[null, null, false]')

    def test_artifact_and_association_all_81_truth_combinations(self):
        self.assertEqual(self.sql("WITH truth(v) AS (VALUES(true),(false),(NULL::boolean)) "
                                  "SELECT count(*) FROM truth a CROSS JOIN truth b CROSS JOIN truth c CROSS JOIN truth d "
                                  "WHERE ((a.v AND b.v) AND c.v AND d.v) IS DISTINCT FROM (a.v AND b.v AND c.v AND d.v)"), "0")
        form = self.forms["public.playbook_artifact.chk_playbook_artifact_size"]
        canonical, historical = form.canonical[len("CHECK "):], form.historical[len("CHECK "):]
        result = self.sql("WITH sizes(v) AS (VALUES(NULL::bigint),(-9223372036854775808),(-1),(0),(1),(2097152),"
                          "(2097153),(9223372036854775807)), texts(v) AS (VALUES(NULL::text),(''),('é'),"
                          "(repeat('é',1048576)),(repeat('é',1048577))), jsons(v) AS (VALUES(NULL::jsonb),"
                          "('null'::jsonb),('{}'::jsonb),('{\"unicode\":\"é\"}'::jsonb),"
                          "(to_jsonb(repeat('é',1048576)))), inputs AS (SELECT sizes.v AS content_size_bytes,"
                          "texts.v AS content_text,jsons.v AS content_json FROM sizes CROSS JOIN texts CROSS JOIN jsons) "
                          f"SELECT count(*) FROM inputs WHERE ({canonical}) IS DISTINCT FROM ({historical})")
        self.assertEqual(result, "0")
        self.assertEqual(self.sql("SELECT jsonb_build_array(octet_length('é'), octet_length(to_jsonb('é'::text)::text),"
                                  "octet_length('{\"a\":1}'::jsonb::text))"), '[2, 4, 8]')


if __name__ == "__main__":
    unittest.main()
