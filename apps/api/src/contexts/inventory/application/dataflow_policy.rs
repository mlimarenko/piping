//! Pure, deterministic host-policy projection over one inventory snapshot.
use crate::contexts::inventory::domain::{
    ObjectRef,
    native::dataflow::{
        Dataflow, DataflowAddressMode, DataflowEndpoint, DataflowEndpointInput, DataflowStatus,
        DataflowTransport, EndpointType,
    },
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};
use uuid::Uuid;

pub const MAX_ACTIVE_FLOWS: usize = 1000;
pub const MAX_ENDPOINT_ADDRESSES: usize = 256;
pub const MAX_HOST_MEMBERSHIP: usize = 256;
pub const MAX_ALL_ADDRESSES: usize = 4096;
pub const MAX_POLICY_RULES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyNetwork {
    pub address: IpAddr,
    pub prefix: u8,
}
impl PolicyNetwork {
    pub fn parse(value: &str, host: bool) -> Result<Self, String> {
        let (ip, mask) = value
            .split_once('/')
            .ok_or("Address lacks a prefix length")?;
        let address: IpAddr = ip.parse().map_err(|_| "Malformed address")?;
        let bits = if address.is_ipv4() { 32 } else { 128 };
        let prefix: u8 = mask.parse().map_err(|_| "Malformed prefix length")?;
        if prefix > bits
            || (host && address.is_unspecified())
            || address.is_multicast()
            || address.is_loopback()
            || matches!(address, IpAddr::V4(ip) if ip.is_broadcast() || ip.is_link_local())
            || matches!(address, IpAddr::V6(ip) if ip.is_unicast_link_local() || ip.to_ipv4_mapped().is_some())
        {
            return Err("Unsuitable address or prefix".into());
        }
        let mut result = Self {
            address,
            prefix: if host { bits } else { prefix },
        };
        if !host {
            result.address = result.masked();
        }
        Ok(result)
    }
    fn masked(&self) -> IpAddr {
        match self.address {
            IpAddr::V4(ip) => IpAddr::V4(
                (u32::from(ip)
                    & if self.prefix == 0 {
                        0
                    } else {
                        u32::MAX << (32 - self.prefix)
                    })
                .into(),
            ),
            IpAddr::V6(ip) => IpAddr::V6(
                (u128::from(ip)
                    & if self.prefix == 0 {
                        0
                    } else {
                        u128::MAX << (128 - self.prefix)
                    })
                .into(),
            ),
        }
    }
    pub fn contains(&self, other: &Self) -> bool {
        self.address.is_ipv4() == other.address.is_ipv4()
            && Self {
                address: other.address,
                prefix: self.prefix,
            }
            .masked()
                == self.masked()
    }
    pub fn cidr(&self) -> String {
        format!("{}/{}", self.address, self.prefix)
    }
}
#[derive(Debug, Clone)]
pub struct PolicyAddress {
    pub network: PolicyNetwork,
    pub vrf_id: Option<Uuid>,
    pub owners: BTreeSet<DataflowEndpoint>,
    pub errors: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct ResolvedEndpoint {
    pub owners: BTreeSet<DataflowEndpoint>,
    /// Typed context of explicit candidates, retained even when eligibility fails.
    pub ownership_addresses: Vec<PolicyAddress>,
    pub addresses: Vec<PolicyAddress>,
    pub errors: Vec<String>,
    pub exclusions: usize,
}
#[derive(Debug)]
pub struct PolicySnapshot {
    pub host: DataflowEndpoint,
    pub host_projection: ObjectRef,
    pub host_membership: Vec<PolicyAddress>,
    /// Demanded addresses and collision-complete witnesses, including unresolved records.
    pub inventory_addresses: Vec<PolicyAddress>,
    pub flows: Vec<Dataflow>,
    pub endpoints: BTreeMap<DataflowEndpointInput, ResolvedEndpoint>,
    pub errors: Vec<String>,
    pub authorization_facets: BTreeMap<
        DataflowEndpoint,
        super::connection_target_authorization::ConnectionTargetAuthorizationFacets,
    >,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicyDirection {
    Ingress,
    Egress,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicyFamily {
    Ipv4,
    Ipv6,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
pub struct FirewallRule {
    pub direction: PolicyDirection,
    pub family: PolicyFamily,
    pub transport: DataflowTransport,
    pub source: String,
    pub destination: String,
    pub destination_ports: Option<Vec<i32>>,
    pub source_ports: Option<Vec<i32>>,
    pub icmp_types: Option<Vec<i32>>,
    pub vrf_id: Option<Uuid>,
    pub dataflow_ids: Vec<Uuid>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
pub struct PolicyDiagnostic {
    pub dataflow_id: Option<Uuid>,
    pub code: String,
    pub message: String,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowPolicyResponse {
    pub host: ObjectRef,
    pub apply_ready: bool,
    pub rules: Vec<FirewallRule>,
    pub diagnostics: Vec<PolicyDiagnostic>,
}

impl PolicySnapshot {
    fn involves(&self, selector: &DataflowEndpointInput, endpoint: &ResolvedEndpoint) -> bool {
        if selector.object_type == EndpointType::Internet {
            return false;
        }
        if selector.identity().as_ref() == Some(&self.host) || endpoint.owners.contains(&self.host)
        {
            return true;
        }
        (selector.object_type == EndpointType::Prefix || selector.object_type.is_range())
            && endpoint.addresses.iter().any(|prefix| {
                self.host_membership
                    .iter()
                    .any(|ip| ip.vrf_id == prefix.vrf_id && prefix.network.contains(&ip.network))
            })
    }
    fn directions(
        &self,
        source: &DataflowEndpointInput,
        src: &ResolvedEndpoint,
        destination: &DataflowEndpointInput,
        dst: &ResolvedEndpoint,
    ) -> Vec<PolicyDirection> {
        let ingress = self.involves(destination, dst);
        let egress = self.involves(source, src);
        [
            (ingress, PolicyDirection::Ingress),
            (egress, PolicyDirection::Egress),
        ]
        .into_iter()
        .filter_map(|(enabled, direction)| enabled.then_some(direction))
        .collect()
    }
    fn local_addresses(
        &self,
        selector: &DataflowEndpointInput,
        endpoint: &ResolvedEndpoint,
    ) -> Vec<PolicyAddress> {
        if selector.object_type == EndpointType::Prefix || selector.object_type.is_range() {
            return self
                .host_membership
                .iter()
                .filter(|ip| {
                    endpoint.addresses.iter().any(|prefix| {
                        prefix.vrf_id == ip.vrf_id && prefix.network.contains(&ip.network)
                    })
                })
                .cloned()
                .collect();
        }
        endpoint
            .addresses
            .iter()
            .filter(|ip| ip.owners.contains(&self.host))
            .cloned()
            .collect()
    }
    /// Authorize peers that can contribute, not unrelated cross-VRF All members.
    pub fn required_hosts(&self) -> BTreeSet<DataflowEndpoint> {
        let mut required = BTreeSet::from([self.host.clone()]);
        for flow in self
            .flows
            .iter()
            .filter(|flow| flow.effective_status == DataflowStatus::Active)
        {
            let empty = ResolvedEndpoint::default();
            for source in &flow.sources {
                let src = self.endpoints.get(source).unwrap_or(&empty);
                for destination in &flow.destinations {
                    let dst = self.endpoints.get(destination).unwrap_or(&empty);
                    for direction in self.directions(source, src, destination, dst) {
                        self.require_direction_hosts(
                            flow,
                            source,
                            src,
                            destination,
                            dst,
                            direction,
                            &mut required,
                        );
                    }
                }
            }
        }
        required
    }
    #[allow(clippy::too_many_arguments)]
    fn require_direction_hosts(
        &self,
        flow: &Dataflow,
        source: &DataflowEndpointInput,
        src: &ResolvedEndpoint,
        destination: &DataflowEndpointInput,
        dst: &ResolvedEndpoint,
        direction: PolicyDirection,
        required: &mut BTreeSet<DataflowEndpoint>,
    ) {
        let (local_selector, local, remote_selector, remote) =
            side_endpoints(source, src, destination, dst, direction);
        let local_ips = self.local_addresses(local_selector, local);
        if !local_selector.object_type.is_collection()
            && local_selector.object_type != EndpointType::Prefix
        {
            required.extend(local.owners.iter().cloned());
        }
        required.extend(
            local_ips
                .iter()
                .flat_map(|address| address.owners.iter().cloned()),
        );
        if requires_remote_identity(
            flow,
            source,
            destination,
            remote_selector,
            remote,
            &local_ips,
        ) {
            required.extend(remote.owners.iter().cloned());
        }
        if remote_selector.object_type.is_collection()
            && remote_selector.address_mode == DataflowAddressMode::Selected
        {
            // Unresolved selections retain identity, but not unrelated realms.
            require_compatible_owners(
                &local_ips,
                &remote.ownership_addresses,
                flow.allow_cross_vrf,
                required,
            );
        }
        require_compatible_owners(
            &local_ips,
            &remote.addresses,
            flow.allow_cross_vrf,
            required,
        );
    }
}
fn requires_remote_identity(
    flow: &Dataflow,
    source: &DataflowEndpointInput,
    destination: &DataflowEndpointInput,
    selector: &DataflowEndpointInput,
    remote: &ResolvedEndpoint,
    local_ips: &[PolicyAddress],
) -> bool {
    let collection = source.object_type.is_collection() || destination.object_type.is_collection();
    !selector.object_type.is_collection()
        && (!collection
            || remote.addresses.is_empty()
            || local_ips.iter().any(|ip| {
                remote
                    .addresses
                    .iter()
                    .any(|address| compatible(ip, address, flow.allow_cross_vrf))
            }))
}
fn require_compatible_owners(
    local_ips: &[PolicyAddress],
    remote_ips: &[PolicyAddress],
    cross_vrf: bool,
    required: &mut BTreeSet<DataflowEndpoint>,
) {
    required.extend(
        remote_ips
            .iter()
            .filter(|address| {
                local_ips
                    .iter()
                    .any(|ip| compatible(ip, address, cross_vrf))
            })
            .flat_map(|address| address.owners.iter().cloned()),
    );
}
fn side_endpoints<'a>(
    source: &'a DataflowEndpointInput,
    src: &'a ResolvedEndpoint,
    destination: &'a DataflowEndpointInput,
    dst: &'a ResolvedEndpoint,
    direction: PolicyDirection,
) -> (
    &'a DataflowEndpointInput,
    &'a ResolvedEndpoint,
    &'a DataflowEndpointInput,
    &'a ResolvedEndpoint,
) {
    match direction {
        PolicyDirection::Ingress => (destination, dst, source, src),
        PolicyDirection::Egress => (source, src, destination, dst),
    }
}
fn compatible(local: &PolicyAddress, remote: &PolicyAddress, cross_vrf: bool) -> bool {
    local.network.address.is_ipv4() == remote.network.address.is_ipv4()
        && (cross_vrf || local.vrf_id == remote.vrf_id)
}

pub fn resolve_host_policy(snapshot: &PolicySnapshot) -> DataflowPolicyResponse {
    let mut diagnostics: Vec<_> = snapshot
        .errors
        .iter()
        .map(|message| PolicyDiagnostic {
            dataflow_id: None,
            code: "snapshot_unresolved".into(),
            message: message.clone(),
        })
        .collect();
    let mut failed = !diagnostics.is_empty();
    let mut rules = RuleAccumulator::default();
    let mut relevant = 0;
    for flow in snapshot
        .flows
        .iter()
        .filter(|flow| flow.effective_status == DataflowStatus::Active)
    {
        let empty = ResolvedEndpoint::default();
        let mut relevant_pairs = 0;
        let mut flow_exclusions = 0;
        let mut flow_same_host = false;
        for source in &flow.sources {
            let src = snapshot.endpoints.get(source).unwrap_or(&empty);
            for destination in &flow.destinations {
                let dst = snapshot.endpoints.get(destination).unwrap_or(&empty);
                let directions = snapshot.directions(source, src, destination, dst);
                if directions.is_empty() {
                    continue;
                }
                relevant_pairs += 1;
                let errors = endpoint_errors(src, dst);
                if !errors.is_empty() {
                    failed = true;
                    diagnostics.extend(
                        errors
                            .into_iter()
                            .map(|message| diagnostic(flow.id, "endpoint_unresolved", message)),
                    );
                    continue;
                }
                let result = flow_rules(
                    snapshot,
                    flow,
                    source,
                    src,
                    destination,
                    dst,
                    &directions,
                    &mut rules,
                );
                failed |= !result.errors.is_empty();
                diagnostics.extend(
                    result
                        .errors
                        .into_iter()
                        .map(|message| diagnostic(flow.id, "endpoint_unresolved", message)),
                );
                flow_exclusions += result.exclusions;
                flow_same_host |= result.same_host;
            }
        }
        if relevant_pairs == 0 {
            continue;
        }
        relevant += 1;
        if flow_exclusions > 0 {
            diagnostics.push(diagnostic(flow.id, "inventory_excluded", "Inventory members without eligible addresses or outside this host family/VRF were excluded".into()));
        }
        if flow_same_host {
            diagnostics.push(diagnostic(
                flow.id,
                "same_host_omitted",
                "Same-host traffic has no network-firewall rule".into(),
            ));
        }
        if rules.exceeded {
            failed = true;
            diagnostics.push(diagnostic(
                flow.id,
                "rule_budget_exceeded",
                "Policy rule budget exceeded".into(),
            ));
            break;
        }
    }
    if relevant == 0 && !failed {
        diagnostics.push(PolicyDiagnostic { dataflow_id: None, code: "no_relevant_flows".into(), message: "No relevant active intent; empty output is not proof of a safe default firewall policy".into() });
    }
    diagnostics.sort();
    diagnostics.dedup();
    DataflowPolicyResponse {
        host: snapshot.host_projection.clone(),
        apply_ready: !failed,
        rules: if failed {
            Vec::new()
        } else {
            rules
                .entries
                .into_iter()
                .map(|(rule, ids)| FirewallRule {
                    direction: rule.direction,
                    family: rule.family,
                    transport: rule.transport,
                    source: rule.source,
                    destination: rule.destination,
                    destination_ports: rule.destination_ports.map(<[i32]>::to_vec),
                    source_ports: rule.source_ports.map(<[i32]>::to_vec),
                    icmp_types: rule.icmp_types.map(<[i32]>::to_vec),
                    vrf_id: rule.vrf_id,
                    dataflow_ids: ids.into_iter().collect(),
                })
                .collect()
        },
        diagnostics,
    }
}
fn diagnostic(id: Uuid, code: &str, message: String) -> PolicyDiagnostic {
    PolicyDiagnostic {
        dataflow_id: Some(id),
        code: code.into(),
        message,
    }
}
fn endpoint_errors(src: &ResolvedEndpoint, dst: &ResolvedEndpoint) -> BTreeSet<String> {
    let mut errors: BTreeSet<_> = src.errors.iter().chain(&dst.errors).cloned().collect();
    // Internet is materialized with two wildcard networks by the snapshot adapter.
    if src.addresses.is_empty() || dst.addresses.is_empty() {
        errors.insert("Endpoint has no eligible addresses".into());
    }
    errors
}
#[derive(Default)]
struct FlowRules {
    errors: BTreeSet<String>,
    exclusions: usize,
    same_host: bool,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RuleKey<'a> {
    direction: PolicyDirection,
    family: PolicyFamily,
    transport: DataflowTransport,
    source: String,
    destination: String,
    destination_ports: Option<&'a [i32]>,
    source_ports: Option<&'a [i32]>,
    icmp_types: Option<&'a [i32]>,
    vrf_id: Option<Uuid>,
}
#[derive(Default)]
struct RuleAccumulator<'a> {
    entries: BTreeMap<RuleKey<'a>, BTreeSet<Uuid>>,
    exceeded: bool,
}
impl<'a> RuleAccumulator<'a> {
    fn insert(&mut self, key: RuleKey<'a>, id: Uuid) {
        if let Some(ids) = self.entries.get_mut(&key) {
            ids.insert(id);
        } else if self.entries.len() == MAX_POLICY_RULES {
            self.exceeded = true;
        } else {
            self.entries.insert(key, BTreeSet::from([id]));
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn flow_rules<'a>(
    snapshot: &PolicySnapshot,
    flow: &'a Dataflow,
    source: &DataflowEndpointInput,
    src: &ResolvedEndpoint,
    destination: &DataflowEndpointInput,
    dst: &ResolvedEndpoint,
    directions: &[PolicyDirection],
    rules: &mut RuleAccumulator<'a>,
) -> FlowRules {
    let mut result = FlowRules {
        exclusions: src.exclusions + dst.exclusions,
        ..Default::default()
    };
    for direction in directions {
        let (local_selector, local, remote_selector, remote) =
            side_endpoints(source, src, destination, dst, *direction);
        let local_ips = snapshot.local_addresses(local_selector, local);
        if local_ips.is_empty() {
            result
                .errors
                .insert("Local endpoint has no eligible chosen host address".into());
        }
        if !local_ips.is_empty()
            && !local_ips.iter().any(|ip| {
                remote
                    .addresses
                    .iter()
                    .any(|peer| peer.network.address.is_ipv4() == ip.network.address.is_ipv4())
            })
        {
            result
                .errors
                .insert("Endpoints have no common address family".into());
        }
        if flow.transport.is_icmp()
            && !local_ips
                .iter()
                .any(|ip| icmp_family_matches(flow.transport, ip.network.address))
        {
            result.errors.insert(format!(
                "{} transport has no compatible {} addresses",
                flow.transport.as_str(),
                if flow.transport == DataflowTransport::Icmp {
                    "IPv4"
                } else {
                    "IPv6"
                }
            ));
        }
        for ip in &local_ips {
            expand_local_rules(
                snapshot,
                flow,
                source,
                destination,
                *direction,
                (ip, remote_selector, remote),
                &mut result,
                rules,
            );
            if rules.exceeded {
                return result;
            }
        }
    }
    result
}
#[allow(clippy::too_many_arguments)]
fn expand_local_rules<'a>(
    snapshot: &PolicySnapshot,
    flow: &'a Dataflow,
    source: &DataflowEndpointInput,
    destination: &DataflowEndpointInput,
    direction: PolicyDirection,
    side: (&PolicyAddress, &DataflowEndpointInput, &ResolvedEndpoint),
    result: &mut FlowRules,
    rules: &mut RuleAccumulator<'a>,
) {
    let (local, remote_selector, remote) = side;
    for address in &remote.addresses {
        if !policy_pair_contributes(
            snapshot,
            flow,
            source,
            destination,
            remote_selector,
            local,
            address,
            result,
        ) {
            continue;
        }
        insert_pair_rules(flow, direction, local, address, rules);
        if rules.exceeded {
            return;
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn policy_pair_contributes(
    snapshot: &PolicySnapshot,
    flow: &Dataflow,
    source: &DataflowEndpointInput,
    destination: &DataflowEndpointInput,
    remote_selector: &DataflowEndpointInput,
    local: &PolicyAddress,
    remote: &PolicyAddress,
    result: &mut FlowRules,
) -> bool {
    if local.network.address.is_ipv4() != remote.network.address.is_ipv4() {
        return false;
    }
    let wildcard = remote_selector.object_type == EndpointType::Internet;
    if !wildcard && !compatible(local, remote, flow.allow_cross_vrf) {
        if source.object_type.is_collection() || destination.object_type.is_collection() {
            result.exclusions += 1;
        } else {
            result.errors.insert("Endpoint VRFs differ; set allow_cross_vrf only for explicit intent (routing/NAT is not inferred)".into());
        }
        return false;
    }
    if !local.errors.is_empty() || !remote.errors.is_empty() {
        result
            .errors
            .extend(local.errors.iter().chain(&remote.errors).cloned());
        return false;
    }
    if remote_selector.object_type != EndpointType::Prefix && remote.owners.contains(&snapshot.host)
    {
        result.same_host = true;
        return false;
    }
    if ambiguous_cross_vrf_pair(snapshot, flow, local, remote, wildcard) {
        result.errors.insert(
            "Cross-VRF intent has ambiguous overlapping ownership in the exported host context"
                .into(),
        );
        return false;
    }
    true
}
fn ambiguous_cross_vrf_pair(
    snapshot: &PolicySnapshot,
    flow: &Dataflow,
    local: &PolicyAddress,
    remote: &PolicyAddress,
    wildcard: bool,
) -> bool {
    !wildcard
        && flow.allow_cross_vrf
        && remote.vrf_id != local.vrf_id
        && snapshot.inventory_addresses.iter().any(|known| {
            known.vrf_id == local.vrf_id
                && remote.network.contains(&known.network)
                && !known.owners.is_subset(&remote.owners)
        })
}
fn insert_pair_rules<'a>(
    flow: &'a Dataflow,
    direction: PolicyDirection,
    local: &PolicyAddress,
    remote: &PolicyAddress,
    rules: &mut RuleAccumulator<'a>,
) {
    let (source, destination) = match direction {
        PolicyDirection::Ingress => (remote.network.cidr(), local.network.cidr()),
        PolicyDirection::Egress => (local.network.cidr(), remote.network.cidr()),
    };
    let ports_constrained = flow.destination_ports.is_some() || flow.source_ports.is_some();
    for transport in flow.transport.expanded_with_ports(ports_constrained) {
        if !icmp_family_matches(transport, local.network.address) {
            continue;
        }
        rules.insert(
            RuleKey {
                direction,
                family: if local.network.address.is_ipv4() {
                    PolicyFamily::Ipv4
                } else {
                    PolicyFamily::Ipv6
                },
                transport,
                source: source.clone(),
                destination: destination.clone(),
                destination_ports: flow.destination_ports.as_deref(),
                source_ports: flow.source_ports.as_deref(),
                icmp_types: flow.icmp_types.as_deref(),
                vrf_id: local.vrf_id,
            },
            flow.id,
        );
        if rules.exceeded {
            return;
        }
    }
}

fn icmp_family_matches(transport: DataflowTransport, address: IpAddr) -> bool {
    match transport {
        DataflowTransport::Icmp => address.is_ipv4(),
        DataflowTransport::Icmpv6 => !address.is_ipv4(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::inventory::domain::native::dataflow::DataflowAddressMode;
    #[test]
    fn regression_rule_expansion_collection_is_bounded() {
        let mut snapshot = fixture();
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].sources[0])
            .unwrap()
            .addresses = (1..=17)
            .map(|i| address(&format!("10.1.0.{i}/32"), Some(host(1)), None))
            .collect();
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].destinations[0])
            .unwrap()
            .addresses = (1..=241)
            .map(|i| address(&format!("10.2.0.{i}/32"), Some(host(2)), None))
            .collect();
        let flow = &snapshot.flows[0];
        let mut rules = RuleAccumulator::default();
        flow_rules(
            &snapshot,
            flow,
            &flow.sources[0],
            &snapshot.endpoints[&flow.sources[0]],
            &flow.destinations[0],
            &snapshot.endpoints[&flow.destinations[0]],
            &[PolicyDirection::Egress],
            &mut rules,
        );
        assert!(
            rules.entries.len() <= MAX_POLICY_RULES,
            "expansion allocated {} rules",
            rules.entries.len()
        );
        assert!(rules.exceeded);
        let policy = resolve_host_policy(&snapshot);
        assert!(!policy.apply_ready);
        assert!(policy.rules.is_empty());
    }
    #[test]
    fn regression_projected_remote_network_checks_third_host_and_prefix() {
        let mut snapshot = fixture();
        let blue = Some(Uuid::from_u128(99));
        snapshot.flows[0].allow_cross_vrf = true;
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].destinations[0])
            .unwrap()
            .addresses[0]
            .vrf_id = blue;
        assert!(resolve_host_policy(&snapshot).apply_ready);
        snapshot
            .inventory_addresses
            .push(address("192.0.2.2/32", Some(host(77)), None));
        assert!(!resolve_host_policy(&snapshot).apply_ready);
        snapshot.inventory_addresses.last_mut().unwrap().vrf_id = blue;
        assert!(
            resolve_host_policy(&snapshot).apply_ready,
            "separate realm alone is not a projection collision"
        );
        snapshot.inventory_addresses.last_mut().unwrap().vrf_id = None;
        let prefix = selector(EndpointType::Prefix, Some(88));
        snapshot.flows[0].destinations[0] = prefix.clone();
        snapshot.endpoints.insert(
            prefix,
            ResolvedEndpoint {
                addresses: vec![PolicyAddress {
                    network: PolicyNetwork::parse("192.0.2.0/24", false).unwrap(),
                    vrf_id: blue,
                    owners: BTreeSet::from([host(2)]),
                    errors: vec![],
                }],
                owners: BTreeSet::from([host(2)]),
                ..Default::default()
            },
        );
        assert!(!resolve_host_policy(&snapshot).apply_ready);
    }
    #[test]
    fn regression_shared_budget_deduplicates_before_reserving_slots() {
        let mut snapshot = fixture();
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].sources[0])
            .unwrap()
            .addresses = (1..=16)
            .map(|i| address(&format!("10.1.0.{i}/32"), Some(host(1)), None))
            .collect();
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].destinations[0])
            .unwrap()
            .addresses = (0..256)
            .map(|i| address(&format!("10.2.0.{i}/32"), Some(host(2)), None))
            .collect();
        let mut duplicate = snapshot.flows[0].clone();
        duplicate.id = Uuid::from_u128(100);
        snapshot.flows.push(duplicate);
        let policy = resolve_host_policy(&snapshot);
        assert!(policy.apply_ready);
        assert_eq!(policy.rules.len(), MAX_POLICY_RULES);
        assert!(policy.rules.iter().all(|rule| rule.dataflow_ids.len() == 2));
        snapshot.flows[1].destination_ports = Some(vec![443]);
        let policy = resolve_host_policy(&snapshot);
        assert!(!policy.apply_ready);
        assert!(policy.rules.is_empty());
    }
    fn host(id: u128) -> DataflowEndpoint {
        DataflowEndpoint {
            object_type: EndpointType::Device,
            object_id: Uuid::from_u128(id),
        }
    }
    fn selector(kind: EndpointType, id: Option<u128>) -> DataflowEndpointInput {
        DataflowEndpointInput {
            object_type: kind,
            object_id: id.map(Uuid::from_u128),
            address_mode: DataflowAddressMode::Default,
            ipaddresses: Vec::new(),
            tag_object_type: None,
        }
    }
    fn address(
        value: &str,
        owner: Option<DataflowEndpoint>,
        vrf_id: Option<Uuid>,
    ) -> PolicyAddress {
        PolicyAddress {
            network: PolicyNetwork::parse(value, !value.ends_with("/0")).unwrap(),
            owners: owner.into_iter().collect(),
            vrf_id,
            errors: Vec::new(),
        }
    }
    fn fixture() -> PolicySnapshot {
        let local = host(1);
        let peer = host(2);
        let source = selector(EndpointType::Device, Some(1));
        let destination = selector(EndpointType::Device, Some(2));
        let src = ResolvedEndpoint {
            owners: BTreeSet::from([local.clone()]),
            addresses: vec![
                address("192.0.2.1/24", Some(local.clone()), None),
                address("2001:db8::1/64", Some(local.clone()), None),
            ],
            ..Default::default()
        };
        let dst = ResolvedEndpoint {
            owners: BTreeSet::from([peer.clone()]),
            addresses: vec![
                address("192.0.2.2/24", Some(peer.clone()), None),
                address("2001:db8::2/64", Some(peer), None),
            ],
            ..Default::default()
        };
        PolicySnapshot {
            host: local.clone(),
            inventory_addresses: src
                .addresses
                .iter()
                .chain(&dst.addresses)
                .cloned()
                .collect(),
            host_projection: local.projection(),
            host_membership: src.addresses.clone(),
            flows: vec![Dataflow {
                id: Uuid::from_u128(3),
                name: "flow".into(),
                description: String::new(),
                comments: String::new(),
                status: DataflowStatus::Active,
                sources: vec![source.clone()],
                destinations: vec![destination.clone()],
                transport: DataflowTransport::Any,
                destination_ports: None,
                source_ports: None,
                icmp_types: None,
                allow_cross_vrf: false,
                group_id: None,
                application_id: None,
                owner_id: None,
                tenant_id: None,
                tags: Vec::new(),
                custom_field_data: serde_json::Value::Object(serde_json::Map::new()),
                effective_status: DataflowStatus::Active,
                blocked_by_group_id: None,
                inherited_tags: Vec::new(),
                effective_tags: Vec::new(),
            }],
            endpoints: BTreeMap::from([(source, src), (destination, dst)]),
            errors: Vec::new(),
            authorization_facets: BTreeMap::new(),
        }
    }
    #[test]
    fn exact_masks_directions_dedup_and_stable_provenance() {
        let mut snapshot = fixture();
        let mut duplicate = snapshot.flows[0].clone();
        duplicate.id = Uuid::from_u128(4);
        snapshot.flows.push(duplicate);
        let policy = resolve_host_policy(&snapshot);
        assert!(policy.apply_ready);
        assert_eq!(policy.rules.len(), 2);
        assert_eq!(policy.rules[0].source, "192.0.2.1/32");
        assert_eq!(policy.rules[1].source, "2001:db8::1/128");
        assert_eq!(policy.rules[0].dataflow_ids.len(), 2);
        let sorted = serde_json::to_value(&policy).unwrap();
        snapshot.flows.reverse();
        assert_eq!(
            serde_json::to_value(resolve_host_policy(&snapshot)).unwrap(),
            sorted
        );
        snapshot.host = host(2);
        assert!(
            resolve_host_policy(&snapshot)
                .rules
                .iter()
                .all(|rule| rule.direction == PolicyDirection::Ingress)
        );
    }
    #[test]
    fn internet_is_explicit_any_with_target_family_context_not_ownership() {
        let mut snapshot = fixture();
        let internet = selector(EndpointType::Internet, None);
        snapshot.flows[0].sources[0] = internet.clone();
        snapshot.flows[0].destinations[0] = selector(EndpointType::Device, Some(1));
        snapshot.endpoints.insert(
            internet,
            ResolvedEndpoint {
                addresses: vec![
                    address("0.0.0.0/0", None, None),
                    address("::/0", None, None),
                ],
                ..Default::default()
            },
        );
        let context = Some(Uuid::from_u128(8));
        for ip in &mut snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].destinations[0])
            .unwrap()
            .addresses
        {
            ip.vrf_id = context;
        }
        let policy = resolve_host_policy(&snapshot);
        assert!(policy.apply_ready);
        assert_eq!(policy.rules.len(), 2);
        assert_eq!(policy.rules[0].source, "0.0.0.0/0");
        assert_eq!(policy.rules[0].vrf_id, context);
        assert_eq!(policy.rules[0].transport, DataflowTransport::Any);
        assert!(policy.rules[0].destination_ports.is_none());
        assert_eq!(policy.rules[0].direction, PolicyDirection::Ingress);
        snapshot.host = host(99);
        assert!(resolve_host_policy(&snapshot).rules.is_empty());
    }
    #[test]
    fn any_with_ports_expands_only_port_bearing_transports() {
        let mut snapshot = fixture();
        snapshot.flows[0].destination_ports = Some(vec![443]);
        let policy = resolve_host_policy(&snapshot);
        assert_eq!(policy.rules.len(), 6);
        assert!(
            policy
                .rules
                .iter()
                .all(|rule| rule.transport != DataflowTransport::Any)
        );
    }
    #[test]
    fn selected_missing_and_explicit_vrf_mismatch_clear_all_output() {
        let mut snapshot = fixture();
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].destinations[0])
            .unwrap()
            .errors
            .push("Selected IP is missing".into());
        let policy = resolve_host_policy(&snapshot);
        assert!(!policy.apply_ready);
        assert!(policy.rules.is_empty());
        snapshot = fixture();
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].destinations[0])
            .unwrap()
            .addresses[0]
            .vrf_id = Some(Uuid::from_u128(9));
        assert!(!resolve_host_policy(&snapshot).apply_ready);
        snapshot.flows[0].allow_cross_vrf = true;
        let policy = resolve_host_policy(&snapshot);
        assert!(policy.apply_ready);
        assert!(policy.rules[0].vrf_id.is_none());
    }
    #[test]
    fn all_scopes_local_ips_and_excludes_other_realms_without_hiding_peers() {
        let mut snapshot = fixture();
        let all = selector(EndpointType::All, None);
        let mut members = snapshot.endpoints[&snapshot.flows[0].sources[0]].clone();
        members.addresses.push(address(
            "192.0.2.77/24",
            Some(host(77)),
            Some(Uuid::from_u128(77)),
        ));
        members.owners.insert(host(77));
        members.exclusions = 1;
        snapshot.flows[0].sources[0] = all.clone();
        snapshot.endpoints.insert(all, members);
        snapshot.host = host(2);
        snapshot.host_membership = snapshot.endpoints[&snapshot.flows[0].destinations[0]]
            .addresses
            .clone();
        let policy = resolve_host_policy(&snapshot);
        assert!(policy.apply_ready);
        assert!(policy.rules.iter().all(
            |rule| rule.direction == PolicyDirection::Ingress && rule.source != "192.0.2.77/32"
        ));
        assert!(!snapshot.required_hosts().contains(&host(77)));
        assert!(
            policy
                .diagnostics
                .iter()
                .any(|d| d.code == "inventory_excluded")
        );
    }
    #[test]
    fn inactive_same_host_prefix_and_empty_intent_semantics() {
        let mut snapshot = fixture();
        snapshot.flows[0].status = DataflowStatus::Draft;
        snapshot.flows[0].effective_status = DataflowStatus::Draft;
        assert!(resolve_host_policy(&snapshot).rules.is_empty());
        snapshot.flows[0].status = DataflowStatus::Disabled;
        snapshot.flows[0].effective_status = DataflowStatus::Disabled;
        assert!(resolve_host_policy(&snapshot).rules.is_empty());
        snapshot.flows[0].status = DataflowStatus::Active;
        snapshot.flows[0].effective_status = DataflowStatus::Active;
        snapshot.flows[0].destinations[0] = snapshot.flows[0].sources[0].clone();
        let same = resolve_host_policy(&snapshot);
        assert!(same.apply_ready);
        assert!(same.rules.is_empty());
        let prefix = selector(EndpointType::Prefix, Some(7));
        snapshot.flows[0].sources[0] = prefix.clone();
        snapshot.endpoints.insert(
            prefix,
            ResolvedEndpoint {
                addresses: vec![PolicyAddress {
                    network: PolicyNetwork::parse("198.51.100.7/24", false).unwrap(),
                    owners: BTreeSet::new(),
                    vrf_id: None,
                    errors: Vec::new(),
                }],
                ..Default::default()
            },
        );
        let policy = resolve_host_policy(&snapshot);
        assert!(policy.apply_ready);
        assert_eq!(policy.rules[0].source, "198.51.100.0/24");
        assert_eq!(policy.rules[0].direction, PolicyDirection::Ingress);
    }
    #[test]
    fn malformed_networks_and_bounded_ports_are_rejected() {
        for value in [
            "invalid",
            "192.0.2.1/33",
            "::/128",
            "127.0.0.1/8",
            "224.0.0.1/24",
            "fe80::1/64",
        ] {
            assert!(PolicyNetwork::parse(value, true).is_err());
        }
        let mut flow = fixture().flows.remove(0);
        for ports in [vec![], vec![0], vec![65536], vec![443; 257]] {
            flow.destination_ports = Some(ports);
            assert!(flow.normalize().is_err());
        }
        flow.destination_ports = Some(vec![443, 80, 443]);
        flow.normalize().unwrap();
        assert_eq!(flow.destination_ports, Some(vec![80, 443]));
    }
    #[test]
    fn budgets_and_same_context_ambiguity_fail_closed() {
        let mut snapshot = fixture();
        snapshot.errors.push("Snapshot budget exceeded".into());
        let policy = resolve_host_policy(&snapshot);
        assert!(!policy.apply_ready);
        assert!(policy.rules.is_empty());
        snapshot = fixture();
        snapshot
            .endpoints
            .get_mut(&snapshot.flows[0].sources[0])
            .unwrap()
            .addresses[0]
            .errors
            .push("Ambiguous same-context ownership".into());
        assert!(!resolve_host_policy(&snapshot).apply_ready);
        snapshot = fixture();
        snapshot.flows = (0..MAX_POLICY_RULES + 1)
            .map(|index| {
                let mut flow = snapshot.flows[0].clone();
                flow.id = Uuid::from_u128(index as u128 + 100);
                flow.destination_ports = Some(vec![index as i32 + 1]);
                flow
            })
            .collect();
        let policy = resolve_host_policy(&snapshot);
        assert!(!policy.apply_ready);
        assert!(policy.rules.is_empty());
    }
}
