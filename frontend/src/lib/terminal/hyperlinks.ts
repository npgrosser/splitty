import { openExternalTarget } from "../window.ts";

/** Open a clicked terminal link through the pane's opener or system browser. */
export function openHyperlink(uri: string, open: (url: string) => void = openInBrowser): void {
  open(uri);
}

/** A local pane's link: the system browser, web URLs only. */
export function openInBrowser(url: string): void {
  openExternalTarget({ url });
}
