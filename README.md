# Papo

Unofficial GTK client for WhatsApp, built with Rust. This project is in an early stage and under active development.

## What does "Papo" mean?

"Papo" is Brazilian Portuguese slang for "chat" or "conversation." It is commonly used in the expression "bater um papo," which means to have a chat.

## Install

### Nix

```sh
nix build github:AmanoTeam/Papo
nix run github:AmanoTeam/Papo
```

### Flatpak

```sh
flatpak-builder --install --user build build-aux/com.amanoteam.Papo.json
```

### Build from source

Requires Rust nightly, GTK4 (>= 4.20), libadwaita (>= 1.8), and Meson.

```sh
meson setup build
meson compile -C build
meson install -C build
```

## Roadmap

- [x] QR code login
- [x] Chat list
- [x] Message history
- [x] Bidirectional infinite scroll pagination
- [x] Go-to-bottom button
- [x] Read receipts
- [x] Local message storage (libSQL)
- [ ] Send messages
- [ ] Media messages (images, videos, documents)
- [ ] Voice messages
- [ ] Stickers and animated stickers
- [ ] Contact/chat info panel
- [ ] Notifications
- [ ] History sync (after pairing)
- [ ] Chat search
- [ ] Chat filters (unread, favorites, groups, non-contact)
- [ ] Chat functions (pin, mute, delete, archive)
- [ ] Chat admin functions (ban, change info/settings)
- [ ] Message search
- [ ] Message reactions
- [ ] Reply/quote messages
- [ ] Database encryption
- [ ] Online status indicators
- [ ] Typing indicators
- [ ] Profile pictures

## Translations

The project uses gettext for internationalization. Brazilian Portuguese (pt_BR) is currently supported. Translation files are located in the po/ directory, and contributions for new languages are welcome.

## Acknowledgements

- [relm4](https://github.com/Relm4/Relm4) — Idiomatic GUI framework for GTK4 in Rust
- [whatsapp-rust](https://github.com/jlucaso1/whatsapp-rust) — Rust implementation of the WhatsApp Web API

## Contributing

We accept pull requests from our [Forjego instance](https://git.amanoteam.com/AmanoTeam/Papo) and [GitHub](https://github.com/AmanoTeam/Papo). Fork either repository, create a feature branch, and submit a pull request. Bug reports and feature requests are also welcome via the issue tracker.

## License

Licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.

Author: Andriel Ferreira
