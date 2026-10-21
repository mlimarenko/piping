/**
 * Demo-only canned SSH session scripts.
 *
 * Each demo host gets a believable shell transcript: a banner/MOTD, a colored
 * prompt, and a handful of "typed" commands with realistic output. The demo WS
 * shim (`ssh-socket-shim.ts`) replays these line-by-line with small delays so
 * the mocked terminal looks like a live, freshly-attached session.
 *
 * This module is imported only by demo-only code; it never reaches the normal
 * product build.
 */

// ── ANSI helpers ─────────────────────────────────────────────────────────────
const ESC = "[";
const reset = `${ESC}0m`;
const c = (code: string, s: string) => `${ESC}${code}m${s}${reset}`;

export const ansi = {
  reset,
  bold: (s: string) => c("1", s),
  dim: (s: string) => c("2", s),
  red: (s: string) => c("31", s),
  green: (s: string) => c("32", s),
  yellow: (s: string) => c("33", s),
  blue: (s: string) => c("34", s),
  magenta: (s: string) => c("35", s),
  cyan: (s: string) => c("36", s),
  white: (s: string) => c("37", s),
  gray: (s: string) => c("90", s),
  brightGreen: (s: string) => c("92", s),
  brightCyan: (s: string) => c("96", s),
};

/** A single scripted step: either instant output or a "typed" command. */
export type SshStep =
  | { kind: "out"; text: string; delayMs?: number }
  | { kind: "prompt"; delayMs?: number }
  | { kind: "type"; text: string; delayMs?: number }
  | { kind: "enter" };

export interface SshScript {
  /** Shell prompt drawn after every command, e.g. `root@db-primary-01:~#`. */
  prompt: string;
  /** The full scripted session played once on connect. */
  steps: SshStep[];
}

const CRLF = "\r\n";

function motd(hostname: string, kernel: string, distro: string): string {
  const line = ansi.gray("─".repeat(64));
  return [
    "",
    line,
    `  ${ansi.bold(ansi.brightCyan("Piping"))} ${ansi.gray("secure session relay")}  ${ansi.dim("·")}  ${ansi.green("● connected")}`,
    line,
    `  ${ansi.dim("Host")}     ${ansi.white(hostname)}`,
    `  ${ansi.dim("System")}   ${ansi.white(distro)}`,
    `  ${ansi.dim("Kernel")}   ${ansi.white(kernel)}`,
    `  ${ansi.dim("Access")}   ${ansi.white("audited — all keystrokes recorded")}`,
    line,
    "",
    ansi.yellow("The programs included with this system are free software."),
    ansi.dim("Last login: Mon Jun  2 11:14:07 2026 from 192.0.2.9"),
    "",
  ].join(CRLF);
}

function prompt(promptStr: string): string {
  // user@host in green, path in blue, trailing symbol in white.
  const at = promptStr.indexOf("@");
  const colon = promptStr.indexOf(":");
  const user = promptStr.slice(0, at);
  const host = promptStr.slice(at + 1, colon);
  const tail = promptStr.slice(colon); // ":~#" etc
  return `${ansi.brightGreen(`${user}@${host}`)}${ansi.white(tail)} `;
}

interface HostProfile {
  hostname: string;
  promptUser: string;
  promptHost: string;
  promptSym: string; // "#" for root/admin, "$" otherwise
  distro: string;
  kernel: string;
  commands: { cmd: string; output: string[] }[];
}

function buildScript(p: HostProfile): SshScript {
  const promptStr = `${p.promptUser}@${p.promptHost}:~${p.promptSym}`;
  const steps: SshStep[] = [];
  steps.push({ kind: "out", text: motd(p.hostname, p.kernel, p.distro), delayMs: 250 });
  for (const { cmd, output } of p.commands) {
    steps.push({ kind: "prompt", delayMs: 500 });
    steps.push({ kind: "type", text: cmd, delayMs: 45 });
    steps.push({ kind: "enter" });
    if (output.length) {
      steps.push({ kind: "out", text: output.join(CRLF) + CRLF, delayMs: 320 });
    }
  }
  // Leave the session at a fresh, blinking prompt.
  steps.push({ kind: "prompt", delayMs: 500 });
  return { prompt: prompt(promptStr), steps };
}

// ── Per-host profiles ────────────────────────────────────────────────────────

