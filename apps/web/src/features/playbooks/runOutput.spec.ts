import { describe, expect, it } from "vitest";

import {
  ansibleRunAliasSuffix,
  consoleOutputEvents,
  filterMessageForSelectedHost,
  formatRunOutputEvent,
  isRawConsoleEvent,
  isRawRunnerStatusMessage,
  messageLineMatchesSelectedHost,
  normalizeAnsibleHostToken,
  parseAnsiLine,
  stripAnsi,
} from "./runOutput";

import type { PlaybookRunEvent } from "@/features/playbooks/api";

function event(partial: Partial<PlaybookRunEvent>): PlaybookRunEvent {
  return {
    id: partial.id ?? "event-1",
    run_group_id: partial.run_group_id ?? "group-1",
    run_id: partial.run_id ?? "run-1",
    host: partial.host ?? null,
    event_sequence: partial.event_sequence ?? 1,
    event_kind: partial.event_kind ?? "stdout",
    event_type: partial.event_type ?? "log",
    message: partial.message ?? null,
    payload: partial.payload ?? {},
    created_at: partial.created_at ?? "2026-07-06T00:00:00Z",
  };
}

describe("playbook run output host scoping", () => {
  it("derives Ansible inventory alias suffixes from run ids", () => {
    expect(ansibleRunAliasSuffix("019f34d8-1111-2222-3333-444444444444")).toBe(
      "3333444444444444",
    );
    expect(ansibleRunAliasSuffix("not-a-uuid")).toBeNull();
  });

  it("normalizes Ansible host labels like the runner inventory alias", () => {
    expect(normalizeAnsibleHostToken(" ПК оператора 1-1 ")).toBe("пк_оператора_1-1");
  });

  it("matches Ansible suffixed host aliases without matching same-prefix hosts", () => {
    expect(messageLineMatchesSelectedHost("ok: [cash-1_019f32f9 -> localhost]", "cash-1")).toBe(true);
    expect(messageLineMatchesSelectedHost("fatal: [cash-10_019f32f9]: UNREACHABLE!", "cash-1")).toBe(false);
    expect(messageLineMatchesSelectedHost("PLAY RECAP cash-1a_019f32f9 : failed=1", "cash-1")).toBe(false);
  });

  it("filters multi-host recap blocks to the selected target", () => {
    const output = [
      "PLAY RECAP *********************************************************************",
      "cash-1_019f32f9 : ok=1 failed=0",
      "cash-10_019f32f9 : ok=0 failed=1",
      "pc-1_019f32f9 : ok=1 failed=0",
    ].join("\n");

    expect(filterMessageForSelectedHost(output, "cash-1")).toBe(
      ["PLAY RECAP *********************************************************************", "cash-1_019f32f9 : ok=1 failed=0"].join("\n"),
    );
  });

  it("hides single-line host warnings for other selected targets", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "warning",
        message:
          "[WARNING]: Host 'Касса_606-1_019f34d8' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
      }),
      "ПК оператора 603-1",
    );

    expect(rendered).toBeNull();
  });

  it("keeps selected target warnings by run alias when Cyrillic display names differ from Ansible aliases", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "warning",
        message:
          "[WARNING]: Host 'Kacca_606-1_3333444444444444' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
      }),
      "Касса 606-1",
      "019f34d8-1111-2222-3333-444444444444",
    );

    expect(rendered).toBe(
      "[WARNING]: Host 'Kacca_606-1_3333444444444444' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
    );
  });

  it("keeps selected target warnings by the short run alias for already-running jobs", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "warning",
        message:
          "[WARNING]: Host 'Kacca_606-1_019f34d8' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
      }),
      "Касса 606-1",
      "019f34d8-1111-2222-3333-444444444444",
    );

    expect(rendered).toBe(
      "[WARNING]: Host 'Kacca_606-1_019f34d8' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
    );
  });

  it("drops other target warnings by run alias even when display-name matching is impossible", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "warning",
        message:
          "[WARNING]: Host 'Kacca_606-1_aaaaaaaa' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
      }),
      "Касса 606-1",
      "019f34d8-1111-2222-3333-444444444444",
    );

    expect(rendered).toBeNull();
  });

  it("hides every line of multi-line host warnings for other selected targets", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "warning",
        message: [
          "[WARNING]: Host 'TEST_CASH__3_019f361a' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
          "interpreter to be discovered. See https://docs.ansible.com/ansible-core/2.21/reference_appendices/interpreter_discovery.html for more information.",
        ].join("\n"),
      }),
      "Касса 1-2",
    );

    expect(rendered).toBeNull();
  });

  it("keeps selected-host warning continuations while dropping following other-host blocks", () => {
    const output = [
      "[WARNING]: Host 'Касса_1-2_019f361a' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
      "interpreter to be discovered. See https://docs.ansible.com/ansible-core/2.21/reference_appendices/interpreter_discovery.html for more information.",
      "[WARNING]: Host 'TEST_CASH__3_019f361a' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
      "interpreter to be discovered. See https://docs.ansible.com/ansible-core/2.21/reference_appendices/interpreter_discovery.html for more information.",
      "TASK [Gather Linux inventory facts] **************************************",
    ].join("\n");

    expect(filterMessageForSelectedHost(output, "Касса 1-2")).toBe(
      [
        "[WARNING]: Host 'Касса_1-2_019f361a' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
        "interpreter to be discovered. See https://docs.ansible.com/ansible-core/2.21/reference_appendices/interpreter_discovery.html for more information.",
        "TASK [Gather Linux inventory facts] **************************************",
      ].join("\n"),
    );
  });

  it("keeps single-line host warnings for the selected target", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "warning",
        message:
          "[WARNING]: Host 'ПК_оператора_603-1_019f34d8' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
      }),
      "ПК оператора 603-1",
    );

    expect(rendered).toBe(
      "[WARNING]: Host 'ПК_оператора_603-1_019f34d8' is using the discovered Python interpreter at '/usr/local/bin/python3.14'",
    );
  });

  it("collapses noisy successful REST result dictionaries", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "result",
        event_type: "runner_on_ok",
        message:
          "ok: [cash-1_019f32f9 -> localhost] => {'changed': False, 'cookies_string': '', 'content_type': 'application/json', 'x-request-id': 'abc', 'json': {'id': '1'}, 'url': 'http://backend:7000/api/v1/inventory/ip-addresses/1'}",
        payload: {
          task: "Upsert interface IPv4 addresses through Piping REST",
          res: {
            changed: false,
            status: 200,
          },
        },
      }),
    );

    expect(rendered).toBe(
      "ok: [cash-1_019f32f9 -> localhost] => changed: false, status: 200",
    );
  });

  it("collapses verbose selected-host result objects that include warnings from other targets", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "runner_on_ok",
        message:
          "ok: [ПК_оператора_603-1_019f34d8] => {'changed': False, 'warnings': [\"Host 'Касса_606-1_019f34d8' is using Python\", \"Host 'Касса_606-2_019f34d8' is using Python\"], 'ansible_facts': {'ansible_system': 'Linux'}}",
      }),
      "ПК оператора 603-1",
    );

    expect(rendered).toBe("ok: [ПК_оператора_603-1_019f34d8] => changed: false");
  });

  it("hides skipped loop results from the readable output stream", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "result",
        event_type: "runner_on_skipped",
        message:
          "skipping: [cash-1_019f32f9] => (item={'changed': False, 'skip_reason': 'Conditional result was False'})",
      }),
    );

    expect(rendered).toBeNull();
  });

  it("detects raw runner status labels so they do not become user-facing subtitles", () => {
    expect(isRawRunnerStatusMessage("runner_on_start cash-1 Gather facts")).toBe(true);
    expect(isRawRunnerStatusMessage("playbook_on_start")).toBe(true);
    expect(isRawRunnerStatusMessage("Run started by worker lease owner")).toBe(true);
    expect(isRawRunnerStatusMessage("Ansible job dispatched; waiting for runner events")).toBe(true);
  });

  it("removes Ansible banner asterisks from readable output", () => {
    const rendered = formatRunOutputEvent(
      event({
        event_kind: "stdout",
        event_type: "playbook_on_task_start",
        message: "TASK [Upsert VM disks through Piping REST] *******************************",
      }),
    );

    expect(rendered).toBe("TASK [Upsert VM disks through Piping REST]");
  });

  it("keeps raw console lines byte-for-byte enough for an Ansible console view", () => {
    const raw = event({
      event_kind: "stdout",
      event_type: "console_line",
      message: "TASK [Gather facts] *******************************************************",
      payload: { stream: "stdout", raw: true, scope: "global" },
    });

    expect(isRawConsoleEvent(raw)).toBe(true);
    expect(formatRunOutputEvent(raw)).toBe(
      "TASK [Gather facts] *******************************************************",
    );
  });

  it("renders a message-less console line as an empty line, never the event type token", () => {
    const raw = event({
      event_kind: "stdout",
      event_type: "console_line",
      message: null,
      payload: { stream: "stdout", raw: true, scope: "global" },
    });

    expect(formatRunOutputEvent(raw)).toBe("");
    expect(formatRunOutputEvent(raw)).not.toBe("console_line");
  });

  it("prefers raw console events over structured projection events without dropping runner notices", () => {
    const runnerNotice = event({
      id: "notice",
      event_kind: "stdout",
      event_type: "stdout",
      message: "Starting ansible-playbook: 1694 targets, forks=100",
      payload: { stream: "runner" },
    });
    const raw = event({
      id: "raw",
      event_kind: "stdout",
      event_type: "console_line",
      message: "fatal: [cash-1]: UNREACHABLE!",
      payload: { stream: "stdout", raw: true, scope: "global" },
    });
    const structured = event({
      id: "structured",
      event_kind: "result",
      event_type: "runner_on_unreachable",
      message: "fatal: [cash-1]: UNREACHABLE! => compacted",
    });

    expect(consoleOutputEvents([runnerNotice, raw, structured]).map((item) => item.id)).toEqual([
      "notice",
      "raw",
    ]);
  });

  it("hides generic runner lifecycle lines from readable output", () => {
    expect(
      formatRunOutputEvent(
        event({
          event_kind: "status",
          event_type: "runner_dispatched",
          message: "Ansible job dispatched; waiting for runner events",
        }),
      ),
    ).toBeNull();
  });

  it("keeps global Ansible events in the all-targets output", () => {
    expect(
      formatRunOutputEvent(
        event({
          event_kind: "stdout",
          event_type: "playbook_on_play_start",
          message: "PLAY [Auto-update inventory]",
          payload: { scope: "global" },
        }),
      ),
    ).toBe("PLAY [Auto-update inventory]");
  });

  it("keeps global Ansible context events in selected-target output", () => {
    expect(
      formatRunOutputEvent(
        event({
          event_kind: "stdout",
          event_type: "playbook_on_play_start",
          message: "PLAY [Auto-update inventory]",
          payload: { scope: "global" },
        }),
        "Касса 1-2",
        "019f34d8-1111-2222-3333-444444444444",
      ),
    ).toBe("PLAY [Auto-update inventory]");
  });
});

