/**
 * Demo-only static mock of a remote desktop session.
 *
 * The RDP (guacamole) and VNC (RFB) wire protocols aren't worth faking
 * byte-for-byte, so in DEMO MODE ONLY the live viewer renders this styled
 * placeholder instead of the real canvas: a clean CSS/SVG desktop framed like a
 * live session with a "● connected" indicator. `variant="windows"` draws a
 * Windows-style desktop with a taskbar + a couple of windows (for RDP jump
 * hosts); `variant="linux"` draws a lighter Linux desktop (for VNC lab boxes).
 *
 * Strictly gated behind `IS_DEMO` at the call site (LiveRDP / LiveVNC) so the
 * real product keeps using the live RDP/VNC adapters untouched.
 */

import { useEffect } from "react";

interface MockRemoteDesktopProps {
  variant: "windows" | "linux";
  /** Host name shown in the title bar / status chip, e.g. "win-rdp-jump". */
  hostName: string;
  /** Fired once after mount so the panel flips the session badge to "connected". */
  onReady?: () => void;
}

const ConnectedChip = ({ label }: { label: string }) => (
  <div className="absolute right-3 top-3 z-20 flex items-center gap-1.5 rounded-full bg-black/55 px-2.5 py-1 text-[11px] font-medium text-white backdrop-blur-sm">
    <span className="relative flex h-2 w-2">
      <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-75" />
      <span className="relative inline-flex h-2 w-2 rounded-full bg-emerald-400" />
    </span>
    {label}
  </div>
);

