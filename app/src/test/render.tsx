import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import type { Language } from "../i18n/i18n";
import { I18nProvider } from "../i18n/i18n-provider";
import { type HostApi, HostContext } from "../ipc/host";
import { fakeHost } from "./fake-host";

/** Renders `element` in a language, with a host. */
export function renderWith(
  element: ReactElement,
  { host = fakeHost(), language = "en" }: { host?: HostApi; language?: Language } = {},
) {
  return render(
    <HostContext.Provider value={host}>
      <I18nProvider language={language}>{element}</I18nProvider>
    </HostContext.Provider>,
  );
}
