use std::{cell::Cell, rc::Rc, time::Duration};

use adw::prelude::*;
use futures_util::FutureExt;
use gtk::{gdk, pango};
use relm4::{component::Connector, prelude::*};
use relm4_components::alert::{Alert, AlertMsg, AlertResponse, AlertSettings};
use rlibphonenumber::{PhoneNumber, PhoneNumberFormat};
use strum::{AsRefStr, EnumString};
use tokio::time::{self, Instant};

use crate::{
    i18n,
    utils::generate_qr_code,
    widgets::{PairStep, PairingCell},
};

#[derive(Debug)]
pub struct Login {
    /// Current login state.
    state: LoginState,
    /// Current QR code texture.
    qr_code: Option<gdk::Paintable>,
    /// Current bottom page.
    bottom_page: LoginBottomPage,
    /// Pairing box containing all pair cells.
    pairing_box: gtk::Box,
    /// Pair code character.
    pairing_cells: Option<[PairingCell; 8]>,
    /// Input entry containing the user phone number.
    phone_number_entry: gtk::Entry,

    error_dialog: Connector<Alert>,  // TODO: use a custom alert dialog
    reset_dialog: Controller<Alert>, // TODO: use a custom alert dialog
}

#[derive(AsRefStr, Clone, Copy, Debug, EnumString)]
#[strum(serialize_all = "kebab-case")]
enum LoginBottomPage {
    /// Confirm code view.
    ConfirmCode,
    /// Enter phone number view.
    EnterPhoneNumber,
}

