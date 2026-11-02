"""Lossless, server-native catalog used by migration and deployment guards."""

import json
import hashlib
import re
from dataclasses import dataclass, field


BASELINE_SQLX_SHA384 = "55c65893bff52263eb225d3b54436be85a2fd7af3be2cd36b6c375adb9f154142c25f91d39431ea1610439b657c5081e"

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
       '8d41638c8a534ec7ab6c693d2cd326a74ec3e9dbf0c11e95186139ab40af799fc35943a85fbe3ee0b9728831c65958a1')}

CONTENT_TYPE_SEED_MIGRATIONS = {
    **{version: SCHEMA_MIGRATIONS[version] for version in (5, 7, 9, 10, 11, 13)},
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
    **SCHEMA_MIGRATIONS,
    **CONTENT_TYPE_SEED_MIGRATIONS,
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
                                                            'a4f2cfbe428b26486300d2e345a3ed689dc111c07c0703bc6d57742ceff4e476')),
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
                                                                                                                                                                                                         '1536a3225eb1bdfdf41ac42b576e48745470a2ec1d2d732aba68731dc3da882b'),)}


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
    ) if pending)


def transformation_columns(columns, transforms):
    result = service_cutover_columns(columns) if "Service0003" in transforms else {
        table: list(names) for table, names in columns.items()}
    if "FHRP0004" in transforms:
        if FHRP_TABLE not in result or "auth_key" not in result[FHRP_TABLE]:
            raise ValueError("FHRP cutover requires the prepared pre-DDL source column")
        result[FHRP_TABLE].remove("auth_key")
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


def migration_catalog(before, *, additions=(), replacements=(), removals=()):
    """Apply an explicitly authorized delta, binding both sides of each replacement."""
    expected = _catalog_index(before)
    removed = set()
    for removal in removals:
        if not isinstance(removal, dict) or set(removal) != {"kind", "name", "before"}:
            raise ValueError("Invalid authorized catalog removal")
        key = removal["kind"], removal["name"]
        if key in removed or key not in expected or expected[key] != removal["before"]:
            raise ValueError("Catalog removal does not bind the sealed target")
        source_object = {"kind": key[0], "name": key[1], "definition": removal["before"]}
        if not (service_cutover_removal(source_object) or fhrp_cutover_removal(source_object)
                or _schema_pair_authorized(key, definition_hash(removal["before"]), None)):
            raise ValueError("Catalog removal is outside the closed authority")
        removed.add(key)
        del expected[key]
    replaced = set()
    for replacement in replacements:
        if (not isinstance(replacement, dict)
                or set(replacement) != {"kind", "name", "before", "after"}
                or replacement["kind"] not in ("constraint", "index", "trigger", "function")):
            raise ValueError("Invalid authorized schema replacement")
        key = replacement["kind"], replacement["name"]
        if key in replaced or key not in expected or expected[key] != replacement["before"]:
            raise ValueError("Schema replacement does not bind the sealed target")
        if not _schema_pair_authorized(key, definition_hash(replacement["before"]),
                                      definition_hash(replacement["after"])):
            raise ValueError("Schema replacement is outside the closed authority")
        replaced.add(key)
        expected[key] = replacement["after"]
    for key, value in _catalog_index(additions).items():
        if key in expected:
            raise ValueError("An addition cannot replace a sealed target object")
        if key in SCHEMA_TRANSITIONS and not _schema_pair_authorized(key, None, definition_hash(value)):
            raise ValueError("Schema creation is outside the closed authority")
        expected[key] = value
    return [{"kind": key[0], "name": key[1], "definition": value} for key, value in sorted(expected.items())]


def exact_target_preservation(before, after, *, additions=(), replacements=(), removals=()):
    """A sealed target remains exact outside the source-authorized migration delta."""
    expected = _catalog_index(migration_catalog(before, additions=additions, replacements=replacements, removals=removals))
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
