// Server commands (`command` events) arrive per connection: see
// `ServerConnection.onCommand` and `resolveCommandTarget` in lib/connection.

/** Client-only UI events, never sent to or received from the backend. */
export type UICommand =
  | { type: "drawer.toggle" }
  | { type: "overlay.toggle" }
  // Move a pane of the screen `screenKey` to the screen `target`, or to a new screen (null).
  | { type: "pane.move-to-screen"; screenKey: string; paneId: string; target: string | null };

type UICommandHandler = (command: UICommand) => void;

const uiHandlers = new Set<UICommandHandler>();

export function emitUICommand(command: UICommand) {
  for (const handler of uiHandlers) {
    handler(command);
  }
}

export function subscribeUICommands(handler: UICommandHandler): () => void {
  uiHandlers.add(handler);
  return () => uiHandlers.delete(handler);
}
