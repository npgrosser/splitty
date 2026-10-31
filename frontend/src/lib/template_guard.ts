import type { Template, TemplatePane } from "./protocol/types.ts";

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
    listener = null;
  };
}

/** Every command the template would execute, in pane order. */
export function commandsIn(template: Template): TemplateCommand[] {
  return Object.entries(template.panes ?? {})
    .filter(([, pane]) => !!(pane as TemplatePane)?.command)
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

/**
 * Apply a template, asking first if it would execute anything.
 *
 * Templates without commands apply directly — there is nothing to warn about,
 * and a prompt on every layout restore would train users to dismiss it.
 */
export function guardedApplyTemplate(
  template: Template,
  apply: (template: Template) => void,
): void {
  const commands = commandsIn(template);
  if (commands.length === 0 || !listener) {
    apply(template);
    return;
  }

  listener({
    templateName: template.name ?? "template",
    commands,
    approveFull: () => apply(template),
    approveLayoutOnly: () => apply(withoutCommands(template)),
    cancel: () => {},
  });
}
