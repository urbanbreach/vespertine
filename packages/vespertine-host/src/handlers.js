// Event handler map keyed by element identity, then event type (APP-007 R4).

/** @returns {Map<number, Map<string, Function>>} */
export function createHandlerMap() {
  return new Map();
}

export function registerEventHandler(map, id, type, handler) {
  let byType = map.get(id);
  if (!byType) {
    byType = new Map();
    map.set(id, byType);
  }
  byType.set(type, handler);
}

export function unregisterEventHandler(map, id, type) {
  const byType = map.get(id);
  if (!byType) return;
  byType.delete(type);
  if (byType.size === 0) map.delete(id);
}

export function unregisterEventHandlers(map, id) {
  map.delete(id);
}
