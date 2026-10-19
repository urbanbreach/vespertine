# @vespertine/dev

Native development workflows for Vespertine (DEV-009). Linux-hosted; Node >= 20 or Bun.

## `vespertine-dev`

Watches the Rust sources (`<root>/crates/**/*.rs`, recursively), waits for a 120 ms quiet
period after the last change, and rebuilds. The first build runs at start; rebuilds are
serialized with at most one queued while a build runs. A failed build leaves the running
application untouched.

```sh
node src/cli.js --app <example>        # restart the cargo example after every successful build
node src/cli.js --shots [pattern]      # run `cargo test <pattern>` (default: showcase)
```

Options: `-p/--package <crate>`, `--watch <dir>` (repeatable), `--root <dir>`. The cargo
program is `$CARGO` or `cargo`. SIGINT/SIGTERM terminate the application (SIGTERM) before the
watcher exits. The scheduling core is `createRebuildScheduler({ clock, build, onBuilt })`.

## `vespertine-screenshots`

Runs the curated jobs of `screenshots.config.json` one after another (stopping at the first
failing job or missing image) and writes each image with ImageMagick (`-strip`, 256 colors,
PNG compression level 9). Without ImageMagick, or if it fails, the original is copied and
the fact is logged.

```sh
node src/screenshots-cli.js [--config <file>] [--root <dir>] [--destination <dir>]
```
