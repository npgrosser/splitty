import { afterEach, describe, expect, it, vi } from "vitest";
import { escapeDisclosure, focusInlineForm, unclaimedEscape } from "./inline_focus.ts";

// Unit event/focus targets; these tests do not stand in for browser layout or DOM tests.
function key(key = "Escape", isComposing = false): KeyboardEvent {
  return Object.assign(new Event("keydown", { cancelable: true, bubbles: true }), { key, isComposing }) as KeyboardEvent;
}

function editor() {
  class FocusTarget extends EventTarget {
    isConnected = true;
    focus = vi.fn();
    closest = vi.fn(() => null);
  }
  vi.stubGlobal("HTMLElement", FocusTarget);
  const trigger = new FocusTarget();
  const input = new FocusTarget();
  const document = { activeElement: trigger };
  input.focus.mockImplementation(() => { document.activeElement = input; });
  const form = Object.assign(new EventTarget(), {
    ownerDocument: document,
    querySelector: () => input,
    contains: (element: unknown) => element === input,
  }) as unknown as HTMLFormElement;
  return { trigger, input, document, form };
}

afterEach(() => vi.unstubAllGlobals());

describe("inline editor keyboard ownership", () => {
  it("claims Escape so the drawer guard cannot dismiss the drawer", async () => {
    const { form, trigger, input } = editor();
    const cancel = vi.fn();
    const action = focusInlineForm(form, cancel);
    await Promise.resolve();
    expect(input.focus).toHaveBeenCalledOnce();
    const event = key();
    const stop = vi.spyOn(event, "stopPropagation");
    form.dispatchEvent(event);
    expect(cancel).toHaveBeenCalledOnce();
    expect(stop).toHaveBeenCalledOnce();
    expect(unclaimedEscape(event)).toBe(false);
    action.destroy();
    await Promise.resolve();
    expect(trigger.focus).toHaveBeenCalledOnce();
  });

  it("does not take Escape during IME composition or steal focus after moving elsewhere", async () => {
    const { form, document, trigger } = editor();
    const cancel = vi.fn();
    const action = focusInlineForm(form, cancel);
    await Promise.resolve();
    form.dispatchEvent(key("Escape", true));
    form.dispatchEvent(key("Enter"));
    expect(cancel).not.toHaveBeenCalled();
    document.activeElement = trigger;
    action.destroy();
    await Promise.resolve();
    expect(trigger.focus).not.toHaveBeenCalled();
    expect(unclaimedEscape(key())).toBe(true);
  });

  it("closes only the current disclosure and returns focus to its summary", () => {
    const focus = vi.fn();
    const details = Object.assign(new EventTarget(), { open: true, querySelector: () => ({ focus }) }) as unknown as HTMLDetailsElement;
    const action = escapeDisclosure(details);
    const event = key();
    details.dispatchEvent(event);
    expect(details.open).toBe(false);
    expect(focus).toHaveBeenCalledOnce();
    expect(unclaimedEscape(event)).toBe(false);
    action.destroy();
  });
});
