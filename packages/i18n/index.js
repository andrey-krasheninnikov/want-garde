import catalog from "./catalog.json" with { type: "json" };

export const defaultLocale = catalog.default;
export const locales = Object.keys(catalog.locales);

export function resolveLocale(value) {
  return typeof value === "string" && Object.hasOwn(catalog.locales, value) ? value : defaultLocale;
}

export function messages(value) {
  return catalog.locales[resolveLocale(value)];
}
