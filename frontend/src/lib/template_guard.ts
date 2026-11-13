import type { Template, TemplateLayoutNode, TemplatePane } from "./protocol/types.ts";

/**
 * Confirmation gate for templates that carry shell commands.
 *
 * A template looks like layout data, and users treat it that way: they are
 * shared in chats, committed to repositories and downloaded from gists. But a
 * terminal pane may carry a `command`, and applying the template writes that
 * command straight into a PTY after a short delay (security review
 * SR-12). "Restore my layout" therefore used to be able to mean
 * "execute this", with nothing shown beforehand.
 *
 * This module makes the executable content explicit before anything runs. It
 * follows the same shape as `close_guard.ts`: the dialog registers a listener,
 * and the guard routes through it when there is something to confirm.
 *
 * # Why approvals are not remembered
 *
 * The review suggests storing a trust decision per template hash. That was
 * deliberately not built: the only client-side store available is keyed by
 * origin, and the app's origin contains the backend's port, which is random per
 * launch. A remembered approval would therefore be forgotten on every restart
 * while still costing a persistence mechanism and a staleness rule. Asking each
 * time a template with commands is applied is both simpler and stricter.
 */

/** One command a template would run, as it will be shown to the user. */
export interface TemplateCommand {
  paneName: string;
  command: string;
  dir?: string;
  shellProfile?: string;
}

export interface TemplateApprovalRequest {
  templateName: string;
  commands: TemplateCommand[];
  /** Apply the template including its commands. */
  approveFull: () => void;
  /** Apply only the layout, discarding every command. */
  approveLayoutOnly: () => void;
  cancel: () => void;
}

type Listener = (request: TemplateApprovalRequest) => void;
let listener: Listener | null = null;

/** Register the approval dialog handler (called by App). */
export function onTemplateApprovalRequest(fn: Listener): () => void {
  listener = fn;
  return () => {
    if (listener === fn) listener = null;
  };
}

/** Every command the template would execute, in pane order. */
export function commandsIn(template: Template): TemplateCommand[] {
  return Object.entries(template.panes ?? {})
    // A blank command is no command, as in the native client.
    .filter(([, pane]) => !!(pane as TemplatePane)?.command?.trim())
    .map(([paneName, pane]) => {
      const p = pane as TemplatePane;
      return {
        paneName,
        command: p.command as string,
        dir: p.dir,
        shellProfile: p.shell_profile,
      };
    });
}

/** A copy of the template with every command removed. */
export function withoutCommands(template: Template): Template {
  const panes: Record<string, TemplatePane> = {};
  for (const [name, pane] of Object.entries(template.panes ?? {})) {
    const { command: _command, restore_mode: _restoreMode, ...rest } = pane as TemplatePane;
    panes[name] = rest as TemplatePane;
  }
  return { ...template, panes };
}

function captureLayout(node: TemplateLayoutNode): TemplateLayoutNode {
  return {
    ...node,
    ...(Array.isArray(node.slot) ? { slot: [...node.slot] } : {}),
    ...(node.children ? { children: node.children.map(captureLayout) } : {}),
  };
}

function captureTemplate(template: Template): Template {
  // Reading the typed data also works for the reactive proxy held by the root
  // prompt. structuredClone rejects that proxy even though its data is valid.
  return {
    ...template,
    panes: Object.fromEntries(Object.entries(template.panes).map(([name, pane]) => [name, { ...pane }])),
    layout: captureLayout(template.layout),
    ...(template.floating ? {
      floating: template.floating.map(entry => ({ layout: captureLayout(entry.layout), bounds: { ...entry.bounds } })),
    } : {}),
  };
}

/**
 * Apply a template, asking first if it would execute anything.
 *
 * Templates without commands apply directly — there is nothing to warn about,
 * and a prompt on every layout restore would train users to dismiss it.
 * Returns false when a command approval request cannot be presented.
 */
export function guardedApplyTemplate(
  template: Template,
  apply: (template: Template) => void,
): boolean {
  // The review and the later operation must see the same content even if the
  // caller refreshes its source template while the dialog is open.
  const captured = captureTemplate(template);
  const commands = commandsIn(captured);
  if (commands.length === 0) {
    apply(captured);
    return true;
  }

  const handler = listener;
  if (!handler) return false;

  let consumed = false;
  const once = (decision: () => void) => () => {
    if (consumed) return;
    consumed = true;
    decision();
  };
  try {
    handler({
      templateName: captured.name ?? "template",
      commands,
      approveFull: once(() => apply(captured)),
      approveLayoutOnly: once(() => apply(withoutCommands(captured))),
      cancel: once(() => {}),
    });
    return true;
  } catch {
    // A failed dialog handoff must not leave its callbacks able to authorize
    // command execution later.
    consumed = true;
    return false;
  }
}
