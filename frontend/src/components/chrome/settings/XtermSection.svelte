<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import {
    updateXtermSetting,
    defaults,
    selectValue,
    inputNumber,
    type FontWeight,
    type TerminalRenderer,
    type XtermSettings,
  } from "../../../lib/settings.ts";
  import SettingRow from "./SettingRow.svelte";
  import ToggleSwitch from "./ToggleSwitch.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
    renderer: TerminalRenderer;
  }

  let { collapsed, onToggle, renderer }: Props = $props();

  const s = $derived(settings());

  /** Settings not supported by ghostty-web renderer */
  const GHOSTTY_UNSUPPORTED: Set<keyof XtermSettings> = new Set([
    "gpuAcceleration",
    "drawBoldTextInBrightColors",
    "customGlyphs",
    "rescaleOverlappingGlyphs",
    "minimumContrastRatio",
    "lineHeight",
    "letterSpacing",
    "fontWeight",
    "fontWeightBold",
    "scrollbarWidth",
    "scrollbarOverlay",
    "scrollbarFade",
  ]);

  function dis(key: keyof XtermSettings): boolean {
    return renderer === "ghostty" && GHOSTTY_UNSUPPORTED.has(key);
  }

  const sectionTitle = $derived(renderer === "ghostty" ? "RENDERER" : "XTERM.JS");

  const fontWeightOptions: { value: FontWeight; label: string }[] = [
    { value: "normal", label: "Normal" },
    { value: "bold", label: "Bold" },
    { value: "100", label: "100" },
    { value: "200", label: "200" },
    { value: "300", label: "300" },
    { value: "400", label: "400" },
    { value: "500", label: "500" },
    { value: "600", label: "600" },
    { value: "700", label: "700" },
    { value: "800", label: "800" },
    { value: "900", label: "900" },
  ];
</script>