#[derive(Clone, Debug, Default)]
struct LoginState {
    /// 8-character pair code.
    code: Option<[char; 8]>,
    /// Current pair state.
    pair_state: PairState,
    /// QR code scan attempts.
    scan_attempts: u8,
    /// QR code expiration bar's progress.
    progress_fraction: f64,
    /// Whether the phone number is valid.
    valid_phone_number: Rc<Cell<bool>>,
    /// Whether all QR codes from session has been expired.
    session_scan_expired: Rc<Cell<bool>>,
    /// Phone number country emoji.
    phone_number_country_emoji: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PairState {
    /// The client was paired successfully.
    Paired,
    /// The client is still pairing.
    #[default]
    Pairing,
    /// The client is pairing with phone number.
    PairingWithPhoneNumber,
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
    /// Request the session to pair with a phone number.
    PairWithPhoneNumber { phone_number: String },

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
                        set_spacing: 20,
                        set_orientation: gtk::Orientation::Vertical,

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
                                #[watch]
                                set_visible: model.qr_code.is_none(),
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
                                        set_label: &if model.state.session_scan_expired.get() {
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
                                        set_visible: !model.state.session_scan_expired.get(),
                                        set_width_request: 32,
                                        set_height_request: 32
                                    },

                                    gtk::Button {
                                        set_label: &i18n!("Reset Session"),
                                        #[watch]
                                        set_visible: model.state.session_scan_expired.get(),
                                        set_css_classes: &["pill", "suggested-action"],

                                        connect_clicked => LoginInput::ResetRequest,
                                    }
                                }
                            },

                            add_overlay = &gtk::ProgressBar {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::End,
                                #[watch]
                                set_visible: model.qr_code.is_some(),
                                #[watch]
                                set_fraction: model.state.progress_fraction,
                                set_margin_bottom: 1,
                                set_width_request: 180
                            }
                        },

                        gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_hexpand: true,
                            set_spacing: 10,
                            set_orientation: gtk::Orientation::Vertical,

                            #[template]
                            PairStep {
                                #[template_child]
                                number {
                                    set_label: "1"
                                },

                                #[template_child]
                                step {
                                    set_label: &i18n!("Open WhatsApp on your phone.")
                                }
                            },

                            #[template]
                            PairStep {
                                #[template_child]
                                number {
                                    set_label: "2"
                                },

                                #[template_child]
                                step {
                                    set_label: &i18n!("Go to: <i>Menu > Connected devices > Connect device</i>")
                                }
                            },

                            #[template]
                            PairStep {
                                #[template_child]
                                number {
                                    set_label: "3"
                                },

                                #[template_child]
                                step {
                                    set_label: &i18n!("Scan this QR code.")
                                }
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
                                        sender.oneshot_command(async { LoginCommand::PairWithPhoneNumber { phone_number, } });
                                    }
                                },

                                gtk::Button {
                                    #[watch]
                                    set_css_classes: if model.state.valid_phone_number.get() {
                                        &["suggested-action"]
                                    } else {
                                        &[]
                                    },

                                    #[wrap(Some)]
                                    set_child = &gtk::Box {
                                        gtk::Image {
                                            #[watch]
                                            set_visible: model.state.pair_state != PairState::PairingWithPhoneNumber,
                                            set_icon_name: Some("go-next-symbolic")
                                        },

                                        adw::Spinner {
                                            #[watch]
                                            set_visible: model.state.pair_state == PairState::PairingWithPhoneNumber,
                                        }
                                    },

                                    connect_clicked[sender, phone_number_entry] => move |_| {
                                        let phone_number = phone_number_entry.text().to_string();
                                        sender.oneshot_command(async { LoginCommand::PairWithPhoneNumber { phone_number, } });
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

                            #[local_ref]
                            pairing_box -> gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_hexpand: true,
                                set_homogeneous: true,
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

        let pairing_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let phone_number_entry = gtk::Entry::new();

        let model = Self {
            state: LoginState::default(),
            qr_code: None,
            bottom_page: LoginBottomPage::EnterPhoneNumber,
            pairing_box,
            pairing_cells: None,
            phone_number_entry: phone_number_entry.clone(),

            error_dialog,
            reset_dialog,
        };

        let pairing_box = &model.pairing_box;

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
                    let mut cells = Vec::with_capacity(8);
                    let mut split_code = [' '; 8];

                    for (i, c) in code.chars().enumerate() {
                        split_code[i] = c;

                        let cell = PairingCell::init(c);
                        self.pairing_box.append(cell.widget_ref());
                        cells.push(cell);

                        if (i + 1) == 4 {
                            let separator = gtk::Label::builder()
                                .label("—")
                                .halign(gtk::Align::Center)
                                .valign(gtk::Align::Center)
                                .css_classes(["title-2"])
                                .build();
                            self.pairing_box.append(&separator);
                        }
                    }

                    self.state.code = Some(split_code);
                    self.bottom_page = LoginBottomPage::ConfirmCode;
                    self.pairing_cells = cells.as_array().map(|c| c.to_owned());
                } else if let Some(qr_code) = qr_code {
                    sender.oneshot_command(async move {
                        LoginCommand::UpdateQrCode {
                            data: qr_code,
                            timeout,
                        }
                    });
                }
            }
            LoginInput::PairSuccess => {
                self.state.pair_state = PairState::Paired;

                if let Some(cells) = self.pairing_cells.as_ref() {
                    for cell in cells {
                        cell.remove_css_class("accent");
                        cell.add_css_class("success");
                    }
                }
            }

            LoginInput::Error { message } => {
                if self.state.pair_state == PairState::PairingWithPhoneNumber {
                    // Reset session and start pair with phone number
                    sender.oneshot_command(async { LoginCommand::ResetSession });

                    let phone_number = self.phone_number_entry.text().to_string();
                    sender.oneshot_command(async {
                        LoginCommand::PairWithPhoneNumber { phone_number }
                    });
                } else {
                    self.state.pair_state = PairState::Pairing;

                    self.error_dialog.widgets().gtk_label_2.set_text(&message);
                    self.error_dialog.emit(AlertMsg::Show);
                }
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
                self.qr_code = None;
                self.state.code = None;
                self.state.scan_attempts = 0;
                self.state.progress_fraction = 0.0;
                self.state.session_scan_expired.set(true);

                let _ = sender.output(LoginOutput::ResetSession);
            }

            LoginCommand::UpdateQrCode { data, timeout } => {
                // Reset the QR code and progress bar.
                self.state.progress_fraction = 0.0;

                if self.state.scan_attempts >= 5 {
                    self.state.session_scan_expired.set(true);
                    return;
                }
                self.state.scan_attempts += 1;

                // Generate the QR code.
                let texture = generate_qr_code(&data)
                    .await
                    .expect("Failed to generate QR code");
                self.qr_code = Some(texture.into());

                // Make sure to not reset the qr code after it refreshes.
                let timeout = timeout.checked_sub(Duration::from_secs(2)).unwrap();

                let start = Instant::now();
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
                self.qr_code = None;
                self.state.progress_fraction = 0.0;
            }
            LoginCommand::UpdateExpirationBar(progress) => {
                self.state.progress_fraction = f64::from(progress);
            }
            LoginCommand::PairWithPhoneNumber { phone_number } => {
                if self.state.valid_phone_number.get() {
                    self.state.pair_state = PairState::PairingWithPhoneNumber;

                    let _ = sender.output(LoginOutput::PairWithPhoneNumber { phone_number });
                }
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
                            if !self.state.valid_phone_number.get() {
                                let region_code = number.get_region_code().unwrap();
                                let country_emoji = country_emoji::flag(region_code);

                                self.state.valid_phone_number.set(true);
                                self.state.phone_number_country_emoji = country_emoji;

                                let formatted = number.format_as(PhoneNumberFormat::International);
                                entry.set_text(&formatted);
                                entry.set_position(-1);
                            }
                        } else {
                            self.state.valid_phone_number.set(false);
                            self.state.phone_number_country_emoji = None;
                        }
                    } else {
                        self.state.valid_phone_number.set(false);
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
