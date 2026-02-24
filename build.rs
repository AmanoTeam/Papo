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
            "pin",
            "menu",
            "go-next",
            "speaker-0",
            "speaker-1",
            "speaker-2",
            "speaker-3",
            "speaker-4",
            "view-more",
            "image-round",
            "paper-plane",
            "info-outline",
            "chat-bubbles-text",
            "chat-bubbles-empty",
        ],
    );
}