const WEB_PROD: HostProfile = {
  hostname: "web-prod-01",
  promptUser: "root",
  promptHost: "web-prod-01",
  promptSym: "#",
  distro: "Ubuntu 24.04.1 LTS",
  kernel: "Linux 6.8.0-45-generic x86_64",
  commands: [
    {
      cmd: "uname -a",
      output: [
        "Linux web-prod-01 6.8.0-45-generic #45-Ubuntu SMP PREEMPT_DYNAMIC x86_64 GNU/Linux",
      ],
    },
    {
      cmd: "uptime",
      output: [
        ` 11:20:14 up 42 days,  3:17,  2 users,  load average: ${ansi.green("0.18, 0.24, 0.21")}`,
      ],
    },
    {
      cmd: "systemctl status nginx --no-pager",
      output: [
        `${ansi.green("●")} nginx.service - A high performance web server and a reverse proxy server`,
        `     Loaded: loaded (/lib/systemd/system/nginx.service; enabled; preset: enabled)`,
        `     Active: ${ansi.green("active (running)")} since Sun 2026-04-21 08:03:11 UTC; 42 days ago`,
        `   Main PID: 921 (nginx)`,
        `      Tasks: 9 (limit: 4915)`,
        `     Memory: 18.4M`,
        `        CPU: 4min 12.881s`,
      ],
    },
    {
      cmd: "docker ps --format 'table {{.Names}}\\t{{.Status}}\\t{{.Ports}}'",
      output: [
        `${ansi.bold("NAMES")}            ${ansi.bold("STATUS")}              ${ansi.bold("PORTS")}`,
        `web-frontend     ${ansi.green("Up 6 days")}           0.0.0.0:443->443/tcp`,
        `web-api          ${ansi.green("Up 6 days")}           127.0.0.1:8080->8080/tcp`,
        `redis-cache      ${ansi.green("Up 6 days (healthy)")} 127.0.0.1:6379->6379/tcp`,
      ],
    },
    {
      cmd: "tail -n 4 /var/log/nginx/access.log",
      output: [
        `${ansi.dim("192.0.2.9 - - [02/Jun/2026:11:20:02]")} "GET /api/health HTTP/1.1" ${ansi.green("200")} 17`,
        `${ansi.dim("192.0.2.9 - - [02/Jun/2026:11:20:09]")} "GET / HTTP/1.1" ${ansi.green("200")} 5821`,
        `${ansi.dim("203.0.113.7 - - [02/Jun/2026:11:20:11]")} "POST /api/login HTTP/1.1" ${ansi.green("200")} 412`,
        `${ansi.dim("203.0.113.7 - - [02/Jun/2026:11:20:12]")} "GET /assets/app.js HTTP/1.1" ${ansi.green("200")} 88213`,
      ],
    },
  ],
};

const DB_PRIMARY: HostProfile = {
  hostname: "db-primary-01",
  promptUser: "root",
  promptHost: "db-primary-01",
  promptSym: "#",
  distro: "Debian GNU/Linux 12 (bookworm)",
  kernel: "Linux 6.1.0-21-amd64 x86_64",
  commands: [
    {
      cmd: "uname -a",
      output: [
        "Linux db-primary-01 6.1.0-21-amd64 #1 SMP PREEMPT_DYNAMIC Debian 6.1.90-1 x86_64 GNU/Linux",
      ],
    },
    {
      cmd: "uptime",
      output: [
        ` 11:20:18 up 118 days, 22:41,  1 user,  load average: ${ansi.yellow("1.04, 0.92, 0.88")}`,
      ],
    },
    {
      cmd: "systemctl status postgresql@16-main --no-pager",
      output: [
        `${ansi.green("●")} postgresql@16-main.service - PostgreSQL Cluster 16-main`,
        `     Loaded: loaded (/lib/systemd/system/postgresql@.service; enabled-runtime)`,
        `     Active: ${ansi.green("active (running)")} since Wed 2026-02-04 02:11:54 UTC; 118 days ago`,
        `   Main PID: 1184 (postgres)`,
        `     Memory: 1.9G`,
      ],
    },
    {
      cmd: "sudo -u postgres psql -c 'SELECT count(*) FROM pg_stat_activity;'",
      output: [
        " count ",
        "-------",
        `    ${ansi.cyan("37")}`,
        "(1 row)",
      ],
    },
    {
      cmd: "df -h /var/lib/postgresql",
      output: [
        `${ansi.bold("Filesystem")}      Size  Used Avail Use% Mounted on`,
        `/dev/vda2       512G  287G  225G  ${ansi.yellow("57%")} /var/lib/postgresql`,
      ],
    },
  ],
};

