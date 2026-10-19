# Welcome to VESPERTINE!

VESPERTINE is a hybrid immediate and retained mode, GPU accelerated, UI framework
for Rust, designed to support a wide variety of applications.

VESPERTINE targets Linux, macOS, and Windows desktops. Linux is the verified
platform for this fork; the upstream macOS and Windows backends are preserved.

## Getting Started

VESPERTINE is still in active development as we work on the Zed code editor, and is still pre-1.0. There will often be breaking changes between versions. You'll also need to use the latest version of stable Rust. Add `vespertine`, and optionally `vespertine_platform`, to your `Cargo.toml`:

```toml
vespertine = { version = "*" }
vespertine_platform = { version = "*", features = ["font-kit", "wayland", "x11"] }
```

Everything in a standalone VESPERTINE app starts with an `Application`. You can create one with `vespertine_platform::application()`, which picks the windowing and text backends for the host OS, and kick off your application by passing a callback to `Application::run()`. Inside this callback, you can create a new window with `App::open_window()` and register your first root view.

```rust,no_run
use vespertine::*;

fn main() {
    vespertine_platform::application().run(|cx: &mut App| {
        // ..
    });
}
```

### `vespertine_platform`

The features on `vespertine_platform` are platform-specific, so the list above is a safe cross-platform default. If you build for a single platform, you can trim it:

- **macOS** — Rendering uses Metal and is always available, but glyph rasterization needs `font-kit`. Without it, VESPERTINE falls back to a placeholder text system that lays text out but renders no glyphs.

    ```toml
    vespertine_platform = { version = "*", features = ["font-kit"] }
    ```

- **Linux / FreeBSD** — enable at least one windowing backend for desktop windows: `wayland`, `x11`, or both. These features also compile the renderer and text system, so no separate text feature is needed.

    ```toml
    vespertine_platform = { version = "*", features = ["wayland", "x11"] }
    ```

- **Windows** — no features are required. Windowing uses Win32 and text uses DirectWrite. `font-kit` has no effect here.

### Additional Topics

- [Ownership and data flow](_ownership_and_data_flow)
- [Accessibility](_accessibility)

### Dependencies

VESPERTINE has various system dependencies that it needs in order to work.

#### macOS

On macOS, VESPERTINE uses Metal for rendering. In order to use Metal, you need to do the following:

- Install [Xcode](https://apps.apple.com/us/app/xcode/id497799835?mt=12) from the macOS App Store, or from the [Apple Developer](https://developer.apple.com/download/all/) website. Note this requires a developer account.

> Ensure you launch Xcode after installing, and install the macOS components, which is the default option.

- Install [Xcode command line tools](https://developer.apple.com/xcode/resources/)

  ```sh
  xcode-select --install
  ```

- Ensure that the Xcode command line tools are using your newly installed copy of Xcode:

  ```sh
  sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
  ```

## The Big Picture

VESPERTINE offers three different [registers](<https://en.wikipedia.org/wiki/Register_(sociolinguistics)>) depending on your needs:

- State management and communication with `Entity`'s. Whenever you need to store application state that communicates between different parts of your application, you'll want to use VESPERTINE's entities. Entities are owned by VESPERTINE and are only accessible through an owned smart pointer similar to an `Rc`. See the `app::context` module for more information.

- High level, declarative UI with views. All UI in VESPERTINE starts with a view. A view is simply an `Entity` that can be rendered, by implementing the `Render` trait. At the start of each frame, VESPERTINE will call this render method on the root view of a given window. Views build a tree of `elements`, lay them out and style them with a tailwind-style API, and then give them to VESPERTINE to turn into pixels. See the `div` element for an all purpose swiss-army knife of rendering.

- Low level, imperative UI with Elements. Elements are the building blocks of UI in VESPERTINE, and they provide a nice wrapper around an imperative API that provides as much flexibility and control as you need. Elements have total control over how they and their child elements are rendered and can be used for making efficient views into large lists, implement custom layouting for a code editor, and anything else you can think of. See the `element` module for more information.

Each of these registers has one or more corresponding contexts that can be accessed from all VESPERTINE services. This context is your main interface to VESPERTINE, and is used extensively throughout the framework.

## Other Resources

In addition to the systems above, VESPERTINE provides a range of smaller services that are useful for building complex applications:

- Actions are user-defined structs that are used for converting keystrokes into logical operations in your UI. Use this for implementing keyboard shortcuts, such as cmd-q. See the `action` module for more information.

- Platform services, such as `quit the app` or `open a URL` are available as methods on the `app::App`.

- An async executor that is integrated with the platform's event loop. See the `executor` module for more information.,

- The `[vespertine::test]` macro provides a convenient way to write tests for your VESPERTINE applications. Tests also have their own kind of context, a `TestAppContext` which provides ways of simulating common platform input. See `app::test_context` and `test` modules for more details.

Currently, the best way to learn about these APIs is to read the Zed source code or drop a question in the [Zed Discord](https://zed.dev/community-links). We're working on improving the documentation, creating more examples, and will be publishing more guides to VESPERTINE on our [blog](https://zed.dev/blog).
