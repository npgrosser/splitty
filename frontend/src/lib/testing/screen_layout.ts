/**
 * A screen's layout for in-process tests in a DOM environment: a real
 * dockview bound to a fake server's connection (`fake_server.ts`), with the
 * real viewer pane and a stand-in for the terminal pane that holds a
 * focusable input and attaches nothing. Used by vitest only.
 */

import { createDockview, type DockviewApi, type GroupPanelPartInitParameters, type IContentRenderer } from "dockview-core";
import { ViewerPaneRenderer } from "../../panes/viewer_pane.ts";
import { ScreenBinding, registerScreenApi, unregisterScreenApi } from "../connection/screen_binding.ts";
import { toBase64 } from "../protocol/base64.ts";
import { VIEWER_CAPABILITY } from "../viewer/limits.ts";
import { VIEWER_COMPONENT } from "../viewer/panel.ts";
import { fakeServer, type FakeSocket } from "./fake_server.ts";

/** A terminal pane's stand-in: an input that can hold the keyboard. */
export class TerminalStandIn implements IContentRenderer {
  readonly element: HTMLElement;
  readonly input: HTMLTextAreaElement;
  paneId = "";
  disposed = false;

  constructor(doc: Document) {
    this.element = doc.createElement("div");
    this.input = doc.createElement("textarea");
    this.element.append(this.input);
  }

  init(params: GroupPanelPartInitParameters): void {
    this.paneId = params.api.id;
  }

  dispose(): void {
    this.disposed = true;
  }
}

export interface TestScreen {
  readonly api: DockviewApi;
  readonly binding: ScreenBinding;
  readonly socket: FakeSocket;
  /** Every terminal stand-in built so far, the disposed ones too. */
  readonly terminals: TerminalStandIn[];
  /** The content renderer of a pane's panel. */
  content<T extends IContentRenderer = IContentRenderer>(paneId: string): T;
  /** The server answers every read not answered yet with its pane's content, in one chunk. */
  answerReads(contents: Readonly<Record<string, string>>): void;
  /** A new connection: another welcome, with another client ID. */
  reconnect(clientId: string): void;
  dispose(): void;
}

/** Dockview measures with it; jsdom has none. */
class NoResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** `capabilities` are what the server announces; viewer panes without more. */
export function testScreen(options: { clientId?: string; capabilities?: readonly string[] } = {}): TestScreen {
  const globals = globalThis as { ResizeObserver?: unknown };
  globals.ResizeObserver ??= NoResizeObserver;
  const { socket, conn } = fakeServer("local");
  const capabilities = options.capabilities ?? [VIEWER_CAPABILITY];
  const welcome = (clientId: string) => socket.message({ type: "mux.welcome", clientId, screens: [], capabilities });
  welcome(options.clientId ?? "client-1");

  const root = document.createElement("div");
  document.body.append(root);
  const terminals: TerminalStandIn[] = [];
  const api = createDockview(root, {
    createComponent: ({ name }) => {
      if (name === VIEWER_COMPONENT) return new ViewerPaneRenderer();
      const terminal = new TerminalStandIn(document);
      terminals.push(terminal);
      return terminal;
    },
    defaultRenderer: "always",
  });
  api.layout(1200, 800);
  const binding = new ScreenBinding("screen-1", conn, "s1", { active: true });
  registerScreenApi(api, binding);

  const encoder = new TextEncoder();
  const answered = new Set<string>();
  return {
    api,
    binding,
    socket,
    terminals,
    content: <T extends IContentRenderer>(paneId: string) => api.getPanel(paneId)!.view.content as T,
    answerReads: (contents) => {
      for (const read of socket.sentOfType("viewer.read")) {
        if (answered.has(read.requestId)) continue;
        answered.add(read.requestId);
        const bytes = encoder.encode(contents[read.paneId] ?? "");
        socket.message({
          type: "viewer.chunk", requestId: read.requestId, offset: 0, size: bytes.length,
          version: "v1", data: toBase64(bytes), eof: true,
        });
      }
    },
    reconnect: (clientId) => {
      socket.drop();
      welcome(clientId);
    },
    dispose: () => {
      unregisterScreenApi(api);
      api.dispose();
      root.remove();
      conn.dispose();
    },
  };
}
