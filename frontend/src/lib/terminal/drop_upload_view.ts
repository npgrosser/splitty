import { mount, unmount } from "svelte";
import { toast } from "svelte-sonner";
import DropUpload from "../../components/dialogs/DropUpload.svelte";
import type { DropUploadTarget } from "./drop_upload.ts";

export interface DropContext {
  host: string;
  cwd: string;
  supported: boolean;
}

/** One transfer UI per target; disposal stops pending work and removes its UI. */
export function createDropUpload(container: HTMLElement, target: DropUploadTarget, context: () => DropContext) {
  let active: (() => void) | undefined;
  let disposed = false;
  return {
    drop(files: File[]) {
      if (disposed || !files.length) return;
      if (active) {
        toast.info("An upload is already open for this pane.");
        return;
      }
      const controller = new AbortController();
      const holder = document.createElement("div");
      // A sibling of the terminal avoids its capture listeners handling dialog input.
      (container.parentElement ?? container).appendChild(holder);
      const close = () => {
        controller.abort();
        void unmount(component);
        holder.remove();
        active = undefined;
      };
      const component = mount(DropUpload, { target: holder, props: {
        files, target, context: context(), controller, onClose: close,
      } });
      active = close;
    },
    dispose() {
      disposed = true;
      active?.();
    },
  };
}
