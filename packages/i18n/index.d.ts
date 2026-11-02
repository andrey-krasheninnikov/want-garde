import catalog from "./catalog.json";
export type Locale = keyof typeof catalog.locales;
export const defaultLocale: Locale;
export const locales: Locale[];
export function resolveLocale(value: unknown): Locale;
export function messages(value: unknown): (typeof catalog.locales)[Locale];
