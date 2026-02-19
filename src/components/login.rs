use std::{fmt, time::Duration};

use adw::prelude::*;
use futures_util::FutureExt;
use gtk::{gdk, glib, pango};
use image::{ExtendedColorType, ImageEncoder, Luma, codecs::png::PngEncoder};
use qrcode::QrCode;
use relm4::{component::Connector, prelude::*};
use relm4_components::alert::{Alert, AlertMsg, AlertResponse, AlertSettings};
use rlibphonenumber::{PhoneNumber, PhoneNumberFormat};
use strum::{AsRefStr, EnumString};
use tokio::time::{self, Instant};

use crate::i18n;

pub struct Login {
    state: LoginState,
    qr_code: Option<gdk::Paintable>,
    bottom_page: LoginBottomPage,
    error_dialog: Connector<Alert>,
    reset_dialog: Controller<Alert>,
    phone_number_entry: gtk::Entry,
}

#[derive(AsRefStr, Clone, Copy, Debug, EnumString)]
#[strum(serialize_all = "kebab-case")]
enum LoginBottomPage {
    /// Confirm code view.
    ConfirmCode,
    /// Enter phone number view.
    EnterPhoneNumber,
}

#[derive(Clone, Default)]
struct LoginState {
    code: Option<[char; 8]>,
    paired: bool,
    qr_code: Option<QrCode>,
    scan_attempts: u8,
    progress_fraction: f64,
    valid_phone_number: bool,
    session_scan_expired: bool,
    phone_number_country_emoji: Option<String>,
}

impl fmt::Debug for LoginState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginState")
            .field("code", &self.code)
            .field("paired", &self.paired)
            .field("scan_attempts", &self.scan_attempts)
            .field("progress_fraction", &self.progress_fraction)
            .field("valid_phone_number", &self.valid_phone_number)
            .field("session_scan_expired", &self.session_scan_expired)
            .field("phone_number_country_emoji", &self.phone_number_country_emoji)
            .finish()
    }
}

#[derive(Debug)]
pub enum LoginInput {
    /// Request to reset the session.
    ResetRequest,

    /// 8-character pairing code received.
    PairCode {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    /// Client has paired successfully.
    PairSuccess,

    /// Error occurred.
    Error { message: String },
}

#[derive(Debug)]
pub enum LoginOutput {
    /// Reset the session to able to receive new qr codes.
    ResetSession,

    /// Request the session to pair with a phone number.
    PairWithPhoneNumber { phone_number: String },
}

#[derive(Debug)]
pub enum LoginCommand {
    /// Reset the session to able to receive new qr codes.
    ResetSession,

    /// Update the QR Code.
    UpdateQrCode { data: String, timeout: Duration },
    /// QR code expired by timeout.
    QrCodeExpired,
    /// Update the expiration bar.
    UpdateExpirationBar(f32),

    /// Validate the phone number.
    ValidatePhoneNumber,

    /// Ignore the command.
    Ignore,
}

#[relm4::component(async, pub)]
impl AsyncComponent for Login {
    type Init = ();
    type Input = LoginInput;
    type Output = LoginOutput;
    type CommandOutput = LoginCommand;

