/**
 * Vocabulary → i18n coverage guard (web half of the access-model parity gates).
 *
 * The permission dictionary is owned by the backend registry
 * (`access_control::domain::vocabulary`) and reaches the frontend through the
 * OpenAPI contract enums. Adding a ResourceType/Action variant fails backend
 * compile until described in the registry and fails a backend test until the
 * contract is regenerated — this test closes the LAST hop: every enum value in
 * the committed contract must have a label AND a description in BOTH locales,
 * so a new permission can never render as a bare wire string in the pickers.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import en from "../locales/en";
import ru from "../locales/ru";

// The committed contract, verbatim. Read via node fs (vitest's Vite server
// denies ?raw imports outside its fs allow-list). Parsed textually below — no
// YAML dependency needed for a two-space-indented component enum block.
const openapiYaml = readFileSync(
  resolve(__dirname, "../../../../../api/contracts/piping.openapi.yaml"),
  "utf8",
);

/** Extract a top-level component schema's `enum:` values from the contract. */
function contractEnumValues(schemaName: string): string[] {
  const header = `\n    ${schemaName}:\n`;
  const start = openapiYaml.indexOf(header);
  expect(start, `schema ${schemaName} present in contract`).toBeGreaterThan(-1);
  const after = openapiYaml.slice(start + header.length);
  const enumAt = after.indexOf("      enum:\n");
  expect(enumAt, `schema ${schemaName} has an enum block`).toBeGreaterThan(-1);
  const values: string[] = [];
  for (const line of after.slice(enumAt + "      enum:\n".length).split("\n")) {
    const value = /^ {6}- (.+)$/.exec(line)?.[1];
    if (value === undefined) break;
    values.push(value.trim());
  }
  expect(values.length, `schema ${schemaName} enum non-empty`).toBeGreaterThan(0);
  return values;
}

type LocaleTree = Record<string, unknown>;

function policiesBlock(locale: LocaleTree, block: string): Record<string, unknown> {
  const accessControl = locale.accessControl as LocaleTree;
  const policies = accessControl.policies as LocaleTree;
  return (policies[block] ?? {}) as Record<string, unknown>;
}

describe("access vocabulary i18n coverage", () => {
  const resources = contractEnumValues("ResourceType").map((value) => value.toLowerCase());
  const actions = contractEnumValues("Action").map((value) => value.toLowerCase());

  for (const [name, locale] of [
    ["en", en],
    ["ru", ru],
  ] as const) {
    it(`${name}: every contract resource type has a label and a description`, () => {
      const labels = policiesBlock(locale, "resourceTypes");
      const descriptions = policiesBlock(locale, "resourceDescriptions");
      for (const resource of resources) {
        expect(labels[resource], `${name} label for ${resource}`).toBeTypeOf("string");
        expect(
          descriptions[resource],
          `${name} description for ${resource}`,
        ).toBeTypeOf("string");
      }
    });

    it(`${name}: every contract action has a label and a description`, () => {
      const labels = policiesBlock(locale, "actionLabels");
      const descriptions = policiesBlock(locale, "actionDescriptions");
      for (const action of actions) {
        expect(labels[action], `${name} label for ${action}`).toBeTypeOf("string");
        expect(descriptions[action], `${name} description for ${action}`).toBeTypeOf("string");
      }
    });
  }
});
