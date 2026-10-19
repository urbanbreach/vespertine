// Signed packaged-application updates (APP-004), implemented by the native addon
// (`vespertine_updater` through `vespertine_js_node`).

import { loadNative } from "./native.js";

/**
 * @typedef {object} UpdateOptions
 * @property {string[]} endpoints Feed URLs, tried in order; `{{current_version}}`, `{{target}}`
 *   and `{{arch}}` are substituted, github.com repository URLs use the latest-release API.
 * @property {string} pubkey Base64 text of the Minisign public-key file contents.
 * @property {{name: string, value: string}[]} [headers] Sent on feed, signature and package requests.
 * @property {number} [timeoutMs] Per-request timeout in milliseconds (none when absent).
 * @property {string[]} [installerArgs] Carried; no effect on Linux.
 * @property {"passive"|"quiet"|"basicUi"|"Passive"|"Quiet"|"BasicUi"} [installMode] Carried; no effect on Linux.
 */

/**
 * @typedef {object} AvailableUpdate
 * @property {string} currentVersion
 * @property {string} version
 * @property {string | undefined} notes
 * @property {string | undefined} date RFC 3339 timestamp.
 * @property {string} downloadUrl
 * @property {"app"|"appimage"|"nsis"|"wix"} format
 * @property {() => Promise<void>} downloadAndInstall Downloads, verifies the Minisign signature and
 *   installs the package. It never relaunches or quits the process.
 */

/**
 * Checks the release feeds for a version newer than `currentVersion` (a semantic version; leading
 * `v` characters are accepted). Resolves with an available update or `null`; invalid input and
 * failures reject with a human-readable message. Network and installation work runs off the
 * JavaScript thread.
 *
 * @param {string} currentVersion
 * @param {UpdateOptions} options
 * @returns {Promise<AvailableUpdate | null>}
 */
export async function checkUpdate(currentVersion, options) {
  const update = await loadNative().checkUpdate(currentVersion, options);
  return update ?? null;
}
