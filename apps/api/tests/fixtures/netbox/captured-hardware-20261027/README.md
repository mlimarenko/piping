# Captured hardware serializer maps

Synthetic data captured from NetBox 4.7.1-Docker-5.1.1 on 2026-10-27.
`authentic-census.json` records the collection counts and pagination closure.
The 16 primary collections contain 21 objects; `interfaces` adds four complete
ancillary rows. No serializer fields were projected out or rewritten.

`authentic/full-*-page-0.json` and `options/*.json` are byte-for-byte copies of
the captured response files. The capture client decoded and re-encoded complete
JSON maps; these are not claimed to be the original HTTP wire bytes.
`authentic-artifact-sha256.json` is the original capture
manifest; entries outside this fixture directory describe other capture artifacts.
The three prerequisite GET responses are preserved as source evidence, not
additional supported importer models. All data is synthetic.

The product tests feed the complete hardware and Interface maps into production
preparation. Native planner coverage requires an explicitly selected PostgreSQL
connection and plans only; it does not apply inventory rows or run migrations.

Cable arrays are reverse assertions checked against full Interface rows. The
Interface `cable` and `cable_end` fields remain the only endpoint writer. A side
omitted from a Cable imposes no reverse assertion. An explicit empty side must
agree with the supplied forward rows; detachment is written only by an explicit
Interface `cable: null` and `cable_end: null` pair. Reverse-only endpoint snapshots
are unsupported. Selected reverse sides survive preparation into native graph
preflight. Every existing association on a selected side must have a consistent
explicit planned Interface mutation. Unseen members, stale extras, ambiguous
native sides and unsupported native endpoint kinds block the operation. Empty
sides never clear foreign or unseen endpoints implicitly; callers must supply
the forward Interface cohort when detaching endpoints. Omitted sides impose no
new target assertion.
