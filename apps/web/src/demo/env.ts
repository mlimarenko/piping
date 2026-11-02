/**
 * Demo-mode runtime flag.
 *
 * `true` only in the dedicated demo build/entry (`vite --mode demo`, served at
 * `/demo-app/` by the public portal). `vite.config.ts` `define`s
 * `import.meta.env.VITE_PIPING_DEMO` as `"1"` under `--mode demo` and `""`
 * otherwise, so `IS_DEMO` is statically `false` in the normal app build and
 * every demo-only branch (mock SSH stream, mock RDP/VNC desktops) tree-shakes
 * away.
 *
 * Gate ALL demo-only behaviour on this constant so the real product keeps using
 * the live SSH/RDP/VNC adapters untouched.
 */
export const IS_DEMO: boolean = import.meta.env.VITE_PIPING_DEMO === "1";
