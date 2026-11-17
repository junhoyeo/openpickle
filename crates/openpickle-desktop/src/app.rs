//! The root view: shows sign-in or the signed-in sections, and applies what the backend
//! reports.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;
use futures::{StreamExt, channel::mpsc::UnboundedReceiver};
use gpui::{
    AnyView, Context, Entity, Focusable, FontWeight, ImageSource, SharedString, Subscription, Task,
    Window, div, prelude::*, px,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    content::LinkTarget,
    convert,
    model::{Message, Part, RoomInfo, Social, UNCONFIRMED_SEQ, build_rows},
    protocol::{Command, Update},
    theme,
    ui::{
        Identity, OpenArchive, OpenChat, OpenExplore, OpenFriends, Pictures, Tab,
        archive::{ArchiveEvent, ArchiveView},
        attachment::{AttachmentEvent, AttachmentView},
        chat::{ChatEvent, ChatView},
        drawer::{DrawerEvent, DrawerView},
        explore::{ExploreEvent, ExploreView},
        friends::{FriendsEvent, FriendsView},
        invite::{InviteEvent, InviteView},
        login::{LoginEvent, LoginView},
        search::{SearchEvent, SearchView},
    },
};

enum Screen {
    /// Nothing is known yet; the backend answers within the first frames.
    Starting,
    Login(Entity<LoginView>),
    /// Signed in. The chat always exists; the other sections are made when first opened.
    Chat(Entity<ChatView>),
    /// The backend stopped; the text says why.
    Failed(SharedString),
}

/// What is open on top of a section.
enum Over {
    Drawer(Entity<DrawerView>),
    Invite(Entity<InviteView>),
    Attachment(Entity<AttachmentView>),
    /// The search screen, which lives in `Shell::search`.
    Search,
}

enum PrimaryAction {
    Search,
    Ask { text: String, offer: String },
}

pub struct Shell {
    commands: UnboundedSender<Command>,
    screen: Screen,
    /// The section that shows while signed in.
    tab: Tab,
    explore: Option<Entity<ExploreView>>,
    friends: Option<Entity<FriendsView>>,
    archive: Option<Entity<ArchiveView>>,
    over: Option<Over>,
    /// The search screen. It is kept when it closes, so that opening it again shows the same
    /// search: a result is opened to read around it, and then usually the next one.
    search: Option<Entity<SearchView>>,
    /// The character's name and picture, for the header of every section.
    identity: Identity,
    /// The pictures on disk, which the sections draw from.
    pictures: Entity<Pictures>,
    /// The friends and the invite link as last read.
    social: Option<Social>,
    /// The other chat rooms as last listed.
    rooms: Vec<RoomInfo>,
    /// The open conversation, oldest first.
    messages: Vec<Message>,
    room_id: String,
    is_character: bool,
    can_send: bool,
    opening_room: Option<String>,
    after_primary: Option<PrimaryAction>,
    room_header: Identity,
    drafts: std::collections::HashMap<String, String>,
    pending: std::collections::HashMap<String, Vec<Message>>,
    notice: Option<SharedString>,
    /// How many messages were typed here, which numbers the ones not confirmed yet.
    sent: u64,
    /// The offers that messages on their way ask for, by the message's id.
    offers_asked: std::collections::HashMap<String, String>,
    activity: Option<Option<String>>,
    /// For each message, the index of its first row in the chat's list.
    row_starts: Vec<usize>,
    _screen_events: Option<Subscription>,
    _section_events: Vec<Subscription>,
    _over_events: Option<Subscription>,
    _search_events: Option<Subscription>,
    _activation: Subscription,
    _updates: Task<()>,
}

impl Shell {
    pub fn new(
        commands: UnboundedSender<Command>,
        mut updates: UnboundedReceiver<Update>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let activation =
            cx.observe_window_activation(window, |this, window, _| this.report_reading(window));
        // The backend takes the chat to be out of sight until told otherwise, and the
        // observer only reports changes.
        commands
            .send(Command::Active(window.is_window_active()))
            .ok();
        let mut shell = Self {
            commands,
            screen: Screen::Starting,
            tab: Tab::Chat,
            explore: None,
            friends: None,
            archive: None,
            over: None,
            search: None,
            identity: Identity::default(),
            pictures: cx.new(|_| Pictures::default()),
            social: None,
            rooms: Vec::new(),
            messages: Vec::new(),
            room_id: String::new(),
            is_character: true,
            can_send: false,
            opening_room: None,
            after_primary: None,
            room_header: Identity::default(),
            drafts: std::collections::HashMap::new(),
            pending: std::collections::HashMap::new(),
            notice: None,
            sent: 0,
            offers_asked: std::collections::HashMap::new(),
            activity: None,
            row_starts: Vec::new(),
            _screen_events: None,
            _section_events: Vec::new(),
            _over_events: None,
            _search_events: None,
            _activation: activation,
            _updates: Task::ready(()),
        };
        // The backend started before the window did, so the cached conversation is usually
        // waiting already. Applying it here puts it in the very first frame.
        while let Ok(update) = updates.try_recv() {
            shell.apply(update, window, cx);
        }
        shell._updates = cx.spawn_in(window, async move |this, cx| {
            while let Some(update) = updates.next().await {
                let applied = this.update_in(cx, |this, window, cx| this.apply(update, window, cx));
                if applied.is_err() {
                    break;
                }
            }
        });
        shell
    }

