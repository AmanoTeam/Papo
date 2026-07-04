# Agent Guidelines for Papo

**Papo** is an unofficial GTK4/libadwaita WhatsApp client written in Rust using [Relm4](https://relm4.org/) 0.11. It targets GNOME 49 (libadwaita 1.8+, GTK 4.20+).

## Architecture

- **Framework**: Relm4 with async components (`AsyncComponent`). UI is declared in `view!` macros.
- **Pattern**: Components communicate via typed `Input`/`Output` messages. The root `Application` component orchestrates child components (`Login`, `ChatList`, `ChatView`, `Client`).
- **Async**: Heavy work (database, network) must be offloaded via `sender.oneshot_command()` or `relm4::spawn()`. Never block the main thread.
- **State**: Shared state uses `Arc<Database>` and `Arc<RuntimeCache>`. Chat/message state lives in `src/state/`.
- **Database**: libsql (with encryption feature). Migrations and queries in `src/store/`.
- **WhatsApp API**: `wacore`/`waproto`/`whatsapp-rust` crates (from crates.io, v0.5).

## Project Structure

```
src/
├── main.rs                  # Entry point: logger, i18n, resource loading, RelmApp init
├── application.rs           # Root AsyncComponent: orchestrates pages, state machine, action wiring
├── config.rs / config.rs.in # Build-time constants (APP_ID, VERSION, PROFILE, paths)
├── utils.rs                 # Shared helpers: QR generation, date formatting, phone number parsing
│
├── components/              # Relm4 UI components (AsyncComponent/SimpleAsyncComponent)
│   ├── mod.rs               # Re-exports ChatList, ChatView, Login and their I/O types
│   ├── chat_list.rs         # Sidebar chat list with AdwToggleGroup filters, TypedListView rows
│   ├── chat_view.rs         # Chat history with bidirectional infinite scroll, message input, read receipts
│   └── login.rs             # QR-code + phone-number pairing flow, pair-code cells
│
├── modals/                  # SimpleComponent dialogs launched from Application actions
│   ├── mod.rs
│   ├── about.rs             # AdwAboutDialog with app metadata
│   └── shortcuts.rs         # AdwShortcutsDialog with keyboard shortcuts
│
├── session/                 # WhatsApp client runtime and caches
│   ├── mod.rs
│   ├── client.rs            # AsyncComponent wrapping whatsapp-rust Client (connection, sync, events)
│   └── cache.rs             # AvatarCache (disk) and RuntimeCache (in-memory Moka caches)
│
├── state/                   # Plain data models (no UI logic)
│   ├── mod.rs               # Re-exports Chat, ChatMessage, Media, MessageStatus
│   ├── chat.rs              # Chat struct with DB save/load, participants, unread helpers
│   ├── message.rs           # Message struct with status enum (Sent/Read/Delivered/etc.), reactions
│   └── media.rs             # Media attachment with download info, MIME type, dimensions
│
├── store/                   # Database layer (libsql with encryption)
│   ├── mod.rs               # Re-exports Database, Contact
│   └── database.rs          # Schema creation, CRUD for chats/messages/contacts, search queries
│
└── widgets/                 # Custom GTK widgets reused in components
    ├── mod.rs               # Re-exports PairStep, PairingCell
    ├── pair_step.rs         # Single character cell for phone-number pair code
    └── pairing_cell.rs      # Character display widget for pair code grid

data/
├── com.amanoteam.Papo.desktop.in.in      # Desktop entry template
├── com.amanoteam.Papo.metainfo.xml.in.in # AppStream metadata template
├── com.amanoteam.Papo.gschema.xml.in     # GSettings schema (preferences)
├── com.amanoteam.Papo.service.in         # D-Bus service file
├── icons/                                # App icon (SVG, symbolic)
└── resources/
    ├── style.scss                        # Compiled SCSS → CSS gresource
    └── stylesheet/                       # Partial SCSS files (chat_list, chat_view, login, etc.)

po/                        # Gettext translations (pt_BR.po, POTFILES.in, LINGUAS)
build-aux/                 # Flatpak manifests (JSON) and dist-vendor script
```

## Code Quality — Non-Negotiable

The project enforces **extremely strict** linting. Code will fail CI if these are violated.

- **Clippy**: `#![deny(clippy::all)]`, `#![deny(clippy::cargo)]`, `#![deny(clippy::pedantic)]`, plus specific denied lints (see `src/main.rs` lines 5–21). **Do not suppress with `#![allow(...)]` or `#[allow(...)]` unless specifically marked as a FIXME/WIP.**
- **No `println!`/`eprintln!`**: Use the `tracing` crate (`tracing::info!`, `tracing::error!`, etc.). These are denied by clippy.
- **No dead code**: `#![allow(dead_code)]` exists temporarily for WIP features. Do not add new dead code.
- **Formatting**: `rustfmt`. A pre-commit hook is installed in development builds.
- **Rust edition**: 2024. Requires Rust nightly for certain dependencies.

## Internationalization (i18n)

**Every user-facing string must be translatable.**

- Use the macros: `i18n!("Text")`, `i18n_f!("Hello {0}", name)`, `ni18n!("1 item", "{n} items", count)`.
- Do not hardcode English strings in UI code.
- Translations live in `po/`. Brazilian Portuguese is currently supported.

## UI / HIG Rules

- **Container**: `adw::ApplicationWindow` + `adw::HeaderBar` for main windows.
- **Settings**: `AdwPreferencesWindow` with `AdwPreferencesGroup` rows.
- **Icons**: Symbolic only (e.g., `list-add-symbolic`, `chat-bubbles-text-symbolic`).
- **New widgets** (libadwaita 1.6–1.8):
  - Use `AdwSpinner` instead of `GtkSpinner`.
  - Use `AdwToggleGroup` for exclusive toggles.
  - Use `AdwShortcutsDialog` instead of `GtkShortcutsWindow`.
  - Use `.dimmed` CSS class instead of `.dim-label`.
- **Feedback**: `AdwToast` for transient actions, `AdwBanner` for persistent states, `AdwDialog` for decisions.
- **Actions**: Define with Relm4 macros: `relm4::new_action_group!`, `relm4::new_stateless_action!`.
- **Responsive**: Use `AdwBreakpointBin` + `adw::Breakpoint` (see `application.rs` for the collapsed sidebar pattern).
- **Empty states**: Provide `AdwStatusPage` with icon, title, description, and action button.

## Async & Threading

- **GTK is single-threaded.** All UI mutations happen on the main thread via Relm4's message loop.
- **Database/network**: Use `sender.oneshot_command(async { ... })` for Relm4 commands, `relm4::spawn(async move { ... })` for fire-and-forget async tasks, or `relm4::spawn_blocking(move || { ... })` for CPU-bound or blocking work.
- **Tokio**: Available for the WhatsApp client runtime (`tokio::time::sleep`, etc.). **Never use `tokio::spawn` or `tokio::task::spawn_blocking`** — always prefer the `relm4` equivalents so tasks run on the correct runtime and do not violate GTK thread assumptions.
- Do not use `std::thread::sleep` on the main thread.

## Adding a New Component

Follow the existing structure in `src/components/`:

1. Define `Model` (component state), `Input` (messages in), `Output` (messages out).
2. Implement `AsyncComponent` with `#[relm4::component(async, pub)]`.
3. Declare UI in the `view!` macro using GTK4/libadwaita widgets.
4. Wire into `Application` (`src/application.rs`): add to model, instantiate with `.builder().launch(...).forward(...)`.
5. Export from `src/components/mod.rs`.

## Adding New Dependencies

- Cargo deps go in `Cargo.toml`.
- System deps (GTK, libadwaita, OpenSSL, etc.) are declared in `meson.build`.
- If adding a system library, update the Flatpak manifest in `build-aux/com.amanoteam.Papo.json`.
- Keep Cargo deps sorted. The project uses `lto = true`, `panic = "abort"`, `strip = true`, `opt-level = "z"` for release.

## Data & Resources

- **App ID**: `com.amanoteam.Papo` (or `com.amanoteam.Papo.Devel` in development).
- **GSettings schema**: `data/com.amanoteam.Papo.gschema.xml.in`.
- **Desktop/metainfo**: `data/com.amanoteam.Papo.desktop.in.in` / `data/com.amanoteam.Papo.metainfo.xml.in.in`.
- **Icons**: Compiled via `relm4-icons-build`. Ship symbolic icons only.
- **Styles**: `data/resources/style.css`. Loaded as a gresource in `main.rs`.
- **Data dir**: `~/.local/share/papo/` (accessed via `DATA_DIR` static).

## Testing & Debugging

- Build: `meson setup build && meson compile -C build`
- Run: `meson compile -C build && ./build/src/papo` (or `GTK_DEBUG=interactive` for inspector)
- Flatpak: `flatpak-builder --install --user --install-deps-from=flathub build build-aux/com.amanoteam.Papo.json`

## Code Style & Patterns

- **Struct field ordering**: Fields are sorted first by name length (shortest first), then alphabetically for ties of equal length. Example:
  ```rust
  pub struct Chat {
      db: Arc<Database>,          // 2 chars
      jid: String,                // 3 chars
      name: String,               // 4 chars
      muted: bool,                // 5 chars
      pinned: bool,               // 6 chars
      available: Option<bool>,    // 9 chars
      last_seen: Option<DateTime<Utc>>, // 9 chars (alphabetical: a < l)
      avatar_path: Option<String>, // 11 chars
      participants: HashMap<String, String>, // 12 chars
      last_message_time: DateTime<Utc>, // 17 chars
  }
  ```
- **Imports**: Always import structs directly (e.g., `use crate::state::Chat;`). Only use fully-qualified names (`crate::state::Chat`) when there is a naming clash with another imported type. Use module aliases (e.g., `use waproto::whatsapp as wa;`) when the imported name conflicts with a local type.
- **Import groups**: Order imports as: (1) `std`, (2) external crates, (3) internal `crate::` imports. Keep each group separated by a blank line.

## Common Mistakes to Avoid

- Hardcoding strings without `i18n!()`.
- Blocking the main thread with DB or network calls.
- Using `GtkSpinner`, `GtkShortcutsWindow`, or `.dim-label`.
- Adding non-symbolic/full-color icons to the UI.
- Forgetting to update `meson.build` or Flatpak manifest when adding system deps.
- Adding `allow` attributes to silence clippy without a WIP/FIXME justification.
- Adding struct fields in random order instead of length-then-alphabetical.
