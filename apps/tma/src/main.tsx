import { createRoot } from "react-dom/client";
import { PRODUCT_VERSION } from "@want-garde/contracts";
import { locales, messages, resolveLocale } from "@want-garde/i18n";
import "./style.css";

const locale = resolveLocale(new URLSearchParams(window.location.search).get("lang"));
const copy = messages(locale);
document.documentElement.lang = locale;
document.documentElement.dir = copy.dir;

function App() {
  return (
    <main>
      <h1>Want Garde</h1>
      <p>{copy.scaffold}</p>
      <p>
        <code>{PRODUCT_VERSION}</code>
      </p>
      <label htmlFor="language">{copy.language}</label>
      <select
        id="language"
        value={locale}
        onChange={(event) => {
          const url = new URL(window.location.href);
          url.searchParams.set("lang", event.target.value);
          window.location.assign(url);
        }}
      >
        {locales.map((value) => (
          <option key={value} value={value} lang={value}>
            {messages(value).nativeName}
          </option>
        ))}
      </select>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
