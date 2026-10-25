fn main() {
    relm4_icons_build::bundle_icons(
        // Name of the file that will be generated at `OUT_DIR`
        "icon_names.rs",
        // Optional app ID
        Some("com.amanoteam.Papo"),
        // Custom base resource path:
        // * defaults to `/com/example/myapp` in this case if not specified explicitly
        // * or `/org/relm4` if app ID was not specified either
        None::<&str>,
        // Directory with custom icons (if any)
        None::<&str>,
        // List of icons to include
        [
            "menu",
            "go-next",
            "view-pin",
            "clock-alt",
            "view-more",
            "down-small",
            "check-plain",
            "image-round",
            "paper-plane",
            "info-outline",
            "speaker-cross",
            "phonelink-setup",
            "exclamation-mark",
            "chat-bubbles-text",
            "chat-bubbles-empty",
            "phone-right-facing",
            "arrows-bidirectional",
            "check-round-outline",
            "check-round-outline2",
        ],
    );
}
