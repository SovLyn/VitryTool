/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import { I18nProvider } from "./i18n";
import { ThemeProvider } from "./theme";

render(
  () => (
    <I18nProvider>
      <ThemeProvider>
        <App />
      </ThemeProvider>
    </I18nProvider>
  ),
  document.getElementById("root") as HTMLElement
);
