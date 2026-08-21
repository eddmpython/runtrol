/// The grid several conversation tabs are spread into, as VS Code's own editor layout.
///
/// One command, one screen full of agents: the tabs are distributed over editor groups arranged as close
/// to square as they come (two side by side, then two by two, then three by two, three by three). VS Code
/// draws, sizes and lets the operator drag the groups afterwards; this only names the shape and which
/// column each conversation goes to. Nine is the most the editor addresses by column, so nine is the bound;
/// a tenth tab stays where it was and the command says so.

/// The most editor groups VS Code addresses by `ViewColumn` number.
export const MAX_GRID_CELLS = 9;

/// An editor layout as `vscode.setEditorLayout` takes it: columns side by side, each stacked into rows.
export type EditorLayout = {
  orientation: 0;
  groups: Array<{ groups: Array<Record<never, never>> }>;
};

/// How many tabs a grid of `count` conversations holds per column, left to right.
///
/// As many columns as the square root rounds up to; rows distributed so the leftmost columns are the taller
/// ones, which is where the eye starts.
export function gridColumns(count: number): number[] {
  const cells = Math.max(0, Math.min(MAX_GRID_CELLS, Math.floor(count)));
  if (cells === 0) return [];
  const columns = Math.ceil(Math.sqrt(cells));
  const base = Math.floor(cells / columns);
  const extra = cells % columns;
  return Array.from({ length: columns }, (_column, index) => base + (index < extra ? 1 : 0));
}

/// The editor layout for that many conversations.
export function gridLayout(count: number): EditorLayout {
  return {
    orientation: 0,
    groups: gridColumns(count).map((rows) => ({
      groups: Array.from({ length: rows }, () => ({})),
    })),
  };
}

/// The editor column (1-based, column-major as VS Code numbers groups) each of the first `count` tabs goes
/// to. The numbers follow the layout above: column one's rows first, then column two's.
export function gridCells(count: number): number[] {
  const cells = gridColumns(count).reduce((sum, rows) => sum + rows, 0);
  return Array.from({ length: cells }, (_cell, index) => index + 1);
}
