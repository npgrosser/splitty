// The commands the command palette offers. Parts of the UI register what they
// can do while they are mounted; the palette lists and filters them.

export interface PaletteCommand {
  id: string;
  label: string;
  /** The key combo shown next to the label, if the command has one. */
  shortcut?: string;
  run: () => void;
}

let commands: PaletteCommand[] = [];

/** Offer `list` in the palette until the returned function is called. */
export function registerCommands(list: PaletteCommand[]): () => void {
  commands = [...commands, ...list];
  return () => {
    commands = commands.filter((command) => !list.includes(command));
  };
}

export function listCommands(): PaletteCommand[] {
  return commands;
}

/** The commands whose label contains every word of the query, in their order. */
export function filterCommands(list: PaletteCommand[], query: string): PaletteCommand[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return list.filter((command) => {
    const label = command.label.toLowerCase();
    return words.every((word) => label.includes(word));
  });
}
