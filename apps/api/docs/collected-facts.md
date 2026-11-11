# Collected facts field contract

`POST /api/v1/inventory/collected-facts` is the only path that writes host
observations into the typed inventory. This document is the field matrix for
that path: which entity and field an observation reaches, the identity it is
matched by, the authority required, and what happens when a value is missing.
The request structs and SQL in `collected_facts_repository.rs` remain the
implementation source of truth; change the matrix with them.

## Owners

| Concern | Owner |
| --- | --- |
| Entry point, job-token requirement, Create+Update check | `apps/api/src/api/rest/routes/inventory/native/collected_facts.rs:39-79` |
| Request/response structs, write policy, write path | `apps/api/src/contexts/inventory/infrastructure/persistence/repositories/native/collected_facts_repository.rs` |
| Collector | `ansible/playbooks/system/auto-update-inventory.yml` |
| Typed launch flags | `ansible/playbooks/system/auto-update-inventory.vars-schema.json` |
| Catalog entry and token scopes | `ansible/playbooks/piping-playbooks.yaml:158-211` |
| Model columns | `apps/api/migrations/0001_init.sql` |
| Display units | `apps/web/src/shared/i18n/locales/en.ts:2765-2850` |

## Identity and authority

The request carries `inventory_kind` (`device` or `virtual_machine`) and
`inventory_id`; that pair identifies the object to update. `connection_target_id`,
`run_id`, `run_group_id`, `source_kind` and `source_id` scope the run.
`target_name` is a label only and is never used for matching: a host name does
not select, create or classify an object.

The request must be authenticated with a playbook job token: audience
`sandbox`, active, not revoked, not expired, scoped to the same run, run group,
connection target, source kind and source id
(`collected_facts_repository.rs:260-301`). The caller must also hold Create and
Update on the Inventory resource (`collected_facts.rs:54-60`). The catalog
grants the collector `inventory:read`, `inventory:create` and
`inventory:update`. Host-side fact gathering uses the run's connection
credentials; those credentials grant no additional inventory authority.

## Matrix

`Source` is the collector group that supplies the value. `Identity` is what the
write is matched by. `Flag` is the `write_policy` group that must be enabled;
all flags default to `true`. `Missing` is what the path does when the value is
absent, empty or a sentinel.

### Device and virtual machine

| Entity.field | Source | Identity | Value / unit | Update rule | Missing |
| --- | --- | --- | --- | --- | --- |
| `dcim_device.platform_id` | Linux distribution, Windows distribution | platform slug or name, case-insensitive | platform reference | set when a platform resolves; `write_platform` | no platform → unchanged |
| `dcim_device.device_type_id` | hardware manufacturer and model | `(manufacturer, model slug)` unique | device-type reference | created and set when model is present; `write_device_type`; devices only | no model → unchanged |
| `dcim_device.serial` | hardware serial, else product UUID | object id | text, 50 chars | set when not a placeholder; `write_serial` | placeholder or empty → unchanged |
| `dcim_device.local_context_data.ansible` | `raw_facts` plus run metadata | object id | JSON object | merged under `ansible`; `write_ansible_custom_fields` | absent → unchanged |
| `virtualization_virtualmachine.platform_id` | Linux distribution, Windows distribution | platform slug or name, case-insensitive | platform reference | set when a platform resolves; `write_platform` | no platform → unchanged |
| `virtualization_virtualmachine.serial` | hardware serial, else product UUID | object id | text, 50 chars | set when not a placeholder; `write_serial` | placeholder or empty → unchanged |
| `virtualization_virtualmachine.vcpus` | hardware CPU count | object id | count | set when greater than zero; `write_resources` | zero, negative or absent → unchanged |
| `virtualization_virtualmachine.memory` | hardware memory | object id | MB | set when greater than zero; `write_resources` | zero, negative or absent → unchanged |
| `virtualization_virtualmachine.disk` | hardware total disk | object id | GB | set when greater than zero; `write_resources` | zero, negative or absent → unchanged |
| `virtualization_virtualmachine.local_context_data.ansible` | `raw_facts` plus run metadata | object id | JSON object | merged under `ansible`; `write_ansible_custom_fields` | absent → unchanged |

Device CPU, memory and disk totals have no device column in the model. They are
not written; see the type-specific mapping.

### Platform and device type