<SettingsSection title={sectionTitle} {collapsed} {onToggle}>
  <!-- GPU Acceleration -->
  <SettingRow
    label="GPU Acceleration"
    hint="Use WebGL for rendering"
    showReset={s.terminal.xterm.gpuAcceleration !== defaults.terminal.xterm.gpuAcceleration}
    onReset={() => updateXtermSetting("gpuAcceleration", defaults.terminal.xterm.gpuAcceleration)}
    disabled={dis("gpuAcceleration")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.gpuAcceleration === "on"}
      onchange={() => updateXtermSetting("gpuAcceleration", s.terminal.xterm.gpuAcceleration === "on" ? "off" : "on")}
      disabled={dis("gpuAcceleration")}
    />
  </SettingRow>

  <!-- Scrollbar Width -->
  <SettingRow
    label="Scrollbar Width"
    hint={`${s.terminal.xterm.scrollbarWidth}px`}
    showReset={s.terminal.xterm.scrollbarWidth !== defaults.terminal.xterm.scrollbarWidth}
    onReset={() => updateXtermSetting("scrollbarWidth", defaults.terminal.xterm.scrollbarWidth)}
    disabled={dis("scrollbarWidth")}
  >
    <input type="range" min="0" max="30" step="1" value={s.terminal.xterm.scrollbarWidth} onchange={(e) => updateXtermSetting("scrollbarWidth", inputNumber(e))} disabled={dis("scrollbarWidth")} />
  </SettingRow>

  <!-- Scrollbar Overlay -->
  <SettingRow
    label="Scrollbar Overlay"
    hint="Float scrollbar over content"
    showReset={s.terminal.xterm.scrollbarOverlay !== defaults.terminal.xterm.scrollbarOverlay}
    onReset={() => updateXtermSetting("scrollbarOverlay", defaults.terminal.xterm.scrollbarOverlay)}
    disabled={dis("scrollbarOverlay")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.scrollbarOverlay}
      onchange={() => updateXtermSetting("scrollbarOverlay", !s.terminal.xterm.scrollbarOverlay)}
      disabled={dis("scrollbarOverlay")}
    />
  </SettingRow>

  <!-- Scrollbar Fade -->
  <SettingRow
    label="Scrollbar Fade"
    hint="Hide scrollbar when idle"
    showReset={s.terminal.xterm.scrollbarFade !== defaults.terminal.xterm.scrollbarFade}
    onReset={() => updateXtermSetting("scrollbarFade", defaults.terminal.xterm.scrollbarFade)}
    disabled={dis("scrollbarFade")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.scrollbarFade}
      onchange={() => updateXtermSetting("scrollbarFade", !s.terminal.xterm.scrollbarFade)}
      disabled={dis("scrollbarFade")}
    />
  </SettingRow>

  <!-- Scroll Sensitivity -->
  <SettingRow
    label="Scroll Sensitivity"
    hint={`${s.terminal.xterm.scrollSensitivity}`}
    showReset={s.terminal.xterm.scrollSensitivity !== defaults.terminal.xterm.scrollSensitivity}
    onReset={() => updateXtermSetting("scrollSensitivity", defaults.terminal.xterm.scrollSensitivity)}
    disabled={dis("scrollSensitivity")}
  >
    <input type="range" min="0.1" max="10" step="0.1" value={s.terminal.xterm.scrollSensitivity} onchange={(e) => updateXtermSetting("scrollSensitivity", inputNumber(e))} disabled={dis("scrollSensitivity")} />
  </SettingRow>

  <!-- Fast Scroll Sensitivity -->
  <SettingRow
    label="Fast Scroll Sensitivity"
    hint={`${s.terminal.xterm.fastScrollSensitivity}`}
    showReset={s.terminal.xterm.fastScrollSensitivity !== defaults.terminal.xterm.fastScrollSensitivity}
    onReset={() => updateXtermSetting("fastScrollSensitivity", defaults.terminal.xterm.fastScrollSensitivity)}
    disabled={dis("fastScrollSensitivity")}
  >
    <input type="range" min="1" max="20" step="0.5" value={s.terminal.xterm.fastScrollSensitivity} onchange={(e) => updateXtermSetting("fastScrollSensitivity", inputNumber(e))} disabled={dis("fastScrollSensitivity")} />
  </SettingRow>

  <!-- Line Height -->
  <SettingRow
    label="Line Height"
    hint={`${s.terminal.xterm.lineHeight}`}
    showReset={s.terminal.xterm.lineHeight !== defaults.terminal.xterm.lineHeight}
    onReset={() => updateXtermSetting("lineHeight", defaults.terminal.xterm.lineHeight)}
    disabled={dis("lineHeight")}
  >
    <input type="range" min="0.8" max="2.0" step="0.05" value={s.terminal.xterm.lineHeight} onchange={(e) => updateXtermSetting("lineHeight", inputNumber(e))} disabled={dis("lineHeight")} />
  </SettingRow>

  <!-- Letter Spacing -->
  <SettingRow
    label="Letter Spacing"
    hint={`${s.terminal.xterm.letterSpacing}px`}
    showReset={s.terminal.xterm.letterSpacing !== defaults.terminal.xterm.letterSpacing}
    onReset={() => updateXtermSetting("letterSpacing", defaults.terminal.xterm.letterSpacing)}
    disabled={dis("letterSpacing")}
  >
    <input type="range" min="-5" max="10" step="0.5" value={s.terminal.xterm.letterSpacing} onchange={(e) => updateXtermSetting("letterSpacing", inputNumber(e))} disabled={dis("letterSpacing")} />
  </SettingRow>

  <!-- Font Weight -->
  <SettingRow
    label="Font Weight"
    hint="Normal text weight"
    showReset={s.terminal.xterm.fontWeight !== defaults.terminal.xterm.fontWeight}
    onReset={() => updateXtermSetting("fontWeight", defaults.terminal.xterm.fontWeight)}
    disabled={dis("fontWeight")}
  >
    <select
      value={s.terminal.xterm.fontWeight}
      onchange={(e) => updateXtermSetting("fontWeight", selectValue<FontWeight>(e))}
      disabled={dis("fontWeight")}
    >
      {#each fontWeightOptions as opt}
        <option value={opt.value}>{opt.label}</option>
      {/each}
    </select>
  </SettingRow>

  <!-- Font Weight Bold -->
  <SettingRow
    label="Font Weight Bold"
    hint="Bold text weight"
    showReset={s.terminal.xterm.fontWeightBold !== defaults.terminal.xterm.fontWeightBold}
    onReset={() => updateXtermSetting("fontWeightBold", defaults.terminal.xterm.fontWeightBold)}
    disabled={dis("fontWeightBold")}
  >
    <select
      value={s.terminal.xterm.fontWeightBold}
      onchange={(e) => updateXtermSetting("fontWeightBold", selectValue<FontWeight>(e))}
      disabled={dis("fontWeightBold")}
    >
      {#each fontWeightOptions as opt}
        <option value={opt.value}>{opt.label}</option>
      {/each}
    </select>
  </SettingRow>

  <!-- Minimum Contrast Ratio -->
  <SettingRow
    label="Min Contrast Ratio"
    hint={`${s.terminal.xterm.minimumContrastRatio}`}
    showReset={s.terminal.xterm.minimumContrastRatio !== defaults.terminal.xterm.minimumContrastRatio}
    onReset={() => updateXtermSetting("minimumContrastRatio", defaults.terminal.xterm.minimumContrastRatio)}
    disabled={dis("minimumContrastRatio")}
  >
    <input type="range" min="1" max="21" step="0.5" value={s.terminal.xterm.minimumContrastRatio} onchange={(e) => updateXtermSetting("minimumContrastRatio", inputNumber(e))} disabled={dis("minimumContrastRatio")} />
  </SettingRow>

  <!-- Bold in Bright Colors -->
  <SettingRow
    label="Bold in Bright Colors"
    hint="Render bold text with bright colors"
    showReset={s.terminal.xterm.drawBoldTextInBrightColors !== defaults.terminal.xterm.drawBoldTextInBrightColors}
    onReset={() => updateXtermSetting("drawBoldTextInBrightColors", defaults.terminal.xterm.drawBoldTextInBrightColors)}
    disabled={dis("drawBoldTextInBrightColors")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.drawBoldTextInBrightColors}
      onchange={() => updateXtermSetting("drawBoldTextInBrightColors", !s.terminal.xterm.drawBoldTextInBrightColors)}
      disabled={dis("drawBoldTextInBrightColors")}
    />
  </SettingRow>

  <!-- Custom Glyphs -->
  <SettingRow
    label="Custom Glyphs"
    hint="Use built-in box drawing characters"
    showReset={s.terminal.xterm.customGlyphs !== defaults.terminal.xterm.customGlyphs}
    onReset={() => updateXtermSetting("customGlyphs", defaults.terminal.xterm.customGlyphs)}
    disabled={dis("customGlyphs")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.customGlyphs}
      onchange={() => updateXtermSetting("customGlyphs", !s.terminal.xterm.customGlyphs)}
      disabled={dis("customGlyphs")}
    />
  </SettingRow>

  <!-- Rescale Overlapping Glyphs -->
  <SettingRow
    label="Rescale Overlapping Glyphs"
    hint="Shrink wide glyphs that overlap"
    showReset={s.terminal.xterm.rescaleOverlappingGlyphs !== defaults.terminal.xterm.rescaleOverlappingGlyphs}
    onReset={() => updateXtermSetting("rescaleOverlappingGlyphs", defaults.terminal.xterm.rescaleOverlappingGlyphs)}
    disabled={dis("rescaleOverlappingGlyphs")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.rescaleOverlappingGlyphs}
      onchange={() => updateXtermSetting("rescaleOverlappingGlyphs", !s.terminal.xterm.rescaleOverlappingGlyphs)}
      disabled={dis("rescaleOverlappingGlyphs")}
    />
  </SettingRow>

  <!-- Allow Transparency -->
  <SettingRow
    label="Allow Transparency"
    hint="Enable background transparency"
    showReset={s.terminal.xterm.allowTransparency !== defaults.terminal.xterm.allowTransparency}
    onReset={() => updateXtermSetting("allowTransparency", defaults.terminal.xterm.allowTransparency)}
    disabled={dis("allowTransparency")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.allowTransparency}
      onchange={() => updateXtermSetting("allowTransparency", !s.terminal.xterm.allowTransparency)}
      disabled={dis("allowTransparency")}
    />
  </SettingRow>

  <!-- Cursor Blink -->
  <SettingRow
    label="Cursor Blink"
    hint="Blinking cursor animation"
    showReset={s.terminal.xterm.cursorBlink !== defaults.terminal.xterm.cursorBlink}
    onReset={() => updateXtermSetting("cursorBlink", defaults.terminal.xterm.cursorBlink)}
    disabled={dis("cursorBlink")}
  >
    <ToggleSwitch
      checked={s.terminal.xterm.cursorBlink}
      onchange={() => updateXtermSetting("cursorBlink", !s.terminal.xterm.cursorBlink)}
      disabled={dis("cursorBlink")}
    />
  </SettingRow>
</SettingsSection>
