/**
 * Monaco editor theme definitions.
 *
 * Built-in themes (vs-dark, vs, hc-black, hc-light) do not need custom data;
 * Monaco knows them natively. Custom themes provide a full
 * `IStandaloneThemeData` with token rules and editor colors.
 */

import type { editor } from "monaco-editor";

/** Descriptor for a selectable Monaco editor theme. */
interface MonacoThemeDef {
  /** Theme identifier used by `monaco.editor.defineTheme` / `<DiffEditor theme>`. */
  readonly id: string;
  /** Human-readable label shown in the settings dropdown. */
  readonly label: string;
  /**
   * Whether this is a Monaco built-in theme (`true`) or a custom theme that
   * must be registered via `defineTheme` before use.
   */
  readonly builtin: boolean;
  /**
   * Full theme data for custom themes. `undefined` for built-in themes that
   * Monaco already knows about.
   */
  readonly data: editor.IStandaloneThemeData | undefined;
}

// ---------------------------------------------------------------------------
// Custom theme: GitHub Dark
// ---------------------------------------------------------------------------

const githubDarkTheme: editor.IStandaloneThemeData = {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "comment", foreground: "8b949e", fontStyle: "italic" },
    { token: "keyword", foreground: "ff7b72" },
    { token: "string", foreground: "a5d6ff" },
    { token: "number", foreground: "79c0ff" },
    { token: "type", foreground: "ffa657" },
    { token: "type.identifier", foreground: "ffa657" },
    { token: "function", foreground: "d2a8ff" },
    { token: "variable", foreground: "ffa657" },
    { token: "constant", foreground: "79c0ff" },
    { token: "operator", foreground: "ff7b72" },
    { token: "delimiter", foreground: "c9d1d9" },
    { token: "tag", foreground: "7ee787" },
    { token: "attribute.name", foreground: "79c0ff" },
    { token: "attribute.value", foreground: "a5d6ff" },
    { token: "regexp", foreground: "7ee787" },
  ],
  colors: {
    "editor.background": "#0d1117",
    "editor.foreground": "#c9d1d9",
    "editor.lineHighlightBackground": "#161b22",
    "editor.selectionBackground": "#264f78",
    "editorCursor.foreground": "#c9d1d9",
    "editorWhitespace.foreground": "#484f58",
    "editorLineNumber.foreground": "#8b949e",
    "editorLineNumber.activeForeground": "#c9d1d9",
    "editorIndentGuide.background": "#21262d",
    "editorIndentGuide.activeBackground": "#30363d",
    "diffEditor.insertedTextBackground": "#23863633",
    "diffEditor.removedTextBackground": "#da363433",
  },
};

// ---------------------------------------------------------------------------
// Custom theme: One Dark Pro (Atom-inspired)
// ---------------------------------------------------------------------------

const oneDarkProTheme: editor.IStandaloneThemeData = {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "comment", foreground: "5c6370", fontStyle: "italic" },
    { token: "keyword", foreground: "c678dd" },
    { token: "string", foreground: "98c379" },
    { token: "number", foreground: "d19a66" },
    { token: "type", foreground: "e5c07b" },
    { token: "type.identifier", foreground: "e5c07b" },
    { token: "function", foreground: "61afef" },
    { token: "variable", foreground: "e06c75" },
    { token: "constant", foreground: "d19a66" },
    { token: "operator", foreground: "56b6c2" },
    { token: "delimiter", foreground: "abb2bf" },
    { token: "tag", foreground: "e06c75" },
    { token: "attribute.name", foreground: "d19a66" },
    { token: "attribute.value", foreground: "98c379" },
    { token: "regexp", foreground: "98c379" },
  ],
  colors: {
    "editor.background": "#282c34",
    "editor.foreground": "#abb2bf",
    "editor.lineHighlightBackground": "#2c313c",
    "editor.selectionBackground": "#3e4451",
    "editorCursor.foreground": "#528bff",
    "editorWhitespace.foreground": "#3b4048",
    "editorLineNumber.foreground": "#495162",
    "editorLineNumber.activeForeground": "#abb2bf",
    "editorIndentGuide.background": "#3b4048",
    "editorIndentGuide.activeBackground": "#4b5263",
    "diffEditor.insertedTextBackground": "#98c37933",
    "diffEditor.removedTextBackground": "#e06c7533",
  },
};