| Entity.field | Source | Identity | Update rule | Missing |
| --- | --- | --- | --- | --- |
| `dcim_platform.name`, `.slug` | OS distribution and version | slug or name, case-insensitive | find or create once per slug; `write_platform` | empty name → no platform |
| `dcim_manufacturer.name`, `.slug` | hardware manufacturer | slug or name, case-insensitive | find or create; `write_device_type` | model without manufacturer → `Unknown` |
| `dcim_devicetype.model`, `.slug` | hardware model | `(manufacturer, model slug)` unique | find or create; `write_device_type` | empty model → no device type |

### Interfaces

| Entity.field | Source | Identity | Value | Update rule | Missing |
| --- | --- | --- | --- | --- | --- |
| `dcim_interface.name` | interface name | `(device_id, name)` unique | text, 64 chars | upsert; `write_interfaces` | empty name or `lo` → skipped |
| `dcim_interface.label` | interface device name | same | text | overwrite | empty → empty string |
| `dcim_interface.description` | collector text | same | text | overwrite | empty → empty string |
| `dcim_interface.enabled` | interface active flag | same | boolean | overwrite | absent → `true` |
| `dcim_interface.type` | interface type | same | choice | overwrite | absent → `other` |
| `dcim_interface.mtu` | interface MTU | same | bytes | write when present; `write_interfaces` | absent → keep stored value |
| `virtualization_vminterface.name` | interface name | `(virtual_machine_id, name)` unique | text, 64 chars | upsert; `write_interfaces` | empty name or `lo` → skipped |
| `virtualization_vminterface.description` | collector text | same | text | overwrite | empty → empty string |
| `virtualization_vminterface.enabled` | interface active flag | same | boolean | overwrite | absent → `true` |
| `virtualization_vminterface.mtu` | interface MTU | same | bytes | overwrite | absent → NULL |

The device write path persists `mtu` and keeps the stored value when the
observation omits it. The VM interface has no `type` or `label` column and its
path does not persist them.

### IP addresses

| Entity.field | Source | Identity | Value | Update rule | Missing |
| --- | --- | --- | --- | --- | --- |
| `ipam_ipaddress.address` | every observed interface address, IPv4 and IPv6 in `interfaces[].addresses` | address, `inet` unique | CIDR with prefix | canonicalized and deduplicated per interface at the request boundary, then upserted and attached; status `active`; `write_ip_addresses` | no address for an interface → that address is not written; an invalid address rejects the request |
| `ipam_ipaddress.description` | collector text | same | text | set only when empty | — |
| `dcim_device.primary_ip4_id`, `virtualization_virtualmachine.primary_ip4_id` | every IPv4 candidate across the object's interfaces | object id | IP reference | set only when empty and exactly one IPv4 candidate exists for the object | two or more candidates, or an existing primary → unchanged |
| `primary_ip6_id` | — | — | — | not set by this path | — |

An address already attached to another interface is not reassigned; the request
reports it as a conflict.

### MAC addresses

| Entity.field | Source | Identity | Value | Update rule | Missing |
| --- | --- | --- | --- | --- | --- |
| `dcim_macaddress.mac_address` | interface MAC | MAC address unique | six colon-separated octets | upsert and attach to the interface; `write_mac_addresses` | invalid or empty → skipped |
| `dcim_interface.primary_mac_address_id`, `virtualization_vminterface.primary_mac_address_id` | interface MAC | interface id | MAC reference | set on every successful upsert | — |

### Virtual disks

| Entity.field | Source | Identity | Value | Update rule | Missing |
| --- | --- | --- | --- | --- | --- |
| `virtualization_virtualdisk.name` | mount point, else device | `(virtual_machine_id, name)` unique | text, 64 chars | upsert; virtual machines only; `write_virtual_disks` | empty name → skipped |
| `virtualization_virtualdisk.size` | mount size | same | GB | set from the observation | absent → `0` (column is `NOT NULL`) |
| `virtualization_virtualdisk.description` | backing device | same | text | overwrite | empty → empty string |

## Update, ownership and missing semantics

`write_policy` remains the group-level switch. A group flag set to `false` is
the manual-protection switch: the run leaves that group untouched.

On top of that, every object-level field this path writes records its own
provenance in `inventory_collected_field`: the run source (`source_kind` is
`playbook_run` with the run id, or `run_group` for an internal no-token call),
the observation time and the value the collector last wrote. Before a write, the
path compares the current stored value with that recorded value:

- equal and the new observation is newer → the collector owns the value and
  writes it;
- different → a person edited the field after the last collection. The run
  leaves it unchanged and reports the group as skipped with reason
  `manual_override`;
- the new observation is not newer than the recorded one → the run leaves the
  value unchanged and reports `stale`.

