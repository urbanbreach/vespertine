// Deployment configuration of Vespertine's own starter and package identities. The command's
// callers (and the environment, see cli.js) may override every value.

/** The package whose latest published version the new project depends on. */
export const ADAPTER_PACKAGE = "@vespertine/react";

/** Gzipped tarball of the starter repository (GitHub codeload layout: one root directory). */
export const STARTER_ARCHIVE_URL =
  "https://codeload.github.com/vespertine/starter/tar.gz/refs/heads/main";

/** Location of the starter project inside the archive, below the archive's root directory. */
export const STARTER_SUBTREE = "packages/starter";

/** Metadata of the adapter package's latest published version (npm registry layout). */
export const REGISTRY_URL = `https://registry.npmjs.org/${encodeURIComponent(ADAPTER_PACKAGE)}/latest`;