// ---------------------------------------------------------------------------
// Custom theme: Solarized Dark
// ---------------------------------------------------------------------------

const solarizedDarkTheme: editor.IStandaloneThemeData = {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "comment", foreground: "586e75", fontStyle: "italic" },
    { token: "keyword", foreground: "859900" },
    { token: "string", foreground: "2aa198" },
    { token: "number", foreground: "d33682" },
    { token: "type", foreground: "b58900" },
    { token: "type.identifier", foreground: "b58900" },
    { token: "function", foreground: "268bd2" },
    { token: "variable", foreground: "b58900" },
    { token: "constant", foreground: "cb4b16" },
    { token: "operator", foreground: "859900" },
    { token: "delimiter", foreground: "839496" },
    { token: "tag", foreground: "268bd2" },
    { token: "attribute.name", foreground: "93a1a1" },
    { token: "attribute.value", foreground: "2aa198" },
    { token: "regexp", foreground: "dc322f" },
  ],
  colors: {
    "editor.background": "#002b36",
    "editor.foreground": "#839496",
    "editor.lineHighlightBackground": "#073642",
    "editor.selectionBackground": "#274642",
    "editorCursor.foreground": "#839496",
    "editorWhitespace.foreground": "#073642",
    "editorLineNumber.foreground": "#586e75",
    "editorLineNumber.activeForeground": "#839496",
    "editorIndentGuide.background": "#073642",
    "editorIndentGuide.activeBackground": "#0a4a5e",
    "diffEditor.insertedTextBackground": "#2aa19833",
    "diffEditor.removedTextBackground": "#dc322f33",
  },
};

// ---------------------------------------------------------------------------
// Theme registry
// ---------------------------------------------------------------------------

/**
 * All available Monaco themes, including built-in and custom definitions.
 *
 * Custom themes must be registered with `monaco.editor.defineTheme(id, data)`
 * before they can be used. Use {@link registerCustomThemes} to do this once
 * on editor mount.
 */
export const MONACO_THEMES = [
  // Built-in themes: Monaco knows these natively, no defineTheme needed.
  { id: "vs-dark", label: "Dark (Default)", builtin: true, data: undefined },
  { id: "vs", label: "Light", builtin: true, data: undefined },
  { id: "hc-black", label: "High Contrast Dark", builtin: true, data: undefined },
  { id: "hc-light", label: "High Contrast Light", builtin: true, data: undefined },
  // Custom themes: require defineTheme registration.
  { id: "github-dark", label: "GitHub Dark", builtin: false, data: githubDarkTheme },
  { id: "one-dark-pro", label: "One Dark Pro", builtin: false, data: oneDarkProTheme },
  { id: "solarized-dark", label: "Solarized Dark", builtin: false, data: solarizedDarkTheme },
] as const satisfies readonly MonacoThemeDef[];

/** The set of valid theme ID strings drawn from `MONACO_THEMES`. */
export type MonacoThemeId = (typeof MONACO_THEMES)[number]["id"];

/**
 * Register all custom (non-built-in) themes with the Monaco editor instance.
 *
 * Call this once in the `onMount` / `handleEditorDidMount` callback to ensure
 * custom themes are available before setting the editor's theme.
 */
export function registerCustomThemes(
  monacoInstance: typeof import("monaco-editor"),
): void {
  for (const theme of MONACO_THEMES) {
    if (!theme.builtin && theme.data !== undefined) {
      monacoInstance.editor.defineTheme(theme.id, theme.data);
    }
  }
}
