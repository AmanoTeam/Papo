use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use adw::prelude::*;
use futures_util::FutureExt;
use gtk::{gdk, glib, pango};
use relm4::{RelmRemoveAllExt, component::Connector, prelude::*};
use relm4_components::alert::{Alert, AlertMsg, AlertSettings};
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
    /// Page main stack is displaying.
    page: LoginPage,
    /// Current login state.
    state: LoginState,
    /// Current QR code texture.
    qr_code: Option<gdk::Paintable>,
    /// Error dialog (TODO: use a custom alert dialog).
    error_dialog: Connector<Alert>,
    /// Pairing box containing all pair cells.
    pairing_box: gtk::Box,
    /// Pair code character.
    pairing_cells: Option<[PairingCell; 8]>,
    /// Cancel token for the QR code expiration bar animation.
    progress_cancel: Arc<AtomicBool>,
    /// Current pair phone number view.
    phone_number_view: LoginPhoneNumberView,
    /// Input entry containing the user phone number.
    phone_number_entry: adw::EntryRow,
}

#[derive(Clone, Copy, Debug, AsRefStr, PartialEq, EnumString)]
#[strum(serialize_all = "kebab-case")]
enum LoginPage {
    QrCode,
    PhoneNumber,
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
    valid_phone_number: Arc<AtomicBool>,
    /// Whether all QR codes from session has been expired.
    session_scan_expired: Arc<AtomicBool>,
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
}

#[derive(Clone, Copy, Debug, AsRefStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
enum LoginPhoneNumberView {
    /// Confirm code view.
    ConfirmCode,
    /// Enter phone number view.
    EnterPhoneNumber,
}

#[derive(Debug)]
pub enum LoginInput {
    /// 8-character pairing code received.
    PairCode {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    /// Client has paired successfully.
    PairSuccess,
    /// Request the login to change the page to `QrCode`.
    PairWithQrCode,
    /// Request the login to change the page to `PhoneNumber`.
    PairWithPhoneNumber {
        /// Change the view to `EnterPhoneNumber`.
        edit: bool,
    },

    /// Error occurred.
    Error { message: String },

    /// Reset all login state to initial values.
    Reset,
}

#[derive(Debug)]
pub enum LoginOutput {
    /// Reset the session to be able to receive new qr codes.
    ResetSession,

    /// Request the session to pair with a QR code.
    PairWithQrCode,
    /// Request the session to pair with a phone number.
    PairWithPhoneNumber { phone_number: String },
}

#[derive(Debug)]
pub enum LoginCommand {
    /// Reset the session to be able to receive new qr codes.
    ResetSession,

    /// Update the QR Code.
    UpdateQrCode { data: String, timeout: Duration },
    /// QR code expired by timeout.
    QrCodeExpired,
    /// Request the session to pair with a QR code.
    PairWithQrCode,
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

                gtk::Stack {
                    set_transition_type: gtk::StackTransitionType::SlideLeftRight,

                    add_named[Some("qr-code")] = &gtk::Box {
                        set_halign: gtk::Align::Center,
                        set_valign: gtk::Align::Center,
                        set_spacing: 5,
                        set_orientation: gtk::Orientation::Vertical,

                        gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_valign: gtk::Align::Center,
                            set_hexpand: true,
                            set_vexpand: true,
                            set_spacing: 10,
                            #[watch]
                            set_css_classes: if model.qr_code.is_none() { &["card", "view"] } else { &[] },
                            set_orientation: gtk::Orientation::Vertical,
                            set_width_request: 200,
                            set_height_request: 200,

                            gtk::Picture {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::Center,
                                set_hexpand: true,
                                set_vexpand: true,
                                #[watch]
                                set_visible: model.qr_code.is_some(),
                                #[watch]
                                set_paintable: model.qr_code.as_ref(),
                                set_css_classes: &["qr-code"]
                            },

                            gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::Center,
                                set_hexpand: true,
                                set_vexpand: true,
                                #[watch]
                                set_visible: model.qr_code.is_none(),
                                set_spacing: 20,
                                set_orientation: gtk::Orientation::Vertical,

                                gtk::Label {
                                    #[watch]
                                    set_label: &if model.state.session_scan_expired.load(Ordering::Acquire) {
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
                                    set_visible: !model.state.session_scan_expired.load(Ordering::Acquire),
                                    set_width_request: 32,
                                    set_height_request: 32
                                },

                                gtk::Button {
                                    set_label: &i18n!("Reset Session"),
                                    #[watch]
                                    set_visible: model.state.session_scan_expired.load(Ordering::Acquire),
                                    set_css_classes: &["pill", "suggested-action"],

                                    connect_clicked[sender] => move |_| {
                                        sender.oneshot_command(async { LoginCommand::ResetSession });
                                    }
                                }
                            }
                        },

                        gtk::Revealer {
                            #[watch]
                            set_visible: model.page == LoginPage::QrCode,
                            #[watch]
                            set_reveal_child: model.qr_code.is_some(),
                            set_margin_bottom: 20,
                            set_transition_type: gtk::RevealerTransitionType::SwingDown,
                            set_transition_duration: 300,

                            gtk::ProgressBar {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::End,
                                #[watch]
                                set_fraction: model.state.progress_fraction,
                                set_width_request: 200
                            }
                        },

                        gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_hexpand: true,
                            set_spacing: 10,
                            set_orientation: gtk::Orientation::Vertical,
                            set_margin_bottom: 15,

                            PairStep::new(1, &i18n!("Open WhatsApp on your phone.")).main_box {},
                            PairStep::new(2, &i18n!("Go to: <i>Menu &gt; Linked devices &gt; Link a device</i>")) .main_box {},
                            PairStep::new(3, &i18n!("Scan this QR code.")).main_box {}
                        },

