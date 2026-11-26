import { useSyncExternalStore } from "react";

/** Whether the page is in dark mode, kept current when the theme is switched. */
export function useDark(): boolean {
  return useSyncExternalStore(subscribe, () => document.documentElement.classList.contains("dark"));
}

function subscribe(onChange: () => void) {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  return () => observer.disconnect();
}
