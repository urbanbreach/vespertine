// The filterable task list with sidebar, header and composer (APP-011 R1-R3).

import { useWindowSize } from "@vespertine/react";
import { React, h } from "../h.js";
import { palette } from "../theme.js";
import { DEFAULT_VIEW, VIEWS, VIEW_TITLES, addTask, counts, deleteTask, initialTasks, tasksInView, toggleComplete, toggleStar } from "./model.js";

export const SIDEBAR_WIDTH = 236;
export const HEADER_HEIGHT = 52;
export const ROW_HEIGHT = 48;
export const COMPOSER_HEIGHT = 46;
export const CONTENT_MAX_WIDTH = 640;
/** The top clearance the sidebar keeps for window controls on Linux. */
export const TOP_CLEARANCE = 20;
export const COLLAPSE_SECONDS = 0.2;

const colors = palette();

function Row({ task, hovered, onHover, onToggle, onStar, onDelete }) {
  const hoverHandlers = { onEnter: () => onHover(task.id), onLeave: () => onHover(null) };
  return h(
    "div",
    {
      testId: `task-row-${task.id}`,
      ...hoverHandlers,
      style: {
        width: "100%",
        height: ROW_HEIGHT,
        flexDirection: "row",
        alignItems: "center",
        gap: 10,
        paddingLeft: 12,
        paddingRight: 12,
        backgroundColor: hovered ? colors.hover : colors.surface,
        borderColor: colors.border,
        borderWidth: 1,
      },
    },
    h(
      "div",
      {
        testId: `task-check-${task.id}`,
        role: "checkbox",
        "aria-label": `Complete ${task.title}`,
        "aria-selected": task.complete,
        onClick: () => onToggle(task.id),
        style: { width: 16, height: 16, borderRadius: 8, borderWidth: 2, borderColor: colors.textSecondary, backgroundColor: task.complete ? colors.accent : colors.surface },
      },
    ),
    h("text", { testId: `task-title-${task.id}`, style: { flexGrow: 1, fontSize: 14, color: task.complete ? colors.textTertiary : colors.text } }, task.title),
    // Hovering reveals the interactive star and delete controls; the interactive star replaces
    // an already-shown static star indicator instead of duplicating it.
    task.starred && !hovered
      ? h("text", { testId: `task-star-static-${task.id}`, style: { fontSize: 14, color: colors.accent } }, "\u2605")
      : null,
    hovered
      ? h(
          "div",
          { testId: `task-star-${task.id}`, role: "button", "aria-label": task.starred ? "Remove star" : "Add star", onClick: () => onStar(task.id), style: { width: 24, height: 24, alignItems: "center", justifyContent: "center" } },
          h("text", { style: { fontSize: 14, color: task.starred ? colors.accent : colors.textSecondary } }, task.starred ? "\u2605" : "\u2606"),
        )
      : null,
    hovered
      ? h(
          "div",
          { testId: `task-delete-${task.id}`, role: "button", "aria-label": "Delete task", onClick: () => onDelete(task.id), style: { width: 24, height: 24, alignItems: "center", justifyContent: "center" } },
          h("text", { style: { fontSize: 14, color: colors.danger } }, "\u00d7"),
        )
      : null,
  );
}

/**
 * @param {{ width?: number, height?: number, initial?: object[] }} props explicit sizes keep
 *   automated input coordinates independent of live window geometry
 */
