import type { NativeRunChunk } from "@/features/playbooks/NativeRunConsoleModel";

export const SYNTHETIC_RUN_GROUP_ID = "00000000-0000-4000-8000-000000000198";
const encoder = new TextEncoder();
/** Independent simulated bytes; these are never derived from structured events. */
export const SYNTHETIC_RUN_CHUNKS: NativeRunChunk[] = [
  { sequence: 0, bytes: encoder.encode("\x1b[36mSIMULATED DEMO OUTPUT\x1b[0m\r\nPLAY [Synthetic check] ************************\r\n") },
  { sequence: 1, bytes: encoder.encode("TASK [Example check] **************************\r\n\x1b[32mok: [demo-host]\x1b[0m\r\n") },
  { sequence: 2, bytes: encoder.encode("PLAY RECAP ************************************\r\ndemo-host : ok=1 changed=0 failed=0\r\n") },
];
