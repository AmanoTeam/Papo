# AGENTS.md - AI Agent Guide for Papo

## Quick Orientation

**Papo** is a GTK4/Libadwaita WhatsApp client built with Rust + Relm4.

- **Stack**: Rust (Edition 2024, nightly), GTK4, Libadwaita, Relm4 0.10, libSQL, Tokio
- **Build**: Meson (primary) + Cargo
- **App ID**: `com.amanoteam.Papo`

## Project Structure

```
src/
├── main.rs              # Entry point, i18n, logging setup
├── application.rs       # Root component, state management, message routing
├── components/          # Relm4 components
│   ├── chat_list.rs     # Sidebar chat list (AddChat, UpdateChat, Select)
│   ├── chat_view.rs     # Message view with bidirectional pagination
│   └── login.rs         # QR/phone login
├── session/
│   ├── client.rs        # WhatsApp client actor (ClientInput/ClientOutput)
│   └── cache.rs         # RuntimeCache for WhatsApp data
├── state/               # Data models (Chat, ChatMessage, Media)
├── store/               # Database layer (libSQL with AES256CBC encryption)
└── modals/              # Dialogs (about, shortcuts)
```

## Key Patterns

### Component Architecture (Relm4)
- Components implement `AsyncComponent` or `SimpleAsyncComponent`
- Communication via `Input`/`Output` enums, wired with `forward()`
- UI declared in `view!` macro with reactive `#[watch]` attributes
- See `components/chat_list.rs` for `SimpleAsyncComponent` example

### State Flow
```
WhatsApp events → Client::update() → ClientOutput → Application::update() → UI updates
UI actions → Component::Input → Component::Output → Application::Input → State changes
```

### Database
- `Arc<Database>` shared across components
- libSQL with AES256CBC encryption (key currently empty - TODO)
- Methods in `store/database.rs`, models in `state/`

### i18n
- **Mandatory**: Use `i18n!()`, `i18n_f!()`, `ni18n!()` for all user strings
- Never hardcode display strings

## Build & Test

```bash
# Development
meson setup _build
meson compile -C _build
meson install -C _build

# Flatpak
flatpak-builder --install --user build build-aux/com.amanoteam.Papo.json

# Nix
nix build github:AmanoTeam/Papo
nix run github:AmanoTeam/Papo

# Logs
RUST_LOG=papo=trace ./papo  # verbose WhatsApp protocol debugging
```

## Code Conventions

### Strict Linting (from main.rs)
```rust
#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![deny(clippy::use_self)]      // Use Self instead of type name
#![deny(clippy::print_stdout)]  // Use tracing!
#![deny(clippy::print_stderr)]
```

### Required Patterns
1. **Logging**: Use `tracing::info!()`, `tracing::error!()` - never println!
2. **Errors**: Use `tracing::error!()` and propagate; avoid unwrap in async
3. **Clone**: Avoid redundant clones (clippy enforces)
4. **Self**: Use `Self` when referring to own type

## Common Tasks

### Adding a Component
1. Create in `src/components/`, implement `SimpleAsyncComponent` or `AsyncComponent`
2. Define `Init`, `Input`, `Output`, `Widgets` types
3. Export in `components/mod.rs`
4. Wire in `application.rs` with `builder().launch().forward()`

### Adding Database Operations
1. Add SQL methods to `store/database.rs`
2. Add business methods to `state/chat.rs` or `state/message.rs`
3. Handle errors with `tracing::error!()`

### Handling WhatsApp Events
1. Add event handling in `session/client.rs` → emit `ClientOutput`
2. Handle in `application.rs` `update()` match arm
3. Update state, trigger UI via component inputs

### Adding UI Breakpoints
```rust
add_breakpoint = bp_with_setters(
    adw::Breakpoint::new(
        adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            600.0,
            adw::LengthUnit::Sp,
        )
    ),
    &[(&split_view, "collapsed", true)]
),
```

## Critical Implementation Details

### ChatView Pagination
- `INITIAL_LOAD_COUNT = 120` messages on open
- `LOAD_MORE_COUNT = 70` when scrolling
- `MAX_LOADED_ROWS = 600` - trims older rows when exceeded
- Uses `load_messages_before()` / `load_messages_after()`

### ChatList Selection
- Uses `suppress_selection: Rc<Cell<bool>>` to prevent signal loops
- Always set `suppress_selection.set(true)` before programmatic selection changes

### Presence Tracking
- Chat model has `available: Option<bool>` and `last_seen: Option<DateTime<Utc>>`
- Updated via `PresenceUpdate` from Client, displayed in header

### Client Capabilities
- Call handling: `StartCall`, `AcceptCall`, `DeclineCall`
- Typing indicators: `SendTyping`, `StopTyping`
- Read receipts: `MarkRead`

## Where to Look

| Task | File |
|------|------|
| WhatsApp connection logic | `src/session/client.rs` |
| Chat list UI/selection | `src/components/chat_list.rs` |
| Message view/pagination | `src/components/chat_view.rs` |
| Database schema/ops | `src/store/database.rs` |
| Chat data model | `src/state/chat.rs` |
| Message data model | `src/state/message.rs` |
| App state routing | `src/application.rs` |
| i18n macros | `src/main.rs` (macro definitions) |

## Roadmap Priorities

Current focus areas (check README.md for full list):
- Send messages (UI ready, backend pending)
- Media messages (images, videos)
- Notifications
- Message reactions
- Database encryption key management

## Troubleshooting

- **Build fails**: Ensure Rust nightly (`rust-toolchain.toml`), GTK4 >= 4.20, libadwaita >= 1.8
- **No UI updates**: Check you're sending to correct component input, verify `#[watch]` attributes
- **Database issues**: Located at `~/.local/share/papo/papo.db` (AES256CBC encrypted)
- **WhatsApp protocol issues**: Enable `RUST_LOG=papo=trace`