The response reports one outcome per group: `success`, `skipped` (with a
reason such as `write_policy_disabled`, `no_data`, `not_applicable`,
`manual_override` or `stale`) or `error`. A failed group is rolled back to its
savepoint and reported as `error`; the other groups still commit, so a partial
result is visible instead of a false success. `observed_at` is the effective
observation time stored with the accepted fields.

Field-level ownership currently covers the object fields: platform, device
type, serial, VM resources and the `ansible` context. Interface fields keep the
normalization below until the stable-identity work (PIP-552) extends ownership
to them.

- An absent object-level field (platform, device type, serial, resources, extra
  facts) never clears or deletes an existing value. Interface `label`,
  `description`, `type` and `enabled` are normalized instead: an absent value
  becomes an empty string, `other` or `true` as the matrix states, so those
  four can overwrite a manual value.
- Nothing in this path deletes an object or marks it absent. Skipped rows and
  conflicts are counted in the response (`skipped_count`, `conflict_count`); a
  failed or skipped collection sends no request and proves nothing about
  absence.
- Missing is distinct from `0`, empty and `null` per the `Missing` column,
  except where the matrix names a normalization (interface label, description,
  type, enabled). An absent virtual-disk size is recorded in provenance with
  `absent = true` and the value currently stored; the column keeps its
  `NOT NULL` NetBox-mirror contract, a stored `0` from an absent observation is
  not an observed zero, and a later manual edit is still protected because the
  recorded value is what ownership is compared against.
- Repeated identical runs are idempotent for object fields and upsert keys;
  counters may count the same rows again.
- An older observation does not overwrite a newer one: the recorded
  `observed_at` makes an out-of-order run skip with reason `stale`.
- Renaming an interface or a virtual disk creates a new row; the old row stays.
  Stable identity for renames is PIP-552.
- Business attributes — site, tenant, role, cluster, status, name, asset tag,
  rack and virtual-machine type — are never inferred from host names or facts.

## Type applicability

| Observation | Device | Virtual machine |
| --- | --- | --- |
| OS platform | `platform_id` | `platform_id` |
| Manufacturer and model | `device_type_id` | not applicable |
| Serial / product UUID | `serial` | `serial` |
| CPU count | no column; not written | `vcpus` |
| Memory | no column; not written | `memory` |
| Total disk | no column; not written | `disk` |
| Virtual disks | not applicable | `virtualization_virtualdisk` |
| Interfaces, IP, MAC | `dcim_interface` | `virtualization_vminterface` |
| Extra facts | `local_context_data.ansible` | `local_context_data.ansible` |

OS name and version map to `dcim_platform`; there is no separate OS-version
entity. Physical disks have no universal entity in the model and are not
collected into one. The virtual-machine type is never inferred.

## Type-specific mapping and units (PIP-550)

The matrix above fixes identity, authority and ownership. This section fixes
the model column, unit and current-versus-intended behaviour for each object
type. It adds no field, table or relation.

### Units: model contract

| Value | Payload unit | Model column | Model unit | Required conversion |
| --- | --- | --- | --- | --- |
| `hardware.cpu_count` | count | `virtualization_virtualmachine.vcpus` (`numeric(6,2)`) | count | none |
| `hardware.memory_mb` | MB | `virtualization_virtualmachine.memory` (`integer`) | MB | none |
| `hardware.disk_mb` | MB | `virtualization_virtualmachine.disk` (`integer`) | GB | `floor(mb / 1024)` |
| `virtual_disks[].size_mb` | MB | `virtualization_virtualdisk.size` (`integer`) | GB | `floor(mb / 1024)` |
| `interfaces[].mtu` | bytes | `dcim_interface.mtu`, `virtualization_vminterface.mtu` | bytes | none |
| `interfaces[].addresses[]` | CIDR | `ipam_ipaddress.address` (`inet`) | CIDR | none |
| `interfaces[].mac` | text | `dcim_macaddress.mac_address` (`macaddr`) | six octets | none |

The model units follow the NetBox mirror and the product UI: `memory` is MB,
`disk` and virtual-disk `size` are GB
(`apps/web/src/shared/i18n/locales/en.ts:2765-2850`; the NetBox fixture carries
`memory: 4096`, `disk: 50`).

### Units: implemented conversion

The ingest converts `hardware.disk_mb` and `virtual_disks[].size_mb` from the
payload's megabytes to the model's gigabytes with `floor(mb / 1024)`
(`collected_facts_repository.rs`, `mb_to_gb`). A 100 GB disk arrives as
`102400` MB and is stored as `100`. `hardware.memory_mb` is already MB and is
stored unchanged. The `VirtualDisk` domain comment that called `size`
megabytes (`apps/api/src/contexts/inventory/domain/virtualization/virtual_disk.rs:10`)
is corrected to gigabytes with the same change.

