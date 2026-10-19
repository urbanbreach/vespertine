# create-vespertine

Creates a new Vespertine project from the published starter (DEV-009).

```sh
bunx create-vespertine new my-app   # or: node src/cli.js new my-app
create-vespertine new my-app --no-install
create-vespertine completion zsh
```

`new <target>` refuses an existing directory, downloads the starter archive and the latest
`@vespertine/react` metadata, copies the starter subtree, names the project after the
directory, pins the adapter to `^<latest>` and runs `bun install` (skip with `--no-install`).
`GITHUB_TOKEN` is sent as a bearer token on the archive request only.
`VESPERTINE_STARTER_ARCHIVE_URL` and `VESPERTINE_REGISTRY_URL` override the endpoints.

Programmatic use: `createProject({ targetDirectory, githubToken, fetch, install })`.
