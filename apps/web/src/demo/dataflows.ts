import type { Dataflow } from "@/features/inventory/dataflows-api";

type DemoEndpoint = Dataflow["source"];
const endpoint = (object_type: DemoEndpoint["object_type"], object_id: string | null, label: string): DemoEndpoint => ({
  object_type, object_id, label, address_mode: "default", ipaddresses: [],
});

const web = endpoint("dcim.device", "host-0000-0000-0000-0000-000000000001", "web-prod-01");
const database = endpoint("dcim.device", "host-0000-0000-0000-0000-000000000002", "db-primary-01");
const application = endpoint("virtualization.virtualmachine", "host-0000-0000-0000-0000-000000000003", "app-node-02");
const router = endpoint("dcim.device", "host-0000-0000-0000-0000-000000000005", "edge-router-01");
const servers = endpoint("dcim.devicerole", "role-srv-0000-0000-0000-000000000001", "Linux Server");
const site = endpoint("dcim.site", "site-ams-0000-0000-0000-000000000001", "Amsterdam Edge");
const internet = endpoint("internet", null, "Internet");

const patterns: Array<{ name: string; source: DemoEndpoint; destination: DemoEndpoint; port: number; transport?: "tcp" | "udp" }> = [
  { name: "Web application", source: web, destination: application, port: 443 },
  { name: "Application database", source: application, destination: database, port: 5432 },
  { name: "Database health check", source: database, destination: database, port: 5432 },
  { name: "Application callback", source: application, destination: web, port: 8443 },
  { name: "Server metrics", source: application, destination: servers, port: 9100 },
  { name: "Site DNS", source: site, destination: router, port: 53, transport: "udp" },
  { name: "Time synchronization", source: servers, destination: router, port: 123, transport: "udp" },
  { name: "Package downloads", source: servers, destination: internet, port: 443 },
];

/** Synthetic directed traffic, including parallel, reverse and self flows. */
export function createDemoDataflows(count = 24): Dataflow[] {
  return Array.from({ length: count }, (_, index) => {
    const pattern = patterns[index % patterns.length]!;
    const round = Math.floor(index / patterns.length);
    const status = (["active", "draft", "disabled"] as const)[round % 3]!;
    const destination = index < 24 ? pattern.destination : endpoint(
      "dcim.device", `demo-edge-${index}`, `Edge node ${String(index).padStart(3, "0")}`,
    );
    return {
      id: `demo-dataflow-${index}`, name: `${pattern.name} ${round + 1}`,
      description: "Synthetic demo traffic. This is not a captured production connection or an applied firewall rule.",
      status, source: { ...pattern.source }, destination: { ...destination },
      transport: pattern.transport ?? "tcp", destination_ports: [pattern.port], allow_cross_vrf: false,
    };
  });
}
