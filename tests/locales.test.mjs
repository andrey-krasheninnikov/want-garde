import assert from "node:assert/strict";
import test from "node:test";
import { defaultLocale, locales, messages, resolveLocale } from "../packages/i18n/index.js";

test("the public locale catalog covers all agreed languages and RTL", () => {
  assert.deepEqual(locales, [
    "ru",
    "en",
    "zh-Hans",
    "es",
    "pt-BR",
    "hi",
    "ja",
    "ar",
    "he",
    "tr",
    "fr",
    "de",
  ]);
  assert.equal(defaultLocale, "en");
  for (const locale of locales) {
    const copy = messages(locale);
    for (const key of ["nativeName", "scaffold", "unavailable", "usage", "language"]) {
      assert.equal(typeof copy[key], "string");
      assert.ok(copy[key].length > 0);
    }
    assert.equal(copy.dir, ["ar", "he"].includes(locale) ? "rtl" : "ltr");
  }
});

test("invalid locale input has an English fallback", () => {
  for (const value of [undefined, null, 1, "__proto__", "constructor", "en-us", "unknown"]) {
    assert.equal(resolveLocale(value), "en");
  }
  assert.equal(resolveLocale("ar"), "ar");
});
