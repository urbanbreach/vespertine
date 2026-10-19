// The task list's data model (APP-011 R1, R2): views, membership, counts and edits.

export const VIEWS = Object.freeze(["general", "today", "starred", "complete"]);
export const VIEW_TITLES = Object.freeze({ general: "Tasks", today: "Today", starred: "Starred", complete: "Completed" });
export const DEFAULT_VIEW = "today";

/**
 * Eight initial tasks: two complete (current-day, not starred), two incomplete current-day of
 * which one is starred, one incomplete starred that is not current-day, three incomplete that are
 * neither. Counts: 6 general incomplete, 2 current-day, 2 starred, 2 complete.
 */
export function initialTasks() {
  const task = (id, title, { complete = false, starred = false, today = false } = {}) => ({ id, title, complete, starred, today });
  return [
    task(1, "Review the open questions", { complete: true, today: true }),
    task(2, "Send the weekly summary", { complete: true, today: true }),
    task(3, "Prepare the walkthrough", { today: true, starred: true }),
    task(4, "Update the checklist", { today: true }),
    task(5, "Plan the next milestone", { starred: true }),
    task(6, "Sort the reference notes"),
    task(7, "Archive the old drafts"),
    task(8, "Check the shared calendar"),
  ];
}

const MEMBERS = {
  general: (task) => !task.complete,
  today: (task) => task.today && !task.complete,
  starred: (task) => task.starred && !task.complete,
  complete: (task) => task.complete,
};

/** The tasks that belong to a view, in list order. */
export const tasksInView = (tasks, view) => tasks.filter(MEMBERS[view]);

/** The sidebar counts: one per view. */
export function counts(tasks) {
  return Object.fromEntries(VIEWS.map((view) => [view, tasksInView(tasks, view).length]));
}

/**
 * Creates a task: the title is trimmed, a title that is blank after trimming is rejected (the
 * list is returned unchanged), the new task is prepended incomplete and unstarred, and is marked
 * current-day unless it is created while the general view is selected.
 */
export function addTask(tasks, title, view, nextId) {
  const trimmed = title.trim();
  if (trimmed === "") return tasks;
  return [{ id: nextId, title: trimmed, complete: false, starred: false, today: view !== "general" }, ...tasks];
}

export const toggleComplete = (tasks, id) => tasks.map((task) => (task.id === id ? { ...task, complete: !task.complete } : task));
export const toggleStar = (tasks, id) => tasks.map((task) => (task.id === id ? { ...task, starred: !task.starred } : task));
export const deleteTask = (tasks, id) => tasks.filter((task) => task.id !== id);