### Device mapping

| Observation | Target | Stored | Note |
| --- | --- | --- | --- |
| OS name and version | `dcim_platform` through `dcim_device.platform_id` | yes | slug derived, created when absent |
| Manufacturer and model | `dcim_manufacturer`, `dcim_devicetype` through `device_type_id` | yes | devices only |
| Serial or product UUID | `dcim_device.serial` | yes | placeholder filtered |
| CPU count | — | no | no device column |
| Memory | — | no | no device column |
| Total disk | — | no | no device column |
| Physical disks | — | no | no universal entity; do not add one |
| Interfaces | `dcim_interface` name, label, description, enabled, type, mtu | yes | `mtu` is written when present; an omitted value keeps the stored one |
| IP and MAC | `ipam_ipaddress`, `dcim_macaddress` | yes | primary IPv4 only when empty |
| Extra facts | `dcim_device.local_context_data.ansible` | yes | curated raw facts |

### Virtual machine mapping

| Observation | Target | Stored | Note |
| --- | --- | --- | --- |
| OS name and version | `dcim_platform` through `platform_id` | yes | slug derived, created when absent |
| Serial or product UUID | `virtualization_virtualmachine.serial` | yes | placeholder filtered |
| CPU count | `virtualization_virtualmachine.vcpus` | yes | greater than zero |
| Memory | `virtualization_virtualmachine.memory` | yes | MB |
| Total disk | `virtualization_virtualmachine.disk` | yes | GB, converted |
| Virtual disks | `virtualization_virtualdisk` name, description, size | yes | GB, converted |
| Interfaces | `virtualization_vminterface` name, description, enabled, mtu | yes | the table has no `type` or `label` column; do not promise them |
| IP and MAC | `ipam_ipaddress`, `dcim_macaddress` | yes | primary IPv4 only when empty |
| Extra facts | `virtualization_virtualmachine.local_context_data.ansible` | yes | curated raw facts |
| Manufacturer and model | — | no | `virtual_machine_type_id` is not inferred |

### Not modeled — do not invent

Device CPU, memory and disk totals, physical disks, OS version beyond the
platform name, a VM device type, and the VM type are not model fields for this
path. Site, tenant, role, cluster, status, name, asset tag and rack are business
attributes and are never derived from facts or host names.

### Partial and missing

- A partial payload updates only the groups it carries; a group with no data
  leaves existing values.
- `0` or negative CPU, memory and disk values are treated as missing and are
  not written.
- Empty or placeholder serial is not written.
- Invalid MAC and CIDR values are skipped and counted.
- Intended: an absent virtual-disk size remains distinguishable from an
  observed zero. Current: `virtualization_virtualdisk.size` is `NOT NULL` and
  the ingest inserts `0` when `size_mb` is absent
  (`collected_facts_repository.rs:993-1025`). PIP-551 owns the absence
  representation; do not treat the stored `0` as an observed zero.
- A failed or skipped host sends no request; no absence is recorded.

### Handoff

PIP-551 and PIP-554 implement provenance, ownership, ordering and the unit and
absence corrections on this same matrix and the same
`POST /api/v1/inventory/collected-facts` path. There is no second API, schema or
collector.

## Source providers: Linux and Windows

The collector selects the fact provider from the gathered machine fact
`ansible_facts.os_family`, not from the connection: WinRM, PSRP and Windows
OpenSSH take the same Windows path, and the transport is recorded in
`raw_facts` only. A first bounded gather (`platform`, `distribution`) identifies
the OS; the collector then runs the matching provider with explicit subsets, so
`env` is never collected on either family.

| Provider | Subsets | Keys the payload reads |
| --- | --- | --- |
| `ansible.builtin.setup` (Linux) | `distribution`, `hardware`, `network`, `virtual` | `distribution`, `distribution_version`, `distribution_major_version`, `system_vendor`, `product_name`, `product_serial`, `product_uuid`, `bios_version`, `processor_vcpus`, `memtotal_mb`, `mounts`, `interfaces` and the per-interface `device`, `active`, `mtu`, `macaddress`, `ipv4`, `ipv4_secondaries`, `ipv6` |
| `ansible.windows.setup` (Windows) | `platform`, `distribution`, `hardware`, `interfaces` | the same hardware and address keys plus `os_product_type`, `os_installation_type`, `architecture2` |

