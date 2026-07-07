<div align="center">

<img src="data/icons/com.amanoteam.Papo.svg" width="128" height="128" alt="Papo logo">

# Papo

Unofficial GTK client for WhatsApp, built with Rust.

[![Translation status](https://weblate.amanoteam.com/widgets/papo/-/svg-badge.svg)](https://weblate.amanoteam.com/engage/papo/)
[![Telegram](https://img.shields.io/badge/Telegram-AmanoChat-26A5E4?logo=telegram&logoColor=white)](https://t.me/AmanoChat)

</div>

> [!WARNING]
> This project is in an early stage and under active development.

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
flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --install --user --install-deps-from=flathub build build-aux/com.amanoteam.Papo.json
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
- [x] Phone number pairing (pair code)
- [x] Chat list
- [x] Message history
- [x] Bidirectional infinite scroll pagination
- [x] Go-to-bottom button
- [x] Read receipts
- [x] Send text messages
- [x] History sync (after pairing)
- [x] Local message storage (libSQL)
- [x] Chat filters (all, unread, groups)
- [x] Profile pictures (chat list)
- [x] Online status indicators (chat view subtitle)
- [x] Profile pictures
- [ ] Media messages (images, videos, documents)
- [ ] Voice messages
- [ ] Stickers and animated stickers
- [ ] Contact/chat info panel
- [ ] Notifications
- [ ] Chat search
- [ ] Chat filters (favorites, non-contact)
- [ ] Chat functions (pin, mute, delete, archive)
- [ ] Chat admin functions (ban, change info/settings)
- [ ] Message search
- [ ] Message reactions
- [ ] Reply/quote messages
- [ ] Database encryption
- [ ] Typing indicators

## Translations

The project uses gettext for internationalization. Brazilian Portuguese (pt_BR) is currently supported. Translation files are located in the po/ directory, and contributions for new languages are welcome.

## Disclaimer

Using custom WhatsApp clients may violate Meta's Terms of Service and could result in account suspension.

## Acknowledgements

- [relm4](https://github.com/Relm4/Relm4) — Idiomatic GUI framework for GTK4 in Rust
- [whatsapp-rust](https://github.com/jlucaso1/whatsapp-rust) — Rust implementation of the WhatsApp Web API

## Contributing

We accept pull requests from our [Forjego instance](https://git.amanoteam.com/AmanoTeam/Papo) and [GitHub](https://github.com/AmanoTeam/Papo). Fork either repository, create a feature branch, and submit a pull request. Bug reports and feature requests are also welcome via the issue tracker.

## License

Licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.

Author: Andriel Ferreira
