"""Lossless, server-native catalog used by migration and deployment guards."""

import json
import hashlib
import re
from dataclasses import dataclass, field


BASELINE_SQLX_SHA384 = "55c65893bff52263eb225d3b54436be85a2fd7af3be2cd36b6c375adb9f154142c25f91d39431ea1610439b657c5081e"
CURRENT_MIGRATION_CONTRACT_SHA256 = "91629d441685bbc09ce55327f9c8d897fc6d13576bd6221249c5713a119797b7"


def validate_baseline_layout_contract(contract):
    """Authenticate the complete source-sealed baseline layout contract."""
    if not (isinstance(contract, dict)
            and set(contract) == {"schema", "installed_prefix", "pending_tail", "deltas", "position_pairs"}
            and contract["schema"] == "piping.compose-migrations/v1"
            and type(contract["installed_prefix"]) is int and contract["installed_prefix"] == 1
            and contract["pending_tail"] == [] and contract["deltas"] == []):
        raise ValueError("unsupported source-bound migration contract")
    try:
        digest = hashlib.sha256(json.dumps(contract, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    except (TypeError, ValueError) as error:
        raise ValueError("unsupported source-bound migration contract") from error
    if digest != CURRENT_MIGRATION_CONTRACT_SHA256:
        raise ValueError("unsupported source-bound migration contract")
    return contract


def baseline_layout_positions(contract, catalog):
    """Select one complete sealed layout, never a partial position mapping."""
    pairs = validate_baseline_layout_contract(contract)["position_pairs"]
    objects = _catalog_index(catalog)
    positions = {name: objects.get(("column", name), {}).get("position")
                 if isinstance(objects.get(("column", name)), dict) else None for name in pairs}
    for orientation in (0, 1):
        if all(type(positions[name]) is int and positions[name] == pair[orientation]
               for name, pair in pairs.items()):
            return {} if orientation == 0 else pairs
    raise ValueError("unrecognized native baseline column layout")

# This is one reviewed data transformation, not permission for arbitrary DROP.
# A changed SQL file requires a changed proof and an independently reviewed helper.
SERVICE_CUTOVER_PATH = "apps/api/migrations/0003_service_port_mappings.sql"
SERVICE_CUTOVER_SHA384 = "329c2fa002e8cd1b93db91bb63c5989ba7367f57799040bfc9a7349c002e4a3430495bb04fa4db8d80faa6abbba1a0a6"
SERVICE_TABLES = ("public.ipam_service", "public.ipam_servicetemplate")
SERVICE_MAPPING_TABLE = "public.ipam_service_port_mapping"
FHRP_CUTOVER_PATH = "apps/api/migrations/0004_fhrp_auth_key_secret.sql"
FHRP_CUTOVER_SHA384 = "4e448d34de9c0b8f3f47602caf7e1364dbe4bd57ce30e5655c11d0d7fdf0dadc74e1a431a1c21c77bab1ec0e45f22e17"
FHRP_TABLE = "public.ipam_fhrpgroup"
TAG_CUTOVER_PATH = "apps/api/migrations/0006_canonical_tag_ids.sql"
TAG_CUTOVER_SHA384 = "433906473689b082c2ea627cd5dd0b0a66b90d9c8c25378b2b89f5af4a20522d9506bc60e1e8edb5227faf7ed37c5859"
TAG_SELECTOR_TABLE = "public.playbook_target_set"
TAG_CATALOG_TABLE = "public.inventory_tag"
CONTENT_TYPE_TABLE = "public.inventory_content_type"
TAG_FACTORY_ROW = {
    "id": "00000000-0000-7000-8000-000000001000",
    "name": "Factory default", "slug": "factory-default", "color": "9e9e9e",
    "description": "Marks native factory inventory definitions.", "weight": 0,
    "external_id": None, "piping_synced_at": None,
}

DATAFLOW_CUTOVER_PATH = "apps/api/migrations/0017_dataflow_catalog.sql"
DATAFLOW_CUTOVER_SHA384 = "0b82359e7fcf5a7c8cb7032bc2be46839ac6b2174312453377a46ce3484260cfe9be77f97870ff1f5d253d16e710c14f"
DATAFLOW_FLOW_TABLE = "public.inventory_dataflow"
DATAFLOW_REMOVED_TABLE = "public.inventory_dataflow_selected_ip"
DATAFLOW_SELECTOR_TABLE = "public.inventory_dataflow_selector"
DATAFLOW_SELECTOR_IP_TABLE = "public.inventory_dataflow_selector_ip"
DATAFLOW_OLD_FLOW_COLUMNS = (
    "id", "name", "description", "status", "transport", "allow_cross_vrf", "destination_ports",
    "source_kind", "destination_kind", "source_mode", "destination_mode",
    "source_device_id", "source_vm_id", "source_service_id", "source_ipaddress_id", "source_prefix_id",
    "destination_device_id", "destination_vm_id", "destination_service_id",
    "destination_ipaddress_id", "destination_prefix_id",
    "source_region_id", "source_site_id", "source_sitegroup_id", "source_devicerole_id",
    "source_tenant_id", "source_tenantgroup_id", "source_platform_id", "source_manufacturer_id",
    "source_devicetype_id", "source_rack_id", "source_location_id", "source_cluster_id",
    "source_clustergroup_id", "source_clustertype_id",
    "destination_region_id", "destination_site_id", "destination_sitegroup_id",
    "destination_devicerole_id", "destination_tenant_id", "destination_tenantgroup_id",
    "destination_platform_id", "destination_manufacturer_id", "destination_devicetype_id",
    "destination_rack_id", "destination_location_id", "destination_cluster_id",
    "destination_clustergroup_id", "destination_clustertype_id",
)
DATAFLOW_OLD_SELECTOR_COLUMNS = DATAFLOW_OLD_FLOW_COLUMNS[7:]
DATAFLOW_NEW_FLOW_COLUMNS = (
    "id", "name", "description", "status", "transport", "allow_cross_vrf", "destination_ports",
    "application_id", "group_id", "source_ports", "icmp_types", "comments", "tenant_id",
    "owner_id", "tags", "custom_field_data",
)
DATAFLOW_NEW_TABLES = {
    "public.inventory_application_role": (
        "id", "name", "description", "comments", "owner_id"),
    "public.inventory_application": (
        "id", "name", "role_id", "description", "comments", "owner_id", "tenant_id",
        "tags", "custom_field_data"),
    "public.inventory_dataflow_group": (
        "id", "parent_id", "application_id", "name", "description", "comments", "status",
        "owner_id", "tenant_id", "tags", "custom_field_data"),
    "public.inventory_address_group": (
        "id", "name", "description", "comments", "owner_id", "tenant_id", "tags",
        "custom_field_data"),
    DATAFLOW_SELECTOR_TABLE: (
        "id", "dataflow_id", "group_id", "side", "position", "object_type", "address_mode",
        "tag_object_type", "device_id", "vm_id", "service_id", "ipaddress_id", "prefix_id",
        "iprange_id", "tag_id", "addressgroup_id", "region_id", "site_id", "sitegroup_id",
        "devicerole_id", "tenant_id", "tenantgroup_id", "platform_id", "manufacturer_id",
        "devicetype_id", "rack_id", "location_id", "cluster_id", "clustergroup_id",
        "clustertype_id"),
    DATAFLOW_SELECTOR_IP_TABLE: ("selector_id", "ipaddress_id"),
}
DATAFLOW_CHANGED_TABLES = (DATAFLOW_FLOW_TABLE, *DATAFLOW_NEW_TABLES)
DATAFLOW_CONTENT_TYPE_SEEDS = (
    ("inventory", "applicationrole"),
    ("inventory", "application"),
    ("inventory", "dataflowgroup"),
    ("inventory", "addressgroup"),
    ("inventory", "dataflow"),
)

# PostgreSQL 18 definitions for the reviewed, immutable schema transitions.
# Each edge binds the complete native JSON definition, including RI semantics.
# Creation edges make composed upgrades require the intended replacement objects.
SCHEMA_MIGRATIONS = {2: ('apps/api/migrations/0002_interface_networking.sql',
     'd742c8c7affa6a040db4d5ea383bacd7b877e1e0e26499cd1b50aafc2487012bff1c8c8c16d7883be29f149e0a59cf07'),
 5: ('apps/api/migrations/0005_module_bay_types.sql',
     'a7112aaa191bd822e823b5cfa19929d207ed9258a645147b91eec6f253cfa35ac47a727cc69bb4fb92778dd3b004b118'),
 6: (TAG_CUTOVER_PATH, TAG_CUTOVER_SHA384),
 7: ('apps/api/migrations/0007_cooling_hardware.sql',
     '7279c96caca50908c8d8b5d5b14eb021f5b30dc504363678cb628fd40395fbe882d1d4e174b130c5144c4b3669466c9f'),
 9: ('apps/api/migrations/0009_circuit_group_assignments.sql',
     '94f2af444b62a96feaa8b06ba98e96635167cea7034217909e5c9d5f3d60421830e820e1f78a2a2675b79fe15d4c560e'),
 10: ('apps/api/migrations/0010_vlan_translation.sql',
      '846362e107c7f4c475e1c2df02bcc7fc2cf31c3b538110f5e3fdd45173c6b0dcd1a273596690d0e5a35f7480b6997081'),
 11: ('apps/api/migrations/0011_termination_graph.sql',
      '9786c04a85f83c85b2efcfb2d7b49852180c0fe3651c185e800c8f1223185880355732743c09f96112fcaef20dcf40ef'),
 13: ('apps/api/migrations/0013_component_template_instantiation.sql',
      '187898cf55116340ee6156faf5181983961a6d11d5c98f970105e0a923fd636b792afc3337b89b8609da2c747fc2405c'),
 14: ('apps/api/migrations/0014_tag_object_type_scope.sql',
      '24bcd216407476d526dbc54c7a9975ee6b7564bf435b4f90226df51311a016c80fcac916cad73e321e0de66303cc0f89'),
 15: ('apps/api/migrations/0015_device_bay_template_enabled.sql',
       '8d41638c8a534ec7ab6c693d2cd326a74ec3e9dbf0c11e95186139ab40af799fc35943a85fbe3ee0b9728831c65958a1'),
 16: ('apps/api/migrations/0016_project_workspace.sql',
       'f40ecaf2eea00601d45e7445cda832cb59c785cf4d43234a7e01d40b5068d6b52f8ba336a76430c5c7f4cc3bd6845e7c'),
  17: (DATAFLOW_CUTOVER_PATH, DATAFLOW_CUTOVER_SHA384),
  18: ('apps/api/migrations/0018_target_set_delete_restrict.sql',
       '9c9a75d090fd046458937c382a37cfbe8586fbcf8ff311745cc7c8e374a0af0825d53cf84ac591ee382337d6d9633254'),
  19: ('apps/api/migrations/0019_config_context_semantics.sql',
       '8c219a3ee012790bc6b4fb55c0ef6780ad3920299fbd8e8632cf598a93660fac02ed6e1b064718a6a2e5024d1e7afd4f'),
  20: ('apps/api/migrations/0020_monitoring_profiles.sql',
       '2a685856b4a0f57992277f86d903a1fb5340cceea36ccea276412cb5827ff9f42b6ec2c808f283d30eb5879904b67442'),
  21: ('apps/api/migrations/0021_playbook_schedule_owner.sql',
        'c1ebb5452cd722a2bc417c22ba8b911a4d3f25386b4224d679812005af778146343082cdca11ea90c4f4f4d046d27538'),
  22: ('apps/api/migrations/0022_launch_parameters.sql',
        'ea0563cd2f461b0ea3f21e2a647f853d9d64a9516499646498017478d5c6fe868ebecc1aa2c96f0e4025cb47f58a03b0'),
  23: ('apps/api/migrations/0023_collected_facts_provenance.sql',
        '9e4bc0b805ec71d9119aab95f95b0b4daf826542a73a1b8cf22db390b5a9645675f6834e58152a8f0154f3c4328bb39b'),
  24: ('apps/api/migrations/0024_connconf_winrm_protocol.sql',
       '3b203a9b5a289835a8855744903897283f8125bfe4b23f8018d3f73d3e6a78c116328fc4240d82f8e68d6d22fbfd45b1')}

CONTENT_TYPE_SEED_MIGRATIONS = {
    **{version: SCHEMA_MIGRATIONS[version] for version in (5, 7, 9, 10, 11, 13, 17)},
    8: ("apps/api/migrations/0008_module_type_profile_scope.sql",
        "f8735a77acc8ac59bc16184bb580cdeca6a46391c0ec2746bcd2bc03ed857b2fef0839abcaa545826c6be63fda539889"),
}

# Non-seed SQL also executes in the native registry oracle, so its entire
# source cohort must be pinned rather than only the INSERT-bearing files.
CONTENT_TYPE_SOURCE_MIGRATIONS = {
    1: ("apps/api/migrations/0001_init.sql", BASELINE_SQLX_SHA384),
    3: (SERVICE_CUTOVER_PATH, SERVICE_CUTOVER_SHA384),
    4: (FHRP_CUTOVER_PATH, FHRP_CUTOVER_SHA384),
    12: ("apps/api/migrations/0012_inventory_transfer_run.sql",
         "77a3bae0baef94bc6fff3d5171a4d040bd689998cba291fa2ea79cd5995f1f4285b9652ef79b59919ea0e6f1f0eac544"),
    25: ("apps/api/migrations/0025_auth_session_remember_device.sql",
         "98702b3a8a6f87152cfbb8cc090de8a29d3cff809d04338dd2c68aaeb30054e3b9c60b0bba717388c7d52639e2c465a5"),
    26: ("apps/api/migrations/0026_sync_pull_schedule.sql",
         "dd8d4b7995f5cb263b3a19c5faeab58eb39049a117d3c2d7a7b05c4c0aa6104eed8213520c72e1da97b483c370107261"),
    27: ("apps/api/migrations/0027_source_ownership_reconciliation.sql",
         "6ae0ab7623f173dffbb4930dc7dfe79bac7e8bd59f5e0b1fa3bd83fbe69f7be9b895aad1b2b41a3e4e4ff1bd09d884b8"),
    28: ("apps/api/migrations/0028_custom_field_lifecycle_coverage.sql",
         "250f47f0220103094746c2c0755b53fccad4352813bd2d833d944d5b7f0635ec0047828f85c680894141029e390433ec"),
    **SCHEMA_MIGRATIONS,
    **CONTENT_TYPE_SEED_MIGRATIONS,
}

MONITORING_LEGACY_TABLE = "public.monitoring_exporter_assignment"
MONITORING_LEGACY_COLUMNS = (
    "id", "exporter_id", "name", "scope_type", "scope_id", "weight", "enabled",
    "target", "job", "scheme", "port", "metrics_path", "labels", "created_at", "updated_at",
)
MONITORING_COLUMNS = {
    "public.monitoring_profile": (
        "id", "name", "slug", "description", "metric_set_id", "query_source_id",
        "alert_source_id", "enabled", "created_at", "updated_at",
    ),
    "public.monitoring_profile_item": (
        "id", "profile_id", "exporter_id", "job", "scheme", "port", "metrics_path",
        "labels", "enabled", "sort_order", "created_at", "updated_at",
    ),
    "public.monitoring_profile_assignment": (
        "id", "profile_id", "scope_type", "scope_id", "weight", "enabled", "target",
        "created_at", "updated_at",
    ),
}
MONITORING_NEW_TABLES = tuple(MONITORING_COLUMNS)
# All 139 identities and complete before/after definition hashes, captured from
# native execution of the pinned 0020 SQL. Ownership below selects observations;
# only this complete digest grants schema authority.
MONITORING_SCHEMA_DELTA_SHA256 = "8fabea5feeafd8973ba10c7bb1d64d5074e93b7b2239446993d2a4222b7bf923"
MONITORING_SCHEMA_INDEXES = frozenset((
    "public.idx_monitoring_exporter_assignment_exporter",
    "public.idx_monitoring_exporter_assignment_scope",
    "public.monitoring_exporter_assignment_pkey",
    "public.monitoring_profile_assignment_pkey",
    "public.monitoring_profile_assignment_scope_idx",
    "public.monitoring_profile_assignment_scope_key",
    "public.monitoring_profile_item_identity_key",
    "public.monitoring_profile_item_order_idx",
    "public.monitoring_profile_item_pkey",
    "public.monitoring_profile_pkey",
    "public.monitoring_profile_slug_key",
))
SCHEDULE_TABLE = "public.playbook_schedule"
SCHEDULE_OWNER_RENAMES = {
    SCHEDULE_TABLE + ".owner_user_id": SCHEDULE_TABLE + ".delegated_by_user_id",
}

SCHEMA_TRANSITIONS = {('constraint', 'public.circuits_circuitgroupassignment.circuits_circuitgrou_group_id_1a7b6580_fk_circuits_'): ((9,
                                                                                                                 '61f0b2acd63ef7156a2ce2a86e2aae78c33ea7a9b30cd64665e8d59aaa2a4fe1',
                                                                                                                 'cb3dd67ea33397fe4b202af7abfe10e4e2a301150692d87075a5000174c70733'),),
 ('constraint', 'public.circuits_virtualcircuittermination.circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte'): ((11,
                                                                                                                        '3e2424de15c7c8d0a96719bf925120647bb63deb0f60954ad0904e12324a1ddc',
                                                                                                                        '3e6714fd054c312ad5b91943b4f033619f8467cbc05ee978c56c88567e8345a6'),),
 ('constraint', 'public.circuits_virtualcircuittermination.circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_'): ((11,
                                                                                                                              'e467549bc0bc4c6247561b844e1cb0a4ddbc9fc8d54d44399b160492e7d47acc',
                                                                                                                              '0ec2ca65b3480f2384bb74fc7a02d43d097d2088b732fab63b347154b78965d8'),),
 ('constraint', 'public.dcim_interface.dcim_interface_mtu_check'): ((2,
                                                                     '96855110ee4c76152b1c8fbccf6cebadb5017a31800fca0cde650cb0037f1947',
                                                                     'a0c2e2235de170b6d2bdc8bde9ef5171e0dcaed45dbbb4ea3f2c0160eb208dce'),),
 ('constraint', 'public.dcim_modulebay.dcim_modulebay_unique_device_module_name'): ((5,
                                                                                     '9f39fc322cbdec05ff5e2dfa99ddc7630e7d18b18606419798232c334e296426',
                                                                                     'eb47ecd6732b5311e4a6d81bf60b57d669335fa18bc8e5de52efa75c9ede15c5'),),
 ('constraint', 'public.ipam_vlantranslationrule.ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan'): ((10,
                                                                                                           'abfb7fb5638cca42533b1caddba42ef35b915d22b02916cce140baa29e58fdaf',
                                                                                                           '1034861e1410adeb3cae28327a5ff16f728997deb6f7f8700ae1b988c1f43495'),),
 ('constraint', 'public.ipam_vlantranslationrule.ipam_vlantranslationrule_local_vid_check'): ((10,
                                                                                               'dcc0977a9621a9c12195e7408ac75afe35857e05a395f1dd381771478cdd6a08',
                                                                                               '235f67d403722296494e1fecf3a51c7de356092cf917251eeeb83c2ac9ae3666'),),
 ('constraint', 'public.ipam_vlantranslationrule.ipam_vlantranslationrule_remote_vid_check'): ((10,
                                                                                                '64c678e818e179201ac31370673244232eefc96edc7cd7ae274ca14ff52a7ae9',
                                                                                                '94fced28ef5e00a80f25f1e6cf215212e80ff8878dbcd026fd220f6e24729bae'),),
 ('constraint', 'public.vpn_l2vpn_export_targets.ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout'): ((11,
                                                                                                                '8e1caac99cf05f0502bcec43ede7d8da1f9304728017e5f750333bdf6c6007eb',
                                                                                                                'e464bea824c4e1226339d1b5d134e268a4b9d8238f6075d3c9c4cf45d593f7f6'),),
 ('constraint', 'public.vpn_l2vpn_export_targets.ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id'): ((11,
                                                                                                                   '3557aa3c9dc282e759e268f5c83b8328ad66bde52920c9c22defb0c962653863',
                                                                                                                   'a0066967b58cb516b342a66608701af5308558fd7460cc6630e12ffef084e490'),),
 ('constraint', 'public.vpn_l2vpn_import_targets.ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout'): ((11,
                                                                                                                '8e1caac99cf05f0502bcec43ede7d8da1f9304728017e5f750333bdf6c6007eb',
                                                                                                                'e464bea824c4e1226339d1b5d134e268a4b9d8238f6075d3c9c4cf45d593f7f6'),),
 ('constraint', 'public.vpn_l2vpn_import_targets.ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id'): ((11,
                                                                                                                   '3557aa3c9dc282e759e268f5c83b8328ad66bde52920c9c22defb0c962653863',
                                                                                                                   'a0066967b58cb516b342a66608701af5308558fd7460cc6630e12ffef084e490'),),
 ('constraint', 'public.vpn_l2vpntermination.vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id'): ((11,
                                                                                                         '3557aa3c9dc282e759e268f5c83b8328ad66bde52920c9c22defb0c962653863',
                                                                                                         'a0066967b58cb516b342a66608701af5308558fd7460cc6630e12ffef084e490'),),
 ('function', 'public.circuits_lock_group_assignments()'): ((9,
                                                             None,
                                                             'a751e2ae36d33b3487c14a2602a86ddddab34cf876183aad088450580ed9262f'),
                                                            (11,
                                                             'a751e2ae36d33b3487c14a2602a86ddddab34cf876183aad088450580ed9262f',
                                                             'efcf736b8a180d1c0f5248c5a50f8387cae60b499274802517d934885d12886d')),
 ('function', 'public.dcim_instantiate_hardware_components()'): ((7,
                                                                  None,
                                                                  'ad5aea0d211dd4920d68705c86a2410f02cccd0f28ecf00e54e49f679ff18246'),
                                                                 (13,
                                                                  'ad5aea0d211dd4920d68705c86a2410f02cccd0f28ecf00e54e49f679ff18246',
                                                                  '06a1fd8bc79ef7a6161929ecfb9329ae080c5c576c348e6c2e05fbd17dd7de36'),
                                                                 (15,
                                                                  '06a1fd8bc79ef7a6161929ecfb9329ae080c5c576c348e6c2e05fbd17dd7de36',
                                                                  'ab8c753de82c07d7e80c1bc974dc87cb1a1c9ced3b880db1a4eeb4d37c906c3a')),
 ('function', 'public.inventory_tag_membership_guard()'): ((6,
                                                            None,
                                                            '77f7baacac544aada0e61533352c65d5d49668b524cb71fadd1724eeab9b3d74'),
                                                           (14,
                                                            '77f7baacac544aada0e61533352c65d5d49668b524cb71fadd1724eeab9b3d74',
                                                            'a4f2cfbe428b26486300d2e345a3ed689dc111c07c0703bc6d57742ceff4e476'),
                                                           (16,
                                                            'a4f2cfbe428b26486300d2e345a3ed689dc111c07c0703bc6d57742ceff4e476',
                                                            '931806e7992e28fc75f787bcbf7abf70b96735d7f9bf8858fafe7076c241a840')),
 ('function', 'public.inventory_tag_scope_check(tag uuid)'): (
     (14, None, 'e0c84adc10b6b367e4df84823e825d3140fe783b3039920ab11036a10f7dbd95'),
     (16, 'e0c84adc10b6b367e4df84823e825d3140fe783b3039920ab11036a10f7dbd95',
      '0871033f959a744be0bf3bb2816e02d041d53251254c310b58cb4a6d3c41b2e0')),
 ('function', 'public.inventory_tag_scope_type_guard()'): (
     (14, None, '3d68e5525a8cf768f1c21999fd3d963dd9a6643fa2131d4216e977e91607ebc4'),
     (16, '3d68e5525a8cf768f1c21999fd3d963dd9a6643fa2131d4216e977e91607ebc4',
      '56758f13d8c49c6fdbcbdbd0f890b2b1c2f1afb65b45ae6885e8e537b4b31733')),
 ('index', 'public.dcim_modulebay_unique_device_module_name'): ((5,
                                                                 '3c4eb062d0c48c171a6d80b8b7413666245ded21f25f065efdf28551298f8634',
                                                                 '38c2b7bd99d3c923b81232d59311cade539f360a1aa00eb31455dd3f878dada6'),),
 ('trigger', '["public", "circuits_circuitgroup", "internal", "public", "circuits_circuitgroupassignment", "circuits_circuitgrou_group_id_1a7b6580_fk_circuits_", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((9,
                                                                                                                                                                                                                   None,
                                                                                                                                                                                                                   'c36b6bddb3a89931a339bfceebbefb131b9d4aa00e05bf1b3812dd60df9e1738'),),
 ('trigger', '["public", "circuits_circuitgroup", "internal", "public", "circuits_circuitgroupassignment", "circuits_circuitgrou_group_id_1a7b6580_fk_circuits_", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((9,
                                                                                                                                                                                                                    '614c40189837f89a14c9b5e327a688eeae69e0da78546f90f9be712b03e42dd8',
                                                                                                                                                                                                                    None),),
 ('trigger', '["public", "circuits_circuitgroup", "internal", "public", "circuits_circuitgroupassignment", "circuits_circuitgrou_group_id_1a7b6580_fk_circuits_", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((9,
                                                                                                                                                                                                                     '80d47b83565fc5af8169281420185a042d4ff7ecca93865ccccf1f0fc1ea4982',
                                                                                                                                                                                                                     '58575187b071bad01aede76cc697af4de5bfbf9fccc43e7f61c6d83883dfc70a'),),
 ('trigger', '["public", "circuits_circuitgroupassignment", "internal", "public", "circuits_circuitgroupassignment", "circuits_circuitgrou_group_id_1a7b6580_fk_circuits_", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((9,
                                                                                                                                                                                                                           '43809bfee30f404eecd7579b857501cf702ec9482731f18241884d40f594b796',
                                                                                                                                                                                                                           '26d1af4d44394bf5fe2469fdc69f763b28b296254c0a06ab0f9ba204b686a29f'),),
 ('trigger', '["public", "circuits_circuitgroupassignment", "internal", "public", "circuits_circuitgroupassignment", "circuits_circuitgrou_group_id_1a7b6580_fk_circuits_", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((9,
                                                                                                                                                                                                                            '23ae8581485be92cad1bfd21582d465feae84398dde76d3158a2d88209463731',
                                                                                                                                                                                                                            '9369e4770fe2340a02442185731c1ebde8932d7efcca9972bf69549bee1c4f9f'),),
 ('trigger', '["public", "circuits_virtualcircuit", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((11,
                                                                                                                                                                                                                                  None,
                                                                                                                                                                                                                                  'f5ed2b3cb18fc99468ffc299003106cd5c6d657a2e712136ccec6d43c398c7a8'),),
 ('trigger', '["public", "circuits_virtualcircuit", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((11,
                                                                                                                                                                                                                                   'b634a800421f91a9b451368b8246357f6308de64bcaecc1197f239994fdc6561',
                                                                                                                                                                                                                                   None),),
 ('trigger', '["public", "circuits_virtualcircuit", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((11,
                                                                                                                                                                                                                                    '1315b63e340c18c82221dda4bae0dae3eeb9973e2ca085317b11eed4374a6265',
                                                                                                                                                                                                                                    '49dbf4ffba69d780e1f98c08744cf8465eac1905b9209ab299cbaa5b8bf59053'),),
 ('trigger', '["public", "circuits_virtualcircuittermination", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((11,
                                                                                                                                                                                                                                     'b4e6b027d92b2c64397abf0721464156ff7fbdc8cca3b9ad51ebd48d40e312b0',
                                                                                                                                                                                                                                     'ef207476a0496a76a46d7ded5b744d3c867f82fc665e0858f76604e87c1a53c9'),),
 ('trigger', '["public", "circuits_virtualcircuittermination", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((11,
                                                                                                                                                                                                                                      '5871d8feeeac6ebe585c506c25a7405ec624d1a21b3a203beb7136a5701fb061',
                                                                                                                                                                                                                                      '72ea8cc6655ba4f3f8ba6b296ed43ad1c559848af08a35eb59437487f71b8ec0'),),
 ('trigger', '["public", "circuits_virtualcircuittermination", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((11,
                                                                                                                                                                                                                                           '5505d0c3f61e5d7a306a23de25bc858f770c9e8b0564180dc48bf419df5c3194',
                                                                                                                                                                                                                                           'bd91672afee82b394f306dfaaa63ebdc3c8d3b3f4de276922cf7170432459e03'),),
 ('trigger', '["public", "circuits_virtualcircuittermination", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((11,
                                                                                                                                                                                                                                            '620ef14cbd6565f608f198c1639595841304eadfc5b77f2a8c69cdae1c619cb0',
                                                                                                                                                                                                                                            '523b33d426006c5455d702a825690d16bf598d2f1eabee7a09c753d4eda03b5d'),),
 ('trigger', '["public", "dcim_interface", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((11,
                                                                                                                                                                                                                   None,
                                                                                                                                                                                                                   'e2ba188b8ebf3982fbeb17e255396ab2e9f8fc385573fa5732da7ecdbc910088'),),
 ('trigger', '["public", "dcim_interface", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((11,
                                                                                                                                                                                                                    '7e0eee67a842ecb801ef409d41427b5d87c3d857ad10f3a86ada0e55304f0bf0',
                                                                                                                                                                                                                    None),),
 ('trigger', '["public", "dcim_interface", "internal", "public", "circuits_virtualcircuittermination", "circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((11,
                                                                                                                                                                                                                     '9664da2fd27bf9f25e82bf78378781aa5fea5dee0bb30dd9a8ee107a7f40c772',
                                                                                                                                                                                                                     '4fc403181ee7144a0f2f39b11b56fcb7c4c343a825924b3f1aacd6d17592d7f3'),),
 ('trigger', '["public", "ipam_routetarget", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((11,
                                                                                                                                                                                                             None,
                                                                                                                                                                                                             'ffa49aae6b8680198ec8fea560a396b23097cc600a7013448d2d0e8ce80334fe'),),
 ('trigger', '["public", "ipam_routetarget", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((11,
                                                                                                                                                                                                              'e7e2f5d8506ef14b2072f64e9025a99730be730f7b8e286133069411fcce5563',
                                                                                                                                                                                                              None),),
 ('trigger', '["public", "ipam_routetarget", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((11,
                                                                                                                                                                                                               '0fc5f6d760e89f04004d57a95da1f537dd533c68fa52b9912bcaf751e5608c66',
                                                                                                                                                                                                               'c7afed3707df1ff03783b45f629f136723bc06890ed10dc8ce288c9f90f72958'),),
 ('trigger', '["public", "ipam_routetarget", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((11,
                                                                                                                                                                                                             None,
                                                                                                                                                                                                             'f8ad65c1a8ab1b6127300f6f38112d29c70ac1e684e576869e20683b9472306a'),),
 ('trigger', '["public", "ipam_routetarget", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((11,
                                                                                                                                                                                                              '6a29e5fd994b3f1e93997d46b4adf71b19b02edb3bcf967f8978b9a3a44b0c2c',
                                                                                                                                                                                                              None),),
 ('trigger', '["public", "ipam_routetarget", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((11,
                                                                                                                                                                                                               '09c080223663cdf6dae7d051ce97ed560e900b229a63de1fec373b876e2d3634',
                                                                                                                                                                                                               '3d6ddc87b5656a12b5da5db4aafbe818b6b513c1f1aee6e16f289cdf51a06c0a'),),
 ('trigger', '["public", "ipam_vlantranslationpolicy", "internal", "public", "ipam_vlantranslationrule", "ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((10,
                                                                                                                                                                                                                  None,
                                                                                                                                                                                                                  '5b4adc57d816dd6a0160bf6bd7764a831711eafbbf87c4af3f114fbb327a1405'),),
 ('trigger', '["public", "ipam_vlantranslationpolicy", "internal", "public", "ipam_vlantranslationrule", "ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((10,
                                                                                                                                                                                                                   'decde0b5d9a6f14430d55e7726315f8414e7a0554b49a32ed12a749a786ddee6',
                                                                                                                                                                                                                   None),),
 ('trigger', '["public", "ipam_vlantranslationpolicy", "internal", "public", "ipam_vlantranslationrule", "ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((10,
                                                                                                                                                                                                                    'af73a1ac5aaf3e9d70d188bd0b195fafea90b2f09e0fa3c831afbc4d2fbe6639',
                                                                                                                                                                                                                    '1192cf1ed0f2a77493dcce3ae106b7eff4c088b9e41a08c84a6449da70b8301f'),),
 ('trigger', '["public", "ipam_vlantranslationrule", "internal", "public", "ipam_vlantranslationrule", "ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((10,
                                                                                                                                                                                                              'aec78e1326f89e727b7bb31ef9df8c8e0673e80d2a39ffeb1966c62ad43b1922',
                                                                                                                                                                                                              '5817f5fa08e844053246cb8a1b43c9469a2ed03a7260fe2620539a8b16663d55'),),
 ('trigger', '["public", "ipam_vlantranslationrule", "internal", "public", "ipam_vlantranslationrule", "ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((10,
                                                                                                                                                                                                               '767571582ba929372bd1608e7ad6602fc646b665485df004978287b382d7ed3f',
                                                                                                                                                                                                               '0497b02808749747f16c603bc8170ddfa901d7faf0112acdf7492b56ebf1ffe1'),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((11,
                                                                                                                                                                                                         None,
                                                                                                                                                                                                         '885096fd0e58ca0e86ccd9403093870a43e5a84c4a57de46d0d8e4774f0fb6d7'),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((11,
                                                                                                                                                                                                          '2cd22b42a0a1184986a6f1bf782f738f4deb7bcb49ab4546edb780fde6a8cce9',
                                                                                                                                                                                                          None),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((11,
                                                                                                                                                                                                           'ad5335529677dd97fba51bcaccab937c0e5e501a859d256f0cb14459f1cb5758',
                                                                                                                                                                                                           '113d869ea7ae6bae7464842652e190c706c0946c5be7b1b860be0a7d42c9e5ab'),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((11,
                                                                                                                                                                                                         None,
                                                                                                                                                                                                         '54d4243f1b7d82710363bf589c63d6ea6f58844f7df31a3de7e6d4865ea94725'),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((11,
                                                                                                                                                                                                          '0aec66ef01ade37263ea0d317bb93032b99d40c4610f623bb61ceef6d93a36d6',
                                                                                                                                                                                                          None),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((11,
                                                                                                                                                                                                           '66a73873d00c37191af870250fc72249065ddbd7e1982cde9dcc647dfab47fc4',
                                                                                                                                                                                                           '496bb65a95109b2bdea05904e47c9d47026e9bbae28d899b84191f704bba2539'),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpntermination", "vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((11,
                                                                                                                                                                                               None,
                                                                                                                                                                                               '96d54c005380854dce6203817ef499395aaea4150cc689735206a3ceb89fcc9f'),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpntermination", "vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((11,
                                                                                                                                                                                                '3bcf5fff9e894fb37d5049a94daf9a10800291e0feab40960e2bd8f8e8ee22a2',
                                                                                                                                                                                                None),),
 ('trigger', '["public", "vpn_l2vpn", "internal", "public", "vpn_l2vpntermination", "vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((11,
                                                                                                                                                                                                 'b4f9c587bd28db0ac966dd7c4c6dbb463536a3758055e1fbe0f19bbe8abf391a',
                                                                                                                                                                                                 '7e7c297befe9ecd05ccd99bc9d03a88bd187051d3d450cd4838696b70fa587ba'),),
 ('trigger', '["public", "vpn_l2vpn_export_targets", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((11,
                                                                                                                                                                                                                   'd4e8ecc778773cc7dc22aa057f5c8db12098724066a8b40e0de910eb6a84ea06',
                                                                                                                                                                                                                   '5edb46f2e77b349d897b0d461ebd23e1832bb505873c000550113d92916961a7'),),
 ('trigger', '["public", "vpn_l2vpn_export_targets", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((11,
                                                                                                                                                                                                                    '94adf7aeedcfc9cdf3f499c80246b4bd79f3401174a4238b582b5685f81bd80f',
                                                                                                                                                                                                                    'a8b3f1072238af00037c63f438380020b4d80b86087a9c4be8424d78ec5e4333'),),
 ('trigger', '["public", "vpn_l2vpn_export_targets", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((11,
                                                                                                                                                                                                                      '4b83b4d74cb0f44dd9d684af2dc15c9d9072abdc152143a24ec9cd0f1887b82a',
                                                                                                                                                                                                                      'bee11dc2b987c37299b9e4215f8dca23347ee2a1ff5611f5a04b5e55d46e7f01'),),
 ('trigger', '["public", "vpn_l2vpn_export_targets", "internal", "public", "vpn_l2vpn_export_targets", "ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((11,
                                                                                                                                                                                                                       'f63bba02b3bce31bcd9bee62740b23327221eea8907fa0ad6ad8dc80cfed9988',
                                                                                                                                                                                                                       'e5a7f85a48e9541091a2406a5cef473c74541b06afd036ae8ebb0f98941c5b48'),),
 ('trigger', '["public", "vpn_l2vpn_import_targets", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((11,
                                                                                                                                                                                                                   'eeb5fa190013f5216a5a5c71a56eef0d5ea919b366230397b0c216281a7b4e86',
                                                                                                                                                                                                                   'bf893c5d7b5de0e18317e6b6d9495ce75f98357a43d290cb818b1deeed7bedf0'),),
 ('trigger', '["public", "vpn_l2vpn_import_targets", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((11,
                                                                                                                                                                                                                    '2ea077c0a637d6e0c308b44293f2995fde9fcd7f28b6d109bed8fb4b4246900c',
                                                                                                                                                                                                                    '240e59a54ad5021ea50ddcf91e679ea8ced8a5d4f9dde9b03bb6f45f3c33756c'),),
 ('trigger', '["public", "vpn_l2vpn_import_targets", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((11,
                                                                                                                                                                                                                      'fc2a08e2bf01489ccaa6886f113c1bbfe3eb2e2a0622856efa702e143e753dcd',
                                                                                                                                                                                                                      'ca952d6fc5d993d3255205acf3a7511a6d8793bbc14e07b4d57c35d1ee7690d4'),),
 ('trigger', '["public", "vpn_l2vpn_import_targets", "internal", "public", "vpn_l2vpn_import_targets", "ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((11,
                                                                                                                                                                                                                       '6bda8f635bdd4af07b2314a010e125378e4b19553dbb5d7b1c2fa5d9758741a3',
                                                                                                                                                                                                                       '6c9016ff825d14bd6a5feb58a876f71616e5dfb383ff1b424cd8ae2d61bc354f'),),
 ('trigger', '["public", "vpn_l2vpntermination", "internal", "public", "vpn_l2vpntermination", "vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((11,
                                                                                                                                                                                                        '901a089d79df7304f33da69a5cd9ed45ca3abe74b4f0af284b96644134e6d2e2',
                                                                                                                                                                                                        'd1107b210ccac3dedce29101a72915158c633024cd55897779e9b967d19b42be'),),
 ('trigger', '["public", "vpn_l2vpntermination", "internal", "public", "vpn_l2vpntermination", "vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((11,
                                                                                                                                                                                                         'd0489db1da90d14d0dbc9bc9d64f183146f6cd43c5716bf98333670968fcc617',
                                                                                                                                                                                                         '1536a3225eb1bdfdf41ac42b576e48745470a2ec1d2d732aba68731dc3da882b'),),
  ('column', 'public.inventory_dataflow.destination_cluster_id'): ((16, 'c0d6ea6b64ff42e9bf7114da6c01bbf7def5aaaf7999eb41de7cdb4c0ca073a3', None),),
  ('column', 'public.inventory_dataflow.destination_clustergroup_id'): ((16, '718ec4e67d01384b2c789cf22d445178b2f1918df0eb690b93eba3046114bb5b', None),),
  ('column', 'public.inventory_dataflow.destination_clustertype_id'): ((16, 'e4a515e72e4a901b8f57a5a8d8c515424090afd5f3364b045ff5b749b3aaefaf', None),),
  ('column', 'public.inventory_dataflow.destination_device_id'): ((16, '014fc30d100449ca9188d03f72268a0282b0597c9717d31519639615d3ae2cb4', None),),
  ('column', 'public.inventory_dataflow.destination_devicerole_id'): ((16, '9e9770f42064bcbcda93745ae02808a6ac398565243dc950b36b63f28ecaf060', None),),
  ('column', 'public.inventory_dataflow.destination_devicetype_id'): ((16, '5d1781cbbc632bddcfcfc9110e73574262578f346aa9b0190f84112a094f091e', None),),
  ('column', 'public.inventory_dataflow.destination_ipaddress_id'): ((16, 'acf18838cb93570b4573b51db9ff86c15ecfeb38808d85610efbae3cb25254ea', None),),
  ('column', 'public.inventory_dataflow.destination_kind'): ((16, '4523df20f1f95ea6bba82e632db2a838bc70c39f30c2335ed184f909ed48fa7a', None),),
  ('column', 'public.inventory_dataflow.destination_location_id'): ((16, 'f525f0fffc15af12a873da63aec40d3672de47bdf9f260097dfce7d2045893c0', None),),
  ('column', 'public.inventory_dataflow.destination_manufacturer_id'): ((16, '3aacbb3d8fb2e4a9b1ccb8d90b695956e1d5d67f5dfd2ab33c13272bdbb34263', None),),
  ('column', 'public.inventory_dataflow.destination_mode'): ((16, 'ca4a5224c58fa37cfc077eb40461db861b6e4097ea7db03cc9fc18fa8cf7ced5', None),),
  ('column', 'public.inventory_dataflow.destination_platform_id'): ((16, 'fc25d07cc5fa4584034cc0588ad6616650841a2ce47253696beb9d1f2f05f691', None),),
  ('column', 'public.inventory_dataflow.destination_prefix_id'): ((16, '764817c0be6c3baa223ddd46f10894a045743e47e80a2b24d2b3609792fe3d5c', None),),
  ('column', 'public.inventory_dataflow.destination_rack_id'): ((16, '0d227917a5dc6b9c8606ab1b313dcfb65efa5e70766e1f43bd60d02fc6f286ac', None),),
  ('column', 'public.inventory_dataflow.destination_region_id'): ((16, 'b0116ce60edd461a8e527ee039adcb524eda1d12e9eafc778d7589107378ee2d', None),),
  ('column', 'public.inventory_dataflow.destination_service_id'): ((16, '4b22c8f34be40666729cbc3c00457dabc1914beef3c142c3133d3ff6a21be1bb', None),),
  ('column', 'public.inventory_dataflow.destination_site_id'): ((16, '58b7ab44a2999e6914dce8fcca9e001a6eae9b1021cc5d4de6833b1b758307ba', None),),
  ('column', 'public.inventory_dataflow.destination_sitegroup_id'): ((16, '26f39c526d3febbe6e180cd447c83d67e9d7d10f05dc7446ab1d1b35d4795d17', None),),
  ('column', 'public.inventory_dataflow.destination_tenant_id'): ((16, '467f0b932ad58dc41fad8b539962c1256b28475d05a2d813e596d906418e82a0', None),),
  ('column', 'public.inventory_dataflow.destination_tenantgroup_id'): ((16, 'f544b84cd61b715797f4863a5ee5ae0fd36a9369ba9e462ea1a413f0db7a5e3e', None),),
  ('column', 'public.inventory_dataflow.destination_vm_id'): ((16, '874bae9876695baa4b1f1c85718fbbb5ff03986fd0e3999cba37c6213a6606fe', None),),
  ('column', 'public.inventory_dataflow.source_cluster_id'): ((16, '2530f83bce684b7f0eb9987a67fde7551412f38d95ad14114bcf34084b57ef21', None),),
  ('column', 'public.inventory_dataflow.source_clustergroup_id'): ((16, 'f198bf48490dea0efe06dd25044a72974996edb3b2ebc41e305caae089b684d9', None),),
  ('column', 'public.inventory_dataflow.source_clustertype_id'): ((16, 'a15b9efbfdda0d8b6a8a74f647a0b0b6f538f22ea5989f31de8eedcb70028867', None),),
  ('column', 'public.inventory_dataflow.source_device_id'): ((16, 'ad6fb9621f4fc8d7dab5d5e69ff7d38644705d9a50c59dcfb65d47ef5bcdecb7', None),),
  ('column', 'public.inventory_dataflow.source_devicerole_id'): ((16, '0f7bf13e6b5f7c985180c149254160ff73e54e495f386ea0022bdae459bdf87c', None),),
  ('column', 'public.inventory_dataflow.source_devicetype_id'): ((16, '1dd3a888b92b87b273f872b3b57fabacec7af84e2b2940076dc07b06eb6f2523', None),),
  ('column', 'public.inventory_dataflow.source_ipaddress_id'): ((16, '9262711cdced570aa3528c6d127f24138612384089ec48f9b70612d317f5ecc8', None),),
  ('column', 'public.inventory_dataflow.source_kind'): ((16, 'c2f0895e6da4cb37548415841020777080060dfcf22488ac4107efe97411d51b', None),),
  ('column', 'public.inventory_dataflow.source_location_id'): ((16, '2caf921940496398dded9e35eab0cd7489a5a817b987305b514c520e42e08dca', None),),
  ('column', 'public.inventory_dataflow.source_manufacturer_id'): ((16, '0a0de06b2fbbc34b4c893705bedbcf730d4636f5459075e3220de67028fe85e2', None),),
  ('column', 'public.inventory_dataflow.source_mode'): ((16, '7454479f177741128780ef43e7a455603a379661e25b80c7afecb922c79766d9', None),),
  ('column', 'public.inventory_dataflow.source_platform_id'): ((16, 'd83647df575dd1f08a4691222701c4655f5642e9fe07344711b773f6c971bb48', None),),
  ('column', 'public.inventory_dataflow.source_prefix_id'): ((16, '61b3604941121bdb5c4403dcdf70d945478236cf70b7f729f607ce21b88d97cb', None),),
  ('column', 'public.inventory_dataflow.source_rack_id'): ((16, 'df4c1bb32b9907cd84005826e52163302e4d049914b2415446e831d037536aee', None),),
  ('column', 'public.inventory_dataflow.source_region_id'): ((16, 'c9577f31ff7b9535b97cd59924f816077abaec67d276812b3b973c56389bdb83', None),),
  ('column', 'public.inventory_dataflow.source_service_id'): ((16, 'a77d6e01bdf373187da9e208eb9e336a94e45c9eeb00be338864548d013e8b1c', None),),
  ('column', 'public.inventory_dataflow.source_site_id'): ((16, 'a61c47fc2577da21ecf2466fe9ad923c4ae1b9be2063b9641227b4394932810f', None),),
  ('column', 'public.inventory_dataflow.source_sitegroup_id'): ((16, 'aa4535f7f2a7e64d8e2307c3a60a9120df94d1659fce506d38272e0d9949639b', None),),
  ('column', 'public.inventory_dataflow.source_tenant_id'): ((16, '4b410d3851da8b856c849ee8ab687f26337a15e4add2d30bf9fa5663e428948e', None),),
  ('column', 'public.inventory_dataflow.source_tenantgroup_id'): ((16, '79552ba82524fb6903ccf148eef01174e4032ad3eee9ade71f103cdf25dbb02a', None),),
  ('column', 'public.inventory_dataflow.source_vm_id'): ((16, '94a305e70e825e7e92621a00f241418b4da1ff50aceb14462abf0dd034c9e2f9', None),),
  ('column', 'public.inventory_dataflow_selected_ip.dataflow_id'): ((16, 'd3d6531aecf98bd98a986cff215981e9899897c965b21e750b9e805ce4dff3a1', None),),
  ('column', 'public.inventory_dataflow_selected_ip.ipaddress_id'): ((16, '74a87d478a2e13c63c87a233dd7e9ba2f6132eb50ac9429da5f7eac1310469b7', None),),
  ('column', 'public.inventory_dataflow_selected_ip.side'): ((16, '390f16181f5850138149ecf3671e109f823791aa32b600dd5370415cf7fc9fc4', None),),
  ('constraint', 'public.inventory_dataflow.destination_endpoint_kind'): ((16, 'dfd0e565243b846e4cb3024c3eadc6490ee8efa7fbc1e4857ace16a3099ea35b', None),),
  ('constraint', 'public.inventory_dataflow.destination_endpoint_reference'): ((16, 'ffb77501b94656b3c5c29c102e439f4d377b5b6bafa055cf8901b6df92dc43f8', None),),
  ('constraint', 'public.inventory_dataflow.destination_fixed_endpoint_mode'): ((16, '78f7dd593fb76ccdd66070eef6526a831150ccf06fb6d4c211830957fab44cdf', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_cluster_id_fkey'): ((16, '662106ac2af63a428e858a7929db69e840db4d60536b874a5d3a0e69f7f655a9', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_clustergroup_id_fkey'): ((16, '140426a1c58b0d4387fc1a3f0aadd99f8118927854a708df148333edff3c57fe', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_clustertype_id_fkey'): ((16, 'b352b13eb97f1047444a7a6236a58c83aadef1bfb4a11db6a318713428864340', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_device_id_fkey'): ((16, 'ef0f3b7413c245d351130bc4b378fb8b8f511d8437578e2e3de4f05be2cf7c7e', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_devicerole_id_fkey'): ((16, 'c079e38fe8922b8ed9360a8d81793424e7cbbbe8e7b9eaa9eb96f91e2f62d845', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_devicetype_id_fkey'): ((16, '1c70fe64ab32a4299c1abe112f560f1f2dc6a8fa564c7dd4777e139877eb372a', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_ipaddress_id_fkey'): ((16, '8d4e8ca5cb088080cf95c7e01a0f567610daad2c67fa740a62325c6ef595630c', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_kind_not_null'): ((16, '2e42dc53ddab1f862098ce93d47e0a7bb92fb77659ffec0f40bcac9b62f9c279', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_location_id_fkey'): ((16, '2ac4cb3aef8bf97020d6c1a878ae5cc0b604193b0166916a22f907579148bff9', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_manufacturer_id_fkey'): ((16, '320a5791664fc43772cc23e3ab3384a1452f24e4b32e8f8ad7283c153a6cbc4f', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_mode_check'): ((16, '44a2a9ba34fae616043633c47bba4d7930da013c1e901120a32fb7078156b754', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_mode_not_null'): ((16, '621b9c9422cd3213da87dda325e2480d8b91043d996293da28a5ba63a1b246da', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_platform_id_fkey'): ((16, 'c38280e459c47b1911e9e6f37d43f8ba4b1104124cd7e6a17f7b06c33cf5278b', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_prefix_id_fkey'): ((16, '3330fb74dc756feebe27601bb6b9f3871213d915f05186163ad8b92884a1d6e9', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_rack_id_fkey'): ((16, '0debcf63d306e9a4bc80214425b7ede493c890a4dc0f1d83a7cb19f5f13eda4f', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_region_id_fkey'): ((16, 'c3d5b9534eefa49b849a32af58a554e89c58d2a5ef2f53aaa9b9449f665c0ec8', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_service_id_fkey'): ((16, 'b79c5617f6ad57c21fa8ee3f9a2c58bd22c3f515a835c5582da7c7285368612d', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_site_id_fkey'): ((16, '28241943f1791120a3c95ac2e7ab8858a840ca5c7849e089875397ae4b458848', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_sitegroup_id_fkey'): ((16, '77388986ad7b0ce9f106aacc4956bb19cfb53bc96e056bae058075cbde766f0f', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_tenant_id_fkey'): ((16, '03831ee44480e596ec431edc9f1be6d63d4818dbac14b66b179afea823b9c757', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_tenantgroup_id_fkey'): ((16, '89a16bbdd56594845baf33c7aec653ddbec50e0842f50a954e0ec5959eab3e85', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_destination_vm_id_fkey'): ((16, 'b435711a1a278b9c670a15b38728dacef0e764e44809ef07c5ca57567692cc29', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_selection_consistency'): ((16, '64bf6c09977cba73b0c61696ad5311af8b3930996e1d9d75dacb33f4e6a6775d', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_cluster_id_fkey'): ((16, '2dee15bd2bbb2a6377e6c2018d3a79462376f6b9a3ed90ba32c0b114572ac12e', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_clustergroup_id_fkey'): ((16, '6efac8d542d4cc1bad574d62c3a9c86a4e430ac335b650080b53f186b0e5195b', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_clustertype_id_fkey'): ((16, '25d9fe46988181eee99d7dd273bff8fde1d07f8aa77972340d8c718c5e8190a1', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_device_id_fkey'): ((16, '9f0b0da8359da8e860f1ba4dacf9d10e3ec253c3f5ce7d4bf33361ccd74247a9', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_devicerole_id_fkey'): ((16, '78584da0386dc22870c3465471f7257c579a0d524c49a085a729a78136957917', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_devicetype_id_fkey'): ((16, '30da286fcb091c451cb252926b5fa20b9d586ca10ff92ff89cf02916104a3e1b', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_ipaddress_id_fkey'): ((16, 'c5efc33984a2f167a17a6665ea499c08bedff13e3917c22de55bf706ad56e68c', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_kind_not_null'): ((16, '4acd54e036dab5304d6c39bf7448caf76dbc66dd28376f717a7ab37ea4cf3fa1', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_location_id_fkey'): ((16, '980534b2c577d668aa6c5f374b04d9fd1a82b663a878e33a7ce5d6532bc20e36', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_manufacturer_id_fkey'): ((16, '9bfe79d55f51731062e1e10c6a94490b0ce26c271b27417b58b79405fac19965', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_mode_check'): ((16, 'f443a6054740e210d5b66dd36cbcb6b64c2dcd732fd9eb1c86677a4e88a4b930', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_mode_not_null'): ((16, 'b95c30e0dc66c387a7823e10b49f013c2f06a10a185e0ffc5405800117c3336b', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_platform_id_fkey'): ((16, '3cebc6aafefbb607f5686e32849e3d7eb066e7f47a7d1d97d454526aa7166004', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_prefix_id_fkey'): ((16, '35db8a2288b76fb379d9ca0bbf104e86b3c7731c3e23da9d0f234ffc5d1feeae', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_rack_id_fkey'): ((16, '928a00bf9cc86d9e9f3778a85c728f08b3b4de24a77e37e50ee40e715fe4ae50', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_region_id_fkey'): ((16, '67c518300263db33604f2f7985a4aed8f266235975f8ca3f4a159d06607617ad', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_service_id_fkey'): ((16, '089095f9c991f8a594d305f9b60a0fb0cf51c9c2a8af565b333080cd5e4c442e', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_site_id_fkey'): ((16, 'fe104b9fe50c4741a02cf5d9fe8c14207559289771d4ae10e663d5a15225ea0b', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_sitegroup_id_fkey'): ((16, '688a82f11823429503210673987c6eca6e99f705af8c190c8a5e91d28101b9bd', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_tenant_id_fkey'): ((16, '97b63936514e1ca24961acebc2b4350e359bdba2407d4625a94037193f48417f', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_tenantgroup_id_fkey'): ((16, 'cc7d6e11ccac00853033ca76f3b95430b2bc4660ea84079f5653c6290752390e', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_source_vm_id_fkey'): ((16, '4d779bc9e2c92ed304effadc99eda383952989cc2766f07603b23faeeab02aee', None),),
  ('constraint', 'public.inventory_dataflow.inventory_dataflow_transport_check'): ((16, 'b5887acfcb2f3a4cc47063824fbdcac02d3a3dffc019d9858185a057ff0681d1', 'ca0e3b873a41569bf1a6651b469ec60e38a123064fbc3c999b9097ef74ab7281'),),
  ('constraint', 'public.inventory_dataflow.source_endpoint_kind'): ((16, '65bd2d5004d57906634dbb018c55d54db11cbd83515645d3522ae288fb19f7fe', None),),
  ('constraint', 'public.inventory_dataflow.source_endpoint_reference'): ((16, '4d9640d2cc60f1c08b0c7555a63fd50c77bad9786863dd3fec33f966bd2b5ae1', None),),
  ('constraint', 'public.inventory_dataflow.source_fixed_endpoint_mode'): ((16, '049d9ed6305a435195c3b55e6b695ba2e48cc260b2d9f3dc40b1a9f9b55d1fa5', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_consistency'): ((16, '64bf6c09977cba73b0c61696ad5311af8b3930996e1d9d75dacb33f4e6a6775d', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_dataflow_id_fkey'): ((16, '5d57b2709239f519187c7124a1117502c1c84fd0c556b1583382f432df830e93', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_dataflow_id_not_null'): ((16, 'f75ab82c25afd38a8c04adec367ebf28b20e95565d6a22d7c0c8f8a5c8ca55dc', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_ipaddress_id_fkey'): ((16, '8809a2da922fe9ff859fe91c3c11662edf1405177ca8e40a38585f6751dd2af0', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_ipaddress_id_not_null'): ((16, '45fe18f24e0c3135d696d2604cd6d23cad85d56751f008116c268f81ede0ddcb', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_pkey'): ((16, 'e39266f3ce413a830b0d1050aa8d1f10bc750a5c830bf7406d467f87c3efa077', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_side_check'): ((16, '7bf74aff223f0ec289479fc5475c9a7112c28fb5aff05593f4f535f21dae9753', None),),
  ('constraint', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_side_not_null'): ((16, '0a93bb00862b87269df7a54c867f678037e483f1474a0491b330da5dc5d6b865', None),),
  ('function', 'public.inventory_dataflow_catalog_lock()'): ((16, None, '89d66127e0bb872dccc33933389466449b328c669027ded29ef7d1a64def0b6f'),),
  ('function', 'public.inventory_dataflow_flow_consistency()'): ((16, None, '32c8d8c72971b236c74f0660b383589a91c1ce0a8b83a0988871f4e099bb5f90'),),
  ('function', 'public.inventory_dataflow_group_guard()'): ((16, None, '1bafb314d10e33ca6ef2e5de47eb37f6adc0913604a9dade8614b81e4fcefba3'),),
  ('function', 'public.inventory_dataflow_selection_check()'): ((16, 'a5af9fb95df7f9ce3814ce75c0e39aafae512c1d0e56620174ff71e66960e26b', 'c71f175e5a186ff2889b87516ec589457b68f69f0c74354649f6e0cb2d0100a4'),),
  ('function', 'public.inventory_integer_array_unique(items integer[])'): ((16, None, '640e15ada10a48c51273ba1b111cdd5154164e69cef07c027621ed1d4faefef2'),),
  ('function', 'public.inventory_tag_subjects()'): ((16, 'a2ab1e77b1c5c442c4b484b2ac602dd4821d94753ec9395191cd736299b0da45', '40be63dde0903e3d6dd107f5be53ca4312afc88d16e64b55b256bb881bce1722'),),
  ('index', 'public.inventory_dataflow_destination_cluster_id'): ((16, 'ff0b7a5070e6b1405a7724907167e109c2184884dc05a4ab55a449b0108b8af1', None),),
  ('index', 'public.inventory_dataflow_destination_clustergroup_id'): ((16, 'bda4a5f405dafd47f182765c5b6853366ab986ad6eaeee5b9d36cba7bcb9c848', None),),
  ('index', 'public.inventory_dataflow_destination_clustertype_id'): ((16, '2594bbb2fcb24a241f10e72b5ccc9479277601586737d09403d884763d0ca392', None),),
  ('index', 'public.inventory_dataflow_destination_device'): ((16, 'eb54ebde76e59d8a71c7272738a29d50ce02c71c3d9b08080378679f147075de', None),),
  ('index', 'public.inventory_dataflow_destination_devicerole_id'): ((16, 'e94070526d7048464dd164e99be544c025cfc99fc37f22003a005478f87c782e', None),),
  ('index', 'public.inventory_dataflow_destination_devicetype_id'): ((16, '214bba87a865c1c18486c5834692b81fb01b2d9041f6dcfa74b75f0580cba77d', None),),
  ('index', 'public.inventory_dataflow_destination_ipaddress'): ((16, '57066227bf6e9f88751809ed5e389205569e5ec12073a06c001adab55c1654fa', None),),
  ('index', 'public.inventory_dataflow_destination_location_id'): ((16, 'bc1ac7f343c248ebcedda539747bf62d81110b75110eb7fcb40a8aa00918f662', None),),
  ('index', 'public.inventory_dataflow_destination_manufacturer_id'): ((16, '8a18dd37d9867de3e2caedf0bb2561f37a3eaa8b3c27ef00c52b4dac9f08844b', None),),
  ('index', 'public.inventory_dataflow_destination_platform_id'): ((16, '0a8c627331c1d4f23ff9e767d9e157b94e29447e2e2ab74c827bde515a4a6896', None),),
  ('index', 'public.inventory_dataflow_destination_prefix'): ((16, '184046fc11cda1ac8ea6e1cf991b0848ba45935be3e4102644f51dea505f6470', None),),
  ('index', 'public.inventory_dataflow_destination_rack_id'): ((16, 'c3f428f68102db4cb21eceb62ddffa1812a4d8c8a5c6d69ea28ae72601fd1e5f', None),),
  ('index', 'public.inventory_dataflow_destination_region_id'): ((16, 'b0545c19bc75efa63bc7396abc970dfb2138795d979a6f1bb0acadfc2eb7f447', None),),
  ('index', 'public.inventory_dataflow_destination_service'): ((16, '90001d7fc75230f9ddf8c21c72079e36cd85500c6fc5293dd3fb380e5e64f7eb', None),),
  ('index', 'public.inventory_dataflow_destination_site_id'): ((16, '6f3a334b42ad74051d07fc0632efaf775f452dd29d08c403c079675e49684c78', None),),
  ('index', 'public.inventory_dataflow_destination_sitegroup_id'): ((16, '44bf7d1154ea2aa4756ca960b1b6c20697a39eda40552c51aabfa4ded91465f3', None),),
  ('index', 'public.inventory_dataflow_destination_tenant_id'): ((16, 'c1c05a3167600510b289a855293fd70d3bea16e7f72c2b5b644b373b21e2f38f', None),),
  ('index', 'public.inventory_dataflow_destination_tenantgroup_id'): ((16, '29d0d3a96b3d32bd40662a7163876085724842a33b1454be9867f3acaf531638', None),),
  ('index', 'public.inventory_dataflow_destination_vm'): ((16, '8b17e6430e5807741c6ffab836678662fc7a77edd4b4a1f5c29d5fe91cf5712a', None),),
  ('index', 'public.inventory_dataflow_selected_ip_address'): ((16, '368ce022bebec20ef148b1a9c45a1de6499b0b8da8663939d6fb6601edfc1243', None),),
  ('index', 'public.inventory_dataflow_selected_ip_pkey'): ((16, '4f05d6d892efed4e9dafca90e469370f82cf0329808ab7889f4c77e04347bc60', None),),
  ('index', 'public.inventory_dataflow_source_cluster_id'): ((16, 'caef3907b023e4faa99b8eed8854ad3ea52609cfe8bb320928707ecaee5b848c', None),),
  ('index', 'public.inventory_dataflow_source_clustergroup_id'): ((16, 'b7e1459e91d08c1976283a1343566989baaecfd5847937849dbf0fe50ce43dc5', None),),
  ('index', 'public.inventory_dataflow_source_clustertype_id'): ((16, '850bd66f4a87dcff6ea046ff3da5ab1fd5c4d44fea929239d63edb1e9d674efd', None),),
  ('index', 'public.inventory_dataflow_source_device'): ((16, '8e5a1ac20741afaa13169d37636784a378f9ff0686b4bee7f79c20ff935b63c1', None),),
  ('index', 'public.inventory_dataflow_source_devicerole_id'): ((16, '61749090915164fb7f97c50af58c00ddaa877a23a0ca8c50be9f263132b079a6', None),),
  ('index', 'public.inventory_dataflow_source_devicetype_id'): ((16, '4ba31d5e2c7a00f703ea9b1b689b81af6cf701fd737bb55830b13e1ba0cd9984', None),),
  ('index', 'public.inventory_dataflow_source_ipaddress'): ((16, 'baa5fdc0f7cfd44dadb6072a7be4d65ebfa800978c6b4cf6b7f68a48138f2914', None),),
  ('index', 'public.inventory_dataflow_source_location_id'): ((16, '3cba7cd72fd2505374f36ae4fb422ef5d8ffd01a52cc2646c7d8cbcc166d643e', None),),
  ('index', 'public.inventory_dataflow_source_manufacturer_id'): ((16, '7fbb963f7aeaa9199fc2b8ec2248fc9c51c076d9d2b9ddfc531cb2e06b225e26', None),),
  ('index', 'public.inventory_dataflow_source_platform_id'): ((16, '19ea9a2a4340d75123edee63fb4a6a22ab1fe4e594a0057afeae8ec2f1cf631a', None),),
  ('index', 'public.inventory_dataflow_source_prefix'): ((16, 'ab5d21c3fae8df0f27eaa03fa538a9ee3a593bf747244d845018c3f35e465414', None),),
  ('index', 'public.inventory_dataflow_source_rack_id'): ((16, '486edd477870fbe581b588b66763b9cd86b15f117d3aad0dd5516a5edc7cb2da', None),),
  ('index', 'public.inventory_dataflow_source_region_id'): ((16, '1fd61e4a68fa3e8d4ea8abbd01e48e44f38fbd635aec899ac2069ac9fd8322b1', None),),
  ('index', 'public.inventory_dataflow_source_service'): ((16, 'd10062afc8802ffff661ffb4a059bbbfcca1e37eaa0fbc4dd8f39133f6e4ac1e', None),),
  ('index', 'public.inventory_dataflow_source_site_id'): ((16, 'c2b95e73c27f3f63722d9afecbec7fd94ef337b7df6b5815efc332236492c05e', None),),
  ('index', 'public.inventory_dataflow_source_sitegroup_id'): ((16, 'c7939f28fdb17980adef8a71359377c13c8433f09dcc3449ab2958e4078fa9be', None),),
  ('index', 'public.inventory_dataflow_source_tenant_id'): ((16, '7923ee901fbba00790c17e65d11748de9085d8681352e9f223dd472b43f99a9b', None),),
  ('index', 'public.inventory_dataflow_source_tenantgroup_id'): ((16, '090538139359720e55739fe77bfb02a13e539a892d700581a3c33e2d4b4ccb4b', None),),
  ('index', 'public.inventory_dataflow_source_vm'): ((16, '319bc95e77dbc25ca8a96f811841e4147b45e624c55e44da279572c79ba0572c', None),),
  ('relation', 'public.inventory_address_group'): ((16, None, '08b0b22e8276e49aed4e94e3964dcfb01cecd368e060ec0895135906f2484ec8'),),
  ('relation', 'public.inventory_application'): ((16, None, '08b0b22e8276e49aed4e94e3964dcfb01cecd368e060ec0895135906f2484ec8'),),
  ('relation', 'public.inventory_application_role'): ((16, None, '08b0b22e8276e49aed4e94e3964dcfb01cecd368e060ec0895135906f2484ec8'),),
  ('relation', 'public.inventory_dataflow_group'): ((16, None, '08b0b22e8276e49aed4e94e3964dcfb01cecd368e060ec0895135906f2484ec8'),),
  ('relation', 'public.inventory_dataflow_selected_ip'): ((16, '08b0b22e8276e49aed4e94e3964dcfb01cecd368e060ec0895135906f2484ec8', None),),
  ('relation', 'public.inventory_dataflow_selector'): ((16, None, '08b0b22e8276e49aed4e94e3964dcfb01cecd368e060ec0895135906f2484ec8'),),
  ('relation', 'public.inventory_dataflow_selector_ip'): ((16, None, '08b0b22e8276e49aed4e94e3964dcfb01cecd368e060ec0895135906f2484ec8'),),
  ('trigger', '["public", "dcim_device", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_device_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'ec227919b507d13fe8ac8e473020f82cbfefc876761744dcaebdf4dd48a2a531', None),),
  ('trigger', '["public", "dcim_device", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_device_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '6d72f5eee5bb171ada1e51a67d8ab4b56269b01da0d13103aa0473a47de2a31e', None),),
  ('trigger', '["public", "dcim_device", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_device_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '22357862c0a405f269f3ab74f46a72d2302243a5a55ab2d87b2dd395c992e335', None),),
  ('trigger', '["public", "dcim_device", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_device_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '78316c440c87b6df02ac0b5d624a7d2da65f3ee09df379d11c7b5cdd3efded83', None),),
  ('trigger', '["public", "dcim_device", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_device_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '58b14f954d27e0926e109a911a11c1c37a3c00af739b0af1e5a2bee143396491'),),
  ('trigger', '["public", "dcim_device", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_device_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'b877394e4ed2a9b7810c93fb5ae9d485d8d3c42cee0ea8984495bfc340c1557c'),),
  ('trigger', '["public", "dcim_devicerole", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicerole_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'fd9e2be6db9caeb177c91a00e769f9400418676517cd450cf7f56e7bae1485c5', None),),
  ('trigger', '["public", "dcim_devicerole", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicerole_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'f90b998a7026418a0cebe37c91e6246eee519203407d7b134fffcc7b72c0e997', None),),
  ('trigger', '["public", "dcim_devicerole", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicerole_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'bfbe788570aa4a62bd20bce80ec6a3c3eb2a93a2198c65c7891a228cd33dff32', None),),
  ('trigger', '["public", "dcim_devicerole", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicerole_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'fca79dc9522ecce96e2d61dd48ac56c5ab9b1e61b95b85e28d29e6ac1b519a7e', None),),
  ('trigger', '["public", "dcim_devicerole", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicerole_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'a97e89225ceaf47355e678f88ffc00493510d87805bebbb91427e77d3c177d39'),),
  ('trigger', '["public", "dcim_devicerole", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicerole_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '9746189699e268d74bd6f46daba83ea01153e05daf3fc28a5336f9224d4c3e5b'),),
  ('trigger', '["public", "dcim_devicetype", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicetype_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '05aa5b04453bc9a07665ddd83233b851c971018b5bde381ee890fb1acec7c33e', None),),
  ('trigger', '["public", "dcim_devicetype", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicetype_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '5d9d6bdb88cbda3d391bc833010e250baa0a34e02b9a79849f7493182c5025b6', None),),
  ('trigger', '["public", "dcim_devicetype", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicetype_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '723d732250fb1aa4ceab127e8da2708a96fcc777061bcfb06ee822db21bbeb42', None),),
  ('trigger', '["public", "dcim_devicetype", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicetype_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'f31070349877b84e09e74077e5e3f38d53dd47adbc27423b62ae2a5072d88fb3', None),),
  ('trigger', '["public", "dcim_devicetype", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicetype_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '94e71dad5a1fffcadc6253871ac277fa09e6f6d9752706a517df59b075fd6f98'),),
  ('trigger', '["public", "dcim_devicetype", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicetype_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '93d88c2b6536f80f6e10d342dd4795354505ab830e290cc672721121f2a97595'),),
  ('trigger', '["public", "dcim_location", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_location_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '16663a12fee74771bad691c7cf4a17740421db8d80a2555294de0111e522521a', None),),
  ('trigger', '["public", "dcim_location", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_location_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '0c3d9bf18bfcb1add4631394db488a346219e85016e2431c2dfb6246a66882d9', None),),
  ('trigger', '["public", "dcim_location", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_location_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'ed0fb17e7a172f1223bf407bee11e53027c53a2770bef26479d1af731b168742', None),),
  ('trigger', '["public", "dcim_location", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_location_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '27c5e15a7f2a319e3fb05049aebe833c896635260b0f5c8d62bdf4283292be79', None),),
  ('trigger', '["public", "dcim_location", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_location_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '0915d3112f310176d95be06b5d1dc0e452d226a6eb952253c259fa121cfaf482'),),
  ('trigger', '["public", "dcim_location", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_location_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '638be3ae3ee6c1616c89e680b49d787bc7c10d4a48780b20e23f7899a4adf757'),),
  ('trigger', '["public", "dcim_manufacturer", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_manufacturer_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '0556c045926753be8bfde01255ea2aad472179e2784d0eba45d5c6b229dfedfc', None),),
  ('trigger', '["public", "dcim_manufacturer", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_manufacturer_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '74a058cec92ea4de5e45fea37e077b3a127bb184d454c8cb7fcd08db966f7ae1', None),),
  ('trigger', '["public", "dcim_manufacturer", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_manufacturer_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '74ad31fc9a196157063fcb63b0a05dde2402a59dd4b411741a647a6da3a4b38a', None),),
  ('trigger', '["public", "dcim_manufacturer", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_manufacturer_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '84f4787336592ffad95391348dd04720265689ab642a278961afbe478fea8629', None),),
  ('trigger', '["public", "dcim_manufacturer", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_manufacturer_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'a67c58030dec517ea2257de1adc5fc69271c5f03cd53355fbc341e3b17ffdef5'),),
  ('trigger', '["public", "dcim_manufacturer", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_manufacturer_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '26bda8587c0ef98127752cfa3ecd6099f4a63a5342399bc9aebc2a23fa29facf'),),
  ('trigger', '["public", "dcim_platform", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_platform_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'db5e28b9077ad4c470ff3dc1cf831897a67618b733016fcca6f7940d35a13ad5', None),),
  ('trigger', '["public", "dcim_platform", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_platform_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'c7cbcd2cc050f3a3109b399aef51e3ce567f55ea26d5157c87e223381a409171', None),),
  ('trigger', '["public", "dcim_platform", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_platform_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '6f83f0151a8c5b4ad37a95d39f3d8c9e5576487bbfce14b55c28ed685a3b5d07', None),),
  ('trigger', '["public", "dcim_platform", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_platform_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '971361fe3e00344ba5406f69c8c183b156fdb5cb15ed4cbf2caeb6a2b367b5f3', None),),
  ('trigger', '["public", "dcim_platform", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_platform_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'ddff9108d4f0df139ba9dbd3027614e8a5aec8201257a7e5ca9e4efb891dccbc'),),
  ('trigger', '["public", "dcim_platform", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_platform_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '57716281b49bd20a8a819a00425fac03d088ac48e8f08cc653c8f74814d69627'),),
  ('trigger', '["public", "dcim_rack", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_rack_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '02b0b6a96b1fc89891c3815dcb517fb767966318ec1f19e9e48fd1bfc07b5a5b', None),),
  ('trigger', '["public", "dcim_rack", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_rack_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'fbc776f65b0da5c8a3f4c34f877992fe360fc5c9071e18f0e58bd1c50d516bde', None),),
  ('trigger', '["public", "dcim_rack", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_rack_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '88943aa0aa3d62c6ae25bd4471092be865f41b25d12c68ca96c9b2292a24a7f5', None),),
  ('trigger', '["public", "dcim_rack", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_rack_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '0d3ab122ee33a76ebf2a633ad4bde232c65af48bd4c9b12102735c6d6fe8a24c', None),),
  ('trigger', '["public", "dcim_rack", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_rack_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '4bf8beb698946165fc3826a8bbe65feebe34f55739248fac89f651ee5784b52a'),),
  ('trigger', '["public", "dcim_rack", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_rack_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'eda5f7c9c05d2a30b2593103a655aed96b32f6d2d537c0a472766a63bccf156f'),),
  ('trigger', '["public", "dcim_region", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_region_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '40661e3cd762144e4df60a7f5752b44e085179172e74f4ae334534ee678dce88', None),),
  ('trigger', '["public", "dcim_region", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_region_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'acafe987cfa65dae0e708bc1ef575546f11a0ecc743b377618159c32a47db5e6', None),),
  ('trigger', '["public", "dcim_region", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_region_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'bd0f56e737e344cf6002401b686a08cac12dd33fdf85f688595d3fd43c547ebb', None),),
  ('trigger', '["public", "dcim_region", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_region_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '4b738f00bc72b1b55c8903d9d75fbf88dc9f449715fa88b0bfa357baaaf8639f', None),),
  ('trigger', '["public", "dcim_region", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_region_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '6409a23a3f287ef8b02d01e48efaa0f0f9034286b8301b2d2969f9e7e7f20cf6'),),
  ('trigger', '["public", "dcim_region", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_region_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'd85290ca4bc07da7bbc662374e75e0e482a1b646354752f9ac48d20a8a46237d'),),
  ('trigger', '["public", "dcim_site", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_site_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '750946f44b9052ba6a502ebc2a3235774e3c574c237e7469629512154a98ac30', None),),
  ('trigger', '["public", "dcim_site", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_site_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '3046324cdd6f9003422fae6e26ec657b4c1c7cc548f02de4a906968e29cf9e7e', None),),
  ('trigger', '["public", "dcim_site", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_site_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'd8e4e0f4efa729862c357f221fc408d44af5a057b6a1f01fce2cbdc49b52272a', None),),
  ('trigger', '["public", "dcim_site", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_site_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '13b514a5e9097e687455a20235ba6c13e5ca12eb8eb883c50098ce0c6cd3d711', None),),
  ('trigger', '["public", "dcim_site", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_site_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '125f642eb5a22e828bf9be18269dc4435eaff8904291bf4533dfabc0615f1ee0'),),
  ('trigger', '["public", "dcim_site", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_site_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '2e334cfd425ed2a01f8246159b21bcb101d6f7f1c744bd38629f27a8f137fce0'),),
  ('trigger', '["public", "dcim_sitegroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_sitegroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'ca45a60e76d42da2f47d14c9df79b07f7a06883584bd7e965e6169b24d980d36', None),),
  ('trigger', '["public", "dcim_sitegroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_sitegroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'c85fd63f4821942ec914a8095b2037a4e668453361300d3a52e6ee91e7ca7fa2', None),),
  ('trigger', '["public", "dcim_sitegroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_sitegroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '972e0cf49734fdf6f3850ed9015ec87184facca0b19083066944606c16791f0a', None),),
  ('trigger', '["public", "dcim_sitegroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_sitegroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'e7ae4d03cc50efac15a6cdc7195f4501623f6e682ca44893a4def79ec9207478', None),),
  ('trigger', '["public", "dcim_sitegroup", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_sitegroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'd3f0c8fe6e97c8a548cf70f77e7c8b970c1b8a1d8717298b733f73023931d47b'),),
  ('trigger', '["public", "dcim_sitegroup", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_sitegroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '9f8623693d64eca6c3d059c8b3b24f767e94d4ecdf423090a8ca793c3d89671f'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_address_group", "inventory_address_group_owner_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '670567dd0a3ccefd72786b5740342eb69ff5fbd248523f25a9049e5ca42923c7'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_address_group", "inventory_address_group_owner_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '2cb6b94b93d65d1b486c608db79ca5ae604e536d5fe0a727768600f26d2a141e'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_address_group", "inventory_address_group_tenant_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '5fdfb4d3c2eae1380caf7e12999bc4f3452376666da1f30eb517dae6b9501363'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_address_group", "inventory_address_group_tenant_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '4c13d6a16f068a2756bb86dd3ce250f08d1f78486d8afcefee01245f9d40e43a'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_addressgroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'dd8cfdc6f3c993bf1679f122016aa84436be4353572a05d7cc48a9d01f4f9bca'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_addressgroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'bfb3ddf33b4031df70594d3f07beaa96f64cdc15cbf9efb617b2d43366e3124b'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_group_id_fkey", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((16, None, '6340e84f0ff8177198dd4b5d33f18e40009c2016b916405163afff40ef2c15ee'),),
  ('trigger', '["public", "inventory_address_group", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_group_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '8029614960ea44ea5b65a88c3ef028639a5c5ed2fe5db4b40568ba2065ecad09'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_application", "inventory_application_owner_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'ffcb69eb6b1fbd69aa61c19c113dfbd48e54b2bf48619bab76df4a288caa9c4e'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_application", "inventory_application_owner_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'e064912382000fdd9150ff026a0cf4566bda1c9f363669502eab3df85fcbfd59'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_application", "inventory_application_role_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '6fc7fa2aac72866c646a9ddb7a4ffb0f7a80e84998fb635d5099c008c6277631'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_application", "inventory_application_role_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'e32b3696c6b0a2307a5a16eef97ed10a19e0fb453a6e5284f92a6a2681cc19ef'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_application", "inventory_application_tenant_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '3faaa2f93e657249b3fd4862d5818739bfd435ad3cd447ed89f2c6ea59607c6f'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_application", "inventory_application_tenant_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '64f23dad3a9c767e45cdbb3e2963108d366b98fdd63b198ceb38a7b7c784d22a'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_dataflow", "inventory_dataflow_application_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '01af579289b26107a1bc48773aea6085bca5e3dade5c9675003d4e8b27385490'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_dataflow", "inventory_dataflow_application_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '3fcec599296f969b74e35d92b673539b967ec1d56681ce5b08502912e99fdccf'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_application_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '67a16586390237ea1072912bcfb00d20d631968b790db37e8f8d09eecd554a76'),),
  ('trigger', '["public", "inventory_application", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_application_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '2d6cc2935af408adfc6be4387bdce68e372013f5f086c2fb647ef8f8d779533b'),),
  ('trigger', '["public", "inventory_application_role", "internal", "public", "inventory_application", "inventory_application_role_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'c1e2ccea2da60cb4a4057a5fa9fcfdea6c173d0ce205d6c79b7145f8bd018e86'),),
  ('trigger', '["public", "inventory_application_role", "internal", "public", "inventory_application", "inventory_application_role_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '002234297028e7bb1474310ef4461baeeb84455c0ecabe5be6507442d2259caf'),),
  ('trigger', '["public", "inventory_application_role", "internal", "public", "inventory_application_role", "inventory_application_role_owner_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'ba952c7d2c9258c512951eb63b52401a7dc33057f85656d956930b4af69e4603'),),
  ('trigger', '["public", "inventory_application_role", "internal", "public", "inventory_application_role", "inventory_application_role_owner_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'd151c893241f317571a4bf01e198e259f893126e56b7b3539b7d2d417dc796cf'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_application_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '82816ee4986f7c086c2287c4ab539643b1fcbc4e49cf8e5be23f133bd24d1a4e'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_application_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'a3104eeead1608c2ae9c07aa961ba401557551d44ba9e6a07ceb6df1b90f4e5b'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_cluster_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'dd0fd5d389604c55ffa934fca582d604cf680522df09f96089b26029abaa96f5', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_cluster_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'c0b383086b1e6415a3c01652648be76fff1480cbc0cfe9e87798524f560e5a39', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustergroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '9d01cefb405bf28299ee66c486acaeca0a8487863efd0afd13a4a8232e0e3caa', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustergroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'a4fedea36c5f629bb62d9ac7fe4a8dd5bbee6ea8b9d39d71ec329a9b8765c326', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustertype_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '83c04446c5f4eb63d569bd525e7c9954c9d9447408c2a09796fc2c10d6565cd4', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustertype_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '99335134927f4b69883d3619547cb9cdf57ef017650fbd6a17e5540d6f59fcb9', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_device_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '9ad59b0029e97f53ae4d59ec0f25fa768d7424952e7cd935a83ca89468acfe46', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_device_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '290e59a40f05cd7dc9ee249171042ccac7215dd6ecd06945f06e38348cc0e1c7', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicerole_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '68850bbd37a16a50c2ab73b9fc3e7fc478b6820a31b4ad08d026cb08324d1323', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicerole_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '86693f528333cf09bd60ae653b6b99eaa349990f1bf0a15072a4ca588bbdd2ef', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicetype_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '0a973293c687f56e6c9c776b58da46c29ad4e61639080b4fd479efdb14ee5669', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_devicetype_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'a1842f84cb494686a38cdb9afd3264a6d6eb5ed35e9ffcdd962b5dc8f54c4225', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '7da8eeaa9b0bc20bf2e788ed62a5e926774553ae330d954838979b001e671b3d', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'd1fe7bdc1d1dacb925ccf3ee70df8b650e1af7e124567a3d5c610ca400de1d6c', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_location_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'fb0c770f4391c4789a063cb58f929c17d1fd12195c3d9812c849c506a7774750', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_location_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '1256c192fde9ed4567973bbc189c08081040a7f19e24c59c84d55e7604a8ef7e', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_manufacturer_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'f783526b0b3adb1fe628d10314a6588abb74c465d2f76141cebd9cabe7d540fb', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_manufacturer_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'a8cfab3c03bf3613691173b6e144a7af35fa3488dcd20ec1a60bc963bc4e24cd', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_platform_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '673995f17ce19a02dbbd5bb3039c2621217ab2ebc59100c0f1f3d4f3bee52a7c', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_platform_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'ac4862076b266f944c019d4f353ec78b77f494be79e7eee86ceabcd72a9a9782', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_prefix_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '6ab8ce550b0c576051ea35c2bdb02d2a23be2b28d4dab6b0fdfe9fa2fa029fce', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_prefix_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'aab0b99f9d6e2c1379bb2ee0c53b4e2a3e3f9bf123b75b71026a1c5bd6f46b49', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_rack_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'ef43daca2ba6ead739963fa0e4d8e12ad82aaa7028788e5dc5056141cf857218', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_rack_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '2deadfecbdcbc80adcc93217ce93dc5e203ac9b0d0cd545a57e28a04e3130b35', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_region_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'b68d1e018a50c7fec2238d1ebcae04fe77728c2fe28eac18d319a82d8b789660', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_region_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'a56926cb072197ae9ff5b7ee2ec5641ed725a057517a68105eee21616c44f240', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_service_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'e14f6359d08eca6994ee2a874230ec0bedb880c8fab6fb389566c6de24843251', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_service_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '6caabcf75b161fa534a404dccd6ad8b49b13d1efdd202c250b7cd84115c01ebc', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_site_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '7147a7e59303c77bedbffd29031eb1408f6ef22da2e58ce448067c35c0e2cb97', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_site_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '3c6257c53509638e78f7d3f30cbe404b338a2cec18f1f9fd808c1d7e6a72df24', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_sitegroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '349785b7faaf55b3592c8682fa271b967834161cedd5e5acb466aa8180947bc3', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_sitegroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '931412d0b66f679ee56345d595f9901c635eef73a9e785a37141f60e4586a84d', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenant_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'b9588dd9d0a024e91f3d7e2ce807aef498a19db47626b6c3eee3f16b87c87479', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenant_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'd54e76d06b19ed8048c0f2b3359cb3c8a0c694002ade1b4b1ef7f2ddf8990fb6', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'f6514d6f8fb5f3b54adcd9fd7a920d65e0a81f585f27820a768cc6e39c6e5e32', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '8eaea6bb52b04e49a02fbf4683fc686e7a9950429abace43bb582cec809872b9', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_vm_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '05638f0047b5fecdc13f7025125ebf77a729812ef4f777ded32b4a7a1a1452ac', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_vm_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '65e1c59056ab23df2fc81bfd235b2475cfd46d442018062de17cd51d316f01e8', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_group_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '3faaef8e43baeb31c19dee1c5ccc528843fd68e20287efba21a417fe91aa04f3'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_group_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'e71c787ad62324a5ed656caa045c17e813bfeb7943fa763364dacbb1aa96e934'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_owner_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'cdad8410af3daa6b4fd9a271012d485054ec77e39f891d1e4105810b984c2c4f'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_owner_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '3ca5ec8a7f49db6180d813a7576e3d44257f0b40e8202efd1f797660b132f77a'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_cluster_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '1e0eb0d650445f1a278b44761a8a7a74c6b18e3a69a929026e4aec61b66ec11a', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_cluster_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '0f0b749e7cb539345b788fbdda66ed77ce412cee5e29d085cbf987b988e1ae8c', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustergroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '98ab76832cd941ca88fb798e252765e194efd79b52a466ff620c115dddabd0f6', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustergroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '985fd1f92c4e233f33779dd9918270955d2de44d12986478719d1cd47a6ce15f', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustertype_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '027ed607282e6431932f13383bf8d140964cfe470ceb87e7f4373256230fe9c1', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustertype_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'f924fdd66d9856ca0d3f805ce8464a49af74a14bb47381aa36f47ef23082f458', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_device_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '3ab60e5132a6da2a7027de11d81e3f06ebee3844db8f00601e272357260fa0c4', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_device_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'e385d1d71b98c2b713bdb0f5901ba52c0b1b6bbc0f6f9bb5e36ba102d1d67e21', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicerole_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'e3bc4c87107bc01b3d3ddc4d2965c571f56b80597a56378484b5b804a01ed378', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicerole_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '57c85f26fe2f9517cb685ebd9c1bc71c7dbd5dca3767a8d19271e7af3b925117', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicetype_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'bf1a017a0351becf0a0229f114bea6e7bf45dde9fb9437810ccacae356600a6e', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_devicetype_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'f4fe2669cb982c11e5bc795b6c45af47be167fc66e645c352e18977091e5e317', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'f1762f3d6a6806d709abc1db33b505f51960a1a774b2564b6c2af1616ca3c189', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '682c9602c97009690159beac61f296718413fa679a26ec93e9443fce2d692937', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_location_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '1e57422269bd9e94afab743f56a62cc686af6cfaa039295d467d247110c29928', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_location_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'fa9c984419414a9ae9984e2420601ac0141094ff2d1ff8339f1a7441e9c74790', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_manufacturer_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'f9d2e41cc5dbb202359d76451555b3b4f4cf4e754ccbe8ce0a6999f5c0d60328', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_manufacturer_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'cb5c11378d484ee0172ae3bc37e580c4a0571e1feeab956416fdfd1317415106', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_platform_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '332b0fe8338defdc2880d3326043b2e1bfe24d4ef3f8d3199ebdf259f8bc1ab2', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_platform_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '5f1d693a41fac0c7de73414696c1daeb9260e33dc3fa7cf2bea8300f63c7abf9', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_prefix_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '9473cfd750ad51f424b3cc429389b2fe0ad7b58bb2399141b7ffaa3bb1568adb', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_prefix_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'f258d2237721149f909f65fd82e75a0ac349d9d7cc27e2b81ea31f255b304998', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_rack_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'e190ffdbe2e1cdfcbd4dcc04d5da28ad49006d58cf46560dc12302d29bbad815', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_rack_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'c7b244fb2884ce023b63fc6a934bae5b5d26f4acab0a29e0601ef4abc9de6abd', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_region_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'bb29f62cb9f0d0d64ce06296f2174b36718adb5189af5a0360ab4c0804cc4f0d', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_region_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '522ca1f6c2a48edaf792600708122705231089c9db590035335675a67282c178', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_service_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '650d916549a2f7fe2b847bcd5db921a12185dcbddd69de6cca76570d201b0273', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_service_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'daa0545cee1435384fc3b97c738c8e36cd25dc8cef529ad40bd5b90f818a7f42', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_site_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '1f19a78a7a9c14c525caec047a73bf30c5f4915c3599835ab9240b3bd5da1e49', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_site_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'fa4d96c78b46a77bb1fe9630422b0f189e81ad643b43b8c80888ca04f237e545', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_sitegroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'a5e3c083f5375d2d69a2d2c3e6ef2e31edba64b8b90e5355935d09f5a68e173b', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_sitegroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'a2c03e6ae586ca516dccc273fddc16ca442b6e2b01fd4ba609da3e9b946236bc', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenant_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'a207658d71c211bb18459330fdc2f336ba6ce50544f01f68920decce845000d9', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenant_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '8f9dc77cbf76eae8f93bedf6522713dab11f18795908fcef4aa92e234c9e5bbe', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'c6e4f145bf5e6a3679a5b581eae750a1f41ea7342c01ea3468038888acf3ce54', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '913a42b42f961f3755a35f63698261db4933c60f0b2d143df3b3464ea8f07f1e', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_vm_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'dc7b642b3780762e1942797ed93d8ead5f5654cc09b984e732b1794f19212356', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_vm_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, 'adc64a29ee8c1964e04455a865632e842b2007b77cb01f6a3106a07d64af0058', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_tenant_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '3e4ca09dc522103d7fd20d6863279b8e65eea6980c917b5d3e31a2551f52052b'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow", "inventory_dataflow_tenant_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '5935b5bd69eb69936006ba08d064612d2bf349decf4be809e6f1d363ad743896'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_dataflow_id_fkey", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((16, 'f1a60e78208d92cc055b929aedadfa1cafe634840cb828809c60fe104ae8bb2b', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_dataflow_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '10727eba2edfd5f8f3f13b4c0f588307d756916db2e883da52046e73b9575687', None),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_dataflow_id_fkey", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((16, None, 'e6e254a754ec2412cfdd93b62ea75292c81735d487d24d02a2a8dec7480f1506'),),
  ('trigger', '["public", "inventory_dataflow", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_dataflow_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '638e6e39d7fae00c9898c4352ac1967bd23bde9c3fc1afd81850a7c760539631'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow", "inventory_dataflow_group_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'd726b6adfb621337c7904215d918db311539a4438ae11a50e2e6c4da8811ea0c'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow", "inventory_dataflow_group_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'b33c7c9df31a1f55291313bc7b72e665a88b698673a642ff56b3fa70eb87be6e'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_application_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'b93492f1c928c8d0b50b59a7173a87d9192728e5f88350420941fa6b02d81fd4'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_application_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'c16ccdac236fa8f30e3792010c2d1382e29c7b3fd5a122fb5d85349d5eabc3bd'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_owner_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '2dada7fefb8964124eb4aacd4b281b08e256b1cb160e51038e0f5014ace812f0'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_owner_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '18e86e1969db6d90b7afa16c7a547bf6fd99c85d7be675d9e63402cf88cda9b3'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_parent_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'd0adff717e683966a5cde9fa54f637df1ce37d59c5080efa6e2c3f44e6794e6e'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_parent_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'c6e77413e42ddf9744e62718d271ec0b33e1d94bea2433172f2dc8442a4e1683'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_parent_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'ea0f2174db6bc7a6a20fe643a91d3be9147b214ae87b9e02aeebed1ff807f910'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_parent_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '94053481d83378638477c99061a0d30e9932c3ee4bccebc6dd14d5f44d3267c2'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_tenant_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'b1c1259e1c2a1b03333b7693dae1b9df8ecb64b6e5d549608912e8b6b4bbb46f'),),
  ('trigger', '["public", "inventory_dataflow_group", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_tenant_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'f6b773cf9bb81242146805533013cfacc7dcb57ed278d1054cb42b49a90d1cec'),),
  ('trigger', '["public", "inventory_dataflow_selected_ip", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_dataflow_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, '4296e1da30678d908b753f9ccbb88f89c6ee591005c4a0881433e61e4deac268', None),),
  ('trigger', '["public", "inventory_dataflow_selected_ip", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_dataflow_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '76306d4e1e509267c8cc38ee4807dfa6d99e42a867764283921946df6e783552', None),),
  ('trigger', '["public", "inventory_dataflow_selected_ip", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, 'e7b82521903ecf47992c9f6ac28fded0b02625016533f2d96b0548eb436bc3c3', None),),
  ('trigger', '["public", "inventory_dataflow_selected_ip", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, '4ce0f14832e8aa8f291eab4921fab852cb26bc99d55e04c8d5e70b7f420f457e', None),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_addressgroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '517b90c2811dbfe8b684aae27bb4fb5a951da3b56b6b88d59fc83e3fd5896190'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_addressgroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '84886d22cd89e617cb013488097f4e7f24c1f08b417b45363c9c0eed0a8dc968'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_cluster_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '5ef0a00a0c8a54c89939b64ddf4d1be2a74830493fff0bdccb0c4acc15335168'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_cluster_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '80c819f1f2be4fa7c2f43541369f505719bbbdb7c5f9ad902724c70f4d075428'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustergroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '224f063f071d6cb3c037672df30126f8ec27f92621853473f94200357b8f8162'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustergroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '36fd7745ad0755a6cf80ee8f31ac98f136794b72631ae3f0d546700dfba67e5d'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustertype_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'edc64d20fea592cb05b5307664877c34df44d6624d1f8779cbeb46790714de19'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustertype_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'c759e792b358703237118e6f3e9b4fa8fa116a301e8dc0145d9120518fbda7c3'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_dataflow_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '7bb4b03c0e7e97434b3ee39b0b53e291c1718d341bf6bec6e4e8521c6cb4c7cb'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_dataflow_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '8c27a10924feb4bd2f87acbcd6a50a39b9bb2f49e9baed93c63fefbea6e044e2'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_device_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'd33dbbe6c200ce45ff2c001a7f286f9ad025b40fae6aec45130ceed2d1e698f1'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_device_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'bdd142d3d5322b87de2b00f435196a468856f4d22f833de42b4f1567e136ce71'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicerole_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'fb09e9f8fb456a75d9e1283f67f1da5d4fb99f38c6c688b0e844746cce5e9568'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicerole_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '5be02b5e8875395e954d65d501d0d56f6ee66fa16ec29babc5dd0828fb2c175d'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicetype_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'b3b0deb7b9dff6227e7f6a088f72b62eb19dd64f7f555e364599336bf3c538fd'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_devicetype_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'cb685c0c25ac5cdd776d34090cddf3ddcc0b33f021316f0dc70b1e78d8096474'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_group_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '634521c4f85afa8bd9cca15849c8bb9862c03e9a919660ff2be32329dc993937'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_group_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '4f71c2c1dd634960c8073dc75484d2a07d37564974bf019a68e68f7a37866630'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'daa3f389a0e5203a91c1b5fb5c343dc39e4068b6006785ba8db1ca533c21319e'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '78fe26a9bf2c3078afa7cd912ea7b764270408453462297130267c0e7c5603d9'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_iprange_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '56bdc7f33e0c0ea4730293917bc1e2858a25feda8241b77fb1a72afe8d2bf604'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_iprange_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '4a66b3b3ad612810bbb9c0559a2d36b20c53ddde8a2ac7e8c75783515d90599f'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_location_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '0a831831a205171da05389347623914f1b19afa1530ff0ff675ae278a7e84e38'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_location_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '89edcf860c408ba78085bf32a588a11ee18641e4c81a464e8993ce6294a5bd6f'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_manufacturer_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '3f77e3dcbff02e1cc1ede714b2a2bd193b5aafdcff27e99baf576e11b9968aed'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_manufacturer_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'a23a24f1f34631e10a3e68b6e9cbc745a351884bdf04a6df707bb14fb854cdca'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_platform_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'c43091db4e343ee7a5cd9f56bebdda3cebb9fc166e768a754666465ee055fb45'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_platform_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '6e66f6e94056aefcbcd5327d31145dbe75961d81fcc448151283a89f2326b2a5'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_prefix_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'dfd3aa18e53d84d916a6cbb06460e72ede99b27d1136dd7c981039ccd9fe730b'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_prefix_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '6ba12161d3e76f9978f2933c4f591aaf52c23896de1029fadc9216713505629a'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_rack_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '1ef876d1704a1aab9dac8849a49e20212f0513780fa2c0048204413a8b73838e'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_rack_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'd6177f4554103edc947e2a73ce51ef3a759425d3e1454aec07f5ba0c632b4525'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_region_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '5192adf2768931efe19c6e46307b7f059cafb4e10d16d6725d9499d4814222b6'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_region_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '1729fa3f39aa72383a74de38beac487afcfd67f7431f89155d04357e3199c2fe'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_service_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '363e9d5f6d97927ad7168009e2b2d8c4dab84655d78b078b92c943ac6eabef5e'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_service_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '852615ac03924b6bc6e777cc7ed77762be05b83d1dba1a93e28ad616bc1a62a1'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_site_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '431439b1eea3ed9e69b1cae9a8c2bfdd3f73108089e3bf894a1d7fccdea890f3'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_site_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'ba0eac8f3021b6443457bd62d200c15e10f515f344a83e046293d1ba0d8a1629'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_sitegroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'ea7e0e0b53272d42fa52509c4b375305a08d0cc5144468a3e7d26f47a3f1ce41'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_sitegroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '719fd004761d8935b696c79a1d2c538117a979f62e70dcc92a4acd38e8916db4'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tag_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '6afd115117e14ff3c59c982268e99f76740ce63d8329d311f5a25670a58480f0'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tag_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'd890e7ef6eec7c6362544a0d8fa8f3223552c0c4b1ded0f548a243ef44d09191'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenant_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '3c670a789a000a02dbc38113d2a530052fb7b617c7a9e905f55df92db74d5cff'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenant_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '1481661002aa15d098dbcd301b8031cb5da9b842b03db79b7aea7418d517131a'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'ee2e6ee551846c93325c0d1f505cbf8755cbcec29e334492d2bf99383e4901fb'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '7f5aee77d66bd099b984caf748c9263a7b587d71461d02c581a27ef632d273d6'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_vm_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '85fcc41ddae0ea71844a0b360e3fba17b3e50f212d2518d5fb949048a5b868af'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_vm_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'd2613d4f96b282a0126a3738eae9fffc10a7cde47b1c08cbb43e8a7b2774729b'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_selector_id_fkey", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((16, None, '04359e1a1098ffb3549c79ab696c7beca233f4a0b2ced65b9425ca0d219f30e0'),),
  ('trigger', '["public", "inventory_dataflow_selector", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_selector_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '012ed1b1e11116ddc93fcb18daea8535b5de1647d18a977282515d7a3f591ac8'),),
  ('trigger', '["public", "inventory_dataflow_selector_ip", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, 'dbf5dc74371179b175534da0683356c505afed596b613ff7ab117b7930daf5e8'),),
  ('trigger', '["public", "inventory_dataflow_selector_ip", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, 'dd80a109b7f084b7f4bd6312be750dd21b81587abe9041b46583226824dc3494'),),
  ('trigger', '["public", "inventory_dataflow_selector_ip", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_selector_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((16, None, '0bd4807387d15fb23cbbd03c759879a779d5069641ccff30fd0a6e6eb6ea6070'),),
  ('trigger', '["public", "inventory_dataflow_selector_ip", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_selector_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((16, None, '28f953a8e7c569faa18dd48894d07d9ad5594fe8f51374497fce584ac7d2f60e'),),
  ('trigger', '["public", "inventory_tag", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tag_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '7314a1fd0ec3d523a32180456f8c8ddcdf1d229a9508ecc1cd981f8fc6c27ac3'),),
  ('trigger', '["public", "inventory_tag", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tag_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '32a1d3ea7af2d5c2b64c43c35d36c4bd414ea8b6819950e73d0865e09824b251'),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_ipaddress_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'c85b44593970e482f2a5aa8a3392212b2ee3a3eecefee1f8e527b84b6f2677f5', None),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_ipaddress_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'df6c3a1dc13cb3afa760a25412112fdc487016d444d8f4103d4f3d65dba7425f', None),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_ipaddress_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'fed2e3f24c492c16ab234a43768090cbb673eacab695a42ba288b9262b1aaf28', None),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_ipaddress_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '66dcf93da251e8503156802fbb53bbabd17a58156c473742d42cddc47c8e82f3', None),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '88781e96c26b45744062b648a9160716cd93354103b640bdd987cacd66d8bda1', None),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow_selected_ip", "inventory_dataflow_selected_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '48bf4cae87d85b630bd93696ead6ab91c63c2010eeb07eeb54a95e8e4b1119de', None),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_ipaddress_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '5c22190f3c42fb49541c06208471d18fe6e4b5fbe6d6b70cbe8783f58d7a1385'),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_ipaddress_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'ff135e27171c5c4c2971ad87ef58f8ec8c756c2ec8cc1249db272f2ff0932ad3'),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '55fc444d3ff853859569ecc0eb670c00ac9a0ab782361f129791871b97367c14'),),
  ('trigger', '["public", "ipam_ipaddress", "internal", "public", "inventory_dataflow_selector_ip", "inventory_dataflow_selector_ip_ipaddress_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '3c6e22888c79f1d605cab5d5aaccaa291a77dcd9300ad8a85f36487ec536cff0'),),
  ('trigger', '["public", "ipam_iprange", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_iprange_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'c1cfb248ee32866d785490a05aa688fec54a88d3c2cd4d7683a4b4a0d20eb5b9'),),
  ('trigger', '["public", "ipam_iprange", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_iprange_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '78ccbfd097e183741050b4db4149e98fd8c27395a275c1fd269628c470ceb743'),),
  ('trigger', '["public", "ipam_prefix", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_prefix_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'b5049262b511eab18674d7c992f3759cdb5b4b8f82abe87e0b6d730df91d24bb', None),),
  ('trigger', '["public", "ipam_prefix", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_prefix_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'e61897f4e3c2ff3afdf1d56428d9a8cee3e46336cdd12cbc534e7dd94e8a60d9', None),),
  ('trigger', '["public", "ipam_prefix", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_prefix_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '8e02d35adc47b853c3990081de944e54fb9d40ed53421f6c7c6d0bf729d281ae', None),),
  ('trigger', '["public", "ipam_prefix", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_prefix_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '18978c1bb6f611fee0f6923bd157d4c1ca5ecfc5fd361fb11d44c548f6ff06cc', None),),
  ('trigger', '["public", "ipam_prefix", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_prefix_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '4f65630d84781b0eddbf05b4c81987492a256c162dc6eb5cdd6cac6a4ed23ce8'),),
  ('trigger', '["public", "ipam_prefix", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_prefix_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '53a4661cc2c195026b37c5a9c7bbf6ea3097f557cb84c13f036ec1bcf9a6fcf2'),),
  ('trigger', '["public", "ipam_service", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_service_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'c96fe14c7bb3b0b2bd852044d9bfd33592b3c868f956f3e770a6c875364647c3', None),),
  ('trigger', '["public", "ipam_service", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_service_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'e8450cd7ce24ccdec03738ff708152e2b54616c6bc91aecf7168655f097d0c87', None),),
  ('trigger', '["public", "ipam_service", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_service_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '32e185534fdddbde4ad01afd7299ecd1ea4ff1080eaed7d15a75b78ced955ceb', None),),
  ('trigger', '["public", "ipam_service", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_service_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '346fb78ae81f586a6b82bf788dcc1b6fa0ece4feccc82f495adad31917e42bea', None),),
  ('trigger', '["public", "ipam_service", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_service_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '5a04cc75ec21d1c0985f0ac2edefe95a838de689dc9c326f57a5e07140e7966b'),),
  ('trigger', '["public", "ipam_service", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_service_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '5ec397f881bc0c347fffb58552005c0ae8a8e28394df93de293eedcd9ef49770'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_address_group", "inventory_address_group_tenant_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '7fe8c6435911674ec810d4b5d96a7734cc82311e8de1905dee70db1716eda13a'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_address_group", "inventory_address_group_tenant_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'f04df3509e49b552b6cd6e620d1205cb5eb1fc4810b9abe68d392ea919d4ad65'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_application", "inventory_application_tenant_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '97782423556115dbddd669a5abae9868d132ee470fa10644e7bb5856aaf41957'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_application", "inventory_application_tenant_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '043db67a0690dd21d0840f371ba1b1d6138cd9ba0088f31de4684cd15f179c45'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenant_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '204fd8ebebcda43bab84283fe465c3552dfb73e9cb56fdf169ab6ea6fe1df74d', None),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenant_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'e822db16c7a021e2ea7a890df0ed636e402360ba7aeb9fe7cf803e71bf52fad9', None),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenant_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'a2ba2129eb4258bb6084795fb27f23022dff5597caa7074c18b1c1d8dc8f8dc9', None),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenant_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'e2ec4d378a855d8cddceb56751e8974a6f4d3f693e751ba3cb0cfbb3b5f60a34', None),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow", "inventory_dataflow_tenant_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '0d117953c54093bb9759431ec4f23c7ef1d57946a3ae763d7a139e4e5ced12c9'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow", "inventory_dataflow_tenant_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'd7e6fea694c689df79d3df9c0edd376281c504d1830f3da3160612e4df40d202'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_tenant_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '92d28dbf839d4595997db3b117a4ef8f3c78f443bb574235c64d2eff170cd504'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_tenant_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '6ba66ddf046beabbc3e894933efa408c20bcf9a0f34db0fafcbbc6194425afc2'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenant_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '025d6cf725db6362baff6303b62a931e6201bbbdcc7f2548473a7d0ea561170d'),),
  ('trigger', '["public", "tenancy_tenant", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenant_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '13362fc2480ed93ddb7d3d38e7b8ec7bf62f75d969e7b61a65fa9e81fe035840'),),
  ('trigger', '["public", "tenancy_tenantgroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '8940c5c0c85054b6157b2aa7631841c1789caa91413b69e2a35211c3dc475ca2', None),),
  ('trigger', '["public", "tenancy_tenantgroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '6e3d36646d9a41a75a887b4df523e6888777932626dff98526177c55cb77759b', None),),
  ('trigger', '["public", "tenancy_tenantgroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '01bd7bb17fbc486f6bb90cc964e3d69f0c6103b0e879b2b68635a10c1a00268b', None),),
  ('trigger', '["public", "tenancy_tenantgroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'c90c6bfcb10a8abe77c8755895c1e7618f8843586d91e57e2f2bdd8fd5069393', None),),
  ('trigger', '["public", "tenancy_tenantgroup", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '1d746b4d94445369a9f7d6b8c54dff00301a079abe4b244f360750880ae2c86b'),),
  ('trigger', '["public", "tenancy_tenantgroup", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_tenantgroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '78ac496579e6d2c6ed7e5b9e7f30ff6ca1cb236b86a7f2a47b583fb16ab1696b'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_address_group", "inventory_address_group_owner_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '83685d32957e2cfcf4bacdba477a41fd8c4ac0fcf66def1e7f86e9a1476647dc'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_address_group", "inventory_address_group_owner_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '7fa07ce23438f82211a67fd8ca41b3b97b591a3941236e760a2f962bd19bfc01'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_application", "inventory_application_owner_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'f181eb9036d4d7d31e16363df09967abd47e31760650d07bec3bf8cd2d82e277'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_application", "inventory_application_owner_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'b0d1bb09a3dc81a702b42c8d5b06b5cf6eaf3831ad674848a9c9c8884fa299a6'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_application_role", "inventory_application_role_owner_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'da1f5c1319408915cb589df3bff97fcc7c7cafbc7f717c7ca5ab892d3b13fd3c'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_application_role", "inventory_application_role_owner_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'efb91cb573de38bd93354d9483dc5dbe11da01f03a65336c204f5a9fd9aaeea5'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_dataflow", "inventory_dataflow_owner_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'eaee96fabfb01a17ea94c6d93dc0e5c3c450ca9b9c0cd2a656a3bb741c14252d'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_dataflow", "inventory_dataflow_owner_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '048e3531159b9f0a8bc4351ddc028925f1961403bc9075ad455fcc2a202c8a4a'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_owner_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '78dd8d35dd979bc94660c73a88bf084c3857a09563e9aefe238bb1cb79d1f15a'),),
  ('trigger', '["public", "users_owner", "internal", "public", "inventory_dataflow_group", "inventory_dataflow_group_owner_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '9bd1ef0906b65f9e8863dff002c62457895df07a87a023a91a3678d44bc65f8e'),),
  ('trigger', '["public", "virtualization_cluster", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_cluster_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'b852f56900dc4a7efa1fa7198b740f374a24958f22d172ecb312cb0820659d0c', None),),
  ('trigger', '["public", "virtualization_cluster", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_cluster_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '3d124c20ec046f0a5ad4dd4ca676fd1c1f667b0caf10743955b74c33c4e1c382', None),),
  ('trigger', '["public", "virtualization_cluster", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_cluster_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '533c8a78c03b18fd35dda1b0dca7530c690c18f46846214adf99e2903bb19b74', None),),
  ('trigger', '["public", "virtualization_cluster", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_cluster_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '6df60634bad930772f8713faca2f49a5ff558144439a9c58aea012c356313f8a', None),),
  ('trigger', '["public", "virtualization_cluster", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_cluster_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'c3db9d498825664045a40feb849fdfe804b39b550aa9103584bfa9b82b62b6ad'),),
  ('trigger', '["public", "virtualization_cluster", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_cluster_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '613a63f0ee56feaf9cf3bfe95d54e3cb463de543df29fc493950e46a9830b37d'),),
  ('trigger', '["public", "virtualization_clustergroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustergroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'ff43029a3e859ecbdae50185027d8f308c744a5ea71d3e1d71bb91c4c8e08eb2', None),),
  ('trigger', '["public", "virtualization_clustergroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustergroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'c538e9d91722f8b24ccf1ca67d896e03d0f6e050e235159ff6617797e1c46417', None),),
  ('trigger', '["public", "virtualization_clustergroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustergroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, 'ff5632e7b9af0c58807121457c20d33ecf729245c8e2037ece75d4072b407011', None),),
  ('trigger', '["public", "virtualization_clustergroup", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustergroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'e2536362376eebca02bb18bb6ac1dc5647b3786a6143bd48de6bd411d5567dd1', None),),
  ('trigger', '["public", "virtualization_clustergroup", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustergroup_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, '95f9a6b2aba0d28495220bec2c29aee2648a80677d16ad7c16b56fdb5ed80678'),),
  ('trigger', '["public", "virtualization_clustergroup", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustergroup_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '0f1f25f6469550ea9b0e8b3cdd8d516b3ec3e2e879610124b23035bb44c454c2'),),
  ('trigger', '["public", "virtualization_clustertype", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustertype_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '43ea0082af63a0d46e1ff6437f113b5ed5dfb8a95b21a1e78ff409a80585efe1', None),),
  ('trigger', '["public", "virtualization_clustertype", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_clustertype_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '5def77cdd43bf0c96aef89fd17db4b771f8c46ca54c889193dd033627fda820f', None),),
  ('trigger', '["public", "virtualization_clustertype", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustertype_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '425e0dfbb25a47a33fec1025a36f77f181ba5da87a3aa87cc45b3c66e47dc1b7', None),),
  ('trigger', '["public", "virtualization_clustertype", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_clustertype_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '2cf1fa36406a7b1c4bd3ecd7ba8a6420df019c185912289af0f66a57359a87c2', None),),
  ('trigger', '["public", "virtualization_clustertype", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustertype_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'f146875223ae974439feacc54d54896792f04b0ae58021c6341e041ea8abacf8'),),
  ('trigger', '["public", "virtualization_clustertype", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_clustertype_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, 'a8e37eabcff1a0be9be2359cb3db62355c8b05ed282e3946ab7f8c4d01e2a068'),),
  ('trigger', '["public", "virtualization_virtualmachine", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_vm_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '873688cc8c4c0d80c7a0496ad311c1d92a92aa9e6016568550c0757e9489e6f7', None),),
  ('trigger', '["public", "virtualization_virtualmachine", "internal", "public", "inventory_dataflow", "inventory_dataflow_destination_vm_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, 'abe0f7ecaaa37f3a13967fce1827dd1be94e563d2ae9b3277ece21998a878ded', None),),
  ('trigger', '["public", "virtualization_virtualmachine", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_vm_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, '13433224056d36939f789b58d88414719ad57ea05c1f5cba3bbde8d91a1381f6', None),),
  ('trigger', '["public", "virtualization_virtualmachine", "internal", "public", "inventory_dataflow", "inventory_dataflow_source_vm_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, '9e61c04dd1ca0219e71e82d7454ebeb73b7c7ca25e12cff4af9b51c45a4b2a2c', None),),
  ('trigger', '["public", "virtualization_virtualmachine", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_vm_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((16, None, 'c32ac4dbe21c7a7065739ad5908aba086f030c8285dc933d2f6c79a325853228'),),
  ('trigger', '["public", "virtualization_virtualmachine", "internal", "public", "inventory_dataflow_selector", "inventory_dataflow_selector_vm_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((16, None, '54f42c13e9cf92bacc38bdf930afceca2df7ca103458ec4ede60cb0386606187'),),
  ('trigger', 'public.inventory_address_group.tag_membership_guard'): ((16, None, '47dc044edc2d1a448bfbfd7f81ecebf4b707ad555317c92a206ebe6b64913a93'),),
  ('trigger', 'public.inventory_address_group.tag_usage_lock'): ((16, None, '147887f79b4d27946cb071b391bf5b9e655fd032a74a4ee119a80d3218512eef'),),
  ('trigger', 'public.inventory_application.tag_membership_guard'): ((16, None, 'c0f9de870ba5776e0546f1aaa8f28577eebceb391874a5fde3d3203d15503881'),),
  ('trigger', 'public.inventory_application.tag_usage_lock'): ((16, None, '51b35a24122aa1120c52492c9b85da63bd333983e63e23ec3d991569a332d025'),),
  ('trigger', 'public.inventory_dataflow.inventory_dataflow_catalog_lock'): ((16, None, 'ea779714061cf5ff03a31c261d4e2b5a9546d13a088656233d9d50b56b5142ae'),),
  ('trigger', 'public.inventory_dataflow.inventory_dataflow_group_consistency'): ((16, None, 'a04aeddd9ed089559c8b47c5300d26d35ef7a5cc362c61c7e75b3db96c3ece97'),),
  ('trigger', 'public.inventory_dataflow.inventory_dataflow_selection_consistency'): ((16, 'e7688cba021febb1e15b24c179dfaa83201e8bb8fe95cf910a8bbd3e6a6a558a', None),),
  ('trigger', 'public.inventory_dataflow.tag_membership_guard'): ((16, None, 'e3907e6bb40fafcce74bc2fad85e56207eb65ac91028ff8b2cd0d5246fb624c3'),),
  ('trigger', 'public.inventory_dataflow.tag_usage_lock'): ((16, None, '9ff5589123eaaf9437edc65baa9e0c29b11a9ebe37e2801163a5be36597f3fa6'),),
  ('trigger', 'public.inventory_dataflow_group.inventory_dataflow_group_catalog_lock'): ((16, None, '3276eaaef96aeab0f250f174e2db821c39732d9e2b7ffbb5c83406ff731b64cc'),),
  ('trigger', 'public.inventory_dataflow_group.inventory_dataflow_group_hierarchy_guard'): ((16, None, '6da13b173499f5c77fa643c30ffdb7606b8ad26c02faacf689c3085b21628da1'),),
  ('trigger', 'public.inventory_dataflow_group.tag_membership_guard'): ((16, None, '5f39b1505fab31ff71fb333112e2894bbf6ea5b47848af4270d5deccc2e115ce'),),
  ('trigger', 'public.inventory_dataflow_group.tag_usage_lock'): ((16, None, 'abb4b7d47c7feb2cf6eb4d8c1931251d3612b1b313577ab5b1b058ef1147d85a'),),
  ('trigger', 'public.inventory_dataflow_selected_ip.inventory_dataflow_selected_ip_consistency'): ((16, 'a959b828be9188ff3eab240e670e1c1e694c309571a184ab9fa4d31a61e284d9', None),),
  ('trigger', 'public.inventory_dataflow_selector.inventory_dataflow_selector_selection_consistency'): ((16, None, '121899a63775c17d63c2a5fcab055bf2c085b34c8af3b11f5036baa62e14a06c'),),
  ('trigger', 'public.inventory_dataflow_selector_ip.inventory_dataflow_selector_ip_selection_consistency'): ((16, None, 'a3614d11fed667129cb000fd4d2bc99656d6a740b3b5c1b6361f7cc94c66e51a'),),
}


# The version-16 edges in the preceding PIP-211 catalog are owned by the
# dataflow migration, now numbered 17. Workspace edges are declared below.
SCHEMA_TRANSITIONS = {
    key: tuple((17 if version == 16 else version, before, after)
               for version, before, after in edges)
    for key, edges in SCHEMA_TRANSITIONS.items()
}

# PostgreSQL 18 column nullability changes explicitly made by migration 0016.
SCHEMA_TRANSITIONS.update({
    ('column', 'public.playbook_job_template.execution_environment_id'): ((16, 'd8fde33e54bd6c5a2278fa77a95e6162ec63d110ceb1617c51110cc1331accad', '9a88fa6f3223cfa046ca4cf3ba9c9b51c9a5f6c5b12a0290ddc63803e9cb39d6'),),
    ('column', 'public.playbook_job_template.project_id'): ((16, 'a97717037ad995a188b74944b1a9dc167e870a58ad50cef500341693bd14a6d4', '4703620687f3816ae8813586e79b6889c54066322e0cc83da610bf30d533ba39'),),
    ('column', 'public.playbook_run.source_id'): ((16, '9a88fa6f3223cfa046ca4cf3ba9c9b51c9a5f6c5b12a0290ddc63803e9cb39d6', 'd8fde33e54bd6c5a2278fa77a95e6162ec63d110ceb1617c51110cc1331accad'),),
    ('column', 'public.playbook_run.source_kind'): ((16, 'd2d755ac0c4d9fcf414a9a5eb96869c74103d9972d0425e6224bee601928ae79', '0aa9ecba3631aa4a9bd3f660d7bc41eca30e96a45bd45cbc2ab0d3cfce515597'),),
    ('column', 'public.playbook_run.target_id'): ((16, '929c50003c88cbb3c41ccef693f6a52c642d01c2e3401983d99c8377db48d684', '7133a12f54cb78d0280ce91c408136b2567dae2813244086fa52990b164255b3'),),
    ('column', 'public.playbook_run.target_kind'): ((16, '081fd4d86d3a46a9e1015a9f22f93a3273b8f357c424effe43477fb5b2cbf5ee', 'cc3df0b82772131eee9e336a6043109d119373e0288adbf819dbe52af9edce4c'),),
    ('constraint', 'public.playbook_run.chk_playbook_run_status'): ((
        16, '5606d08b11bc256a81cf1bf1e97010b9bbe3bcfead3b0d24a854c65b6fed8842',
        'd49df5e9119df9d4c75f316b7524051497495a24a45a3ce83ac4613dc83e509f'),),
})

# Native PostgreSQL 18 definitions for the target-set deletion constraint.
SCHEMA_TRANSITIONS.update({
    ('constraint', 'public.playbook_assignment.playbook_assignment_target_set_id_fkey'): ((
        18, '00a1862a287afa7524fad13c6e38cfc6d24206f5a166d1a582258902dd91ebfe',
        '83454e757a812f8d1e35dd1dabec2ba39a02cdde7dde446592832f5d18c530a9'),),
    ('trigger', '["public", "playbook_assignment", "internal", "public", "playbook_assignment", "playbook_assignment_target_set_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((
        18, '6d530779a24a751feed722f0a07a123029084bd0d214f12cad4955a4ea4b0f95',
        'd7345e54d94e9fcc1e18ff7c97108c6b019aa44f2efaa4dd81dac6545fe95206'),),
    ('trigger', '["public", "playbook_assignment", "internal", "public", "playbook_assignment", "playbook_assignment_target_set_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((
        18, 'bf9a60cd60d56af9ce37011b0d85f88808eb257e9433ef84b30cd42cc9c47a1b',
        'ea8ab21378bb8541b6fcfe564ab36268771dada6cf5879c4df2e934f33a81790'),),
    ('trigger', '["public", "playbook_target_set", "internal", "public", "playbook_assignment", "playbook_assignment_target_set_id_fkey", "pg_catalog", "RI_FKey_cascade_del", "", 9]'): ((
        18, '912f45246e3223f22749dfcae019f9ec948675b7a18238e6ffa2eadf1058aebe', None),),
    ('trigger', '["public", "playbook_target_set", "internal", "public", "playbook_assignment", "playbook_assignment_target_set_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((
        18, '73e30b26b436f84280a720b35e4e067ca6a358cfb5e8744b4b5141fb22c2084a',
        'ed2e45b8133f4152686d4ac184fc075dceab41983e9f118bfc1d6172f095e18e'),),
    ('trigger', '["public", "playbook_target_set", "internal", "public", "playbook_assignment", "playbook_assignment_target_set_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((
        18, None, '821e9183de8f2effbfb12b129be96df8af2fe2e4d6aaaff93d8b91f94ec67006'),),
})

# Native PostgreSQL 18 definitions for context selection; no context rows change.
SCHEMA_TRANSITIONS.update({
    ('column', 'public.config_context.weight'): ((
        19, 'cc4305af58f14e09533115bd429ed79ef910cf459f6c41129907e15d75dbc20f',
        '6ed5f4bc5c43e0f5aebb58abef8c3492ba22a0db801941a6fdb482e0f1de2dd6'),),
    ('constraint', 'public.config_context.config_context_data_object'): ((
        19, None, '72cb2c7d5f72a435040ee0564acdde0c6a8e77e7ba0448f6c11eb520a353f3fc'),),
    ('constraint', 'public.config_context.config_context_weight_range'): ((
        19, None, '2339da844cf162e703e39f5886d8cdef5e19f9539f37200844d07c2482a3b5ca'),),
    ('function', 'public.config_context_scope_exists(kind text, target uuid)'): ((
        19, None, '3f380786e816b614c665983763e814db0751eb0197b10743e639ee8b1505482d'),),
})

# Native PostgreSQL 18 definitions for the schedule-owner cutover: the retired
# service-principal column and its FK/index disappear, the delegating user column
# becomes the owner, and its FK/NOT NULL constraints and RI triggers follow.
SCHEMA_TRANSITIONS.update({
    ('column', 'public.playbook_schedule.delegated_by_user_id'): ((21, '066c9cce2aee574d47159df103206bde630c857c89ba6c221fb2dc22e160802a', None),),
    ('column', 'public.playbook_schedule.owner_user_id'): ((21, None, '066c9cce2aee574d47159df103206bde630c857c89ba6c221fb2dc22e160802a'),),
    ('column', 'public.playbook_schedule.service_principal_id'): ((21, '797eceef46e21e212f45eca3d34d83144aa8496c954fabfb4079c5e0ada63bf5', None),),
    ('constraint', 'public.playbook_schedule.playbook_schedule_delegated_by_user_id_fkey'): ((21, '1b708b855db0b2eec8589f25e118a8090b177d626d957ce85505375b46ddf74f', None),),
    ('constraint', 'public.playbook_schedule.playbook_schedule_delegated_by_user_id_not_null'): ((21, '960165c1498650c7ddcaada3f031fff3d76363192d6edafd52edf9d1b4fce5d5', None),),
    ('constraint', 'public.playbook_schedule.playbook_schedule_owner_user_id_fkey'): ((21, None, '6357764fcfd88704415ee2a9ee2866f8a89acb3b554470bee0994db133f10dcc'),),
    ('constraint', 'public.playbook_schedule.playbook_schedule_owner_user_id_not_null'): ((21, None, '526ec2f2d6675a7afeed312bc8a29fc8b8d9ecc87b506740539a0e08bb88369f'),),
    ('constraint', 'public.playbook_schedule.playbook_schedule_service_principal_id_fkey'): ((21, 'f4b70e19809cc90f7d7eb184709a4f0fe80c6a73c6078ed4693ea1953f9a5032', None),),
    ('constraint', 'public.playbook_schedule.playbook_schedule_service_principal_id_not_null'): ((21, 'cb4ae8c7f06c2c07760a5b6001b30ef4af662605e9beb6b8874e8794342ce7df', None),),
    ('index', 'public.idx_playbook_schedule_service_principal'): ((21, 'a4e1605e426da5a4ffdb8ec95150a9b5104c9cd1d48656f04d023ec98e57b267', None),),
    ('trigger', '["public", "identity_user", "internal", "public", "playbook_schedule", "playbook_schedule_delegated_by_user_id_fkey", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((21, '3466bcc03f23c6a9098d9bd7295fde98383d4a671ea88dd092848fc28b50214c', None),),
    ('trigger', '["public", "identity_user", "internal", "public", "playbook_schedule", "playbook_schedule_delegated_by_user_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((21, '0b11d7f44c835ba272e4c61c9275ac73678a8465ba35d97c168f73791dd4dd02', None),),
    ('trigger', '["public", "identity_user", "internal", "public", "playbook_schedule", "playbook_schedule_owner_user_id_fkey", "pg_catalog", "RI_FKey_noaction_del", "", 9]'): ((21, None, '462359bb20a48f002a310e266f4c9bc75c7ed35a4e5faa7c77f842dec15da5c6'),),
    ('trigger', '["public", "identity_user", "internal", "public", "playbook_schedule", "playbook_schedule_owner_user_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((21, None, 'effe61945f9549afed1c9380dcc60afb565f96c8eacefe17f7fdedd4ebf42f5b'),),
    ('trigger', '["public", "playbook_schedule", "internal", "public", "playbook_schedule", "playbook_schedule_delegated_by_user_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((21, 'a1188c638ec9bd3aaa77999ed96ba4abee0bfc1401020a82b8d60abe57c1a75e', None),),
    ('trigger', '["public", "playbook_schedule", "internal", "public", "playbook_schedule", "playbook_schedule_delegated_by_user_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((21, 'b719db7c404487324963417a99b7831a9f7add96edc1ebbf4bbd4bad7eb58324', None),),
    ('trigger', '["public", "playbook_schedule", "internal", "public", "playbook_schedule", "playbook_schedule_owner_user_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((21, None, '960c3174688e7962a320c2bb78c69448c37b6d4a19d19e3fb2b8deb0ae70a169'),),
    ('trigger', '["public", "playbook_schedule", "internal", "public", "playbook_schedule", "playbook_schedule_owner_user_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((21, None, '6784d6cef510a77e9cb2ec1e5e5fd01e3bb68b7e805051447e7fed34f3dd83f9'),),
    ('trigger', '["public", "playbook_schedule", "internal", "public", "playbook_schedule", "playbook_schedule_service_principal_id_fkey", "pg_catalog", "RI_FKey_check_ins", "", 5]'): ((21, 'b210614cfb5b503c05285788b86bf95f0c99321c35741b41a4c501d6e5aafba1', None),),
    ('trigger', '["public", "playbook_schedule", "internal", "public", "playbook_schedule", "playbook_schedule_service_principal_id_fkey", "pg_catalog", "RI_FKey_check_upd", "", 17]'): ((21, 'bc04c8eccbf67194b95e763f2b2738aa2c2ff2b2ce87b0c0ee7ce1eecdaf1a4e', None),),
    ('trigger', '["public", "playbook_service_principal", "internal", "public", "playbook_schedule", "playbook_schedule_service_principal_id_fkey", "pg_catalog", "RI_FKey_noaction_upd", "", 17]'): ((21, '8f9abe819d6f513d2f8e6e03e329af5fd77866aa738967ad0764597efba2a9ba', None),),
    ('trigger', '["public", "playbook_service_principal", "internal", "public", "playbook_schedule", "playbook_schedule_service_principal_id_fkey", "pg_catalog", "RI_FKey_restrict_del", "", 9]'): ((21, '1f534ece3f91a64b0cc506533258a8837a7cc197795b7b19a0cb7f225be128d6', None),),
})

SCHEMA_TRANSITIONS.update({
    ('column', 'public.playbook_job_template.vars_schema'): ((22, None, 'd2184d24f618244c9517081fea938514325e1621f7e68e139be519e484d23dde'),),
    ('column', 'public.playbook_schedule.overrides'): ((22, None, 'b777c5c4c17ec49ccaed2ca355629e74ff512b8f1bdb9629b168155d6392f27c'),),
    ('constraint', 'public.playbook_job_template.chk_playbook_job_template_vars_schema'): ((22, None, 'd049d963057b70d39e796280be4c855c03c226d0088fa1c6326bb256761f0d72'),),
    ('constraint', 'public.playbook_job_template.playbook_job_template_vars_schema_not_null'): ((22, None, '1fc98caa11b745e9c655a38ceaf495cf57a629f0093783e633ee29e053dfe353'),),
    ('constraint', 'public.playbook_schedule.chk_playbook_schedule_overrides'): ((22, None, '5e98bcae40ab264af65ab9de236f12b459a980b8bcf40ac3f1ab24d595fa90d9'),),
    ('constraint', 'public.playbook_schedule.playbook_schedule_overrides_not_null'): ((22, None, '681f3850b079d67ab0f4fb229d6c1dbd021e07e3fbc9bbe6c9e8caa3a55c3100'),),
    # 0024 Windows transport: connection-config protocol check admits winrm/psrp.
    ('constraint', 'public.connconf_record.connconf_record_protocol_check'): ((
        24,
        'd299948f50db5358280d5ad19234f0d2aa571fc4be19b0640843bf6db8003ae5',
        'a87443c6ab5f5e0a69bdc62e3d35f7a64bba377d269516c0d3fe5ab9fa716ee6'),),
})

# Complete PostgreSQL 18 catalog delta for the reviewed 0016 SQL. The digest
# covers every object's identity and both complete definition hashes; it is
# not an authority for arbitrary objects under a playbook name prefix. Column
# positions can shift uniformly per existing table after a native dump/restore.
WORKSPACE_0016_DELTA_SHA256 = '1c38f29486eb65bcecabaaf0f1a79f342bfe18efb3736e78f4e2d954aa00f3c2'
WORKSPACE_0016_RETIRED_TABLES = tuple('public.' + name for name in (
    'playbook', 'playbook_version', 'playbook_assignment', 'playbook_project',
    'playbook_job_template', 'playbook_job_template_survey', 'playbook_run_group',
    'playbook_run', 'playbook_run_event', 'playbook_artifact', 'playbook_job_token',
    'playbook_run_group_callback_token', 'playbook_run_group_event_batch',
    'playbook_schedule', 'playbook_schedule_occurrence',
))
EMPTY_ROW_DIGEST = '0:' + hashlib.sha256(b'').hexdigest()
WORKSPACE_0016_ADDED_COLUMN_POSITIONS = {
    'public.playbook_assignment.job_template_id': 22,
    'public.playbook_job_template.inventory': 27, 'public.playbook_job_template.options': 28,
    'public.playbook_job_template.playbook_paths': 25, 'public.playbook_job_template.revision_selection': 26,
    'public.playbook_job_template.workdir': 24,
    'public.playbook_project.current_revision_id': 21, 'public.playbook_project.source': 20,
    'public.playbook_project_revision.created_at': 6, 'public.playbook_project_revision.created_by': 5,
    'public.playbook_project_revision.entries': 7, 'public.playbook_project_revision.project_id': 2,
    'public.playbook_project_revision.revision_id': 1, 'public.playbook_project_revision.source_commit_oid': 4,
    'public.playbook_project_revision.tree_sha256': 3, 'public.playbook_run.hostname': 36,
    'public.playbook_run_console_chunk.bytes': 5, 'public.playbook_run_console_chunk.created_at': 6,
    'public.playbook_run_console_chunk.dispatch_id': 2, 'public.playbook_run_console_chunk.run_group_id': 1,
    'public.playbook_run_console_chunk.sequence': 3, 'public.playbook_run_console_chunk.stream': 4,
    'public.playbook_run_group.console_final_sequence': 38, 'public.playbook_run_group.console_next_sequence': 37,
    'public.playbook_run_group.launch_spec': 36, 'public.playbook_run_group.template_id': 35,
    'public.playbook_run_group.unreported_runs': 39,
    'public.playbook_schedule.delegated_by_user_id': 20, 'public.playbook_schedule.targets': 19,
    'public.playbook_schedule.template_id': 18,
}


def workspace_0016_delta(additions, replacements, removals):
    """Match the complete reviewed delta, including exact native definitions."""
    entries, shifts = [], {}
    try:
        for obj in additions:
            definition = obj['definition']
            if obj['kind'] == 'column':
                position = WORKSPACE_0016_ADDED_COLUMN_POSITIONS[obj['name']]
                table = obj['name'].rsplit('.', 1)[0]
                shift = definition['position'] - position
                if type(definition['position']) is not int or definition['position'] < 1:
                    return False
                if table in ('public.playbook_project_revision', 'public.playbook_run_console_chunk') and shift:
                    return False
                if table in shifts and shifts[table] != shift:
                    return False
                shifts[table] = shift
                definition = {**definition, 'position': position}
            entries.append((obj['kind'], obj['name'], None, definition_hash(definition)))
        for obj in replacements:
            entries.append((obj['kind'], obj['name'], definition_hash(obj['before']), definition_hash(obj['after'])))
        for obj in removals:
            entries.append((obj['kind'], obj['name'], definition_hash(obj['before']), None))
        if len(entries) != 387 or len({(e[0], e[1]) for e in entries}) != len(entries):
            return False
        encoded = json.dumps(sorted(entries), ensure_ascii=True, separators=(',', ':')).encode()
        return hashlib.sha256(encoded).hexdigest() == WORKSPACE_0016_DELTA_SHA256
    except (KeyError, TypeError, ValueError):
        return False


def workspace_0016_part(delta):
    """Select the workspace candidates, then require the complete pinned digest.

    Names only select candidates; they never authorize an object. The digest
    binds every selected identity and definition, including internal RI triggers.
    """
    functions = {'public.reject_project_revision_mutation()',
                 'public.reject_run_launch_mutation()'}
    workspace_index = ('index', 'public.run_group_hostname_attempt_unique')
    # Later pinned changes to objects unchanged by 0016 are separate edges.
    later_keys = {
        key for key in SCHEMA_TRANSITIONS
        if any(version > 16 for version, _before, _after in _schema_edges(key))
        and _schema_state(_schema_edges(key), 15) == _schema_state(_schema_edges(key), 16)
    }
    workspace = {kind: [obj for obj in delta[kind]
                        if (obj['kind'], obj['name']) not in later_keys and (
                        'playbook' in obj['name'] or
                        (obj['kind'] == 'function' and obj['name'] in functions) or
                        (obj['kind'], obj['name']) == workspace_index)]
                 for kind in ('additions', 'replacements', 'removals')}
    if not workspace_0016_delta(**workspace):
        raise ValueError('Unrecognized workspace migration delta')
    return workspace


def workspace_0016_conservation(baseline, delta):
    """Project only empty retired tables through the closed schema transition."""
    if not workspace_0016_delta(**delta):
        raise ValueError('Unrecognized workspace migration delta')
    columns = {name: list(fields) for name, fields in baseline['columns'].items()}
    rows = dict(baseline['rows'])
    for table in WORKSPACE_0016_RETIRED_TABLES:
        if table not in columns or rows.get(table) != EMPTY_ROW_DIGEST:
            raise ValueError('Workspace migration requires every retired table to be empty')
    for obj in delta['removals']:
        if obj['kind'] == 'column':
            table, field = obj['name'].rsplit('.', 1)
            if table not in WORKSPACE_0016_RETIRED_TABLES or field not in columns[table]:
                raise ValueError('Workspace migration removes an unapproved field')
            columns[table].remove(field)
    for obj in delta['removals']:
        if obj['kind'] == 'relation':
            table = obj['name']
            if table not in ('public.playbook', 'public.playbook_version'):
                raise ValueError('Workspace migration removes an unapproved table')
            del columns[table]
            del rows[table]
    return {**baseline, 'columns': columns, 'rows': rows}


def tag_cutover_pending(prefix_manifest, installed_prefix):
    if (type(installed_prefix) is not int or installed_prefix >= 6
            or not isinstance(prefix_manifest, dict)):
        return False
    sixth = {path: checksum for path, checksum in prefix_manifest.items()
             if re.fullmatch(r"apps/api/migrations/0*6_[^/]+\.sql", path)}
    if not sixth:
        return False
    if (sixth != {TAG_CUTOVER_PATH: TAG_CUTOVER_SHA384}
            or prefix_manifest.get("apps/api/migrations/0001_init.sql") != BASELINE_SQLX_SHA384
            or installed_prefix < 1):
        raise ValueError("Unknown pending Tag0006 source; canonical conservation proof required")
    return True


def tag_cutover_fields(columns):
    """Only native inventory memberships and the executable selector may change."""
    result = {table: "tags" for table, names in columns.items()
              if re.fullmatch(r"public\.(circuits|dcim|ipam|tenancy|virtualization|vpn|wireless)_.+", table)
              and "tags" in names}
    if TAG_SELECTOR_TABLE not in columns or "selector" not in columns[TAG_SELECTOR_TABLE]:
        raise ValueError("Tag cutover requires the pre-DDL executable selector")
    result[TAG_SELECTOR_TABLE] = "selector"
    return result


def dataflow_cutover_pending(prefix_manifest, installed_prefix):
    if (type(installed_prefix) is not int or installed_prefix >= 17
            or not isinstance(prefix_manifest, dict)):
        return False
    seventeenth = {path: checksum for path, checksum in prefix_manifest.items()
                   if re.fullmatch(r"apps/api/migrations/0*17_[^/]+\.sql", path)}
    if not seventeenth:
        return False
    if (seventeenth != {DATAFLOW_CUTOVER_PATH: DATAFLOW_CUTOVER_SHA384}
            or prefix_manifest.get("apps/api/migrations/0001_init.sql") != BASELINE_SQLX_SHA384
            or installed_prefix < 1):
        raise ValueError("Unknown pending Dataflow0017 source; canonical conservation proof required")
    return True


def dataflow_cutover_columns(columns):
    """Replace the dropped selector representation with the normalized layout."""
    if (DATAFLOW_FLOW_TABLE not in columns
            or set(columns[DATAFLOW_FLOW_TABLE]) != set(DATAFLOW_OLD_FLOW_COLUMNS)):
        raise ValueError("Dataflow0017 source flow columns are not the accepted representation")
    if set(columns.get(DATAFLOW_REMOVED_TABLE, ())) != {"dataflow_id", "side", "ipaddress_id"}:
        raise ValueError("Dataflow0017 source selected-IP relation is not the accepted representation")
    result = {table: list(names) for table, names in columns.items() if table != DATAFLOW_REMOVED_TABLE}
    result[DATAFLOW_FLOW_TABLE] = list(DATAFLOW_NEW_FLOW_COLUMNS)
    for table, names in DATAFLOW_NEW_TABLES.items():
        if table in result:
            raise ValueError("Dataflow0017 target relation already exists in the source")
        result[table] = list(names)
    return result


def dataflow_cutover_expression(table, name):
    """Deterministic projection of generated selector identities.

    The normalized key is the proven bijective owner/side/position relation, so
    fresh uuidv7 values do not make otherwise identical states incomparable.
    """
    if table == DATAFLOW_SELECTOR_TABLE and name == "id":
        return ("('flow:'||coalesce(dataflow_id::text,'group:'||group_id::text)"
                "||':'||side||':'||position::text)")
    if table == DATAFLOW_SELECTOR_IP_TABLE and name == "selector_id":
        return ("(SELECT 'flow:'||coalesce(s.dataflow_id::text,'group:'||s.group_id::text)"
                "||':'||s.side||':'||s.position::text "
                "FROM public.inventory_dataflow_selector s WHERE s.id=source.\"selector_id\")")
    return None


def fhrp_cutover_pending(prefix_manifest, installed_prefix):
    return (type(installed_prefix) is int and installed_prefix in (1, 2, 3)
            and isinstance(prefix_manifest, dict)
            and prefix_manifest.get("apps/api/migrations/0001_init.sql") == BASELINE_SQLX_SHA384
            and prefix_manifest.get(FHRP_CUTOVER_PATH) == FHRP_CUTOVER_SHA384)


def pending_transforms(prefix_manifest, installed_prefix):
    """Ordered, pinned transforms of prepared state C, never a plaintext converter."""
    return tuple(name for name, pending in (
        ("Service0003", service_cutover_pending(prefix_manifest, installed_prefix)),
        ("FHRP0004", fhrp_cutover_pending(prefix_manifest, installed_prefix)),
        ("Tag0006", tag_cutover_pending(prefix_manifest, installed_prefix)),
        ("ContentTypeSeeds", content_type_seeds_pending(prefix_manifest, installed_prefix)),
        ("Dataflow0017", dataflow_cutover_pending(prefix_manifest, installed_prefix)),
        ("Monitoring0020", monitoring_cutover_pending(prefix_manifest, installed_prefix)),
        ("ScheduleOwner0021", schedule_owner_cutover_pending(prefix_manifest, installed_prefix)),
    ) if pending)


def monitoring_cutover_pending(prefix_manifest, installed_prefix):
    versions = _validated_source_prefix(prefix_manifest, installed_prefix)
    return installed_prefix < 20 <= max(versions)


def schedule_owner_cutover_pending(prefix_manifest, installed_prefix):
    versions = _validated_source_prefix(prefix_manifest, installed_prefix)
    return installed_prefix < 21 <= max(versions)


def transformation_columns(columns, transforms):
    result = service_cutover_columns(columns) if "Service0003" in transforms else {
        table: list(names) for table, names in columns.items()}
    if "FHRP0004" in transforms:
        if FHRP_TABLE not in result or "auth_key" not in result[FHRP_TABLE]:
            raise ValueError("FHRP cutover requires the prepared pre-DDL source column")
        result[FHRP_TABLE].remove("auth_key")
    if "Dataflow0017" in transforms:
        result = dataflow_cutover_columns(result)
    if "Monitoring0020" in transforms:
        if (set(result.get(MONITORING_LEGACY_TABLE, ())) != set(MONITORING_LEGACY_COLUMNS)
                or set(result) & set(MONITORING_NEW_TABLES)):
            raise ValueError("Monitoring cutover requires the complete legacy-only source")
        del result[MONITORING_LEGACY_TABLE]
        result.update({table: list(names) for table, names in MONITORING_COLUMNS.items()})
    if "ScheduleOwner0021" in transforms:
        names = result.get(SCHEDULE_TABLE, ())
        if ("service_principal_id" not in names or "delegated_by_user_id" not in names
                or "owner_user_id" in names):
            raise ValueError("Schedule owner cutover requires the complete original identity")
        result[SCHEDULE_TABLE] = [
            "owner_user_id" if name == "delegated_by_user_id" else name
            for name in names if name != "service_principal_id"
        ]
    return result


FHRP_CUTOVER_VALIDATE_SQL = """
DO $fhrp_conservation$ BEGIN
 IF EXISTS (SELECT FROM public.ipam_fhrpgroup WHERE auth_key IS DISTINCT FROM '')
 THEN RAISE EXCEPTION 'FHRP cutover requires offline prepared state'; END IF;
END $fhrp_conservation$;
"""


def fhrp_cutover_removal(obj):
    # PostgreSQL18 drops this native NOT NULL constraint with the pinned column.
    key = obj["kind"], obj["name"]
    return (key == ("column", FHRP_TABLE + ".auth_key")
            or (key == ("constraint", FHRP_TABLE + ".ipam_fhrpgroup_auth_key_not_null")
                and obj["definition"] == "NOT NULL auth_key"))


def service_cutover_pending(prefix_manifest, installed_prefix):
    return (type(installed_prefix) is int and installed_prefix in (1, 2)
            and isinstance(prefix_manifest, dict)
            and prefix_manifest.get("apps/api/migrations/0001_init.sql") == BASELINE_SQLX_SHA384
            and prefix_manifest.get(SERVICE_CUTOVER_PATH) == SERVICE_CUTOVER_SHA384)


def service_cutover_removal(obj):
    """The closed catalog footprint of the checksum-bound representation change."""
    key = obj["kind"], obj["name"]
    allowed = {
        ("relation", SERVICE_MAPPING_TABLE),
        ("index", "public.ipam_service_port_mapping_pkey"),
        ("index", "public.idx_ipam_service_port_mapping_service"),
        *(("column", table + "." + column) for table in SERVICE_TABLES for column in ("protocol", "ports")),
        *(("column", SERVICE_MAPPING_TABLE + "." + column)
          for column in ("id", "service_id", "protocol", "port", "ordinal")),
        *(("constraint", SERVICE_MAPPING_TABLE + "." + name) for name in (
            "ipam_service_port_mapping_pkey", "ipam_service_port_mapping_port_check",
            "ipam_service_port_mapping_service_fkey")),
        # PostgreSQL18 represents NOT NULL as native constraints as well as
        # column metadata. Only those owned by the removed columns may vanish.
        *(("constraint", table + "." + table.split(".")[1] + "_" + column + "_not_null")
          for table in SERVICE_TABLES for column in ("protocol", "ports")),
        *(("constraint", SERVICE_MAPPING_TABLE + ".ipam_service_port_mapping_" + column + "_not_null")
          for column in ("id", "service_id", "protocol", "port", "ordinal")),
    }
    if key in allowed:
        return True
    definition = obj["definition"]
    constraint = definition.get("constraint") if isinstance(definition, dict) else None
    return (obj["kind"] == "trigger" and isinstance(definition, dict) and definition.get("internal") is True
            and isinstance(constraint, dict) and constraint.get("schema") == "public"
            and constraint.get("relation") == "ipam_service_port_mapping"
            and constraint.get("name") == "ipam_service_port_mapping_service_fkey")


def service_cutover_columns(columns):
    """Retain every unrelated column and bind both complete transformed rows."""
    if (SERVICE_MAPPING_TABLE not in columns
            or set(columns[SERVICE_MAPPING_TABLE]) != {"id", "service_id", "protocol", "port", "ordinal"}):
        raise ValueError("Service cutover source relation has unknown or missing fields")
    result = {table: list(names) for table, names in columns.items() if table != SERVICE_MAPPING_TABLE}
    for table in SERVICE_TABLES:
        if table not in result or not {"id", "protocol", "ports"} <= set(result[table]) or "port_mappings" in result[table]:
            raise ValueError("Service cutover source columns are not the accepted representation")
        result[table] = [name for name in result[table] if name not in ("protocol", "ports")] + ["port_mappings"]
    return result


def service_cutover_expression(table):
    if table not in SERVICE_TABLES:
        raise ValueError("Unknown service cutover table")
    relation = ("(SELECT array_agg(m.protocol || '/' || m.port::text ORDER BY m.ordinal) "
                "FROM public.ipam_service_port_mapping m WHERE m.service_id=source.id)"
                if table == "public.ipam_service" else "NULL::text[]")
    native = ("(SELECT array_agg(source.protocol || '/' || p::text ORDER BY ordinal) "
              "FROM unnest(source.ports) WITH ORDINALITY endpoints(p,ordinal))")
    return "coalesce(" + relation + "," + native + ",ARRAY[]::text[]) AS port_mappings"


# Read-only checks precede the expected post-state hash. No customer row/value is
# returned: PostgreSQL reports only whether the complete source graph is valid.
SERVICE_CUTOVER_VALIDATE_SQL = """
DO $service_conservation$ BEGIN
 IF EXISTS (
  SELECT FROM public.ipam_service_port_mapping m
  LEFT JOIN public.ipam_service s ON s.id=m.service_id
  WHERE s.id IS NULL OR m.protocol IS NULL OR m.protocol NOT IN ('tcp','udp','sctp')
   OR m.port IS NULL OR m.port NOT BETWEEN 1 AND 65535 OR m.ordinal IS NULL
 ) OR EXISTS (
  SELECT FROM public.ipam_service_port_mapping GROUP BY service_id,protocol,port HAVING count(*)>1
 ) OR EXISTS (
  SELECT FROM public.ipam_service_port_mapping GROUP BY service_id,ordinal HAVING count(*)>1
 ) THEN RAISE EXCEPTION 'Service cutover has invalid or ambiguous relation endpoints'; END IF;
 IF EXISTS (
  SELECT FROM (SELECT protocol,ports FROM public.ipam_service
               UNION ALL SELECT protocol,ports FROM public.ipam_servicetemplate) s
  WHERE (protocol IS NOT NULL OR ports IS NOT NULL) AND
   (protocol IS NULL OR ports IS NULL OR protocol NOT IN ('tcp','udp','sctp')
    OR cardinality(ports)=0 OR array_ndims(ports)<>1
    OR EXISTS (SELECT FROM unnest(ports) p WHERE p IS NULL OR p NOT BETWEEN 1 AND 65535)
    OR cardinality(ports)<>(SELECT count(DISTINCT p) FROM unnest(ports) p))
 ) THEN RAISE EXCEPTION 'Service cutover has unsupported native endpoints'; END IF;
 IF EXISTS (
  SELECT FROM public.ipam_service s
  WHERE s.protocol IS NOT NULL AND EXISTS (SELECT FROM public.ipam_service_port_mapping m WHERE m.service_id=s.id)
   AND NOT (
    ARRAY(SELECT s.protocol || '/' || p::text FROM unnest(s.ports) p)
     @> ARRAY(SELECT m.protocol || '/' || m.port::text FROM public.ipam_service_port_mapping m WHERE m.service_id=s.id)
    AND ARRAY(SELECT s.protocol || '/' || p::text FROM unnest(s.ports) p)
     <@ ARRAY(SELECT m.protocol || '/' || m.port::text FROM public.ipam_service_port_mapping m WHERE m.service_id=s.id)
   )
 ) THEN RAISE EXCEPTION 'Service cutover has conflicting endpoint representations'; END IF;
END $service_conservation$;
"""


def source_migration_history(files):
    """Discover one forward-only SQLx chain; bind names and unmodified SQL bytes."""
    history = []
    for name, content in files:
        match = re.fullmatch(r"([0-9]{4})_([A-Za-z0-9_]+)\.sql", name)
        if match is None or not isinstance(content, bytes):
            raise ValueError("Invalid forward SQLx migration file")
        history.append({"version": int(match[1]), "name": name,
                        "description": match[2].replace("_", " "),
                        "checksum": hashlib.sha384(content).hexdigest()})
    history.sort(key=lambda row: row["version"])
    if (not history or [row["version"] for row in history] != list(range(1, len(history) + 1))
            or history[0]["name"] != "0001_init.sql"
            or history[0]["checksum"] != BASELINE_SQLX_SHA384):
        raise ValueError("Source must extend the unchanged accepted 0001 baseline contiguously")
    return history


def _schema_edges(key):
    edges = SCHEMA_TRANSITIONS.get(key, ())
    previous_version = 1
    previous_after = edges[0][1] if edges else None
    for version, before, after in edges:
        if (type(version) is not int or version <= previous_version
                or version not in SCHEMA_MIGRATIONS or before != previous_after
                or before == after or any(value is not None and (
                    not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None
                ) for value in (before, after))):
            raise ValueError("Invalid closed schema authority")
        previous_version, previous_after = version, after
    return edges


def _schema_state(edges, prefix):
    state = edges[0][1]
    for version, _before, after in edges:
        if version > prefix:
            break
        state = after
    return state


def _schema_pair_authorized(key, before, after):
    """A materialized change must follow a reviewed complete-definition route."""
    if before == after:
        return False
    edges = _schema_edges(key)
    for start, (_version, initial, _next) in enumerate(edges):
        if initial == before and any(edge[2] == after for edge in edges[start:]):
            return True
    return False


def _validated_source_prefix(prefix_manifest, installed_prefix):
    if (type(installed_prefix) is not int or installed_prefix < 1
            or not isinstance(prefix_manifest, dict) or not prefix_manifest
            or prefix_manifest.get("apps/api/migrations/0001_init.sql") != BASELINE_SQLX_SHA384):
        raise ValueError("Forward schema authority requires a canonical installed prefix")
    versions = {}
    for path, checksum in prefix_manifest.items():
        match = re.fullmatch(r"apps/api/migrations/(0*[1-9][0-9]*)_[^/]+\.sql", path) if isinstance(path, str) else None
        if (match is None or not isinstance(checksum, str)
                or re.fullmatch(r"[0-9a-f]{96}", checksum) is None
                or int(match[1]) in versions):
            raise ValueError("Invalid source migration authority")
        versions[int(match[1])] = path, checksum
    latest = max(versions)
    if sorted(versions) != list(range(1, latest + 1)) or installed_prefix > latest:
        raise ValueError("Forward schema authority requires a contiguous source chain")
    for version, identity in SCHEMA_MIGRATIONS.items():
        if version <= latest and versions.get(version) != identity:
            raise ValueError("Unknown checksum-bound schema migration")
    return versions


def content_type_seeds_pending(prefix_manifest, installed_prefix):
    """Recognize only the closed, source-bound content-type seed migrations."""
    versions = _validated_source_prefix(prefix_manifest, installed_prefix)
    latest = max(versions)
    if any(CONTENT_TYPE_SOURCE_MIGRATIONS.get(version) != identity for version, identity in versions.items()):
        raise ValueError("Unknown checksum-bound content-type source migration")
    return any(installed_prefix < version <= latest for version in CONTENT_TYPE_SEED_MIGRATIONS)


def _monitoring_schema_identity(key):
    kind, name = key
    tables = {MONITORING_LEGACY_TABLE, *MONITORING_NEW_TABLES}
    if kind == "relation":
        return name in tables
    if kind in ("column", "constraint"):
        return name.rsplit(".", 1)[0] in tables
    if kind == "index":
        return name in MONITORING_SCHEMA_INDEXES
    if kind == "trigger":
        try:
            parts = json.loads(name)
        except (TypeError, ValueError):
            return False
        return (isinstance(parts, list) and len(parts) == 10
                and all(isinstance(value, str) for value in parts[:5])
                and (parts[0] + "." + parts[1] in tables
                     or (parts[2] == "internal" and parts[3] + "." + parts[4] in tables)))
    return False


def _monitoring_schema_authority(old, new):
    edges = [
        {"kind": key[0], "name": key[1],
         "before": definition_hash(old[key]) if key in old else None,
         "after": definition_hash(new[key]) if key in new else None}
        for key in sorted(old.keys() | new.keys())
        if old.get(key) != new.get(key) and _monitoring_schema_identity(key)
    ]
    if len(edges) != 139 or definition_hash(edges) != MONITORING_SCHEMA_DELTA_SHA256:
        raise ValueError("Monitoring migration differs from its closed native catalog delta")
    return {(edge["kind"], edge["name"]) for edge in edges}


def _monitoring_delta_pairs(delta):
    pairs = {}
    for operation, items in delta.items():
        for obj in items:
            key = obj["kind"], obj["name"]
            if not _monitoring_schema_identity(key):
                continue
            if key in pairs:
                raise ValueError("Duplicate monitoring catalog edge")
            before = obj.get("before") if operation != "additions" else None
            after = obj.get("after") if operation == "replacements" else (
                obj["definition"] if operation == "additions" else None)
            pairs[key] = before, after
    return pairs


def _monitoring_delta_authority(delta, *, source_delta=None):
    removes_legacy = any(isinstance(obj, dict) and (obj.get("kind"), obj.get("name"))
                         == ("relation", MONITORING_LEGACY_TABLE) for obj in delta["removals"])
    if source_delta is not None:
        authority = _monitoring_delta_authority(source_delta)
        if authority or removes_legacy:
            source, translated = _monitoring_delta_pairs(source_delta), _monitoring_delta_pairs(delta)
            if not authority or set(translated) != authority:
                raise ValueError("Translated monitoring delta changed its closed identities")
            for key, pair in translated.items():
                expected = source[key]
                if key[0] == "column":
                    pair = tuple(_column_semantic_hash(value) for value in pair)
                    expected = tuple(_column_semantic_hash(value) for value in expected)
                if pair != expected:
                    raise ValueError("Translated monitoring delta changed non-layout definitions")
            return authority
    if not removes_legacy:
        return set()
    pairs = _monitoring_delta_pairs(delta)
    edges = [
        {"kind": key[0], "name": key[1],
         "before": definition_hash(pair[0]) if pair[0] is not None else None,
         "after": definition_hash(pair[1]) if pair[1] is not None else None}
        for key, pair in sorted(pairs.items())
    ]
    if len(edges) != 139 or definition_hash(edges) != MONITORING_SCHEMA_DELTA_SHA256:
        raise ValueError("Monitoring migration differs from its closed native catalog delta")
    return {(edge["kind"], edge["name"]) for edge in edges}


def _column_semantic_hash(definition):
    if definition is None:
        return None
    if (not isinstance(definition, dict) or type(definition.get("position")) is not int
            or definition["position"] < 1):
        raise ValueError("Invalid native column definition")
    return definition_hash({key: value for key, value in definition.items() if key != "position"})


def native_column_routes(delta):
    """Keep source-authorized column semantics when a proven layout moves slots.

    The caller must prove the complete source-native layout separately. This
    binds every other definition field and never grants arbitrary column DDL.
    """
    if not isinstance(delta, dict) or set(delta) != {"additions", "replacements", "removals"}:
        raise ValueError("Invalid source catalog delta")
    monitoring = _monitoring_delta_authority(delta)
    routes = {}
    for operation, items in delta.items():
        for obj in items:
            key = obj["kind"], obj["name"]
            if key[0] != "column":
                continue
            before = obj.get("before") if operation != "additions" else None
            after = obj.get("after") if operation == "replacements" else (
                obj["definition"] if operation == "additions" else None)
            source_object = {"kind": key[0], "name": key[1], "definition": before}
            removed_source_column = (after is None and (
                service_cutover_removal(source_object) or fhrp_cutover_removal(source_object)))
            if (key in monitoring or removed_source_column or _schema_pair_authorized(
                    key, definition_hash(before) if before is not None else None,
                    definition_hash(after) if after is not None else None)):
                if key in routes:
                    raise ValueError("Duplicate source column route")
                routes[key] = _column_semantic_hash(before), _column_semantic_hash(after)
    return routes


def _schema_forward_authority(old, new, prefix_manifest, installed_prefix):
    versions = _validated_source_prefix(prefix_manifest, installed_prefix)
    latest = max(versions)
    allowed = set()
    for key in SCHEMA_TRANSITIONS:
        edges = _schema_edges(key)
        if not any(installed_prefix < edge[0] <= latest for edge in edges):
            continue
        actual_before = definition_hash(old[key]) if key in old else None
        actual_after = definition_hash(new[key]) if key in new else None
        if (actual_before != _schema_state(edges, installed_prefix)
                or actual_after != _schema_state(edges, latest)):
            raise ValueError("Forward schema differs from its closed native definitions")
        allowed.add(key)
    if installed_prefix < 20 <= latest:
        allowed.update(_monitoring_schema_authority(old, new))
    return allowed


def forward_catalog_delta(before, after, *, prefix_manifest, installed_prefix):
    """Derive the bounded delta from native execution of the source-bound artifact.

    Observing destructive DDL is not authority. Schema changes require the closed,
    checksum-bound native transitions; data transforms retain their separate
    conservation proof in the deployment backup/restore path.
    """
    # A changed Tag migration is not an ordinary additive schema migration.
    tag_cutover_pending(prefix_manifest, installed_prefix)
    old, new = _catalog_index(before), _catalog_index(after)
    schema = _schema_forward_authority(old, new, prefix_manifest, installed_prefix)
    versions = _validated_source_prefix(prefix_manifest, installed_prefix)
    if installed_prefix == 15 and max(versions) in (16, 17, 18, 19):
        additions = [o for o in after if (o['kind'], o['name']) not in old]
        removals = [{'kind': k[0], 'name': k[1], 'before': old[k]} for k in sorted(old.keys() - new.keys())]
        replacements = [{'kind': k[0], 'name': k[1], 'before': old[k], 'after': new[k]}
                        for k in sorted(old.keys() & new.keys()) if old[k] != new[k]]
        delta = {'additions': additions, 'replacements': replacements, 'removals': removals}
        workspace = workspace_0016_part(delta) if max(versions) in (17, 18, 19) else delta
        if not workspace_0016_delta(**workspace):
            raise ValueError('Workspace migration differs from its closed native catalog delta')
        workspace_keys = {(obj['kind'], obj['name']) for items in workspace.values() for obj in items}
        for kind, items in delta.items():
            for obj in items:
                key = obj['kind'], obj['name']
                if kind != 'additions' and key not in workspace_keys and key not in schema:
                    raise ValueError('Combined migration contains an unapproved catalog edge')
        exact_target_preservation(before, after, additions=additions, replacements=replacements, removals=removals)
        return {'additions': additions, 'replacements': replacements, 'removals': removals}
    removals = []
    for obj in before:
        if (obj["kind"], obj["name"]) not in new:
            if not ((service_cutover_pending(prefix_manifest, installed_prefix) and service_cutover_removal(obj))
                    or (fhrp_cutover_pending(prefix_manifest, installed_prefix) and fhrp_cutover_removal(obj))
                    or (obj["kind"], obj["name"]) in schema):
                raise ValueError("Forward migration removed sealed catalog objects without a bound data transformation")
            removals.append({"kind": obj["kind"], "name": obj["name"], "before": obj["definition"]})
    additions = [o for o in after if (o["kind"], o["name"]) not in old]
    replacements = []
    for key in sorted(old):
        if key in new and old[key] != new[key]:
            if key not in schema:
                raise ValueError("Unsupported forward catalog replacement")
            replacements.append({"kind": key[0], "name": key[1],
                                 "before": old[key], "after": new[key]})
    exact_target_preservation(before, after, additions=additions, replacements=replacements, removals=removals)
    return {"additions": additions, "replacements": replacements, "removals": removals}


# RI trigger names contain database-local OIDs. Internal identities instead bind
# constraint relation/name, trigger relation, function and event semantics; the
# firing mode remains part of the definition for both user and internal triggers.
CATALOG_SQL = """
WITH namespaces AS (
 SELECT oid,nspname FROM pg_namespace
 WHERE nspname NOT LIKE 'pg\\_%' ESCAPE '\\' AND nspname <> 'information_schema'
), objects AS (
 SELECT 'namespace' AS kind,nspname::text AS name,to_jsonb(nspname) AS definition
 FROM namespaces
 UNION ALL
 SELECT 'relation',n.nspname || '.' || c.relname,
 jsonb_build_object('kind',c.relkind,'persistence',c.relpersistence,'options',c.reloptions,
                   'row_security',c.relrowsecurity,'force_row_security',c.relforcerowsecurity)
 FROM pg_class c JOIN namespaces n ON n.oid=c.relnamespace
 WHERE c.relkind IN ('r','p','v','m','S','f')
 UNION ALL
 SELECT 'column',n.nspname || '.' || c.relname || '.' || a.attname,
 jsonb_build_object('position',a.attnum,'type',format_type(a.atttypid,a.atttypmod),
 'not_null',a.attnotnull,'identity',a.attidentity,'generated',a.attgenerated,
 'collation',co.collname,'default',pg_get_expr(d.adbin,d.adrelid))
 FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid
 JOIN namespaces n ON n.oid=c.relnamespace
 LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
 LEFT JOIN pg_collation co ON co.oid=a.attcollation
 WHERE a.attnum>0 AND NOT a.attisdropped AND c.relkind IN ('r','p','v','m','f')
 UNION ALL
 SELECT 'constraint',n.nspname || '.' || coalesce(c.relname,t.typname) || '.' || k.conname,
 to_jsonb(pg_get_constraintdef(k.oid))
 FROM pg_constraint k JOIN namespaces n ON n.oid=k.connamespace
 LEFT JOIN pg_class c ON c.oid=k.conrelid LEFT JOIN pg_type t ON t.oid=k.contypid
 UNION ALL
 SELECT 'index',n.nspname || '.' || c.relname,to_jsonb(pg_get_indexdef(c.oid))
 FROM pg_class c JOIN namespaces n ON n.oid=c.relnamespace WHERE c.relkind IN ('i','I')
 UNION ALL
 SELECT 'function',n.nspname || '.' || p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')',
 to_jsonb(pg_get_functiondef(p.oid))
 FROM pg_proc p JOIN namespaces n ON n.oid=p.pronamespace WHERE p.prokind IN ('f','p')
 UNION ALL
 SELECT 'enum',n.nspname || '.' || t.typname || '.' || e.enumsortorder,to_jsonb(e.enumlabel)
 FROM pg_enum e JOIN pg_type t ON t.oid=e.enumtypid JOIN namespaces n ON n.oid=t.typnamespace
 UNION ALL
 SELECT 'view',n.nspname || '.' || c.relname,to_jsonb(pg_get_viewdef(c.oid))
 FROM pg_class c JOIN namespaces n ON n.oid=c.relnamespace WHERE c.relkind IN ('v','m')
 UNION ALL
 SELECT 'trigger',
 CASE WHEN t.tgisinternal THEN jsonb_build_array(
   n.nspname,c.relname,'internal',kn.nspname,kc.relname,k.conname,
   pn.nspname,p.proname,pg_get_function_identity_arguments(p.oid),t.tgtype
 )::text ELSE n.nspname || '.' || c.relname || '.' || t.tgname END,
 jsonb_build_object(
   'enabled',t.tgenabled,'internal',t.tgisinternal,'events',t.tgtype,
   'function',pn.nspname || '.' || p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')',
   'deferrable',t.tgdeferrable,'initially_deferred',t.tginitdeferred,
   'constraint',CASE WHEN k.oid IS NULL THEN NULL ELSE jsonb_build_object(
      'schema',kn.nspname,'relation',kc.relname,'name',k.conname,'definition',pg_get_constraintdef(k.oid)) END,
   'referenced_relation',CASE WHEN rc.oid IS NULL THEN NULL ELSE jsonb_build_array(rn.nspname,rc.relname) END,
   'columns',(SELECT coalesce(jsonb_agg(a.attname ORDER BY a.attnum),'[]'::jsonb)
              FROM pg_attribute a WHERE a.attrelid=t.tgrelid AND a.attnum=ANY(t.tgattr)),
   'arguments',encode(t.tgargs,'hex'),'condition_present',t.tgqual IS NOT NULL,
   'parent_relation',CASE WHEN pc.oid IS NULL THEN NULL ELSE jsonb_build_array(pcn.nspname,pc.relname) END,
   'create',CASE WHEN t.tgisinternal THEN NULL ELSE pg_get_triggerdef(t.oid) END
 )
 FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid
 JOIN namespaces n ON n.oid=c.relnamespace
 JOIN pg_proc p ON p.oid=t.tgfoid JOIN pg_namespace pn ON pn.oid=p.pronamespace
 LEFT JOIN pg_constraint k ON k.oid=t.tgconstraint
 LEFT JOIN pg_namespace kn ON kn.oid=k.connamespace LEFT JOIN pg_class kc ON kc.oid=k.conrelid
 LEFT JOIN pg_class rc ON rc.oid=t.tgconstrrelid LEFT JOIN pg_namespace rn ON rn.oid=rc.relnamespace
 LEFT JOIN pg_trigger pt ON pt.oid=t.tgparentid LEFT JOIN pg_class pc ON pc.oid=pt.tgrelid
 LEFT JOIN pg_namespace pcn ON pcn.oid=pc.relnamespace
 UNION ALL
 SELECT 'type',n.nspname || '.' || t.typname,
 jsonb_build_object('kind',t.typtype,'base',format_type(t.typbasetype,t.typtypmod),
                   'not_null',t.typnotnull,'default',t.typdefault)
 FROM pg_type t JOIN namespaces n ON n.oid=t.typnamespace WHERE t.typtype IN ('d','e')
 UNION ALL
 SELECT 'policy',n.nspname || '.' || c.relname || '.' || p.polname,
 jsonb_build_object('command',p.polcmd,'permissive',p.polpermissive,
 'using',pg_get_expr(p.polqual,p.polrelid),'check',pg_get_expr(p.polwithcheck,p.polrelid))
 FROM pg_policy p JOIN pg_class c ON c.oid=p.polrelid JOIN namespaces n ON n.oid=c.relnamespace
 UNION ALL
  SELECT 'extension',e.extname,to_jsonb(e.extversion) FROM pg_extension e
  UNION ALL
  SELECT 'collation',n.nspname || '.' || c.collname,
  jsonb_build_object('provider',c.collprovider,'deterministic',c.collisdeterministic,
                    'encoding',c.collencoding,'collate',c.collcollate,'ctype',c.collctype,
                    'locale',c.colllocale,'rules',c.collicurules,'version',c.collversion)
  FROM pg_collation c JOIN namespaces n ON n.oid=c.collnamespace
  UNION ALL
 SELECT 'sequence',n.nspname || '.' || c.relname,
 jsonb_build_object('type',format_type(s.seqtypid,NULL),'start',s.seqstart,'increment',s.seqincrement,
 'maximum',s.seqmax,'minimum',s.seqmin,'cache',s.seqcache,'cycle',s.seqcycle)
 FROM pg_sequence s JOIN pg_class c ON c.oid=s.seqrelid JOIN namespaces n ON n.oid=c.relnamespace
 UNION ALL
 SELECT 'event_trigger',evtname,
 jsonb_build_object('event',evtevent,'enabled',evtenabled,'tags',evttags,'function',evtfoid::regprocedure::text)
 FROM pg_event_trigger
)
SELECT coalesce(jsonb_agg(to_jsonb(objects) ORDER BY kind COLLATE "C",name COLLATE "C"),'[]'::jsonb)
FROM objects;
"""


def catalog_objects(raw):
    objects = json.loads(raw)
    if not isinstance(objects, list) or any(
        not isinstance(o, dict) or set(o) != {"kind", "name", "definition"}
        or not isinstance(o["kind"], str) or not isinstance(o["name"], str)
        for o in objects
    ):
        raise ValueError("Invalid native catalog")
    if len({(o["kind"], o["name"]) for o in objects}) != len(objects):
        raise ValueError("Schema catalog identities must remain lossless and unique")
    return objects


def definition_hash(definition):
    """Hash the complete JSON definition, not its display rendering."""
    return hashlib.sha256(json.dumps(
        definition, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
        allow_nan=False,
    ).encode()).hexdigest()


def _catalog_index(objects):
    return {(o["kind"], o["name"]): o["definition"]
            for o in catalog_objects(json.dumps(objects, allow_nan=False))}


def preservation_extensions(canonical, installed, *, trusted_inventory=()):
    """Require the entire extra inventory to equal an externally approved one.

    Approval authentication, target binding and expiry belong to the caller.
    Observing an object never grants approval; names and prefixes grant none.
    """
    expected, actual = _catalog_index(canonical), _catalog_index(installed)
    inventory = {}
    for item in trusted_inventory:
        if (not isinstance(item, dict)
                or set(item) != {"kind", "name", "definition_sha256"}
                or not all(isinstance(value, str) for value in item.values())
                or len(item["definition_sha256"]) != 64
                or any(c not in "0123456789abcdef" for c in item["definition_sha256"])):
            raise ValueError("Invalid trusted extension inventory")
        identity = item["kind"], item["name"]
        if identity in expected or identity in inventory:
            raise ValueError("Extension inventory must be unique and noncanonical")
        inventory[identity] = item["definition_sha256"]
    extras = {key: definition_hash(value) for key, value in actual.items()
              if key not in expected}
    if extras != inventory:
        raise ValueError("Installed extras do not equal the trusted inventory")
    return inventory


def migration_catalog(before, *, additions=(), replacements=(), removals=(), layout_contract=None, source_delta=None):
    """Apply an explicitly authorized delta, binding both sides of each replacement."""
    expected = _catalog_index(before)
    delta = {'additions': additions, 'replacements': replacements, 'removals': removals}
    monitoring_keys = _monitoring_delta_authority(delta, source_delta=source_delta)
    column_routes = native_column_routes(source_delta) if source_delta is not None else {}

    def column_route(key, old, new):
        return (key[0] == "column" and key in column_routes
                and column_routes[key] == (_column_semantic_hash(old), _column_semantic_hash(new)))
    workspace_keys = set()
    try:
        workspace = workspace_0016_part(delta)
    except ValueError:
        workspace = delta if workspace_0016_delta(**delta) else None
    if workspace is not None:
        workspace_keys = {(obj['kind'], obj['name']) for items in workspace.values() for obj in items}
    if layout_contract is not None and workspace is None:
        raise ValueError("Layout binding requires the complete authorized workspace delta")
    positions = baseline_layout_positions(layout_contract, before) if layout_contract is not None else {}
    source_positions = layout_contract["position_pairs"] if layout_contract is not None else {}
    removed = set()
    for removal in removals:
        if not isinstance(removal, dict) or set(removal) != {"kind", "name", "before"}:
            raise ValueError("Invalid authorized catalog removal")
        key = removal["kind"], removal["name"]
        definition = removal["before"]
        if key[0] == "column" and key[1] in source_positions:
            source_position, installed_position = source_positions[key[1]]
            if not isinstance(definition, dict) or definition.get("position") != source_position:
                raise ValueError("Source removal does not bind the sealed layout")
            if key[1] in positions:
                definition = {**definition, "position": installed_position}
        if key in removed or key not in expected or expected[key] != definition:
            raise ValueError("Catalog removal does not bind the sealed target")
        source_object = {"kind": key[0], "name": key[1], "definition": removal["before"]}
        if not (key in workspace_keys or key in monitoring_keys
                or service_cutover_removal(source_object) or fhrp_cutover_removal(source_object)
                or _schema_pair_authorized(key, definition_hash(removal["before"]), None)
                or column_route(key, removal["before"], None)):
            raise ValueError("Catalog removal is outside the closed authority")
        removed.add(key)
        del expected[key]
    replaced = set()
    for replacement in replacements:
        if (not isinstance(replacement, dict)
                or set(replacement) != {"kind", "name", "before", "after"}
                or replacement["kind"] not in ("column", "constraint", "index", "trigger", "function")):
            raise ValueError("Invalid authorized schema replacement")
        key = replacement["kind"], replacement["name"]
        if key in replaced or key not in expected or expected[key] != replacement["before"]:
            raise ValueError("Schema replacement does not bind the sealed target")
        if (key not in workspace_keys and key not in monitoring_keys
                and not _schema_pair_authorized(key, definition_hash(replacement["before"]),
                                                definition_hash(replacement["after"]))
                and not column_route(key, replacement["before"], replacement["after"])):
            raise ValueError("Schema replacement is outside the closed authority")
        replaced.add(key)
        expected[key] = replacement["after"]
    for key, value in _catalog_index(additions).items():
        if key in expected:
            raise ValueError("An addition cannot replace a sealed target object")
        if (key not in workspace_keys and key in SCHEMA_TRANSITIONS
                and not _schema_pair_authorized(key, None, definition_hash(value))
                and not column_route(key, None, value)):
            raise ValueError("Schema creation is outside the closed authority")
        expected[key] = value
    return [{"kind": key[0], "name": key[1], "definition": value} for key, value in sorted(expected.items())]


def exact_target_preservation(before, after, *, additions=(), replacements=(), removals=(), layout_contract=None, source_delta=None):
    """A sealed target remains exact outside the source-authorized migration delta."""
    expected = _catalog_index(migration_catalog(before, additions=additions, replacements=replacements,
                                                removals=removals, layout_contract=layout_contract,
                                                source_delta=source_delta))
    if expected != _catalog_index(after):
        raise ValueError("Sealed target catalog was not preserved exactly")


# Native node trees retain source token offsets. They are not execution semantics.
# No other node field is removed or rewritten (including OIDs and attnums).
CHECK_BINDINGS_SQL = r"""
SELECT coalesce(jsonb_agg(jsonb_build_object(
 'name', n.nspname || '.' || c.relname || '.' || k.conname,
 'definition', pg_get_constraintdef(k.oid),
 'ast', regexp_replace(k.conbin::text, ':(location|list_start|list_end) -?[0-9]+', ':\1 -1', 'g'),
 'validated', k.convalidated, 'enforced', k.conenforced,
 'no_inherit', k.connoinherit,
 'columns', (SELECT jsonb_agg(jsonb_build_object(
    'name', a.attname, 'position', a.attnum,
    'type', format_type(a.atttypid,a.atttypmod),
    'collation', CASE WHEN a.attcollation=0 THEN NULL ELSE
       cn.nspname || '.' || co.collname END,
    'deterministic', co.collisdeterministic) ORDER BY a.attnum)
   FROM pg_attribute a LEFT JOIN pg_collation co ON co.oid=a.attcollation
   LEFT JOIN pg_namespace cn ON cn.oid=co.collnamespace
   WHERE a.attrelid=k.conrelid AND a.attnum=ANY(k.conkey)),
 'dependencies', (SELECT coalesce(jsonb_agg(
    pg_describe_object(d.refclassid,d.refobjid,d.refobjsubid)
    ORDER BY pg_describe_object(d.refclassid,d.refobjid,d.refobjsubid)), '[]'::jsonb)
   FROM pg_depend d WHERE d.classid='pg_constraint'::regclass AND d.objid=k.oid
     AND NOT (d.refclassid='pg_class'::regclass AND d.refobjid=k.conrelid))
) ORDER BY k.conname), '[]'::jsonb)
FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid
JOIN pg_namespace n ON n.oid=c.relnamespace
WHERE k.contype='c' AND k.conrelid=%s::regclass;
"""


@dataclass(frozen=True)
class CheckForm:
    """Exact source-registered syntax, never a general expression normalizer."""

    name: str
    columns: tuple
    canonical: str
    historical: str
    canonical_sql: str
    historical_sql: str
    members: tuple = ()
    atoms: tuple = ()


HISTORICAL_SOURCE_SHA = "77f58d4c7ac58a6792efe1044c9684bb9316da32"
HISTORICAL_POSTGRES_IMAGE = "sha256:54451ecb8ab38c24c3ec123f2fd501303a3a1856a5c66e98cecf2460d5e1e9d7"
# SHA-256 of the canonical JSON {migration path: SQLx SHA-384}, versions 1..22.
HISTORICAL_PREFIX_HASH = "45d7d65473b8fe39947105fe6350824fb9ef224b6968596bf86e2aca39316485"
REGISTERED_CHECK_SOURCES = {
    HISTORICAL_SOURCE_SHA: (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "96c9833e6a4cfdee0eff9c206fcd2cbee9e8dd53": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "70254dc18cd36e8717de88284ba7456a12f50742": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "ccb216481bcbc4d54315205269753e5d9d276d29": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "5e4b78bfe647ea55829f24992b944cd2a8392172": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
}


def validate_check_source(source_sha, prefix_hash, postgres_image):
    """Registration binds the actual candidate, not an alias to the historical oracle."""
    if not isinstance(source_sha, str) or REGISTERED_CHECK_SOURCES.get(source_sha) != (prefix_hash, postgres_image):
        raise ValueError("Unregistered source or PostgreSQL binding")


def historical_check_forms():
    """The twelve source predicates; registration is not permission to accept drift."""
    specs = (
        ("connconf_assignment", "connconf_assignment_scope_type_check", "scope_type", 32,
         "global region site_group site location rack tenant cluster role device_type platform device vm connection_target"),
        ("connections_proxy_assignment", "connections_proxy_assignment_scope_type_check", "scope_type", 32,
         "global region site_group site location rack tenant cluster role device_type platform device vm"),
        ("custom_field", "custom_field_filter_logic_valid", "filter_logic", 10, "disabled loose exact"),
        ("custom_field", "custom_field_type_valid", "field_type", 20,
         "text longtext integer decimal boolean date datetime url json select multiselect object multiobject"),
        ("custom_field", "custom_field_ui_editable_valid", "ui_editable", 10, "yes no hidden"),
        ("custom_field", "custom_field_ui_visible_valid", "ui_visible", 10, "always if-set hidden"),
        ("mapping_configurations", "mapping_configurations_conflict_mode_check", "conflict_mode", 30,
         "netbox_wins piping_wins last_write_wins field_level"),
        ("mapping_configurations", "mapping_configurations_direction_check", "direction", 30, "pull push bidirectional"),
        ("mapping_configurations", "mapping_configurations_status_check", "status", 30, "draft active retired"),
        ("secrets_variable", "chk_secrets_variable_kind", "kind", 32, "field named"),
        ("wizard_progress", "wizard_progress_status_check", "status", 20, "active completed dismissed"),
    )
    forms = []
    for table, name, column, width, values in specs:
        literals = [f"'{value}'::character varying" for value in values.split()]
        element = f"CHECK ((({column})::text = ANY (ARRAY[" + ", ".join(
            f"({literal})::text" for literal in literals) + "])))"
        array = f"CHECK ((({column})::text = ANY ((ARRAY[" + ", ".join(literals) + "])::text[])))"
        canonical, historical = (array, element) if table == "secrets_variable" else (element, array)
        # PostgreSQL 18 rewrites an explicit varchar[] -> text[] cast while
        # parsing DDL. Its native IN compiler still produces that typed array
        # node. Require the resulting deparser text to match the registered
        # form exactly, rather than rewriting or interpreting the node tree.
        native_array = f"CHECK ({column} IN (" + ", ".join(
            f"'{value}'" for value in values.split()) + "))"
        canonical_sql, historical_sql = (native_array, element) if table == "secrets_variable" else (element, native_array)
        forms.append(CheckForm(f"public.{table}.{name}",
                               ((column, f"character varying({width})"),), canonical, historical,
                               canonical_sql, historical_sql, tuple(values.split())))
    atoms = ("(content_size_bytes >= 1)", "(content_size_bytes <= 2097152)",
             "((content_text IS NULL) OR (octet_length(content_text) <= 2097152))",
             "((content_json IS NULL) OR (octet_length((content_json)::text) <= 2097152))")
    flat = "CHECK ((" + " AND ".join(atoms) + "))"
    nested = "CHECK (((" + " AND ".join(atoms[:2]) + ") AND " + " AND ".join(atoms[2:]) + "))"
    forms.append(CheckForm("public.playbook_artifact.chk_playbook_artifact_size",
                           (("content_size_bytes", "bigint"), ("content_text", "text"), ("content_json", "jsonb")),
                           flat, nested, flat, nested, atoms=atoms))
    return {form.name: form for form in forms}


def _identifier(value):
    if not isinstance(value, str) or not value or not value.isascii() or not all(
        c.isalnum() or c == "_" for c in value
    ) or value[0].isdigit():
        raise ValueError("Unsafe native proof identifier")
    return '"' + value + '"'


_NATIVE_PROOF_ISSUER = object()


@dataclass(frozen=True)
class NativeCheckProof:
    name: str
    canonical: str
    historical: str
    binding_hash: str
    planned_expression: str
    source_sha: str
    prefix_hash: str
    postgres_image: str
    canonical_ast_hash: str
    historical_ast_hash: str
    historical_compiler_definition: str
    association_truth_rows: int = 0
    capture_hash: str = ""
    _issuer: object = field(default=None, init=False, repr=False, compare=False)

    def receipt(self):
        """Diagnostic JSON is not an importable authorization receipt."""
        return {key: value for key, value in self.__dict__.items() if not key.startswith("_")}


def _associated_source_ast(references, form):
    """Compose only the registered first-two-atoms AND association.

    Every inserted node is emitted by PostgreSQL for a source-owned expression.
    Exact string equality checks the entire resulting typed tree. No node is
    parsed, no arbitrary tree is normalized, and no field other than the existing
    source-offset projection is ignored.
    """
    native = {r["name"].rsplit(".", 1)[1]: r for r in references}
    atoms = [native[f"proof_atom_{index}"]["ast"] for index in range(4)]
    canonical = native["canonical"]["ast"]
    pair = native["proof_pair"]["ast"]
    consecutive = " ".join(atoms[:2])
    if (not form.atoms or canonical.count(" ".join(atoms)) != 1
            or pair.count(consecutive) != 1 or canonical.count(consecutive) != 1):
        raise ValueError("Native source atom nodes are not the registered AND structure")
    return canonical.replace(consecutive, pair, 1)


def prove_native_check(sql, form, installed_binding, *, source_sha, postgres_image,
                       prefix_sha384, original_conkey=None, original_dependencies=None,
                       original_identity=None):
    """Compile exact registered forms on the target server and bind its real AST.

    ``sql`` executes one native statement and returns its scalar text. The caller
    owns a disposable transaction/session and rollback. It must supply bindings
    read with CHECK_BINDINGS_SQL on that same server, not restored deparser text.
    The planner is the equivalence oracle; sample evaluations are not the proof.
    """
    prefix_hash = definition_hash(prefix_sha384)
    validate_check_source(source_sha, prefix_hash, postgres_image)
    if form != historical_check_forms().get(form.name):
        raise ValueError("Unregistered source or PostgreSQL binding")
    if sql("SELECT current_setting('server_version')") != "18.3":
        raise ValueError("Historical proof requires PostgreSQL 18.3")
    binding = installed_binding
    if (not isinstance(binding, dict)
            or set(binding) != {"name", "definition", "ast", "validated", "enforced", "no_inherit", "columns", "dependencies"}
            or binding["name"] != form.name
            or binding["definition"] not in (form.canonical, form.historical)
            or binding["validated"] is not True or binding["enforced"] is not True
            or binding["no_inherit"] is not False):
        raise ValueError("Installed CHECK is not the registered enforced, validated form")
    columns = binding["columns"]
    if (not isinstance(columns, list) or len(columns) != len(form.columns)
            or any(not isinstance(c, dict) or set(c) != {"name", "position", "type", "collation", "deterministic"}
                   or not isinstance(c["name"], str) or not isinstance(c["type"], str) for c in columns)
            or {c["name"]: c["type"] for c in columns} != dict(form.columns)
            or any(set(c) != {"name", "position", "type", "collation", "deterministic"}
                   or type(c["position"]) is not int or not 1 <= c["position"] <= 1600
                   or c["collation"] not in (None, "pg_catalog.default")
                   or (c["collation"] is not None and c["deterministic"] is not True)
                   for c in columns)
            or len({c["position"] for c in columns}) != len(columns)):
        raise ValueError("CHECK column type, position or collation is unsupported")
    slots = {c["position"]: c for c in columns}
    declarations = []
    for position in range(1, max(slots) + 1):
        column = slots.get(position)
        declarations.append(f'{_identifier(column["name"])} {column["type"]}' if column else
                            f'"_proof_gap_{position}" integer')
    sql("SET search_path = pg_catalog, pg_temp")
    atom_constraints = ""
    if form.atoms:
        atom_constraints = "".join(f", CONSTRAINT proof_atom_{index} CHECK ({atom})"
                                   for index, atom in enumerate(form.atoms))
        atom_constraints += ", CONSTRAINT proof_pair CHECK (" + " AND ".join(form.atoms[:2]) + ")"
    sql("CREATE TEMP TABLE pwa_check_proof (" + ",".join(declarations)
        + ", CONSTRAINT canonical " + form.canonical_sql
        + ", CONSTRAINT historical " + form.historical_sql + atom_constraints + ")")
    try:
        references = json.loads(sql(CHECK_BINDINGS_SQL % "'pg_temp.pwa_check_proof'"))
        canonical_reference = next(r for r in references if r["name"].endswith(".canonical"))
        historical_reference = next(r for r in references if r["name"].endswith(".historical"))
        association_rows = 0
        if form.atoms:
            historical_reference = dict(historical_reference)
            historical_reference["ast"] = _associated_source_ast(references, form)
            historical_reference["definition"] = form.historical
            # Boolean atoms each have exactly three possible SQL values. This
            # exhausts the domain; it is not sampling the unbounded column data.
            truth = json.loads(sql("WITH truth(v) AS (VALUES(true),(false),(NULL::boolean)) "
                                   "SELECT jsonb_build_array(count(*),bool_and("
                                   "((a.v AND b.v) AND c.v AND d.v) IS NOT DISTINCT FROM "
                                   "(a.v AND b.v AND c.v AND d.v))) FROM truth a "
                                   "CROSS JOIN truth b CROSS JOIN truth c CROSS JOIN truth d"))
            if truth != [81, True]:
                raise ValueError("Native SQL three-valued AND association failed")
            association_rows = 81
        reference = canonical_reference if binding["definition"] == form.canonical else historical_reference
        if reference["definition"] != binding["definition"] or any(binding[key] != reference[key] for key in (
            "ast", "validated", "enforced", "no_inherit", "columns", "dependencies"
        )):
            raise ValueError("Installed typed AST or dependencies differ from the native compiler")
        if original_conkey is not None:
            native_key = json.loads(sql("SELECT to_jsonb(conkey) FROM pg_constraint "
                                        "WHERE conrelid='pg_temp.pwa_check_proof'::regclass AND conname='canonical'"))
            if original_conkey != native_key or any(type(position) is not int for position in original_conkey):
                raise ValueError("Original CHECK column key differs from native source binding")
        if original_dependencies is not None:
            native_dependencies = json.loads(sql("SELECT jsonb_agg(jsonb_build_object("
                "'classid',d.classid::text,'refclassid',d.refclassid::text,'deptype',d.deptype,"
                "'objsubid',d.objsubid,'refobjsubid',d.refobjsubid)) FROM pg_depend d "
                "JOIN pg_constraint k ON d.classid='pg_constraint'::regclass AND d.objid=k.oid "
                "WHERE k.conrelid='pg_temp.pwa_check_proof'::regclass AND k.conname='canonical'"))
            expected_dependencies = []
            columns = {column["position"]: column["name"] for column in binding["columns"]}
            relation = form.name.rsplit(".", 1)[0]
            # pg_describe_object renders public names according to the native
            # capture search_path. Resolve the source-owned column by name on
            # the immutable reference, not by the target's physical attnum.
            sql("SET search_path=pg_catalog,public")
            descriptions = {}
            for position, name in columns.items():
                descriptions[position] = sql("SELECT pg_describe_object('pg_class'::regclass,attrelid,attnum) "
                    "FROM pg_attribute WHERE attrelid=" + _sql_literal(relation)
                    + "::regclass AND attname=" + _sql_literal(name) + " AND NOT attisdropped")
            sql("SET search_path=pg_catalog,pg_temp")
            for dependency in native_dependencies:
                expected_dependencies.append(dict(dependency, name=form.name,
                    objid=original_identity["constraint_oid"], refobjid=original_identity["relation_oid"],
                    description=descriptions[dependency["refobjsubid"]]))
            if not _same_native_inventory(original_dependencies, expected_dependencies):
                raise ValueError("Original CHECK native dependency identity differs from the compiler")
        outputs = []
        for definition in (form.canonical_sql, form.historical_sql):
            plan = json.loads(sql("EXPLAIN (VERBOSE, FORMAT JSON, COSTS OFF) SELECT "
                                  + definition[len("CHECK "):]
                                  + " FROM pg_temp.pwa_check_proof"))
            outputs.append(plan[0]["Plan"]["Output"])
        if outputs[0] != outputs[1] or len(outputs[0]) != 1:
            raise ValueError("Native planner did not prove the registered representation pair")
        proof = NativeCheckProof(form.name, form.canonical, form.historical,
                                definition_hash(binding), outputs[0][0], source_sha,
                                prefix_hash, postgres_image,
                                definition_hash(canonical_reference["ast"]),
                                definition_hash(historical_reference["ast"]),
                                historical_reference["definition"], association_rows)
        object.__setattr__(proof, "_issuer", _NATIVE_PROOF_ISSUER)
        return proof
    finally:
        sql("DROP TABLE pg_temp.pwa_check_proof")


HISTORICAL_BUILTINS_SQL = """
SELECT jsonb_build_object(
 'objects',jsonb_build_object(
  'types',(SELECT jsonb_agg(jsonb_build_object('oid',t.oid::text,'name',t.typname,
      'namespace',n.nspname,'kind',t.typtype,'category',t.typcategory,'collation_oid',t.typcollation::text))
    FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace
    WHERE t.oid IN ('varchar'::regtype,'text'::regtype,'int4'::regtype,'text[]'::regtype,'int8'::regtype,'jsonb'::regtype)),
  'functions',(SELECT jsonb_agg(jsonb_build_object('oid',p.oid::text,'namespace',n.nspname,
      'identity',p.oid::regprocedure::text,'strict',p.proisstrict,'volatility',p.provolatile,
      'return_type',p.prorettype::regtype::text))
    FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
    WHERE p.oid IN ('texteq(text,text)'::regprocedure,'int4le(integer,integer)'::regprocedure,
      'int84le(bigint,integer)'::regprocedure,'int84ge(bigint,integer)'::regprocedure,'octet_length(text)'::regprocedure)),
  'operators',(SELECT jsonb_agg(jsonb_build_object('oid',o.oid::text,'namespace',n.nspname,'name',o.oprname,
      'function',o.oprcode::regprocedure::text,'left_type',o.oprleft::regtype::text,
      'right_type',o.oprright::regtype::text,'result_type',o.oprresult::regtype::text))
    FROM pg_operator o JOIN pg_namespace n ON n.oid=o.oprnamespace
    WHERE o.oprcode IN ('texteq(text,text)'::regprocedure,'int4le(integer,integer)'::regprocedure,
      'int84le(bigint,integer)'::regprocedure,'int84ge(bigint,integer)'::regprocedure)),
  'collations',(SELECT jsonb_agg(jsonb_build_object('oid',c.oid::text,'namespace',n.nspname,
      'name',c.collname,'deterministic',c.collisdeterministic))
    FROM pg_collation c JOIN pg_namespace n ON n.oid=c.collnamespace
    WHERE n.nspname='pg_catalog' AND c.collname='default')),
 'casts',(SELECT jsonb_agg(jsonb_build_object('oid',c.oid::text,'source',c.castsource::regtype::text,
      'target',c.casttarget::regtype::text,'method',c.castmethod,'context',c.castcontext,
      'function',CASE WHEN c.castfunc=0 THEN NULL ELSE c.castfunc::regprocedure::text END))
    FROM pg_cast c WHERE (c.castsource,c.casttarget) IN (
      ('int8'::regtype,'int4'::regtype),('int4'::regtype,'int8'::regtype),
      ('text'::regtype,'varchar'::regtype),('varchar'::regtype,'text'::regtype),
      ('varchar'::regtype,'varchar'::regtype),('jsonb'::regtype,'int4'::regtype),('jsonb'::regtype,'int8'::regtype)))
);
"""


def _sql_literal(value):
    if not isinstance(value, str) or "\0" in value:
        raise ValueError("Invalid native proof text")
    return "'" + value.replace("'", "''") + "'"


def _same_native_inventory(left, right):
    return (isinstance(left, list) and isinstance(right, list)
            and sorted(definition_hash(item) for item in left)
            == sorted(definition_hash(item) for item in right))


def prove_original_native_checks(sql, capture_raw, *, trusted_capture_sha256,
                                source_sha, postgres_image, prefix_sha384):
    """Recompute all twelve proofs from an externally authenticated capture.

    The trusted digest must come from the source authority, not from the client
    or from observing the same bytes. The caller authenticates capture provenance,
    binds target/root/project/helper/backend/ledger and freshness, and owns the
    disposable PG session. No client proof JSON is accepted as authorization.
    """
    validate_check_source(source_sha, definition_hash(prefix_sha384), postgres_image)
    if (not isinstance(capture_raw, bytes) or len(capture_raw) > 262144
            or not isinstance(trusted_capture_sha256, str)
            or hashlib.sha256(capture_raw).hexdigest() != trusted_capture_sha256):
        raise ValueError("Original CHECK capture does not match its source-trusted digest")
    capture = json.loads(capture_raw)
    if (not isinstance(capture, dict) or capture.get("postgres_version") != "18.3"
            or capture.get("images", {}).get("postgres") != postgres_image
            or capture.get("ledger") != {"count": 22, "max": 22, "successful": 22}):
        raise ValueError("Original CHECK capture has an unsupported PG or ledger binding")
    sql("SET search_path=pg_catalog,pg_temp")
    native = json.loads(sql(HISTORICAL_BUILTINS_SQL))
    if (not isinstance(capture.get("objects"), dict)
            or set(capture["objects"]) != set(native["objects"])
            or any(not _same_native_inventory(capture["objects"][kind], rows)
                   for kind, rows in native["objects"].items())
            or not _same_native_inventory(capture.get("casts"), native["casts"])):
        raise ValueError("Original builtin operator, function, type, cast or collation binding changed")
    forms = historical_check_forms()
    originals, bindings = {}, {}
    for field_name, destination in (("original", originals), ("bindings", bindings)):
        entries = capture.get(field_name)
        if not isinstance(entries, list) or len(entries) != len(forms):
            raise ValueError("Original CHECK capture must contain exactly twelve bindings")
        for entry in entries:
            if (not isinstance(entry, dict) or entry.get("name") not in forms
                    or entry["name"] in destination):
                raise ValueError("Original CHECK identities must be registered and unique")
            destination[entry["name"]] = entry
    dependencies = capture.get("dependencies")
    if not isinstance(dependencies, list):
        raise ValueError("Missing original native dependency inventory")
    proofs = []
    for name, form in forms.items():
        original, binding = originals[name], bindings[name]
        if (set(original) != {"name", "conkey", "enforced", "validated", "relation_oid", "namespace_oid", "constraint_oid", "original_conbin"}
                or not isinstance(original["conkey"], list) or not original["conkey"]
                or any(type(position) is not int or not 1 <= position <= 1600 for position in original["conkey"])
                or len(set(original["conkey"])) != len(original["conkey"])
                or any(not isinstance(original[key], str) or not original[key].isascii()
                       or not original[key].isdigit() or int(original[key]) <= 0
                       for key in ("relation_oid", "namespace_oid", "constraint_oid"))
                or original.get("enforced") is not True or original.get("validated") is not True
                or not isinstance(original.get("original_conbin"), str)
                or len(original["original_conbin"]) > 32768):
            raise ValueError("Original CHECK is not an enforced, validated native node tree")
        normalized = sql("SELECT regexp_replace(" + _sql_literal(original["original_conbin"])
                         + r", ':(location|list_start|list_end) -?[0-9]+', ':\1 -1', 'g')")
        if normalized != binding.get("ast"):
            raise ValueError("Original stored tree does not match its native source-offset projection")
        own_dependencies = [row for row in dependencies if isinstance(row, dict) and row.get("name") == name]
        if not own_dependencies:
            raise ValueError("Original CHECK native relation dependencies are missing")
        for dependency in own_dependencies:
            if (set(dependency) != {"name", "objid", "classid", "deptype", "objsubid", "refobjid", "refclassid", "description", "refobjsubid"}
                    or dependency["objid"] != original.get("constraint_oid")
                    or dependency["refobjid"] != original.get("relation_oid")
                    or type(dependency["refobjsubid"]) is not int):
                raise ValueError("Original CHECK native dependency identity is unsupported")
        proof = prove_native_check(sql, form, binding, source_sha=source_sha,
                                   postgres_image=postgres_image, prefix_sha384=prefix_sha384,
                                   original_conkey=original.get("conkey"),
                                   original_dependencies=own_dependencies, original_identity=original)
        object.__setattr__(proof, "capture_hash", trusted_capture_sha256)
        proofs.append(proof)
    if any(not isinstance(row, dict) or row.get("name") not in forms for row in dependencies):
        raise ValueError("Unlisted original CHECK dependencies")
    return tuple(proofs)


def canonical_readiness(canonical, installed, *, trusted_inventory=(),
                        position_pairs=None, check_proofs=(), installed_check_bindings=(),
                        source_sha=None, prefix_sha384=None, postgres_image=None):
    """Exact canonical fields except explicitly bound positions and native proofs.

    Position pairs are a source-trusted, target-bound mapping of column name to
    (canonical attnum, installed attnum), not a global layout ignore. This does
    not seal or authorize the target; exact_target_preservation does that.
    """
    expected, actual = _catalog_index(canonical), _catalog_index(installed)
    preservation_extensions(canonical, installed, trusted_inventory=trusted_inventory)
    positions = dict(position_pairs or {})
    if any(not isinstance(name, str) or not isinstance(pair, (list, tuple)) or len(pair) != 2
           or any(type(position) is not int or not 1 <= position <= 1600 for position in pair)
           for name, pair in positions.items()):
        raise ValueError("Invalid target-bound column positions")
    proofs = {}
    if check_proofs:
        validate_check_source(source_sha, definition_hash(prefix_sha384), postgres_image)
    for proof in check_proofs:
        if (not isinstance(proof, NativeCheckProof) or proof.name in proofs
                or proof._issuer is not _NATIVE_PROOF_ISSUER
                or proof.source_sha != source_sha
                or proof.prefix_hash != definition_hash(prefix_sha384)
                or proof.postgres_image != postgres_image):
            raise ValueError("Invalid or duplicate native CHECK proof")
        proofs[proof.name] = proof
    bindings = {}
    for binding in installed_check_bindings:
        if not isinstance(binding, dict) or not isinstance(binding.get("name"), str) or binding["name"] in bindings:
            raise ValueError("Invalid or duplicate installed CHECK binding")
        bindings[binding["name"]] = binding
    used_positions, used_proofs = set(), set()
    for key, definition in expected.items():
        if key not in actual:
            raise ValueError("Canonical catalog object is missing")
        observed = actual[key]
        if key[0] == "column" and key[1] in positions:
            if (not isinstance(definition, dict) or not isinstance(observed, dict)
                    or tuple(positions[key[1]]) != (definition.get("position"), observed.get("position"))):
                raise ValueError("Column position approval does not bind this target")
            used_positions.add(key[1])
            definition, observed = dict(definition), dict(observed)
            definition.pop("position")
            observed.pop("position")
        if definition == observed:
            continue
        proof = proofs.get(key[1]) if key[0] == "constraint" else None
        binding = bindings.get(key[1])
        if (proof is None or binding is None
                or (definition, observed) != (proof.canonical, proof.historical)
                or binding.get("definition") != observed
                or definition_hash(binding) != proof.binding_hash):
            raise ValueError("Canonical catalog definition changed without a native proof")
        used_proofs.add(proof.name)
    if used_positions != set(positions) or used_proofs != set(proofs):
        raise ValueError("Unused position approval or CHECK proof")