function WindowsDesktop({ hostName }: { hostName: string }) {
  return (
    <div
      className="relative h-full w-full overflow-hidden"
      style={{
        background:
          "radial-gradient(120% 120% at 30% 20%, #2b6cb0 0%, #1a365d 45%, #0f1f3d 100%)",
      }}
    >
      {/* Subtle desktop glow */}
      <div className="pointer-events-none absolute -left-24 top-10 h-72 w-72 rounded-full bg-sky-400/20 blur-3xl" />
      <div className="pointer-events-none absolute right-10 bottom-24 h-64 w-64 rounded-full bg-indigo-400/20 blur-3xl" />

      {/* Desktop icons */}
      <div className="absolute left-5 top-6 flex w-16 flex-col items-center gap-1 text-center">
        <div className="flex h-11 w-11 items-center justify-center rounded-md bg-white/10 ring-1 ring-white/15 backdrop-blur-sm">
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
            <rect x="3" y="4" width="18" height="13" rx="1.5" fill="#cbd5e1" />
            <rect x="3" y="4" width="18" height="3" rx="1.5" fill="#94a3b8" />
            <rect x="9" y="19" width="6" height="2" rx="1" fill="#cbd5e1" />
          </svg>
        </div>
        <span className="text-[10px] text-white/85 drop-shadow">This PC</span>
      </div>
      <div className="absolute left-5 top-28 flex w-16 flex-col items-center gap-1 text-center">
        <div className="flex h-11 w-11 items-center justify-center rounded-md bg-amber-300/90 ring-1 ring-white/15">
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
            <path d="M3 7a1 1 0 0 1 1-1h5l2 2h8a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7z" fill="#b45309" />
          </svg>
        </div>
        <span className="text-[10px] text-white/85 drop-shadow">Files</span>
      </div>

      {/* A floating "Server Manager"-ish window */}
      <div className="absolute left-1/2 top-12 w-[60%] max-w-[460px] -translate-x-1/3 overflow-hidden rounded-lg shadow-2xl ring-1 ring-black/30">
        <div className="flex items-center justify-between bg-[#1f2937] px-3 py-1.5">
          <span className="text-[11px] font-medium text-slate-200">Server Manager — {hostName}</span>
          <div className="flex gap-1.5">
            <span className="h-2.5 w-2.5 rounded-full bg-slate-500" />
            <span className="h-2.5 w-2.5 rounded-full bg-slate-500" />
            <span className="h-2.5 w-2.5 rounded-full bg-rose-500" />
          </div>
        </div>
        <div className="bg-[#f3f4f6] p-3">
          <div className="mb-2 text-[11px] font-semibold text-slate-700">Local Server · Properties</div>
          <div className="grid grid-cols-2 gap-1.5 text-[10px] text-slate-600">
            <span className="text-slate-400">Computer name</span><span>{hostName}</span>
            <span className="text-slate-400">Domain</span><span>corp.piping.demo</span>
            <span className="text-slate-400">Windows Firewall</span><span className="text-emerald-600">Public: On</span>
            <span className="text-slate-400">Remote Desktop</span><span className="text-emerald-600">Enabled</span>
            <span className="text-slate-400">Last installed updates</span><span>Today at 06:14</span>
          </div>
          <div className="mt-3 h-1.5 w-full overflow-hidden rounded-full bg-slate-200">
            <div className="h-full w-2/3 rounded-full bg-sky-500" />
          </div>
        </div>
      </div>

      {/* A small terminal window */}
      <div className="absolute bottom-16 right-8 w-[40%] max-w-[300px] overflow-hidden rounded-lg shadow-2xl ring-1 ring-black/30">
        <div className="flex items-center justify-between bg-[#0b1220] px-3 py-1.5">
          <span className="text-[11px] font-medium text-slate-300">Windows PowerShell</span>
        </div>
        <div className="bg-[#0c1626] p-3 font-mono text-[10px] leading-relaxed text-slate-200">
          <div><span className="text-sky-400">PS C:\Users\Administrator&gt;</span> hostname</div>
          <div className="text-slate-400">{hostName}</div>
          <div><span className="text-sky-400">PS C:\Users\Administrator&gt;</span> Get-Service TermService</div>
          <div className="text-emerald-400">Running  TermService  Remote Desktop Services</div>
          <div><span className="text-sky-400">PS C:\Users\Administrator&gt;</span> <span className="inline-block h-3 w-1.5 translate-y-0.5 animate-pulse bg-slate-200" /></div>
        </div>
      </div>

      {/* Taskbar */}
      <div className="absolute inset-x-0 bottom-0 flex h-9 items-center gap-2 bg-[#11182799] px-2 backdrop-blur-md">
        <div className="flex h-7 w-7 items-center justify-center rounded bg-sky-500/90">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="white">
            <path d="M3 5l8-1v7H3V5zm0 8h8v7l-8-1v-6zm9-9l9-1v9h-9V3zm0 10h9v9l-9-1v-8z" />
          </svg>
        </div>
        <div className="h-5 w-px bg-white/15" />
        <div className="flex h-7 w-7 items-center justify-center rounded bg-white/10 ring-1 ring-white/10" />
        <div className="flex h-7 w-7 items-center justify-center rounded bg-white/10 ring-1 ring-white/10" />
        <div className="ml-auto pr-1 text-right text-[10px] leading-tight text-white/80">
          <div>11:20</div>
          <div className="text-white/55">6/2/2026</div>
        </div>
      </div>
    </div>
  );
}

