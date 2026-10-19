// Neutral styling for the examples: every color is a role of one palette (surface, raised
// surface, sidebar surface, border, primary/secondary/tertiary text, accent, danger, hover and
// the snap-guide color), never an application-specific palette. Real applications resolve the
// roles from the active theme; the defaults here are the dark base.

export const ROLES = Object.freeze({
  surface: "#16181d",
  raised: "#1f2229",
  sidebar: "#1a1d23",
  border: "#2c313a",
  text: "#e6e8ec",
  textSecondary: "#a3a9b5",
  textTertiary: "#6f7684",
  accent: "#4c8dff",
  danger: "#e5484d",
  hover: "#272b34",
  guide: "#f2c14e",
});

/** The palette for the current appearance (overridable per call). */
export function palette(overrides) {
  return overrides ? { ...ROLES, ...overrides } : ROLES;
}
