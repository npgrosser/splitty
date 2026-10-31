<script lang="ts">
  import { windowAPI } from "../../lib/window.ts";

  const edges: { cls: string; dir: string }[] = [
    { cls: "n",  dir: "North" },
    { cls: "s",  dir: "South" },
    { cls: "e",  dir: "East" },
    { cls: "w",  dir: "West" },
    { cls: "ne", dir: "NorthEast" },
    { cls: "nw", dir: "NorthWest" },
    { cls: "se", dir: "SouthEast" },
    { cls: "sw", dir: "SouthWest" },
  ];
</script>

{#each edges as edge}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="resize-edge {edge.cls}"
    onmousedown={(e) => { e.preventDefault(); windowAPI.startResizeDragging(edge.dir); }}
  ></div>
{/each}

<style>
  .resize-edge {
    position: fixed;
    z-index: 99999;
  }
  .n  { top: 0; left: 5px; right: 5px; height: 5px; cursor: n-resize; }
  .s  { bottom: 0; left: 5px; right: 5px; height: 5px; cursor: s-resize; }
  .e  { top: 5px; right: 0; bottom: 5px; width: 5px; cursor: e-resize; }
  .w  { top: 5px; left: 0; bottom: 5px; width: 5px; cursor: w-resize; }
  .ne { top: 0; right: 0; width: 5px; height: 5px; cursor: ne-resize; }
  .nw { top: 0; left: 0; width: 5px; height: 5px; cursor: nw-resize; }
  .se { bottom: 0; right: 0; width: 5px; height: 5px; cursor: se-resize; }
  .sw { bottom: 0; left: 0; width: 5px; height: 5px; cursor: sw-resize; }
</style>