function LinuxDesktop({ hostName }: { hostName: string }) {
  return (
    <div
      className="relative h-full w-full overflow-hidden"
      style={{
        background:
          "linear-gradient(160deg, #1b2027 0%, #2d3340 50%, #3a2f4a 100%)",
      }}
    >
      <div className="pointer-events-none absolute right-0 top-0 h-72 w-72 rounded-full bg-fuchsia-500/15 blur-3xl" />
      <div className="pointer-events-none absolute left-6 bottom-10 h-60 w-60 rounded-full bg-orange-400/15 blur-3xl" />

      {/* Top bar (GNOME-ish) */}
      <div className="absolute inset-x-0 top-0 flex h-7 items-center justify-between bg-black/35 px-3 text-[11px] text-white/85 backdrop-blur">
        <span className="font-medium">Activities</span>
        <span>{hostName}</span>
        <span>11:20</span>
      </div>

      {/* Dock */}
      <div className="absolute left-3 top-1/2 flex -translate-y-1/2 flex-col gap-2 rounded-xl bg-black/30 p-2 backdrop-blur">
        {["#e95420", "#3584e4", "#2ec27e", "#9141ac"].map((color) => (
          <div
            key={color}
            className="h-8 w-8 rounded-lg ring-1 ring-white/10"
            style={{ background: color }}
          />
        ))}
      </div>

      {/* Terminal window */}
      <div className="absolute left-1/2 top-14 w-[60%] max-w-[440px] -translate-x-1/3 overflow-hidden rounded-lg shadow-2xl ring-1 ring-black/40">
        <div className="flex items-center gap-2 bg-[#2b2b2b] px-3 py-1.5">
          <div className="flex gap-1.5">
            <span className="h-2.5 w-2.5 rounded-full bg-rose-500" />
            <span className="h-2.5 w-2.5 rounded-full bg-amber-400" />
            <span className="h-2.5 w-2.5 rounded-full bg-emerald-500" />
          </div>
          <span className="text-[11px] text-slate-300">user@{hostName}: ~</span>
        </div>
        <div className="bg-[#1d1f21] p-3 font-mono text-[10px] leading-relaxed text-slate-200">
          <div><span className="text-emerald-400">user@{hostName}</span>:<span className="text-sky-400">~</span>$ neofetch</div>
          <div className="mt-1 grid grid-cols-[auto_1fr] gap-x-3">
            <pre className="text-orange-400">{`   .--.\n  |o_o |\n  |:_/ |\n //   \\ \\\n(|     | )\n/'\\_   _/'\\\n\\___)=(___/`}</pre>
            <div className="text-slate-300">
              <div><span className="text-orange-400">OS</span>  Ubuntu 24.04 LTS</div>
              <div><span className="text-orange-400">Host</span>  {hostName}</div>
              <div><span className="text-orange-400">Kernel</span>  6.8.0-45-generic</div>
              <div><span className="text-orange-400">Uptime</span>  21 days, 4 hours</div>
              <div><span className="text-orange-400">Shell</span>  bash 5.2.21</div>
              <div><span className="text-orange-400">DE</span>  GNOME 46</div>
            </div>
          </div>
          <div className="mt-1"><span className="text-emerald-400">user@{hostName}</span>:<span className="text-sky-400">~</span>$ <span className="inline-block h-3 w-1.5 translate-y-0.5 animate-pulse bg-slate-200" /></div>
        </div>
      </div>

      {/* A files window */}
      <div className="absolute bottom-10 right-8 w-[38%] max-w-[280px] overflow-hidden rounded-lg shadow-2xl ring-1 ring-black/40">
        <div className="flex items-center gap-2 bg-[#3a3f44] px-3 py-1.5">
          <span className="text-[11px] text-slate-200">Files — Home</span>
        </div>
        <div className="grid grid-cols-3 gap-2 bg-[#272b30] p-3">
          {["Documents", "Projects", "logs", "backups", "scripts", "data"].map((name) => (
            <div key={name} className="flex flex-col items-center gap-1">
              <div className="flex h-8 w-9 items-center justify-center rounded bg-amber-400/90">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="#92400e">
                  <path d="M3 7a1 1 0 0 1 1-1h5l2 2h8a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7z" />
                </svg>
              </div>
              <span className="text-[9px] text-slate-300">{name}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

const MockRemoteDesktop = ({ variant, hostName, onReady }: MockRemoteDesktopProps) => {
  useEffect(() => {
    // Small beat so the panel briefly shows "connecting" before "connected",
    // mirroring a real session handshake.
    const id = window.setTimeout(() => onReady?.(), 350);
    return () => window.clearTimeout(id);
  }, [onReady]);

  return (
    <div className="relative h-full w-full" data-testid="mock-remote-desktop">
      {variant === "windows" ? (
        <WindowsDesktop hostName={hostName} />
      ) : (
        <LinuxDesktop hostName={hostName} />
      )}
      <ConnectedChip label="connected" />
    </div>
  );
};

export default MockRemoteDesktop;