describe("ANSI colour parsing", () => {
  it("returns a single unstyled segment for plain text", () => {
    expect(parseAnsiLine("TASK [update] ***")).toEqual([
      { text: "TASK [update] ***", className: "" },
    ]);
  });

  it("maps ansible's green ok / yellow changed / red fatal to theme classes", () => {
    expect(parseAnsiLine("\x1b[0;32mok: [web1]\x1b[0m")).toEqual([
      { text: "ok: [web1]", className: "text-status-positive" },
    ]);
    expect(parseAnsiLine("\x1b[0;33mchanged: [web1]\x1b[0m")[0]?.className).toBe(
      "text-status-warning",
    );
    expect(parseAnsiLine("\x1b[0;31mfatal: [web1]\x1b[0m")[0]?.className).toBe(
      "text-status-negative",
    );
  });

  it("handles multi-param SGR (bold + colour) and both reset forms", () => {
    expect(parseAnsiLine("\x1b[1;35mWARNING\x1b[0m")).toEqual([
      { text: "WARNING", className: "font-medium text-fuchsia-400" },
    ]);
    // Bare reset `\x1b[m` clears style just like `\x1b[0m`.
    expect(parseAnsiLine("\x1b[0;32mA\x1b[mB")).toEqual([
      { text: "A", className: "text-status-positive" },
      { text: "B", className: "" },
    ]);
  });

  it("treats 256-colour / truecolour as default fg and drops non-SGR CSI", () => {
    expect(parseAnsiLine("\x1b[38;5;196mX\x1b[0m")).toEqual([{ text: "X", className: "" }]);
    // A cursor-clear CSI (`\x1b[2K`) is consumed, never rendered as text.
    expect(parseAnsiLine("a\x1b[2Kb")).toEqual([{ text: "ab", className: "" }]);
  });

  it("stripAnsi removes escapes and no-ops on plain text", () => {
    expect(stripAnsi("\x1b[0;32mok: [web1]\x1b[0m")).toBe("ok: [web1]");
    const plain = "ok: [web1]";
    expect(stripAnsi(plain)).toBe(plain);
  });

  it("host filter matches through ANSI and preserves the coloured line", () => {
    // ANSI immediately around the host token would defeat a naive (unstripped)
    // token match; the selected line must still be kept, WITH its colour intact.
    const message = "ok: [\x1b[1;33mweb1\x1b[0m] => changed";
    expect(filterMessageForSelectedHost(message, "web1")).toBe(message);
    // A different run's alias-suffixed line is still dropped through ANSI.
    const twoHosts = [
      "ok: [\x1b[0;32mweb1_019f0001\x1b[0m]",
      "ok: [\x1b[0;32mweb2_019f0002\x1b[0m]",
    ].join("\n");
    const filtered = filterMessageForSelectedHost(twoHosts, null, "019f0001-0000-0000-0000-000000000000");
    expect(filtered).toContain("web1_019f0001");
    expect(filtered).not.toContain("web2_019f0002");
  });
});
