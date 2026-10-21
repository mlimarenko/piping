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
