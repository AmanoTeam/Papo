use adw::prelude::*;
use relm4::prelude::*;

use crate::i18n;

#[derive(Debug)]
pub struct SearchView {
    /// Query input for searching entries.
    entry: gtk::SearchEntry,
    /// `ChatList` component widget.
    chat_list_box: gtk::Box,
}

#[derive(Debug)]
pub enum SearchViewInput {
    /// Focus the search entry.
    FocusEntry,
    /// Update the search query.
    UpdateQuery(String),
}

#[derive(Debug)]
pub enum SearchViewOutput {}

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for SearchView {
    type Init = (gtk::Box,);
    type Input = SearchViewInput;
    type Output = SearchViewOutput;

    view! {
        gtk::Box {
            set_spacing: 4,
            set_orientation: gtk::Orientation::Vertical,

            #[local_ref]
            entry -> gtk::SearchEntry {
                set_margin_start: 8,
                set_margin_end: 8,
                set_margin_top: 4,
                set_margin_bottom: 4,

                connect_search_changed[sender] => move |entry| {
                    let query = entry.text().to_string();
                    sender.input(SearchViewInput::UpdateQuery(query));
                }
            },

            gtk::ScrolledWindow {
                set_vexpand: true,
                set_css_classes: &["undershoot-start", "undershoot-end"],
                set_overlay_scrolling: true,
                set_hscrollbar_policy: gtk::PolicyType::Never,
                set_propagate_natural_height: true,

                adw::PreferencesGroup {
                    set_separate_rows: true,

                    set_margin_start: 8,
                    set_margin_end: 8,

                    adw::ExpanderRow {
                        set_title: &i18n!("Chats"),
                        set_expanded: true,
                        add_css_class: "flat",

                        #[local_ref]
                        add_row = chat_list -> gtk::Box {}
                    }
                }
            }
        }
    }

    async fn init(
        init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let chat_list_box = init.0;

        let model = Self {
            entry: gtk::SearchEntry::new(),
            chat_list_box,
        };

        let entry = &model.entry;
        let chat_list = &model.chat_list_box;

        let widgets = view_output!();

        AsyncComponentParts { model, widgets }
    }
}
