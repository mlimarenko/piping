# Inventory template run references

`inventory-template-references.json` owns the reference targets and field
destinations shared by the API resolver and template editor. Targets are
independent of the resources a template step can create; for example,
`contact_role` is selectable but is not a step creation resource.

A selected object is supplied as a variable value with exactly two properties:

```json
{"resource":"device_type","id":"00000000-0000-4000-8000-000000000001"}
```

Preview and run validate the resource token, UUID, all consuming bindings and
object existence before accepting the selection. A name consumer such as
`device.device_type` projects it to `device_type_id`; an ID consumer receives
the UUID directly. Conflicting name/ID inputs are rejected. The original
structured input remains in the recorded run variables.

Selections cannot be interpolated into text, creation names, repeat controls
or incompatible reference fields. Ordinary variables declared `json` remain
JSON, including objects with `resource` and `id` properties.

Literal/manual name or slug strings retain their meaning. Execution performs
exact name/slug lookup and rejects ambiguous matches. Preview does not create
resources or resolve deferred creation-dependent names: a site name reused
for site creation and a later prefix lookup remains text. Existing saved
factory templates require no migration or payload rewrite.

The run dialog requires a successful server preview before enabling execution.
That preview is not a transaction reservation: deletion after preview can
still fail the next run's existence check or native foreign-key constraint.

## Repeated object inputs

A `json` variable can declare `validation_rules.fields`, a nonempty array of
variable definitions (`key`, `label`, `value_type`, `required`, `validation_rules`,
and optional `default_value` and `description`). The run form renders an editable
list with add/remove controls. An optional empty list is `[]`. Nested field keys
are unique ASCII identifiers; nested object-list declarations and prototype keys
are rejected. Undeclared row properties are rejected rather than discarded.
Defaults apply only to missing properties. Explicit optional nulls and blank
strings remain supplied values, so clearing an input cannot restore its default.
Empty number, boolean and plain JSON controls submit null; literal JSON `[]`
remains an ordinary JSON value.

Each nested `resource_ref` declares a canonical `validation_rules.resource` and
accepts a UUID string. Its target existence is checked before a run writes any
resources. Top-level structured selections retain the contract above. String
options use `validation_rules.options`, an array of trimmed strings parsed as
the declared variable type (for example, integer options `["1", "2"]` accept
the submitted number `2`). Nested fields require an explicit `value_type`.
Numeric bounds use `min` and `max`;
`format: "email"` validates optional nonempty email fields.

A step iterates the supplied rows with:

```json
{
  "repeat": {"for_each": "{{contacts}}", "item_token": "contact"},
  "payload_template": {
    "contact_id": "{{contact.contact_id}}",
    "role_id": "{{contact.role_id}}"
  }
}
```

`for_each` accepts an array or a whole variable token, with at most 500 items.
`item_token` defaults to `item`. This form cannot contain `count`, `start` or
`index_token`. Empty arrays create no resources and satisfy dependencies.
Positions remain one-based, including `steps.<step>.current.id` when a later
loop binds resources created at matching positions. Count-based repeats retain
their existing behavior; position overflow is rejected.

Input validation failures use the canonical `VALIDATION_ERROR` envelope with
`details.fields`, for example `{"contacts.0.role_id":"required"}`. Field paths
use zero-based dotted row indices; codes contain no supplied values. Preview
and run apply the same type, option and collection checks. An authoring preview
may omit top-level required values; supplied collection rows must be complete.