    fn apply(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::SignedOut => {
                self.messages.clear();
                self.forget_account();
                self.pictures.update(cx, |pictures, cx| {
                    *pictures = Pictures::default();
                    cx.notify();
                });
                self.show_login(window, cx)
            }
            Update::CodeSent => {
                if let Screen::Login(login) = &self.screen {
                    login.update(cx, |login, cx| login.code_requested(window, cx));
                }
            }
            Update::SignInFailed(reason) => {
                if let Screen::Login(login) = &self.screen {
                    login.update(cx, |login, cx| login.failed(reason, cx));
                }
            }
            Update::Conversation {
                room_id,
                is_character,
                can_send,
                name,
                avatar,
                messages,
            } => {
                let changed_room = room_id != self.room_id;
                if changed_room {
                    if let Screen::Chat(chat) = &self.screen {
                        self.drafts
                            .insert(self.room_id.clone(), chat.read(cx).draft(cx));
                    }
                    let pending = self
                        .messages
                        .drain(..)
                        .filter(|message| message.seq > UNCONFIRMED_SEQ)
                        .collect();
                    self.pending.insert(self.room_id.clone(), pending);
                    self.messages = self.pending.remove(&room_id).unwrap_or_default();
                    self.search = None;
                    self._search_events = None;
                    self.over = None;
                    self._over_events = None;
                    if let Some(archive) = &self.archive {
                        archive.update(cx, |archive, cx| {
                            archive.set_attachments(Vec::new(), false, cx)
                        });
                    }
                }
                self.room_id = room_id;
                self.is_character = is_character;
                self.can_send = can_send;
                self.opening_room = None;
                self.notice = None;
                open_conversation(&mut self.messages, messages);
                self.activity = None;
                self.show_chat(window, cx);
                self.room_header = Identity {
                    name: name.clone().into(),
                    avatar: avatar.clone().map(ImageSource::from),
                };
                if is_character {
                    self.set_identity(name, avatar.map(ImageSource::from), cx);
                }
                if let Screen::Chat(chat) = &self.screen {
                    let draft = self.drafts.get(&self.room_id).cloned().unwrap_or_default();
                    chat.update(cx, |chat, cx| {
                        if changed_room {
                            chat.switch_room(can_send, &draft, cx);
                        }
                        chat.set_can_send(can_send, cx);
                        chat.set_identity(
                            self.room_header.name.clone(),
                            self.room_header.avatar.clone(),
                            cx,
                        );
                    });
                    if self.tab == Tab::Chat {
                        window.focus(&chat.focus_handle(cx));
                    }
                }
                self.refresh_room_avatar(cx);
                self.refresh_rows(cx);
                if self.tab == Tab::Archive {
                    self.commands.send(Command::LoadArchive).ok();
                }
                self.report_reading(window);
                if is_character && let Some(action) = self.after_primary.take() {
                    match action {
                        PrimaryAction::Search => self.open_search(window, cx),
                        PrimaryAction::Ask { text, offer } => self.send(&text, Some(offer), cx),
                    }
                }
            }
            Update::Identity { name, avatar } => {
                self.set_identity(name, avatar.map(ImageSource::from), cx);
            }
            Update::Upsert { room_id, messages } => {
                if room_id == self.room_id {
                    for message in messages {
                        upsert(&mut self.messages, message);
                    }
                    self.refresh_rows(cx);
                }
            }
            Update::SendFailed {
                room_id,
                id,
                reason,
            } => {
                if room_id == self.room_id {
                    mark_not_sent(&mut self.messages, &id, &reason);
                    self.refresh_rows(cx);
                } else if let Some(pending) = self.pending.get_mut(&room_id) {
                    mark_not_sent(pending, &id, &reason);
                }
                if let (Some(offer), Some(explore)) = (self.offers_asked.remove(&id), &self.explore)
                {
                    explore.update(cx, |explore, cx| explore.ask_failed(&offer, cx));
                }
            }
            Update::Activity { room_id, activity } => {
                if room_id == self.room_id {
                    self.activity = activity;
                    self.refresh_rows(cx);
                }
            }
            Update::RoomOpenFailed { room_id, reason } => {
                if self.opening_room.as_deref() == Some(room_id.as_str()) {
                    self.opening_room = None;
                    if let (Some(PrimaryAction::Ask { offer, .. }), Some(explore)) =
                        (self.after_primary.take(), &self.explore)
                    {
                        explore.update(cx, |explore, cx| explore.ask_failed(&offer, cx));
                    }
                    if let Screen::Chat(chat) = &self.screen {
                        chat.update(cx, |chat, cx| chat.set_can_send(self.can_send, cx));
                    }
                    self.notice = Some(reason.into());
                    self.report_reading(window);
                    cx.notify();
                }
            }
            Update::AttachmentPreview { path, result } => {
                if let Some(Over::Attachment(view)) = &self.over
                    && view.read(cx).path() == path
                {
                    view.update(cx, |view, cx| view.loaded(result, cx));
                }
            }
            Update::Rooms(rooms) => {
                self.rooms = rooms;
                self.refresh_room_avatar(cx);
                if let Some(Over::Drawer(drawer)) = &self.over {
                    let rooms = self.rooms.clone();
                    drawer.update(cx, |drawer, cx| drawer.set_rooms(rooms, cx));
                }
            }
            Update::Found { query, results } => {
                if let Some(search) = &self.search {
                    search.update(cx, |search, cx| search.found(&query, results, cx));
                }
            }
            Update::Social(social) => {
                if let Ok(social) = &social {
                    self.social = Some(social.clone());
                    if let Some(Over::Invite(invite)) = &self.over {
                        invite.update(cx, |invite, cx| {
                            invite.set_invite(social.invite.clone(), cx)
                        });
                    }
                }
                if let Some(friends) = &self.friends {
                    friends.update(cx, |friends, cx| friends.set_social(social, cx));
                }
            }
            Update::Explore(explore) => {
                if let Some(view) = &self.explore {
                    view.update(cx, |view, cx| view.set_explore(explore, cx));
                }
            }
            Update::Archive {
                room_id,
                attachments,
                complete,
            } => {
                if room_id == self.room_id
                    && let Some(archive) = &self.archive
                {
                    archive.update(cx, |archive, cx| {
                        archive.set_attachments(attachments, complete, cx)
                    });
                }
            }
            Update::Pictures(pictures) => {
                self.pictures.update(cx, |known, cx| {
                    known.add(pictures);
                    cx.notify();
                });
                self.refresh_room_avatar(cx);
            }
            Update::Failed(reason) => {
                self.forget_account();
                self._screen_events = None;
                self.screen = Screen::Failed(reason.into());
                cx.notify();
            }
        }
    }

    /// Drops what belongs to the signed-in account besides the conversation: the other
    /// sections, what is open on top, and the search with what it found.
    fn forget_account(&mut self) {
        self.tab = Tab::Chat;
        self.explore = None;
        self.friends = None;
        self.archive = None;
        self.over = None;
        self.search = None;
        self.social = None;
        self.rooms.clear();
        self.drafts.clear();
        self.pending.clear();
        self.offers_asked.clear();
        self.room_id.clear();
        self.opening_room = None;
        self.after_primary = None;
        self.notice = None;
        self.can_send = false;
        self._section_events.clear();
        self._over_events = None;
        self._search_events = None;
    }

    fn refresh_room_avatar(&mut self, cx: &mut Context<Self>) {
        if self.is_character {
            return;
        }
        if let Some(room) = self.rooms.iter().find(|room| room.room_id == self.room_id)
            && let Some(avatar) = self.pictures.read(cx).get(&room.picture)
        {
            self.room_header.avatar = Some(avatar.clone());
            if let Screen::Chat(chat) = &self.screen {
                chat.update(cx, |chat, cx| {
                    chat.set_identity(self.room_header.name.clone(), Some(avatar), cx)
                });
            }
        }
    }

    fn set_identity(&mut self, name: String, avatar: Option<ImageSource>, cx: &mut Context<Self>) {
        self.identity = Identity {
            name: name.into(),
            avatar,
        };
        let identity = &self.identity;
        if self.is_character
            && let Screen::Chat(chat) = &self.screen
        {
            self.room_header = identity.clone();
            chat.update(cx, |chat, cx| {
                chat.set_identity(identity.name.clone(), identity.avatar.clone(), cx)
            });
        }
        if let Some(explore) = &self.explore {
            explore.update(cx, |explore, cx| explore.set_identity(identity.clone(), cx));
        }
        if let Some(friends) = &self.friends {
            friends.update(cx, |friends, cx| friends.set_identity(identity.clone(), cx));
        }
        if let Some(archive) = &self.archive {
            archive.update(cx, |archive, cx| archive.set_identity(identity.clone(), cx));
        }
    }

    fn show_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let login = cx.new(LoginView::new);
        self._screen_events = Some(cx.subscribe(&login, |this, _, event, _| {
            let command = match event {
                LoginEvent::RequestCode(phone) => Command::RequestCode(phone.clone()),
                LoginEvent::VerifyCode(code) => Command::VerifyCode(code.clone()),
            };
            this.commands.send(command).ok();
        }));
        window.focus(&login.focus_handle(cx));
        self.screen = Screen::Login(login);
        cx.notify();
    }

    fn show_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Screen::Chat(_) = &self.screen {
            return;
        }
        let pictures = self.pictures.clone();
        let chat = cx.new(|cx| ChatView::new(pictures, cx));
        self._screen_events = Some(
            cx.subscribe_in(&chat, window, |this, _, event, window, cx| match event {
                ChatEvent::Send(text) => this.send(text, None, cx),
                ChatEvent::Search => this.open_search(window, cx),
                ChatEvent::Menu => this.open_drawer(window, cx),
                ChatEvent::Invite => this.open_invite(window, cx),
                ChatEvent::Open(tab) => this.open_tab(*tab, window, cx),
                ChatEvent::Link(target) => this.open_link(target, window, cx),
                ChatEvent::LoadAttachments(paths) => {
                    this.commands
                        .send(Command::LoadAttachments(paths.clone()))
                        .ok();
                }
            }),
        );
        window.focus(&chat.focus_handle(cx));
        self.screen = Screen::Chat(chat);
        cx.notify();
    }

    /// Goes to a section and closes what was open on top. A section that is not built stays
    /// where the window is.
    fn open_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.screen, Screen::Chat(_)) {
            return;
        }
        match tab {
            Tab::Chat => {}
            Tab::Explore => {
                if self.explore.is_none() {
                    let (identity, pictures) = (self.identity.clone(), self.pictures.clone());
                    let explore = cx.new(|cx| ExploreView::new(identity, pictures, cx));
                    self._section_events.push(cx.subscribe_in(
                        &explore,
                        window,
                        |this, _, event, window, cx| match event {
                            ExploreEvent::Menu => this.open_drawer(window, cx),
                            ExploreEvent::Invite => this.open_invite(window, cx),
                            ExploreEvent::Open(tab) => this.open_tab(*tab, window, cx),
                            ExploreEvent::Ask { offer, text } => {
                                if !this.is_character {
                                    this.after_primary = Some(PrimaryAction::Ask {
                                        text: text.clone(),
                                        offer: offer.clone(),
                                    });
                                    this.open_room("", window, cx);
                                } else {
                                    this.open_tab(Tab::Chat, window, cx);
                                    this.send(text, Some(offer.clone()), cx);
                                }
                            }
                            ExploreEvent::Shown(offers) => {
                                this.commands
                                    .send(Command::OffersShown(offers.clone()))
                                    .ok();
                            }
                            ExploreEvent::Reload => {
                                this.commands.send(Command::LoadExplore).ok();
                            }
                        },
                    ));
                    self.explore = Some(explore);
                }
                self.commands.send(Command::LoadExplore).ok();
            }
            Tab::Friends => {
                if self.friends.is_none() {
                    let (identity, pictures) = (self.identity.clone(), self.pictures.clone());
                    let friends = cx.new(|cx| FriendsView::new(identity, pictures, cx));
                    if let Some(social) = &self.social {
                        friends
                            .update(cx, |friends, cx| friends.set_social(Ok(social.clone()), cx));
                    }
                    self._section_events.push(cx.subscribe_in(
                        &friends,
                        window,
                        |this, _, event, window, cx| match event {
                            FriendsEvent::Menu => this.open_drawer(window, cx),
                            FriendsEvent::Invite => this.open_invite(window, cx),
                            FriendsEvent::Open(tab) => this.open_tab(*tab, window, cx),
                            FriendsEvent::Reload => {
                                this.commands.send(Command::LoadSocial).ok();
                            }
                        },
                    ));
                    self.friends = Some(friends);
                }
                self.commands.send(Command::LoadSocial).ok();
            }
            Tab::Archive => {
                if self.archive.is_none() {
                    let (identity, pictures) = (self.identity.clone(), self.pictures.clone());
                    let archive = cx.new(|cx| ArchiveView::new(identity, pictures, cx));
                    self._section_events.push(cx.subscribe_in(
                        &archive,
                        window,
                        |this, _, event, window, cx| match event {
                            ArchiveEvent::Menu => this.open_drawer(window, cx),
                            ArchiveEvent::Invite => this.open_invite(window, cx),
                            ArchiveEvent::Open(tab) => this.open_tab(*tab, window, cx),
                            ArchiveEvent::ShowInChat(seq) => this.show_message(*seq, window, cx),
                            ArchiveEvent::Load(paths) => {
                                this.commands
                                    .send(Command::LoadAttachments(paths.clone()))
                                    .ok();
                            }
                        },
                    ));
                    self.archive = Some(archive);
                }
                self.commands.send(Command::LoadArchive).ok();
            }
            Tab::Profile => return,
        }
        self.tab = tab;
        self.report_reading(window);
        self.close_over(window, cx);
    }

    /// Tells the backend whether the chat can be read now: the window is in front and shows
    /// the chat. Only then do new messages count as read.
    fn report_reading(&self, window: &Window) {
        let reading = window.is_window_active()
            && self.tab == Tab::Chat
            && self.over.is_none()
            && self.opening_room.is_none();
        self.commands.send(Command::Active(reading)).ok();
    }

    /// The view of the section the window is on.
    fn section(&self) -> Option<AnyView> {
        let Screen::Chat(chat) = &self.screen else {
            return None;
        };
        match self.tab {
            Tab::Explore => self.explore.clone().map(AnyView::from),
            Tab::Friends => self.friends.clone().map(AnyView::from),
            Tab::Archive => self.archive.clone().map(AnyView::from),
            _ => Some(chat.clone().into()),
        }
    }

    /// Closes what is open on top and gives the keyboard back to the section.
    fn close_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.over = None;
        self._over_events = None;
        let Screen::Chat(chat) = &self.screen else {
            return;
        };
        let focus = match (self.tab, &self.explore, &self.friends, &self.archive) {
            (Tab::Explore, Some(explore), _, _) => explore.focus_handle(cx),
            (Tab::Friends, _, Some(friends), _) => friends.focus_handle(cx),
            (Tab::Archive, _, _, Some(archive)) => archive.focus_handle(cx),
            _ => chat.focus_handle(cx),
        };
        window.focus(&focus);
        self.report_reading(window);
        cx.notify();
    }

    fn open_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tab;
        let rooms = self.rooms.clone();
        let pictures = self.pictures.clone();
        let drawer = cx.new(|cx| {
            let mut drawer = DrawerView::new(tab, pictures, cx);
            drawer.set_rooms(rooms, cx);
            drawer
        });
        self._over_events = Some(
            cx.subscribe_in(&drawer, window, |this, _, event, window, cx| match event {
                DrawerEvent::Search => this.open_search(window, cx),
                DrawerEvent::Friends => this.open_tab(Tab::Friends, window, cx),
                DrawerEvent::Open(Tab::Chat) => this.open_room("", window, cx),
                DrawerEvent::Open(tab) => this.open_tab(*tab, window, cx),
                DrawerEvent::Room(room_id) => {
                    this.open_room(room_id, window, cx);
                    this.close_over(window, cx);
                }
                DrawerEvent::Close => this.close_over(window, cx),
            }),
        );
        window.focus(&drawer.focus_handle(cx));
        self.over = Some(Over::Drawer(drawer));
        self.report_reading(window);
        cx.notify();
    }

    fn open_invite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let invite = self.social.as_ref().map(|social| social.invite.clone());
        let pictures = self.pictures.clone();
        let sheet = cx.new(|cx| InviteView::new(invite.unwrap_or_default(), pictures, cx));
        self._over_events = Some(cx.subscribe_in(
            &sheet,
            window,
            |this, _, InviteEvent::Close, window, cx| this.close_over(window, cx),
        ));
        window.focus(&sheet.focus_handle(cx));
        self.over = Some(Over::Invite(sheet));
        self.report_reading(window);
        self.commands.send(Command::LoadSocial).ok();
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_character {
            self.after_primary = Some(PrimaryAction::Search);
            self.open_room("", window, cx);
            return;
        }
        let search = match &self.search {
            Some(search) => search.clone(),
            None => {
                let search = cx.new(SearchView::new);
                self._search_events = Some(cx.subscribe_in(
                    &search,
                    window,
                    |this, _, event, window, cx| match event {
                        SearchEvent::Query(query) => {
                            this.commands.send(Command::Search(query.clone())).ok();
                        }
                        SearchEvent::Open(seq) => this.show_message(*seq, window, cx),
                        SearchEvent::Close => this.close_over(window, cx),
                    },
                ));
                self.search = Some(search.clone());
                search
            }
        };
        window.focus(&search.focus_handle(cx));
        self._over_events = None;
        self.over = Some(Over::Search);
        self.report_reading(window);
        cx.notify();
    }

    /// Whether the search screen is open.
    #[cfg(test)]
    fn searching(&self) -> bool {
        matches!(self.over, Some(Over::Search))
    }

    /// Dispatch only supported links. Attachment targets must also belong to a message in
    /// the current room; arbitrary paths are never handed to the OS or to the backend.
    fn open_link(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        match LinkTarget::parse(target) {
            Some(LinkTarget::Web(url)) => cx.open_url(&url),
            Some(LinkTarget::Room(room)) => self.open_room(&room, window, cx),
            Some(LinkTarget::Attachment(path)) => {
                let attachment = self
                    .messages
                    .iter()
                    .flat_map(|message| &message.parts)
                    .filter_map(|part| match part {
                        Part::Files(files) => Some(&files.items),
                        _ => None,
                    })
                    .flatten()
                    .find(|item| item.path == path)
                    .cloned();
                if let Some(attachment) = attachment {
                    let preview = cx.new(|cx| AttachmentView::new(attachment, cx));
                    self._over_events = Some(cx.subscribe_in(
                        &preview,
                        window,
                        |this, view, event, window, cx| match event {
                            AttachmentEvent::Close => this.close_over(window, cx),
                            AttachmentEvent::Retry => {
                                let path = view.read(cx).path().to_owned();
                                this.commands.send(Command::PreviewAttachment(path)).ok();
                            }
                        },
                    ));
                    window.focus(&preview.focus_handle(cx));
                    self.over = Some(Over::Attachment(preview));
                    self.commands.send(Command::PreviewAttachment(path)).ok();
                    self.report_reading(window);
                    cx.notify();
                }
            }
            None => {
                self.notice = Some("This link is not supported in OpenPickle yet.".into());
                cx.notify();
            }
        }
    }

    /// Keep the old conversation until the backend has opened the target successfully.
    fn open_room(&mut self, room_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(Tab::Chat, window, cx);
        self.opening_room = Some(room_id.to_owned());
        if let Screen::Chat(chat) = &self.screen {
            chat.update(cx, |chat, cx| chat.set_can_send(false, cx));
        }
        self.notice = Some("Opening conversation…".into());
        self.report_reading(window);
        if self
            .commands
            .send(Command::OpenRoom(room_id.to_owned()))
            .is_err()
        {
            self.opening_room = None;
            self.after_primary = None;
            if let Screen::Chat(chat) = &self.screen {
                chat.update(cx, |chat, cx| chat.set_can_send(self.can_send, cx));
                window.focus(&chat.focus_handle(cx));
            }
            self.notice =
                Some("OpenPickle has stopped. Reopen the app to load this conversation.".into());
        }
        cx.notify();
    }

    /// Goes to the chat at the message with this sequence number, and marks it for a moment.
    /// A message that is not on this computer (yet) cannot be shown; nothing changes then.
    fn show_message(&mut self, seq: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.messages.iter().position(|message| message.seq == seq) else {
            return;
        };
        let (Screen::Chat(chat), Some(&start)) = (&self.screen, self.row_starts.get(index)) else {
            return;
        };
        let rows = start..start + self.messages[index].parts.len();
        chat.update(cx, |chat, cx| chat.reveal(rows, window, cx));
        self.open_tab(Tab::Chat, window, cx);
    }

    /// Shows the message at once and hands it to the backend, which may be busy for a while:
    /// connecting, or catching up after a long sleep. `offer` is the Explore offer the
    /// message asks for.
    fn send(&mut self, text: &str, offer: Option<String>, cx: &mut Context<Self>) {
        if !self.can_send || self.opening_room.is_some() {
            self.notice = Some("This conversation is read-only or still opening.".into());
            cx.notify();
            return;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.sent += 1;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0., |elapsed| elapsed.as_secs_f64());
        let message = convert::outgoing(id.clone(), text, UNCONFIRMED_SEQ + self.sent, now);
        upsert(&mut self.messages, message);
        if let Some(offer) = &offer {
            self.offers_asked.insert(id.clone(), offer.clone());
        }
        let command = Command::Send {
            room_id: self.room_id.clone(),
            id: id.clone(),
            text: text.to_owned(),
            offer,
        };
        if self.commands.send(command).is_err() {
            mark_not_sent(&mut self.messages, &id, "Not sent. OpenPickle has stopped.");
        }
        self.refresh_rows(cx);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        if let Screen::Chat(chat) = &self.screen {
            let (rows, starts) = build_rows(
                &self.messages,
                self.activity.as_ref().map(|label| label.as_deref()),
                Local::now(),
            );
            self.row_starts = starts;
            chat.update(cx, |chat, cx| chat.set_rows(rows, cx));
        }
    }
}

