# Agent Guidelines for Papo

**Papo** is an unofficial GTK4/libadwaita WhatsApp client written in Rust using [Relm4](https://relm4.org/) 0.11. It targets GNOME 49 (libadwaita 1.8+, GTK 4.20+).

## Architecture

- **Framework**: Relm4 with async components (`AsyncComponent`). UI is declared in `view!` macros.
- **Pattern**: Components communicate via typed `Input`/`Output` messages. The root `Application` component orchestrates child components (`Login`, `ChatList`, `ChatView`, `Client`).
- **Async**: Heavy work (database, network) must be offloaded via `sender.oneshot_command()` or `relm4::spawn()`. Never block the main thread. Long batch work (sync storms, bulk populates) must keep input handlers short — batch through background readers and queue pumps, never loop DB awaits inside a component's own queue.
- **State**: Shared state lives in the `Application` model (contacts, push names, open chat, pending buffers). Chat/message state lives in `src/state/`.
- **Database**: toasty 0.10 (jiff/migration features) over SQLite with the `sqlcipher` feature. **Encryption comes from a vendored driver** — `vendor/toasty-driver-sqlite` adds `Sqlite::open_encrypted(path, hex_key)` (raw `PRAGMA key = "x'{hex}'"`, WAL, key verification). See the `[patch.crates-io]` entry in `Cargo.toml`. Migrations and queries live in `src/db/`. Keep every write serialized through the shared write lock.
- **WhatsApp API**: `whatsapp-rust` is a **git-pinned fork** (`oxidezap/whatsapp-rust`, rev in `Cargo.toml`, features `signal`/`tokio-*`/`ureq-client`), plus `wacore`/`waproto`. **Never upgrade it without a research round** — the library persists nothing contact-shaped for others, so papo must learn names/pins/mappings from events and sync payloads itself.

## Project Structure

```
src/
├── main.rs                  # Entry point: logger, i18n macros (incl. ni18n_f!), resource loading, RelmApp init
├── application.rs           # Root AsyncComponent: orchestrates pages, state machine, name/contact learning, sync arms
├── config.rs / config.rs.in # Build-time constants (APP_ID, VERSION, PROFILE, paths)
├── utils.rs                 # Shared helpers: jid predicates, bare_jid, name resolution (SenderResolution), date formatting, phone formatting, QR generation
│
├── components/              # Relm4 UI components (AsyncComponent/SimpleAsyncComponent)
│   ├── mod.rs               # Re-exports ChatList, ChatView, Login and their I/O types
│   ├── chat_list.rs         # Sidebar chat list: TypedListView rows, persistent filter state, sorted inserts, background populate + pump
│   ├── chat_view/           # Chat history with bidirectional infinite scroll, message input, read receipts
│   │   ├── history.rs       # ChatHistory: TimelineModel (custom SectionModel GObject), day sections, fill/prepend/append, reset-per-open
│   │   ├── mod.rs           # ChatView model, I/O enums, view!, update/update_cmd, header subtitle, on-demand fetch arms
│   │   └── rows.rs          # ChatRow enum, ChatRowWidgets, RelmListItem impl (setup/bind), sender labels/avatars
│   └── login.rs             # QR-code + phone-number pairing flow, pair-code cells
│
├── modals/                  # SimpleComponent dialogs launched from Application actions
│   ├── mod.rs
│   ├── about.rs             # AdwAboutDialog with app metadata
│   └── shortcuts.rs         # AdwShortcutsDialog with keyboard shortcuts
│
├── session/                 # WhatsApp client runtime, avatar system, caches
│   ├── mod.rs
│   ├── client.rs            # AsyncComponent wrapping whatsapp-rust Client (connection, sync, events, avatar fetches, metadata IQ)
│   ├── avatar.rs            # THE avatar system: disk cache (jid-keyed files), texture memory cache, decode gate (in-flight set + failure backoff + bounded semaphore), sender lookups
│   └── cache.rs             # RuntimeCache (in-memory Moka caches)
│
├── state/                   # Plain data models (no UI logic)
│   ├── mod.rs               # Re-exports Chat, ChatMessage, Media, MessageStatus
│   ├── chat.rs              # Chat struct with DB save/load, participants, unread helpers; ChatTypingState, TypingSender
│   ├── message.rs           # Message struct with status enum (Sent/Read/Delivered/etc.), reactions
│   └── media.rs             # Media attachment with download info, MIME type, dimensions
│
├── db/                      # Database layer (toasty + vendored sqlite driver)
│   ├── mod.rs
│   ├── connection.rs        # Database open (plain + encrypted), WAL, key verification
│   ├── entities/            # toasty models: Chat, Contact, Message, SessionMeta
│   ├── migrations.rs        # Schema migrations (toasty auto-migrates additive fields)
│   ├── keyring.rs           # Secret Service key storage
│   ├── media.rs             # Media blob storage
│   ├── protocol/            # ProtocolStore implementation backing wacore (signal stores, LID/PN mappings, tc tokens)
│   └── store.rs             # SessionStore: CRUD for chats/messages/contacts, lid mappings, unread helpers
│
└── widgets/                 # Custom GTK widgets reused in components
    ├── mod.rs               # Re-exports MessageTail, PairStep, PairingCell, TypingDots
    ├── message_tail.rs      # Cairo-drawn message bubble tail for grouping
    ├── pair_step.rs         # Single character cell for phone-number pair code
    ├── pairing_cell.rs      # Character display widget for pair code grid
    └── typing_dots.rs       # Animated typing indicator dots

vendor/                      # Vendored crates (currently toasty-driver-sqlite with sqlcipher support)
data/                        # Desktop/metainfo/gschema templates, icons, resources/stylesheet partials
po/                          # Gettext translations (pt_BR, de, es, fr, id; papo.pot; POTFILES.in)
build-aux/                   # Flatpak manifests (JSON) and dist-vendor script
```

## Code Quality — Non-Negotiable

The project enforces **extremely strict** linting. Code will fail CI if these are violated.

- **Clippy**: `#![deny(clippy::all)]`, `#![deny(clippy::cargo)]`, `#![deny(clippy::pedantic)]`, plus specific denied lints (see `src/main.rs`). **Do not suppress with `#![allow(...)]` or `#[allow(...)]` unless specifically marked as a FIXME/WIP.**
- **No `println!`/`eprintln!`**: Use the `tracing` crate (`tracing::info!`, `tracing::error!`, etc.). These are denied by clippy.
- **No dead code**: `#![allow(dead_code)]` exists temporarily for WIP features. Do not add new dead code.
- **Formatting**: `rustfmt`. A pre-commit hook is installed in development builds.
- **Rust edition**: 2024. Requires Rust nightly (see `rust-toolchain.toml`).

## Internationalization (i18n)

**Every user-facing string must be translatable.**

- Use the macros: `i18n!("Text")`, `i18n_f!("Hello {0}", name)`, `ni18n!("1 item", "{n} items", count)`, `ni18n_f!("1 item", "{0} items", count)` for plurals (defined in `main.rs`).
- Do not hardcode English strings in UI code.
- Translations live in `po/` (pt_BR, de, es, fr, id). `pt_BR` is the reference locale.
- **Template maintenance**: `po/meson.build` registers an extraction keyword for every i18n macro (`i18n!`, `i18n_f!:1`, `ni18n!:1,2`, `ni18n_f!:1,2`) — any new macro must be listed there. Keep `po/papo.pot` current when user-facing strings change, and `msgmerge` each locale against it (always `--backup=off` — the default drops `.po~` backups all over `po/`).

## WhatsApp Domain Rules

- **JIDs**: never test domains with inline `ends_with`. Use the `utils` predicates only: `is_lid_jid`, `is_phone_jid`, `is_group_jid`, `is_broadcast_jid`, `is_status_broadcast`. Bare-form comparisons go through `bare_jid`.
- **Identities**: a person may exist as a phone jid, a LID, or both. Anything keyed by a sender/chat jid must route through identity resolution (`lookup_name_in`, `resolve_stored_name`, `SenderResolution`, the LID↔PN mapping tables) — exact-match lookups silently drop data when the phone keys things by the other form.
- **Opening a chat means reading it**: on-demand fetches, backfill chunks landing on an open chat, and initial loads must not produce unread badges.
- **Push names** render as `~Name` (italic class on sender labels; avatar initials strip the tilde). Real contact names always win over push names; chat titles prefer real name, then the phone number (`format_lid_as_number`), and never a push name. Group titles keep their sync subject.
- **`status@broadcast` is not a chat**: it is suppressed at creation, at ingest, and in every fetch arm (`utils::is_status_broadcast`). A future status feed is the only surface for it.
- **No queue starvation**: metadata/participant learning, avatar fetches, and bulk saves run through background spawns with collected follow-ups (see `PushNamesLearned`, `PopulateAll`/`ApplyRows`, the avatar decode semaphore). Handler arms must return within a few ms.

## Git & Commit Protocol

- Commits: **English, semantic** (`fix(scope): …`, `feat(scope): …`, `chore: …`); one theme per commit; never mix unrelated hunks.
- **Nothing is committed before the user has tested and approved it.**
- The maintainer edits and stages in parallel: **re-read any file immediately before editing it**, and re-check `git status` before staging/committing.
- Never stage with directory-wide `git add` (`git add po/` sweeps tool backups like `.po~`) — add specific files.
- Commit splits on a mixed tree are done per-theme via hunk extraction (`git apply --cached` subsets), with each intermediate state compile-checked; verify the squashed diff is byte-identical to the original tree when done.

## Tooling Discipline

- **Never run `rg -rn`** — the `-r` flag *replaces matches*. Use `grep -rn` or `rg -n`.
- Shell `workdir:` parameters are unreliable — `cd ~/Projects/papo && …` explicitly in every command.
- Source files are edited with the editor tools only; no shell heredocs/`python` rewrites of source.
- Logs: `RUST_LOG=papo=debug papo 2>&1 | tee /tmp/papo.log` — tracing timestamps are UTC. Debug builds of GTK (for crash forensics) live in `~/.local/gtkdbg` (`LD_LIBRARY_PATH=~/.local/gtkdbg/lib`); core dumps need `ulimit -c unlimited` in the launching shell.
- The verification gate: `cargo fmt --all && cargo check && cargo clippy && cargo test` (0 warnings, all tests passing). The `#[ignore]`d GTK widget tests need a display: `cargo test -- --ignored`.

## Widget & Model Traps (hard-won)

- **`TimelineModel` (custom `SectionModel`)**: after every structural mutation, the section layout must be re-scanned (`sections_changed` spans covering the pre-mutation range *before* `items_changed`, plus a full-span rescan after) — GTK's insert dance mints pending `UNMATCHED_FOOTER` tiles mid-chain, and a tracker claim crossing one aborts in `gtk_list_item_manager_ensure_items`. Each chat open uses a **fresh model** attached with `set_model` (the only path that garbage-collects tile corpses).
- **Never `scroll_to` around splices** — ListBase anchors by item and re-anchoring fights the scroll.
- **Wrapping labels**: a label that wraps has an intrinsic minimum of the *whole* text unless `ellipsize` is set — set it, or the widget floors and never renders.
- **Titles**: single line, `ellipsize: end`, `single_line_mode`, `width_chars` floor (flare recipe). Long titles widen split views otherwise.
- **TypedListView/rows**: row data is authoritative for sort keys — derive keys from fresh data (e.g. the last message's own timestamp), never trust a column/struct copied at snapshot time. Guard recycled widgets with `set_widget_name` before async widget updates.
- **`items_changed(0, 0, 0)`** is illegal on a non-empty model (`n_items > 0` assert) and `sections_changed` needs valid ranges — respect both.

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
- If adding a system library, update the Flatpak manifests in `build-aux/com.amanoteam.Papo.json` (and `…Devel.json`).
- Keep Cargo deps sorted. The project uses `lto = true`, `panic = "abort"`, `strip = true`, `opt-level = "z"` for release.

## Data & Resources

- **App ID**: `com.amanoteam.Papo` (or `com.amanoteam.Papo.Devel` in development).
- **GSettings schema**: `data/com.amanoteam.Papo.gschema.xml.in`.
- **Desktop/metainfo**: `data/com.amanoteam.Papo.desktop.in.in` / `data/com.amanoteam.Papo.metainfo.xml.in.in`.
- **Icons**: Compiled via `relm4-icons-build`. Ship symbolic icons only.
- **Styles**: `data/resources/` SCSS partials compiled into a gresource (`style.scss`).
- **Data dir**: `~/.local/share/papo/` (accessed via `CACHE_DIR`/`DATA_DIR` statics).

## Testing & Debugging

- Build: `meson setup builddir && meson compile -C builddir`
- Run: `meson install -C builddir && papo` (installs `~/.local/bin/papo`)
- Flatpak: `flatpak-builder --install --user --install-deps-from=flathub build build-aux/com.amanoteam.Papo.json`

## Code Style & Patterns

- **Struct field ordering**: Fields are sorted first by name length (shortest first), then alphabetically for ties of equal length. Example:
  ```rust
  pub struct Chat {
      jid: String,                       // 3 chars
      name: String,                      // 4 chars
      muted: bool,                       // 5 chars
      pinned: bool,                      // 6 chars
      pinned_at: Option<i64>,            // 9 chars
      available: Option<bool>,           // 9 chars (alphabetical: a < p)
      avatar_path: Option<String>,       // 11 chars
      participants: HashMap<String, String>, // 12 chars
      last_message_time: Option<Timestamp>,  // 17 chars
  }
  ```
- **Imports**: Always import structs directly (e.g., `use crate::state::Chat;`). Only use fully-qualified names (`crate::state::Chat`) when there is a naming clash with another imported type. Use module aliases (e.g., `use waproto::whatsapp as wa;`) when the imported name conflicts with a local type.
- **Import groups**: Order imports as: (1) `std`, (2) external crates, (3) internal `crate::` imports. Keep each group separated by a blank line.
- **Collect**: Prefer `.collect::<Vec<_>>()` over `let v: Vec<_> = ...collect()`. Let the turbofish carry the type.
- **Documentation**: Document all public structs, enums, and functions with `///` doc comments. Keep docs clear and direct — describe what the thing does, not what it replaces. Do NOT reference implementation technologies (e.g. "libsql", "toasty") in prose. Use backticks for identifiers (`JID`, `local_id`, `FK`).

## Common Mistakes to Avoid

- Hardcoding strings without `i18n!()`, or writing `i18n_f!` for something with plurals (`ni18n_f!` exists for that).
- Blocking the main thread (or a component queue) with DB/network loops — batch to a background spawn and pump results back.
- Using `GtkSpinner`, `GtkShortcutsWindow`, or `.dim-label`.
- Adding non-symbolic/full-color icons to the UI.
- Forgetting to update `meson.build` or Flatpak manifest when adding system deps.
- Adding `allow` attributes to silence clippy without a WIP/FIXME justification.
- Adding struct fields in random order instead of length-then-alphabetical.
- Using `let v: Vec<_> = ...collect()` instead of `.collect::<Vec<_>>()`.
- Inline jid-domain `ends_with` checks instead of the `utils` predicates.
- Editing a file without a fresh read first (the maintainer edits in parallel).
- Trusting stale snapshots for order/display keys — derive from the freshest data available at render.