    view! {
        &adw::ToolbarView {
            set_css_classes: &["login-page"],

            add_top_bar = &adw::HeaderBar {
                pack_end = &gtk::Button {
                    set_icon_name: "info-outline-symbolic",
                    set_action_name: Some("win.about"),
                    set_tooltip_text: Some(&i18n!("About Papo")),
                }
            },

            #[wrap(Some)]
            set_content = &adw::StatusPage {
                set_title: &i18n!("Link your phone"),
                set_vexpand: true,

                gtk::Box {
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Center,
                    set_spacing: 15,
                    set_orientation: gtk::Orientation::Vertical,

                    gtk::Box {
                        set_spacing: 15,
                        set_orientation: gtk::Orientation::Vertical,

                        gtk::Label {
                            set_label: &i18n!("Open WhatsApp on your phone and scan the QR code."),
                            set_justify: gtk::Justification::Center,
                            set_css_classes: &["body"]
                        },

                        gtk::Overlay {
                            #[wrap(Some)]
                            set_child = &gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::Center,
                                set_hexpand: true,
                                set_vexpand: true,
                                set_css_classes: &["card"],
                                set_width_request: 200,
                                set_height_request: 200,

                                gtk::Image {
                                    set_halign: gtk::Align::Center,
                                    set_valign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_vexpand: true,
                                    #[watch]
                                    set_paintable: model.qr_code.as_ref(),
                                    set_pixel_size: 180,
                                }
                            },

                            add_overlay = &gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::Center,
                                set_hexpand: true,
                                set_vexpand: true,
                                set_opacity: 0.96,
                                #[watch]
                                set_visible: model.state.qr_code.is_none(),
                                set_css_classes: &["card", "view"],
                                set_width_request: 200,
                                set_height_request: 200,

                                gtk::Box {
                                    set_halign: gtk::Align::Center,
                                    set_valign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_vexpand: true,
                                    set_spacing: 20,
                                    set_orientation: gtk::Orientation::Vertical,

                                    gtk::Label {
                                        #[watch]
                                        set_label: &if model.state.session_scan_expired {
                                            i18n!("All QR codes for this session were expired.")
                                        } else {
                                            i18n!("Waiting QR code...")
                                        },
                                        set_justify: gtk::Justification::Center,
                                        set_css_classes: &["title-4"],
                                        set_max_width_chars: 14,

                                        set_wrap: true,
                                        set_wrap_mode: pango::WrapMode::WordChar,
                                    },

                                    adw::Spinner {
                                        #[watch]
                                        set_visible: !model.state.session_scan_expired,
                                        set_width_request: 32,
                                        set_height_request: 32
                                    },

                                    gtk::Button {
                                        set_label: &i18n!("Reset Session"),
                                        #[watch]
                                        set_visible: model.state.session_scan_expired,
                                        set_icon_name: "view-refresh-symbolic",
                                        set_css_classes: &["pill", "suggested-action"],

                                        connect_clicked => LoginInput::ResetRequest,
                                    }
                                }
                            },

                            add_overlay = &gtk::ProgressBar {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::End,
                                #[watch]
                                set_visible: model.state.qr_code.is_some(),
                                #[watch]
                                set_fraction: model.state.progress_fraction,
                                set_margin_bottom: 1,
                                set_width_request: 180
                            }
                        }
                    },

                    gtk::Separator {
                        set_halign: gtk::Align::Center,
                        set_margin_top: 10,
                        set_css_classes: &["dimmed"],
                        set_width_request: 300
                    },

                    gtk::Stack {
                        set_transition_type: gtk::StackTransitionType::Crossfade,

                        add_child = &gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_spacing: 10,
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Label {
                                set_label: &i18n!("or use your phone number:"),
                                set_justify: gtk::Justification::Center,
                                set_css_classes: &["body"]
                            },

                            gtk::Box {
                                set_css_classes: &["linked"],
                                set_orientation: gtk::Orientation::Horizontal,

                                gtk::Button {
                                    #[watch]
                                    set_label: model.state.phone_number_country_emoji.as_deref().unwrap_or("🇺🇳"),
                                    set_can_focus: false,
                                    set_width_request: 2,

                                    stop_signal_emission_by_name: "activate",
                                    stop_signal_emission_by_name: "clicked"
                                },

                                #[local_ref]
                                phone_number_entry -> gtk::Entry {
                                    set_max_length: 20,
                                    set_input_hints: gtk::InputHints::PRIVATE,
                                    set_input_purpose: gtk::InputPurpose::Phone,
                                    set_width_request: 200,
                                    set_placeholder_text: Some("+55 99 99999-9999"),

                                    connect_changed[sender] => move |_| { sender.oneshot_command(async { LoginCommand::ValidatePhoneNumber }); },
                                    connect_activate[sender] => move |entry| {
                                        let phone_number = entry.text().to_string();
                                        let _ = sender.output(LoginOutput::PairWithPhoneNumber { phone_number, });
                                    }
                                },

                                gtk::Button {
                                    set_icon_name: "go-next-symbolic",
                                    #[watch]
                                    set_css_classes: if model.state.valid_phone_number {
                                        &["suggested-action"]
                                    } else {
                                        &[]
                                    },

                                    connect_clicked[sender, phone_number_entry] => move |_| {
                                        let phone_number = phone_number_entry.text().to_string();
                                        let _ = sender.output(LoginOutput::PairWithPhoneNumber { phone_number, });
                                    }
                                },
                            }
                        } -> {
                            set_name: "enter-phone-number"
                        },

                        add_child = &gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_spacing: 10,
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Label {
                                set_label: &i18n!("enter this code on your WhatsApp:"),
                                set_justify: gtk::Justification::Center,
                                set_css_classes: &["body"]
                            },

                            gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_hexpand: true,
                                set_spacing: 4,
                                set_orientation: gtk::Orientation::Horizontal,

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[0].to_string()),
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", "frame", if model.state.paired { "success" } else { "accent" }],
                                },

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[1].to_string()),
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", if model.state.paired { "success" } else { "accent" }]
                                },

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[2].to_string()),
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", if model.state.paired { "success" } else { "accent" }]
                                },

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[3].to_string()),
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", if model.state.paired { "success" } else { "accent" }]
                                },

                                gtk::Label {
                                    set_label: "-",
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    set_css_classes: &["title-2"]
                                },

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[4].to_string()),
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", if model.state.paired { "success" } else { "accent" }]
                                },

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[5].to_string()),
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", if model.state.paired { "success" } else { "accent" }]
                                },

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[6].to_string()),
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", if model.state.paired { "success" } else { "accent" }]
                                },

                                gtk::Label {
                                    inline_css: "padding-top: 5px; padding-left: 5px; padding-right: 5px; padding-bottom: 5px;",
                                    #[watch]
                                    set_label?: &model.state.code.map(|c| c[7].to_string()),
                                    set_halign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_justify: gtk::Justification::Center,
                                    #[watch]
                                    set_css_classes: &["title-3", "card", if model.state.paired { "success" } else { "accent" }]
                                },
                            }
                        } -> {
                            set_name: "confirm-code"
                        },

                        #[watch]
                        set_visible_child_name: model.bottom_page.as_ref(),
                    }
                }
            }
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let state = LoginState::default();
        let error_dialog = Alert::builder().transient_for(&root).launch(AlertSettings {
            text: Some(i18n!("An error occurred")),
            secondary_text: None,

            confirm_label: Some(i18n!("Ok")),

            is_modal: true,

            ..Default::default()
        });
        let reset_dialog = Alert::builder()
            .transient_for(&root)
            .launch(AlertSettings {
                text: Some(i18n!("Do you really want to reset the session?")),
                secondary_text: Some(i18n!(
                    "This action will reset your phone number action if you are connecting with it."
                )),

                cancel_label: Some(i18n!("Cancel")),
                confirm_label: Some(i18n!("Continue")),

                is_modal: true,
                destructive_accept: true,

                ..Default::default()
            })
            .forward(sender.command_sender(), |output| match output {
                AlertResponse::Confirm => LoginCommand::ResetSession,
                _ => LoginCommand::Ignore,
            });

        let phone_number_entry = gtk::Entry::new();

        let model = Self {
            state,
            qr_code: None,
            bottom_page: LoginBottomPage::EnterPhoneNumber,
            error_dialog,
            reset_dialog,
            phone_number_entry: phone_number_entry.clone(),
        };

        let widgets = view_output!();

        AsyncComponentParts { model, widgets }
    }

    async fn update(
        &mut self,
        input: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match input {
            LoginInput::ResetRequest => {
                self.reset_dialog.emit(AlertMsg::Show);
            }

            LoginInput::PairCode {
                code,
                qr_code,
                timeout,
            } => {
                if let Some(code) = code {
                    let mut split_code = [' '; 8];
                    for (i, c) in code.chars().enumerate() {
                        split_code[i] = c;
                    }

                    self.state.code = Some(split_code);
                    self.bottom_page = LoginBottomPage::ConfirmCode;
                } else if let Some(qr_code) = qr_code {
                    relm4::spawn(async move {
                        sender.oneshot_command(async move {
                            LoginCommand::UpdateQrCode {
                                data: qr_code,
                                timeout,
                            }
                        });
                    });
                }
            }
            LoginInput::PairSuccess => {
                self.state.paired = true;
            }

            LoginInput::Error { message } => {
                self.error_dialog.widgets().gtk_label_2.set_text(&message);
                self.error_dialog.emit(AlertMsg::Show);
            }
        }
    }

    async fn update_cmd(
        &mut self,
        input: Self::CommandOutput,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match input {
            LoginCommand::ResetSession => {
                // Reset the session.
                self.state.code = None;
                self.state.qr_code = None;
                self.state.scan_attempts = 0;
                self.state.progress_fraction = 0.0;
                self.state.session_scan_expired = true;

                let _ = sender.output(LoginOutput::ResetSession);
            }

            LoginCommand::UpdateQrCode { data, timeout } => {
                // Reset the QR code and progress bar.
                self.state.qr_code = None;
                self.state.progress_fraction = 0.0;

                if self.state.scan_attempts >= 5 {
                    self.state.session_scan_expired = true;
                    return;
                }
                self.state.scan_attempts += 1;

                // Generate the QR code.
                let qr_code = QrCode::new(data.as_bytes()).expect("Failed to generate QR code");
                let image = qr_code.render::<Luma<u8>>().build();

                // Encode the QR code as a PNG.
                let mut bytes = Vec::new();
                let encoder = PngEncoder::new(&mut bytes);
                encoder
                    .write_image(
                        image.as_raw(),
                        image.width(),
                        image.height(),
                        ExtendedColorType::L8,
                    )
                    .expect("Failed to encode QR code");

                // Load the image through glycin.
                let loader = glycin::Loader::new_bytes(glib::Bytes::from_owned(bytes));
                let image = loader.load().await.expect("Failed to load QR code");
                let frame = image
                    .next_frame()
                    .await
                    .expect("Failed to extract QR code frame");
                let texture = frame.texture();

                let start = Instant::now();
                self.qr_code = Some(texture.into());
                self.state.qr_code = Some(qr_code);

                // Make sure to not reset the qr code after it refreshes.
                let timeout = timeout - Duration::from_secs(2);

                sender.command(move |output, shutdown| {
                    shutdown
                        .register(async move {
                            let mut elapsed = start.elapsed();
                            let mut interval = time::interval(Duration::from_millis(100));
                            let mut fraction = elapsed.as_secs_f32() / timeout.as_secs_f32();
                            interval.tick().await;

                            while elapsed < timeout {
                                let _ = output.send(LoginCommand::UpdateExpirationBar(fraction));

                                elapsed = start.elapsed();
                                fraction = elapsed.as_secs_f32() / timeout.as_secs_f32();
                                interval.tick().await;
                            }

                            let _ = output.send(LoginCommand::QrCodeExpired);
                        })
                        .drop_on_shutdown()
                        .boxed()
                });
            }
            LoginCommand::QrCodeExpired => {
                // Reset the QR code and progress bar.
                self.state.qr_code = None;
                self.state.progress_fraction = 0.0;
            }
            LoginCommand::UpdateExpirationBar(progress) => {
                self.state.progress_fraction = progress as f64;
            }

            LoginCommand::ValidatePhoneNumber => {
                let entry = &self.phone_number_entry;

                let text = entry.text();
                let sanitazed = text
                    .trim()
                    .chars()
                    .filter(|char| char.is_ascii_digit() || "+- ".contains(*char))
                    .collect::<String>();

                if text == sanitazed {
                    if let Ok(number) = sanitazed.parse::<PhoneNumber>() {
                        if number.is_valid() {
                            if !self.state.valid_phone_number {
                                let region_code = number.get_region_code().unwrap();
                                let country_emoji = country_emoji::flag(region_code);

                                self.state.valid_phone_number = true;
                                self.state.phone_number_country_emoji = country_emoji;

                                let formatted = number.format_as(PhoneNumberFormat::International);
                                entry.set_text(&formatted);
                                entry.set_position(-1);
                            }
                        } else {
                            self.state.valid_phone_number = false;
                            self.state.phone_number_country_emoji = None;
                        }
                    } else {
                        self.state.valid_phone_number = false;
                        self.state.phone_number_country_emoji = None;
                    }
                } else {
                    entry.set_text(&sanitazed);
                    entry.set_position(-1);
                }
            }

            LoginCommand::Ignore => {}
        }
    }
}