/// Shows another conversation, or the same one read again. Messages that are still on their
/// way to the server belong to neither yet, so they stay.
fn open_conversation(messages: &mut Vec<Message>, conversation: Vec<Message>) {
    let unconfirmed: Vec<Message> = messages
        .drain(..)
        .filter(|message| message.seq > UNCONFIRMED_SEQ)
        .collect();
    *messages = conversation;
    for message in unconfirmed {
        if !messages.iter().any(|known| known.id == message.id) {
            upsert(messages, message);
        }
    }
}

/// Says under a message that it did not reach the server. A message the server has confirmed
/// in the meantime did reach it, and stays as it is.
fn mark_not_sent(messages: &mut [Message], id: &str, reason: &str) {
    if let Some(message) = messages
        .iter_mut()
        .rfind(|message| message.id == id && message.seq > UNCONFIRMED_SEQ)
    {
        message.parts.push(Part::Task(reason.into()));
    }
}

/// Replaces the message with the same id, or inserts a new one, keeping the list ordered by
/// sequence number. Messages still being sent carry the highest numbers, so they stay last.
fn upsert(messages: &mut Vec<Message>, message: Message) {
    if let Some(index) = messages.iter().rposition(|known| known.id == message.id) {
        if messages[index].seq == message.seq {
            messages[index] = message;
            return;
        }
        messages.remove(index);
    }
    let index = messages.partition_point(|known| known.seq <= message.seq);
    messages.insert(index, message);
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = div()
            .size_full()
            .relative()
            .bg(theme::color(theme::PAGE))
            .on_action(
                cx.listener(|this, _: &OpenChat, window, cx| this.open_tab(Tab::Chat, window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenExplore, window, cx| {
                this.open_tab(Tab::Explore, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenFriends, window, cx| {
                this.open_tab(Tab::Friends, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenArchive, window, cx| {
                this.open_tab(Tab::Archive, window, cx)
            }));
        match &self.screen {
            Screen::Starting => screen,
            Screen::Login(login) => screen.child(login.clone()),
            Screen::Chat(_) => {
                let over: Option<AnyView> = match &self.over {
                    Some(Over::Drawer(drawer)) => Some(drawer.clone().into()),
                    Some(Over::Invite(invite)) => Some(invite.clone().into()),
                    Some(Over::Attachment(preview)) => Some(preview.clone().into()),
                    Some(Over::Search) => self.search.clone().map(AnyView::from),
                    None => None,
                };
                screen
                    .children(self.section())
                    .children(over)
                    .children(self.notice.clone().map(|notice| {
                        div()
                            .absolute()
                            .top(px(132.))
                            .left(px(16.))
                            .right(px(16.))
                            .p(px(12.))
                            .rounded(px(12.))
                            .bg(theme::color(theme::CARD))
                            .text_color(theme::color(theme::TEXT))
                            .font_family(theme::FONT)
                            .text_size(px(13.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().child(notice))
                            .child(
                                div()
                                    .id("dismiss-notice")
                                    .cursor_pointer()
                                    .child("×")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.notice = None;
                                        cx.notify();
                                    })),
                            )
                    }))
            }
            Screen::Failed(reason) => screen
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .px(px(36.))
                .font_family(theme::FONT)
                .text_size(theme::BODY)
                .line_height(theme::BODY_LINE)
                .text_center()
                .text_color(theme::color(theme::TEXT_MUTED))
                .child(
                    div()
                        .text_size(theme::LABEL)
                        .line_height(theme::LABEL_LINE)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::color(theme::TEXT))
                        .child("OpenPickle had to stop"),
                )
                .child(reason.clone())
                .child("Quit and open the app again."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SearchResult, Side};

    fn message(id: &str, seq: u64) -> Message {
        Message {
            id: id.into(),
            seq,
            at: seq as f64,
            side: Side::Mine,
            parts: vec![],
        }
    }

    #[gpui::test]
    fn a_typed_message_shows_at_once_and_goes_to_the_backend(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let conversation = Update::Conversation {
            room_id: "primary".into(),
            is_character: true,
            can_send: true,
            name: "Pickle".into(),
            avatar: None,
            messages: vec![message("a", 1)],
        };
        updates.unbounded_send(conversation).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));

        cx.simulate_input("안녕");
        cx.simulate_keystrokes("enter");
        // Nothing answers the command, as when the backend is busy. The message shows anyway.
        let (id, text) = std::iter::from_fn(|| sent.try_recv().ok())
            .find_map(|command| match command {
                Command::Send { id, text, .. } => Some((id, text)),
                _ => None,
            })
            .expect("the message goes to the backend");
        assert_eq!(text, "안녕");
        shell.read_with(cx, |shell, _| {
            let shown = shell.messages.last().unwrap();
            assert_eq!((&shown.id, shown.seq > UNCONFIRMED_SEQ), (&id, true));
            assert_eq!(shown.parts.len(), 1);
        });

        let failed = Update::SendFailed {
            room_id: "primary".into(),
            id,
            reason: "Not sent.".into(),
        };
        updates.unbounded_send(failed).unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            let parts = &shell.messages.last().unwrap().parts;
            assert_eq!(parts.last(), Some(&Part::Task("Not sent.".into())));
        });
    }

    #[gpui::test]
    fn messages_count_as_read_only_while_the_chat_shows(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let conversation = Update::Conversation {
            room_id: "primary".into(),
            is_character: true,
            can_send: true,
            name: "Pickle".into(),
            avatar: None,
            messages: vec![message("a", 1)],
        };
        updates.unbounded_send(conversation).unwrap();
        let (_, cx) = cx.add_window_view(|window, cx| {
            window.activate_window();
            Shell::new(commands, received, window, cx)
        });
        let mut reading = |cx: &mut gpui::VisualTestContext, keys: &str| {
            cx.simulate_keystrokes(keys);
            std::iter::from_fn(|| sent.try_recv().ok())
                .filter_map(|command| match command {
                    Command::Active(reading) => Some(reading),
                    _ => None,
                })
                .last()
        };
        reading(cx, "");
        // Another section hides the chat, so what arrives meanwhile stays unread.
        assert_eq!(reading(cx, "cmd-2"), Some(false));
        assert_eq!(reading(cx, "cmd-3"), Some(false));
        assert_eq!(reading(cx, "cmd-1"), Some(true));
    }

    #[gpui::test]
    fn an_offer_whose_message_failed_is_released_in_explore(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let conversation = Update::Conversation {
            room_id: "primary".into(),
            is_character: true,
            can_send: true,
            name: "Pickle".into(),
            avatar: None,
            messages: vec![message("a", 1)],
        };
        updates.unbounded_send(conversation).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        cx.simulate_keystrokes("cmd-2");
        let explore = shell.read_with(cx, |shell, _| shell.explore.clone().unwrap());
        let offer = std::sync::Arc::new(crate::model::Offer {
            id: "offer-1".into(),
            section: "money_back".into(),
            title: String::new(),
            detail: String::new(),
            ask: "환급금 찾아줘".into(),
            picture: String::new(),
            symbol: String::new(),
            asked_by: String::new(),
            steps: Vec::new(),
            areas: Vec::new(),
        });
        explore.update(cx, |explore, cx| explore.start(offer, cx));
        cx.run_until_parked();
        let id = std::iter::from_fn(|| sent.try_recv().ok())
            .find_map(|command| match command {
                Command::Send { id, .. } => Some(id),
                _ => None,
            })
            .unwrap();
        explore.read_with(cx, |explore, _| assert!(explore.has_asked("offer-1")));
        updates
            .unbounded_send(Update::SendFailed {
                room_id: "primary".into(),
                id,
                reason: "Not sent.".into(),
            })
            .unwrap();
        cx.run_until_parked();
        explore.read_with(cx, |explore, _| assert!(!explore.has_asked("offer-1")));
        shell.read_with(cx, |shell, _| assert!(shell.offers_asked.is_empty()));
    }

    #[gpui::test]
    fn the_sections_load_when_opened_and_an_offer_is_asked_for_in_the_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let conversation = Update::Conversation {
            room_id: "primary".into(),
            is_character: true,
            can_send: true,
            name: "Pickle".into(),
            avatar: None,
            messages: vec![message("a", 1)],
        };
        updates.unbounded_send(conversation).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        let mut asked = |cx: &mut gpui::VisualTestContext, keys: &str| {
            cx.simulate_keystrokes(keys);
            std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<Command>>()
        };

        // Each section asks the backend for its data when it is opened.
        let commands = asked(cx, "cmd-3");
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::LoadSocial))
        );
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Friends));
        let commands = asked(cx, "cmd-2");
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::LoadExplore))
        );

        // Asking for an offer sends its text with the offer's id, and shows it in the chat.
        let explore = shell.read_with(cx, |shell, _| shell.explore.clone().unwrap());
        explore.update(cx, |_, cx| {
            cx.emit(ExploreEvent::Ask {
                offer: "offer-1".into(),
                text: "환급금 찾아줘".into(),
            })
        });
        cx.run_until_parked();
        let commands = asked(cx, "");
        let offer = commands.iter().find_map(|command| match command {
            Command::Send { text, offer, .. } => Some((text.as_str(), offer.as_deref())),
            _ => None,
        });
        assert_eq!(offer, Some(("환급금 찾아줘", Some("offer-1"))));
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.tab, Tab::Chat);
            assert!(shell.messages.last().unwrap().seq > UNCONFIRMED_SEQ);
        });

        // The invite sheet opens over a section and Escape closes it again.
        let chat = shell.read_with(cx, |shell, _| match &shell.screen {
            Screen::Chat(chat) => chat.clone(),
            _ => panic!("signed in"),
        });
        chat.update(cx, |_, cx| cx.emit(ChatEvent::Invite));
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert!(matches!(shell.over, Some(Over::Invite(_))))
        });
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
    }

    #[gpui::test]
    fn a_search_result_opens_its_message_and_closes_the_search(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let part = Part::Task("내일 비 온대요".into());
        let conversation = Update::Conversation {
            room_id: "primary".into(),
            is_character: true,
            can_send: true,
            name: "Pickle".into(),
            avatar: None,
            messages: vec![Message {
                parts: vec![part],
                ..message("a", 1)
            }],
        };
        updates.unbounded_send(conversation).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));

        cx.simulate_resize(gpui::size(gpui::px(440.), gpui::px(900.)));

        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("비");
        cx.simulate_keystrokes("enter");
        let asked = std::iter::from_fn(|| sent.try_recv().ok()).find_map(|command| match command {
            Command::Search(query) => Some(query),
            _ => None,
        });
        assert_eq!(asked.as_deref(), Some("비"));

        let found = |seq| SearchResult {
            seq: Some(seq),
            label: "Chat".into(),
            date: "Nov 16".into(),
            snippet: "내일 비 온대요".into(),
            matches: vec![0..6, 7..10],
        };
        // The first result is a message this computer does not have; the second one it has.
        let results = Update::Found {
            query: "비".into(),
            results: Ok(vec![found(99), found(1)]),
        };
        updates.unbounded_send(results).unwrap();
        cx.run_until_parked();
        let first = gpui::point(gpui::px(120.), gpui::px(75.));
        cx.simulate_click(first, gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert!(shell.searching()));
        cx.simulate_click(
            first + gpui::point(gpui::px(0.), gpui::px(70.)),
            gpui::Modifiers::none(),
        );
        shell.read_with(cx, |shell, _| assert!(!shell.searching()));

        // Escape closes the search too.
        cx.simulate_keystrokes("cmd-f");
        shell.read_with(cx, |shell, _| assert!(shell.searching()));
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| assert!(!shell.searching()));
    }

    #[test]
    fn a_message_on_its_way_survives_the_conversation_arriving_and_is_then_confirmed() {
        // Typed while the backend was still connecting.
        let mut messages = vec![message("typed", UNCONFIRMED_SEQ + 1)];
        open_conversation(&mut messages, vec![message("a", 1), message("b", 2)]);
        assert_eq!(
            messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "typed"]
        );

        // The server's copy takes its place. A failure reported after that is stale.
        upsert(&mut messages, message("typed", 3));
        mark_not_sent(&mut messages, "typed", "Not sent.");
        assert_eq!(messages.last(), Some(&message("typed", 3)));

        // Reading the conversation again does not bring a confirmed message back as pending.
        let mut messages = vec![message("typed", UNCONFIRMED_SEQ + 1)];
        open_conversation(&mut messages, vec![message("a", 1), message("typed", 2)]);
        assert_eq!(messages, [message("a", 1), message("typed", 2)]);
    }

    #[test]
    fn a_message_that_was_not_sent_says_so() {
        let mut messages = vec![message("a", 1), message("typed", UNCONFIRMED_SEQ + 1)];
        mark_not_sent(&mut messages, "typed", "Not sent. Check your connection.");
        assert_eq!(
            messages[1].parts,
            [Part::Task("Not sent. Check your connection.".into())]
        );
        assert!(messages[0].parts.is_empty());
    }

    #[test]
    fn upsert_orders_by_seq_and_confirms_pending_messages() {
        let mut messages = vec![message("a", 1), message("c", 5)];
        upsert(&mut messages, message("b", 3));
        upsert(&mut messages, message("sent", u64::MAX));
        upsert(&mut messages, message("d", 7));
        assert_eq!(
            messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c", "d", "sent"]
        );
        // The server confirms the pending message with its real sequence number.
        upsert(&mut messages, message("sent", 6));
        assert_eq!(
            messages
                .iter()
                .map(|m| (m.id.as_str(), m.seq))
                .collect::<Vec<_>>(),
            [("a", 1), ("b", 3), ("c", 5), ("sent", 6), ("d", 7)]
        );
    }

    #[gpui::test]
    fn room_switches_isolate_updates_and_keep_each_composers_draft(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let conversation = |room: &str, primary: bool| Update::Conversation {
            room_id: room.into(),
            is_character: primary,
            can_send: primary,
            name: room.into(),
            avatar: None,
            messages: vec![message(room, 1)],
        };
        updates
            .unbounded_send(conversation("primary", true))
            .unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        cx.simulate_input("unsent draft");
        shell.update_in(cx, |shell, window, cx| shell.open_room("peer", window, cx));
        updates.unbounded_send(conversation("peer", false)).unwrap();
        updates
            .unbounded_send(Update::Upsert {
                room_id: "primary".into(),
                messages: vec![message("primary-only", 2)],
            })
            .unwrap();
        updates
            .unbounded_send(Update::Activity {
                room_id: "primary".into(),
                activity: Some(None),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.room_id, "peer");
            assert_eq!(shell.messages.len(), 1);
            assert_eq!(shell.messages[0].id, "peer");
            assert!(shell.activity.is_none());
            assert!(!shell.can_send);
        });
        updates
            .unbounded_send(conversation("primary", true))
            .unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        let send = std::iter::from_fn(|| sent.try_recv().ok()).find_map(|command| match command {
            Command::Send { room_id, text, .. } => Some((room_id, text)),
            _ => None,
        });
        assert_eq!(send, Some(("primary".into(), "unsent draft".into())));
    }

    #[gpui::test]
    fn attachment_links_request_previews_and_reject_unknown_paths(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let item = serde_json::from_value(serde_json::json!({
            "id":"file-message", "role":"assistant", "seq":1, "at":1.,
            "text":"<bubble>[Report](downloads/report.pdf)</bubble>",
            "inline_files":{"downloads/report.pdf":"attachments/2026-01-01/report.pdf"},
            "file_meta":{"attachments/2026-01-01/report.pdf":{"name":"Report.pdf","size":4000,"pages":4}}
        })).unwrap();
        updates
            .unbounded_send(Update::Conversation {
                room_id: "primary".into(),
                is_character: true,
                can_send: true,
                name: "Pickle".into(),
                avatar: None,
                messages: vec![convert::message(&item, None)],
            })
            .unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        shell.update_in(cx, |shell, window, cx| {
            shell.open_link("attachment:attachments/2026-01-01/report.pdf", window, cx)
        });
        let requested = std::iter::from_fn(|| sent.try_recv().ok()).any(|command| {
            matches!(command,
            Command::PreviewAttachment(path) if path == "attachments/2026-01-01/report.pdf")
        });
        assert!(requested);
        updates
            .unbounded_send(Update::AttachmentPreview {
                path: "attachments/other.pdf".into(),
                result: Err("unrelated".into()),
            })
            .unwrap();
        updates
            .unbounded_send(Update::AttachmentPreview {
                path: "attachments/2026-01-01/report.pdf".into(),
                result: Err("Preview unavailable".into()),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, cx| match &shell.over {
            Some(Over::Attachment(view)) => {
                assert_eq!(view.read(cx).path(), "attachments/2026-01-01/report.pdf")
            }
            _ => panic!("attachment preview must stay open"),
        });
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
        shell.update_in(cx, |shell, window, cx| {
            shell.open_link("attachment:attachments/unknown.pdf", window, cx)
        });
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
    }

    #[gpui::test]
    fn a_room_switch_hides_the_previous_rooms_archive_before_loading(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::model::{Attachment, AttachmentKind};
        cx.update(crate::ui::bind_keys);
        let (commands, _sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let conversation = |room: &str| Update::Conversation {
            room_id: room.into(),
            is_character: room == "primary",
            can_send: false,
            name: room.into(),
            avatar: None,
            messages: vec![message(room, 1)],
        };
        updates.unbounded_send(conversation("primary")).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        cx.simulate_resize(gpui::size(px(440.), px(900.)));
        cx.simulate_keystrokes("cmd-4");
        let old_archive = || Update::Archive {
            room_id: "primary".into(),
            complete: true,
            attachments: vec![Attachment {
                path: "attachments/2026-01-01/old.jpg".into(),
                seq: 1,
                at: 1.,
                name: "Old image".into(),
                detail: String::new(),
                kind: AttachmentKind::Picture,
            }],
        };
        updates.unbounded_send(old_archive()).unwrap();
        cx.run_until_parked();
        updates.unbounded_send(conversation("peer")).unwrap();
        cx.run_until_parked();
        cx.simulate_click(gpui::point(px(60.), px(280.)), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Archive));
        // An old response cannot refill the view after it was cleared.
        updates.unbounded_send(old_archive()).unwrap();
        cx.run_until_parked();
        cx.simulate_click(gpui::point(px(60.), px(280.)), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Archive));
    }

    #[gpui::test]
    fn a_local_room_open_failure_preserves_the_current_composer(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        updates
            .unbounded_send(Update::Conversation {
                room_id: "primary".into(),
                is_character: true,
                can_send: true,
                name: "Pickle".into(),
                avatar: None,
                messages: vec![],
            })
            .unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        cx.simulate_input("kept draft");
        drop(sent);
        shell.update_in(cx, |shell, window, cx| shell.open_room("peer", window, cx));
        cx.simulate_keystrokes("enter");
        shell.read_with(cx, |shell, _| {
            assert!(shell.opening_room.is_none());
            assert_eq!(shell.room_id, "primary");
            let shown = shell
                .messages
                .last()
                .expect("the composer was still writable");
            assert!(shown.parts.iter().any(|part| matches!(part, Part::Task(_))));
        });
    }
}
