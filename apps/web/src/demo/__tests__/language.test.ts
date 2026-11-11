import { describe, expect, it } from "vitest";
import { demoLanguageFromSearch, normalizeDemoLanguage } from "../language";

describe("demo language handshake", () => {
  it("accepts the explicit en/ru query the portal sends", () => {
    expect(demoLanguageFromSearch("?lang=ru")).toBe("ru");
    expect(demoLanguageFromSearch("?lang=en")).toBe("en");
  });

  it("folds regional tags the same way the app's language resolution does", () => {
    expect(normalizeDemoLanguage("ru-RU")).toBe("ru");
    expect(normalizeDemoLanguage("en-US")).toBe("en");
    expect(normalizeDemoLanguage(" RU ")).toBe("ru");
  });

  it("ignores an absent or unsupported query so the native default stands", () => {
    expect(demoLanguageFromSearch("")).toBeUndefined();
    expect(demoLanguageFromSearch("?lang=")).toBeUndefined();
    expect(demoLanguageFromSearch("?lang=de")).toBeUndefined();
  });
});
