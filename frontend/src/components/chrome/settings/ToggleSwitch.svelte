<script lang="ts">
  interface Props {
    checked: boolean;
    onchange: () => void;
    disabled?: boolean;
  }

  let { checked, onchange, disabled = false }: Props = $props();
</script>

<label class="toggle">
  <input type="checkbox" {checked} {onchange} {disabled} />
  <span class="toggle-slider"></span>
</label>