Windows keys and units the payload depends on:

| Key | Source subset | Use |
| --- | --- | --- |
| `os_family` | distribution | OS selection; recorded as `connection_family` |
| `os_product_type`, `os_installation_type` | distribution | machine product codes in the platform identity and `raw_facts` |
| `distribution_version`, `distribution_major_version`, `kernel` | distribution, platform | Windows build; `kernel` carries the same build string |
| `distribution`, `os_name` | distribution | localized caption; never a platform slug |
| `architecture2` | platform | stable architecture code; the localized `architecture` is not used |
| `system_vendor`, `product_name`, `product_serial`, `product_uuid`, `bios_version` | platform | manufacturer, model, serial, product UUID, BIOS |
| `processor_vcpus` | hardware | vCPU count; `processor_cores` and `processor_count` are cores and sockets, not vcpus |
| `memtotal_mb` | hardware | memory already in MB, written unchanged |
| `interfaces[]` | interfaces | interface objects with `connection_name`, `interface_name`, `macaddress`, `mtu` and the provider `ipv4`/`ipv6` address lists that the projection turns into `addresses` |

`ansible.windows.setup` has no disk or mount fact. Windows storage comes from
`ansible.windows.win_disk_info` (`filter: [partitions, volumes]`, read-only,
pinned collection, PowerShell >= 5.1): `disks[].size` is the physical disk size
in bytes and the logical volumes sit at
`disks[].partitions[].volumes[]` with `drive_letter`, `size` (bytes), `label`,
`type` and `path`. The module needs an elevated administrator account; without
one the run records the `windows_disks` gap and sends no disk value.

Units: the payload carries bytes as MB (`// 1048576`) and the ingest converts
the megabytes once to the model's gigabytes (`floor(mb / 1024)`, see the units
section). Physical disks and logical volumes stay distinct values; there is no
`mounts` key on Windows and no zero or empty fallback.

Platform identity. Linux uses the distribution name and a numeric version.
Windows has no canonical non-localized distribution name, so the platform name
and slug come from machine codes: name `Windows <product type> <build>`, slug
`windows-<product type>-<build>`. A non-numeric version placeholder (for
example `n/a` from a rolling Linux host) is not an observed version, the
localized caption is never used, and a host without a machine identity sends no
platform instead of a catch-all. A product UUID that is not a UUID is not sent.

Missing keys are omitted from the payload, never sent as `0`, an empty string or
`false`; the partial and missing rules above still apply. Placeholder *text*
serials (a firmware `NA`, for example) have no native format and stay with the
ingest placeholder rule `is_placeholder`, which currently lists `none`, `null`,
`unknown`, `default string`, `to be filled by o.e.m.`, `to be filled by oem`,
`system serial number`, `not specified` and `not available`; a `NA` serial is
therefore still accepted, and covering it belongs to that rule's owner.

Interfaces. Ansible stores the per-interface fact under a name where `-` becomes
`_` (`PrefixFactNamespace`), while `ansible_facts.interfaces` and the model keep
the reported name. The collector looks the fact up under the normalized name and
accepts it only when the fact's `device` equals the reported name; an interface
whose facts are missing or shadowed by the other spelling is not written at all
and is reported as `interface_unobserved`, because a name-only row would make the
ingest normalize `label`, `description`, `type` and `enabled` over a manual
value.

IP addresses. The request carries every observed interface address in one list,
`interfaces[].addresses`, as CIDR strings for both families; there are no
separate scalar `ipv4`/`ipv6` fields. The request boundary parses each entry with
the IPAM normalizer, rejects an invalid family or prefix with the whole request
(no group writes), and keeps one canonical form per interface, so duplicates and
equivalent spellings collapse. An empty list is an absent observation, not an
observed empty set, and nothing here deletes a previously stored address.

Verification: the projection is tested against recorded, sanitized Windows fact
shapes and a controlled Linux host with native Ansible. No live Windows host was
contacted in this change, so WinRM, PSRP and OpenSSH reachability, the
privileged disk module output and the managed-run write remain unverified there;
that live proof belongs to the Windows transport and runner work.

## Endpoint probes

No field in this path comes from endpoint probes. Exporter discovery is E4
(PIP-561 and later); when it exists it must feed this same matrix or a
documented extension of it, not a second ingest path.

## Non-goals

This contract adds no table, column, request field or ingest endpoint. It does
not delete or mark objects absent, does not infer business attributes, and does
not add a universal physical-disk or OS-version entity.