                        gtk::Label {
                            set_label: &format!("<a href=\"link-with-phone-number\">{}</a>", i18n!("Link with Phone Number")),
                            set_justify: gtk::Justification::Center,
                            set_use_markup: true,
                            set_css_classes: &["body"],

                            connect_activate_link[sender] => move |_, uri| {
                                if uri == "link-with-phone-number" {
                                    sender.input(LoginInput::PairWithPhoneNumber { edit: false });
                                }

                                glib::Propagation::Stop
                            }
                        }
                    },

                    add_named[Some("phone-number")] = &gtk::Stack {
                        set_transition_type: gtk::StackTransitionType::Crossfade,

                        add_named[Some("enter-phone-number")] = &gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_valign: gtk::Align::Center,
                            set_spacing: 20,
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::Center,
                                set_hexpand: true,
                                set_vexpand: true,
                                set_css_classes: &["card"],
                                set_width_request: 80,
                                set_height_request: 80,

                                gtk::Image {
                                    set_halign: gtk::Align::Center,
                                    set_valign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_vexpand: true,
                                    set_icon_name: Some("phone-right-facing-symbolic"),
                                    set_pixel_size: 50
                                }
                            },

                            gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_hexpand: true,

                                gtk::Label {
                                    set_label: &i18n!("Please enter your phone number."),
                                    set_justify: gtk::Justification::Center,
                                    set_css_classes: &["body", "dimmed"],
                                },
                            },

                            gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_hexpand: true,
                                set_spacing: 15,
                                set_orientation: gtk::Orientation::Vertical,

                                adw::PreferencesGroup {
                                    set_separate_rows: true,
                                    set_width_request: 300,

                                    #[local_ref]
                                    add = &phone_number_entry -> adw::EntryRow {
                                        set_title: &i18n!("Phone Number"),
                                        set_max_length: 20,
                                        set_input_hints: gtk::InputHints::PRIVATE,
                                        set_input_purpose: gtk::InputPurpose::Phone,
                                        set_width_request: 200,

                                        connect_changed[sender] => move |_| { sender.oneshot_command(async { LoginCommand::ValidatePhoneNumber }); },
                                        connect_entry_activated[sender] => move |entry| {
                                            let phone_number = entry.text().to_string();
                                            sender.oneshot_command(async { LoginCommand::PairWithPhoneNumber { phone_number, } });
                                        }
                                    },

                                    add = &adw::ButtonRow{
                                        set_title: &i18n!("Next"),
                                        #[watch]
                                        set_css_classes: if model.state.valid_phone_number.load(Ordering::Acquire) { &["suggested-action"] } else { &[] },
                                        set_end_icon_name: Some("go-next-symbolic"),
                                        set_height_request: 40,

                                        connect_activated[sender, phone_number_entry] => move |_| {
                                            let phone_number = phone_number_entry.text().to_string();
                                            sender.oneshot_command(async { LoginCommand::PairWithPhoneNumber { phone_number, } });
                                        }
                                    }
                                }
                            },

                            gtk::Label {
                                set_label: &format!("<a href=\"link-with-qr-code\">{}</a>", i18n!("Link with QR Code")),
                                set_justify: gtk::Justification::Center,
                                set_use_markup: true,
                                set_css_classes: &["body"],

                                connect_activate_link[sender] => move |_, uri| {
                                    if uri == "link-with-qr-code" {
                                        sender.input(LoginInput::PairWithQrCode);
                                    }

                                    glib::Propagation::Stop
                                }
                            }
                        },

                        add_named[Some("confirm-code")] = &gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_valign: gtk::Align::Center,
                            set_spacing: 20,
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_valign: gtk::Align::Center,
                                set_hexpand: true,
                                set_vexpand: true,
                                set_css_classes: &["card"],
                                set_width_request: 80,
                                set_height_request: 80,

                                gtk::Image {
                                    set_halign: gtk::Align::Center,
                                    set_valign: gtk::Align::Center,
                                    set_hexpand: true,
                                    set_vexpand: true,
                                    set_icon_name: Some("phonelink-setup-symbolic"),
                                    set_pixel_size: 50
                                }
                            },

                            gtk::Label {
                                #[watch]
                                set_label: &format!("{} (<a href=\"edit-phone-number\">{}</a>)", i18n!("Linking WhatsApp account <b>${phone-number}</b>"), i18n!("edit"))
                                    .replace("${phone-number}", model.phone_number_entry.text().to_string().as_str()),
                                set_justify: gtk::Justification::Center,
                                set_use_markup: true,
                                set_css_classes: &["body"],

                                connect_activate_link[sender] => move |_, uri| {
                                    if uri == "edit-phone-number" {
                                        sender.input(LoginInput::PairWithPhoneNumber { edit: true });
                                    }

                                    glib::Propagation::Stop
                                }
                            },

                            #[local_ref]
                            pairing_box -> gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_hexpand: true,
                                set_homogeneous: true,
                            },

                            gtk::Box {
                                set_halign: gtk::Align::Center,
                                set_hexpand: true,
                                set_spacing: 10,
                                set_orientation: gtk::Orientation::Vertical,

                                PairStep::new(1, &i18n!("Open WhatsApp on your phone.")).main_box {},
                                PairStep::new(2, &i18n!("Go to: <i>Menu &gt; Linked devices &gt; Link a device</i>")) .main_box {},
                                PairStep::new(3, &i18n!("Tap <i>Link with phone number</i> and enter this code on your phone.")).main_box {}
                            }
                        },

                        #[watch]
                        set_visible_child_name: model.phone_number_view.as_ref(),
                    },

                    #[watch]
                    set_visible_child_name: model.page.as_ref(),
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

        let pairing_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let phone_number_entry = adw::EntryRow::new();

        let model = Self {
            page: LoginPage::QrCode,
            state: LoginState::default(),
            qr_code: None,
            error_dialog,
            pairing_box,
            pairing_cells: None,
            progress_cancel: Arc::new(AtomicBool::new(false)),
            phone_number_view: LoginPhoneNumberView::EnterPhoneNumber,
            phone_number_entry: phone_number_entry.clone(),
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
            LoginInput::PairCode {
                code,
                qr_code,
                timeout,
            } => {
                if let Some(code) = code {
                    // Empty the pairing box, removing all cells
                    self.pairing_box.remove_all();

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
                    self.pairing_cells = cells.as_array().map(ToOwned::to_owned);
                    self.phone_number_view = LoginPhoneNumberView::ConfirmCode;
                }

                if let Some(qr_code) = qr_code {
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
            LoginInput::PairWithQrCode => {
                self.page = LoginPage::QrCode;
                sender.oneshot_command(async { LoginCommand::PairWithQrCode });
            }
            LoginInput::PairWithPhoneNumber { edit } => {
                if edit {
                    self.phone_number_view = LoginPhoneNumberView::EnterPhoneNumber;
                }

                self.page = LoginPage::PhoneNumber;
                self.phone_number_entry.grab_focus();
            }

            LoginInput::Error { message } => {
                // Show error dialog for all pairing/connection errors,
                // regardless of which login page is active. Previously,
                // the phone number page auto-retried on error, which
                // caused multiple PairWithPhoneNumber requests when the
                // server dropped the connection (error 515).
                self.state.pair_state = PairState::Pairing;

                self.error_dialog.widgets().gtk_label_2.set_text(&message);
                self.error_dialog.emit(AlertMsg::Show);
            }
            LoginInput::Reset => {
                self.page = LoginPage::QrCode;
                self.state = LoginState::default();
                self.qr_code = None;
                self.pairing_cells = None;
                self.pairing_box.remove_all();
                self.progress_cancel.store(true, Ordering::Release);
                self.progress_cancel = Arc::new(AtomicBool::new(false));
                self.phone_number_entry.set_text("");
                self.phone_number_view = LoginPhoneNumberView::EnterPhoneNumber;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
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
                self.state.pair_state = PairState::Pairing;
                self.phone_number_view = LoginPhoneNumberView::EnterPhoneNumber;
                self.state.scan_attempts = 0;
                self.state.progress_fraction = 1.0;
                self.state
                    .session_scan_expired
                    .store(false, Ordering::Release);

                let _ = sender.output(LoginOutput::ResetSession);
            }

            LoginCommand::UpdateQrCode { data, timeout } => {
                self.state.progress_fraction = 1.0;

                if self.state.scan_attempts >= 5 {
                    self.state
                        .session_scan_expired
                        .store(true, Ordering::Release);
                    return;
                }
                self.state.scan_attempts += 1;

                let texture = Box::pin(generate_qr_code(&data, 200))
                    .await
                    .expect("Failed to generate QR code");
                self.qr_code = Some(texture.current_image());

                let timeout = timeout.saturating_sub(Duration::from_secs(2));

                self.progress_cancel.store(true, Ordering::Release);
                let progress_cancel = Arc::new(AtomicBool::new(false));
                self.progress_cancel = progress_cancel.clone();

                let start = Instant::now();
                sender.command(move |command_sender, shutdown| {
                    shutdown
                        .register(async move {
                            let mut elapsed = start.elapsed();
                            let mut interval = time::interval(Duration::from_millis(100));
                            let mut fraction =
                                1.0 - (elapsed.as_secs_f32() / timeout.as_secs_f32());
                            interval.tick().await;

                            while elapsed < timeout && !progress_cancel.load(Ordering::Acquire) {
                                command_sender.emit(LoginCommand::UpdateExpirationBar(fraction));

                                elapsed = start.elapsed();
                                fraction = 1.0 - (elapsed.as_secs_f32() / timeout.as_secs_f32());
                                interval.tick().await;
                            }

                            if !progress_cancel.load(Ordering::Acquire) {
                                command_sender.emit(LoginCommand::QrCodeExpired);
                            }
                        })
                        .drop_on_shutdown()
                        .boxed()
                });
            }
            LoginCommand::QrCodeExpired => {
                // Reset the QR code and progress bar.
                self.qr_code = None;
                self.state.progress_fraction = 1.0;
            }
            LoginCommand::UpdateExpirationBar(progress) => {
                self.state.progress_fraction = f64::from(progress);
            }
            LoginCommand::PairWithQrCode => {
                let _ = sender.output(LoginOutput::PairWithQrCode);
            }
            LoginCommand::PairWithPhoneNumber { phone_number } => {
                if self.state.valid_phone_number.load(Ordering::Acquire) {
                    let _ = sender.output(LoginOutput::PairWithPhoneNumber { phone_number });
                }
            }

            LoginCommand::ValidatePhoneNumber => {
                let entry = &self.phone_number_entry;

                let text = entry.text();
                let mut sanitazed = text
                    .trim()
                    .chars()
                    .filter(|char| char.is_ascii_digit() || "+- ".contains(*char))
                    .collect::<String>();

                if text == sanitazed {
                    if !sanitazed.starts_with('+') {
                        sanitazed = format!("+{sanitazed}");
                    }

                    if let Ok(number) = sanitazed.parse::<PhoneNumber>() {
                        if number.is_valid() {
                            if !self.state.valid_phone_number.load(Ordering::Acquire) {
                                self.state.valid_phone_number.store(true, Ordering::Release);

                                let formatted = number.format_as(PhoneNumberFormat::International);
                                entry.set_text(&formatted);
                                entry.set_position(-1);
                            }
                        } else if self.state.valid_phone_number.load(Ordering::Acquire) {
                            let only_digits = sanitazed
                                .chars()
                                .filter(char::is_ascii_digit)
                                .collect::<String>();
                            entry.set_text(&only_digits);
                            entry.set_position(-1);

                            self.state
                                .valid_phone_number
                                .store(false, Ordering::Release);
                        }
                    } else if self.state.valid_phone_number.load(Ordering::Acquire) {
                        self.state
                            .valid_phone_number
                            .store(false, Ordering::Release);
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