export function TaskList({ width, height, tasks: initial }) {
  const windowSize = useWindowSize();
  const totalWidth = width ?? windowSize.width;
  const totalHeight = height ?? windowSize.height;
  const [tasks, setTasks] = React.useState(() => initial ?? initialTasks());
  const [view, setView] = React.useState(DEFAULT_VIEW);
  const [draft, setDraft] = React.useState("");
  const [hovered, setHovered] = React.useState(null);
  const [collapsed, setCollapsed] = React.useState(false);
  const nextId = React.useRef(Math.max(0, ...(initial ?? initialTasks()).map((task) => task.id)) + 1);

  const members = tasksInView(tasks, view);
  const tally = counts(tasks);
  const blank = draft.trim() === "";

  const submit = () => {
    if (draft.trim() === "") return;
    setTasks((current) => addTask(current, draft, view, nextId.current++));
    setDraft("");
  };

  const sidebarTarget = collapsed ? 0 : SIDEBAR_WIDTH;
  return h(
    "div",
    { testId: "task-list-root", style: { width: totalWidth, height: totalHeight, flexDirection: "row", backgroundColor: colors.surface } },
    // Only the outer clipping container animates; the inner sidebar keeps its fixed width so its
    // text never reflows. The first frame is already at the target width (no initial animation).
    h(
      "div",
      {
        testId: "sidebar-outer",
        motion: { initial: false, animate: { width: sidebarTarget }, transition: { duration: COLLAPSE_SECONDS, ease: "easeOut" } },
        style: { width: sidebarTarget, height: "100%", overflow: "hidden", flexShrink: 0 },
      },
      h(
        "div",
        { testId: "sidebar-inner", style: { width: SIDEBAR_WIDTH, height: "100%", flexShrink: 0, flexDirection: "column", paddingTop: TOP_CLEARANCE, backgroundColor: colors.sidebar, borderColor: colors.border, borderWidth: 1 } },
        VIEWS.map((name) =>
          h(
            "div",
            {
              key: name,
              testId: `view-${name}`,
              role: "button",
              "aria-selected": name === view,
              onClick: () => setView(name),
              style: { height: 36, flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingLeft: 16, paddingRight: 16, backgroundColor: name === view ? colors.hover : colors.sidebar },
            },
            h("text", { style: { fontSize: 14, color: name === view ? colors.text : colors.textSecondary } }, VIEW_TITLES[name]),
            h("text", { testId: `count-${name}`, style: { fontSize: 12, color: colors.textTertiary } }, String(tally[name])),
          ),
        ),
      ),
    ),
    h(
      "div",
      { testId: "main", style: { flexGrow: 1, flexShrink: 1, minWidth: 0, height: "100%", flexDirection: "column", alignItems: "center" } },
      h(
        "div",
        { style: { width: "100%", maxWidth: CONTENT_MAX_WIDTH, height: "100%", flexDirection: "column" } },
        h(
          "div",
          { testId: "header", style: { height: HEADER_HEIGHT, flexShrink: 0, flexDirection: "row", alignItems: "center", gap: 12, paddingLeft: 12, paddingRight: 12 } },
          h("div", { testId: "sidebar-toggle", role: "button", "aria-label": "Toggle sidebar", onClick: () => setCollapsed((value) => !value), style: { width: 20, height: 20, backgroundColor: colors.raised, borderRadius: 4 } }),
          h("text", { testId: "header-title", style: { fontSize: 20, fontWeight: 700, color: colors.text } }, VIEW_TITLES[view]),
          h("text", { testId: "header-count", style: { fontSize: 14, color: colors.textSecondary } }, String(members.length)),
        ),
        members.length === 0
          ? h("div", { testId: "empty-state", style: { flexGrow: 1, alignItems: "center", justifyContent: "center" } }, h("text", { style: { fontSize: 14, color: colors.textTertiary } }, "Nothing here yet"))
          : h(
              "virtual-list",
              { testId: "task-list", estimatedItemHeight: ROW_HEIGHT, alignment: "top", style: { flexGrow: 1, flexShrink: 1, minHeight: 0 } },
              members.map((task) =>
                h(Row, {
                  key: task.id,
                  task,
                  hovered: hovered === task.id,
                  onHover: setHovered,
                  onToggle: (id) => setTasks((current) => toggleComplete(current, id)),
                  onStar: (id) => setTasks((current) => toggleStar(current, id)),
                  onDelete: (id) => setTasks((current) => deleteTask(current, id)),
                }),
              ),
            ),
        h(
          "div",
          { testId: "composer", style: { height: COMPOSER_HEIGHT, flexShrink: 0, flexDirection: "row", alignItems: "center", gap: 8, paddingLeft: 12, paddingRight: 12 } },
          h("input", {
            testId: "composer-input",
            value: draft,
            placeholder: "Add a task",
            onChange: (event) => setDraft(event.value),
            onSubmit: submit,
            style: { flexGrow: 1, height: 30, fontSize: 14, color: colors.text, backgroundColor: colors.raised, borderRadius: 4, borderColor: colors.border, borderWidth: 1, paddingLeft: 8 },
          }),
          h(
            "div",
            {
              testId: "composer-add",
              role: "button",
              "aria-label": "Add",
              // Disabled: out of the tab order and without an activation handler.
              tabIndex: blank ? -1 : 0,
              onClick: blank ? undefined : submit,
              style: { height: 30, paddingLeft: 14, paddingRight: 14, alignItems: "center", justifyContent: "center", borderRadius: 4, backgroundColor: blank ? colors.raised : colors.accent },
            },
            h("text", { style: { fontSize: 14, color: blank ? colors.textTertiary : colors.text } }, "Add"),
          ),
        ),
      ),
    ),
  );
}