const APP_NODE: HostProfile = {
  hostname: "app-node-02",
  promptUser: "deploy",
  promptHost: "app-node-02",
  promptSym: "$",
  distro: "Rocky Linux 9.4 (Blue Onyx)",
  kernel: "Linux 5.14.0-427.el9 x86_64",
  commands: [
    {
      cmd: "uname -a",
      output: [
        "Linux app-node-02 5.14.0-427.el9.x86_64 #1 SMP PREEMPT_DYNAMIC x86_64 x86_64 GNU/Linux",
      ],
    },
    {
      cmd: "uptime",
      output: [
        ` 11:20:21 up 9 days,  4:55,  1 user,  load average: ${ansi.green("0.42, 0.39, 0.31")}`,
      ],
    },
    {
      cmd: "kubectl get pods -n payments",
      output: [
        `${ansi.bold("NAME")}                        ${ansi.bold("READY")}   ${ansi.bold("STATUS")}    ${ansi.bold("RESTARTS")}   ${ansi.bold("AGE")}`,
        `payments-api-7d9c8b6f5-4kx2t   1/1     ${ansi.green("Running")}   0          3d`,
        `payments-api-7d9c8b6f5-q7n9p   1/1     ${ansi.green("Running")}   0          3d`,
        `payments-worker-58c4d9-zl8mr   1/1     ${ansi.green("Running")}   1          3d`,
      ],
    },
    {
      cmd: "systemctl --user status app-node --no-pager",
      output: [
        `${ansi.green("●")} app-node.service - Application node runtime`,
        `     Active: ${ansi.green("active (running)")} since Sun 2026-05-24 06:25:40 UTC; 9 days ago`,
        `   Main PID: 2451 (node)`,
        `     Memory: 312.7M`,
      ],
    },
  ],
};

const ROUTER: HostProfile = {
  hostname: "edge-router-01",
  promptUser: "admin",
  promptHost: "edge-router-01",
  promptSym: "#",
  distro: "VyOS 1.5-rolling (sagitta)",
  kernel: "Linux 6.6.30 x86_64",
  commands: [
    {
      cmd: "show version",
      output: [
        `Version:          ${ansi.cyan("VyOS 1.5-rolling-202606")}`,
        "Release train:    sagitta",
        "Built on:         Mon 02 Jun 2026 04:00 UTC",
        "Hardware vendor:  QEMU",
        "Uptime:           71 days, 14:22",
      ],
    },
    {
      cmd: "show interfaces",
      output: [
        `${ansi.bold("Codes: S - State, L - Link, u - Up, D - Down")}`,
        `Interface    IP Address        S/L   Description`,
        `eth0         192.0.2.1/24      ${ansi.green("u/u")}   LAN`,
        `eth1         203.0.113.2/30    ${ansi.green("u/u")}   WAN-uplink`,
        `wg0          10.255.0.1/24     ${ansi.green("u/u")}   site-to-site`,
      ],
    },
    {
      cmd: "show ip bgp summary",
      output: [
        "BGP router identifier 203.0.113.2, local AS number 64512",
        `Neighbor        V    AS  MsgRcvd  State/PfxRcd`,
        `203.0.113.1     4 64511   184211  ${ansi.green("412")}`,
        `10.255.0.2      4 64513    98233  ${ansi.green("57")}`,
      ],
    },
  ],
};

const PROFILES: Record<string, HostProfile> = {
  "web-prod-01": WEB_PROD,
  "db-primary-01": DB_PRIMARY,
  "app-node-02": APP_NODE,
  "edge-router-01": ROUTER,
  "fra-web-01": { ...WEB_PROD, hostname: "fra-web-01", promptHost: "fra-web-01" },
  "fra-db-01": { ...DB_PRIMARY, hostname: "fra-db-01", promptHost: "fra-db-01" },
};

const FALLBACK: HostProfile = {
  hostname: "linux-host",
  promptUser: "root",
  promptHost: "linux-host",
  promptSym: "#",
  distro: "Ubuntu 24.04 LTS",
  kernel: "Linux 6.8.0-45-generic x86_64",
  commands: [
    { cmd: "uname -a", output: ["Linux linux-host 6.8.0-45-generic #45-Ubuntu SMP x86_64 GNU/Linux"] },
    { cmd: "uptime", output: [` 11:20:00 up 12 days,  6:02,  1 user,  load average: ${ansi.green("0.10, 0.14, 0.09")}`] },
    { cmd: "whoami", output: ["root"] },
  ],
};

/** Resolve a scripted SSH session for a host (by name or id slug). */
export function sshScriptForHost(hostName: string): SshScript {
  const profile = PROFILES[hostName] ?? FALLBACK;
  return buildScript(profile);
}
