use std::sync::Arc;

use adw::prelude::*;
use relm4::prelude::*;

pub enum ChatType {
    Group,
    User,
}

pub struct ChatList {
    /// Shared chat references.
    // chats: Arc<[Chat]>,
    list_box: gtk::ListBox,
}

#[derive(Debug)]
pub enum ChatListMsg {
    Select(String),
}

#[derive(Debug)]
pub enum ChatListOutput {
    ChatSelected(String),
}

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for ChatList {
    type Init = ();
    type Input = ChatListMsg;
    type Output = ChatListOutput;

    view! {
        gtk::ScrolledWindow {
            set_vexpand: true,
            set_hexpand: true,

            #[local_ref]
            list_box -> gtk::ListBox {
                add_css_class: "chat-list",
                set_selection_mode: gtk::SelectionMode::Single,
            }
        }
    }

    async fn init(
        _init: Self::Init,
        _root: Self::Root,
        _sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let list_box = gtk::ListBox::new();

        let model = Self { list_box };

        let list_box = &model.list_box;
        let widgets = view_output!();

        AsyncComponentParts { model, widgets }
    }

    async fn update(&mut self, message: Self::Input, sender: AsyncComponentSender<Self>) {
        match message {
            ChatListMsg::Select(chat_id) => {
                let _ = sender.output(ChatListOutput::ChatSelected(chat_id));
            }
        }
    }
}
