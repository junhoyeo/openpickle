//! The root view: shows sign-in or the signed-in sections, and applies what the backend
//! reports.

use std::{
    collections::HashSet,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::Local;
use futures::{StreamExt, channel::mpsc::UnboundedReceiver};
use gpui::{
    AnyView, Context, Entity, Focusable, FontWeight, ImageSource, PromptButton, PromptLevel,
    SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    content::LinkTarget,
    convert,
    model::{Attachment, Message, Part, RoomInfo, Social, UNCONFIRMED_SEQ, build_rows},
    phone::CallSummary,
    protocol::{Command, HandoffCard, RoomMetadata, SearchRequest, Update},
    theme,
    ui::{
        Identity, OpenArchive, OpenChat, OpenExplore, OpenFriends, OpenPhoneCalls, OpenProfile,
        Pictures, Tab,
        archive::{ArchiveEvent, ArchiveView},
        attachment::{AttachmentEvent, AttachmentView},
        chat::{ChatEvent, ChatView},
        drawer::{DrawerEvent, DrawerView},
        explore::{ExploreEvent, ExploreView},
        friends::{FriendsEvent, FriendsView},
        invite::{InviteEvent, InviteView},
        login::{LoginEvent, LoginView},
        phone::{PhoneEvent, PhoneView},
        profile::{ProfileEvent, ProfileView},
        search::{SearchEvent, SearchView},
        tasks::{TaskEvent, TaskView},
    },
};

enum Screen {
    /// Nothing is known yet; the backend answers within the first frames.
    Starting,
    Login(Entity<LoginView>),
    /// Signed in. The chat always exists; the other sections are made when first opened.
    Chat(Entity<ChatView>),
    /// The user confirmed local sign-out; the backend is clearing the account.
    SigningOut,
    /// The backend stopped; the text says why.
    Failed(SharedString),
}

/// What is open on top of a section.
enum Over {
    Drawer(Entity<DrawerView>),
    Invite(Entity<InviteView>),
    Attachment(Entity<AttachmentView>),
    Phone(Entity<PhoneView>),
    Task(Entity<TaskView>),
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
    profile: Option<Entity<ProfileView>>,
    over: Option<Over>,
    /// The search screen. It is kept when it closes, so that opening it again shows the same
    /// search: a result is opened to read around it, and then usually the next one.
    search: Option<Entity<SearchView>>,
    search_generation: u64,
    pending_search: Option<u64>,
    /// The character's name and picture, for the header of every section.
    identity: Identity,
    /// The pictures on disk, which the sections draw from.
    pictures: Entity<Pictures>,
    /// GPUI caches decoded images by path, and a different account can reuse the same path.
    account_images: HashSet<PathBuf>,
    /// The friends and the invite link as last read.
    social: Option<Social>,
    /// The other chat rooms as last listed.
    rooms: Vec<RoomInfo>,
    calls: Vec<CallSummary>,
    /// The tasks the chat shows a task card for, by their handoff path: what a press on the
    /// card opens.
    tasks: std::collections::HashMap<String, openpickle_api::HandoffItem>,
    /// The title of the task the character works on now.
    active_task: Option<String>,
    pending_phone_task: Option<String>,
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
            profile: None,
            over: None,
            search: None,
            search_generation: 0,
            pending_search: None,
            identity: Identity::default(),
            pictures: cx.new(|_| Pictures::default()),
            account_images: HashSet::new(),
            social: None,
            rooms: Vec::new(),
            calls: Vec::new(),
            tasks: std::collections::HashMap::new(),
            active_task: None,
            pending_phone_task: None,
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
        // An operation already in flight can finish before the backend sees SignOut.
        if matches!(self.screen, Screen::SigningOut)
            && !matches!(update, Update::SignedOut | Update::Failed(_))
        {
            return;
        }
        match &update {
            Update::Conversation {
                avatar: Some(path), ..
            }
            | Update::Identity {
                avatar: Some(path), ..
            }
            | Update::AttachmentPreview {
                result: Ok(path), ..
            } => {
                self.account_images.insert(path.clone());
            }
            Update::Pictures(pictures) => {
                self.account_images
                    .extend(pictures.iter().map(|(_, path)| path.clone()));
            }
            _ => {}
        }
        match update {
            Update::SignedOut => {
                self.forget_account(cx);
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
                    self.set_over(None);
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
                    if self.tab == Tab::Chat && !self.searching() {
                        window.focus(&chat.focus_handle(cx));
                    }
                }
                self.show_active_task(cx);
                self.refresh_room_avatar(cx);
                self.refresh_rows(cx);
                if self.tab == Tab::Archive && changed_room && !self.room_id.is_empty() {
                    self.commands
                        .send(Command::WatchArchive(Some(self.room_id.clone())))
                        .ok();
                }
                self.report_reading(window);
                if is_character && let Some(action) = self.after_primary.take() {
                    match action {
                        PrimaryAction::Search => self.open_search(window, cx),
                        PrimaryAction::Ask { text, offer } => self.send(&text, Some(offer), cx),
                    }
                }
            }
            Update::RoomMetadata(RoomMetadata {
                room_id,
                is_character,
                can_send,
                name,
                avatar,
            }) => {
                if room_id == self.room_id {
                    self.is_character = is_character;
                    self.can_send = can_send;
                    let avatar = avatar.map(ImageSource::from);
                    self.room_header = Identity {
                        name: name.clone().into(),
                        avatar: avatar.clone(),
                    };
                    if is_character {
                        self.set_identity(name, avatar, cx);
                    }
                    if let Screen::Chat(chat) = &self.screen {
                        chat.update(cx, |chat, cx| {
                            chat.set_can_send(can_send && self.opening_room.is_none(), cx);
                            chat.set_identity(
                                self.room_header.name.clone(),
                                self.room_header.avatar.clone(),
                                cx,
                            );
                        });
                    }
                    self.show_active_task(cx);
                    self.refresh_room_avatar(cx);
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
            Update::AttachmentDownloadProgress {
                room_id,
                path,
                received,
                total,
            } => {
                if room_id == self.room_id
                    && let Some(Over::Attachment(view)) = &self.over
                    && view.read(cx).path() == path
                {
                    view.update(cx, |view, cx| view.download_progress(received, total, cx));
                }
            }
            Update::AttachmentDownloaded {
                room_id,
                path,
                result,
            } => {
                if room_id != self.room_id {
                    return;
                }
                if let Some(Over::Attachment(view)) = &self.over
                    && view.read(cx).path() == path
                {
                    view.update(cx, |view, cx| view.downloaded(result, cx));
                } else {
                    // The preview was closed meanwhile: the file is saved all the same.
                    self.notice = Some(match result {
                        Ok(file) => {
                            let name = file.file_name().unwrap_or_default().to_string_lossy();
                            format!("Saved {name} to your downloads.").into()
                        }
                        Err(reason) => reason.into(),
                    });
                    cx.notify();
                }
            }
            Update::PhoneTask { path, result } => {
                if self.pending_phone_task.as_deref() == Some(path.as_str()) {
                    self.pending_phone_task = None;
                    match result {
                        Ok(call_id) => {
                            self.notice = None;
                            self.open_phone(Some(call_id), window, cx);
                        }
                        Err(reason) => {
                            self.notice = Some(reason.into());
                            cx.notify();
                        }
                    }
                }
            }
            Update::PhoneCalls(result) => {
                if let Ok(calls) = &result {
                    self.calls = calls.clone();
                    if let Some(Over::Drawer(drawer)) = &self.over {
                        drawer.update(cx, |drawer, cx| drawer.set_calls(calls.clone(), cx));
                    }
                }
                if let Some(Over::Phone(phone)) = &self.over {
                    phone.update(cx, |phone, cx| phone.set_calls(result, cx));
                }
            }
            Update::HandoffCard { path, card } => {
                match &card {
                    HandoffCard::Task(task) => {
                        self.tasks.insert(path.clone(), task.clone());
                    }
                    HandoffCard::Phone(_) => {
                        self.tasks.remove(&path);
                    }
                }
                if let Screen::Chat(chat) = &self.screen {
                    chat.update(cx, |chat, cx| chat.set_handoff_card(path, card, cx));
                }
            }
            Update::ActiveTask(title) => {
                self.active_task = title;
                self.show_active_task(cx);
            }
            Update::Handoff { path, result } => {
                if let Some(Over::Task(task)) = &self.over {
                    task.update(cx, |task, cx| task.set_handoff(&path, result, cx));
                }
            }
            Update::PhoneCall { call_id, result } => {
                // The chat's card of this call shows what the open transcript just read.
                if let (Ok(call), Screen::Chat(chat)) = (&result, &self.screen) {
                    chat.update(cx, |chat, cx| chat.refresh_phone_card(call, cx));
                }
                if let Some(Over::Phone(phone)) = &self.over {
                    phone.update(cx, |phone, cx| phone.set_call(&call_id, result, cx));
                }
            }
            Update::PhoneCallEnded { call_id, result } => {
                if let Some(Over::Phone(phone)) = &self.over {
                    phone.update(cx, |phone, cx| phone.set_end_result(&call_id, result, cx));
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
            Update::Found { request, results } => {
                if self.accept_search(&request)
                    && let Some(search) = &self.search
                {
                    search.update(cx, |search, cx| search.found(results, cx));
                }
            }
            Update::SearchResultOpened {
                request,
                seq,
                result,
            } => {
                if !self.accept_search(&request) {
                    return;
                }
                let reason = match result {
                    Ok(messages) => {
                        self.apply(
                            Update::Upsert {
                                room_id: request.room_id,
                                messages,
                            },
                            window,
                            cx,
                        );
                        if self.show_message(seq, window, cx) {
                            return;
                        }
                        "This message can no longer be shown. Choose another result.".into()
                    }
                    Err(reason) => reason.into(),
                };
                if let Some(search) = &self.search {
                    search.update(cx, |search, cx| search.set_opening(reason, cx));
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
            Update::Profile(data) => {
                if let Some(profile) = &self.profile {
                    profile.update(cx, |profile, cx| profile.set_profile(data, cx));
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
                if self.tab == Tab::Archive
                    && room_id == self.room_id
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
                self.forget_account(cx);
                self.screen = Screen::Failed(reason.into());
                cx.notify();
            }
        }
    }

    /// Drops every local projection of the signed-in account before another one can open.
    fn forget_account(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Chat;
        self.explore = None;
        self.friends = None;
        self.archive = None;
        self.profile = None;
        self.over = None;
        self.search = None;
        self.pending_search = None;
        self.social = None;
        self.rooms.clear();
        self.calls.clear();
        self.tasks.clear();
        self.active_task = None;
        self.pending_phone_task = None;
        self.messages.clear();
        self.identity = Identity::default();
        self.room_header = Identity::default();
        self.pictures = cx.new(|_| Pictures::default());
        for path in self.account_images.drain() {
            ImageSource::from(path).remove_asset(cx);
        }
        self.activity = None;
        self.row_starts.clear();
        self.sent = 0;
        self.is_character = true;
        self.drafts.clear();
        self.pending.clear();
        self.offers_asked.clear();
        self.room_id.clear();
        self.opening_room = None;
        self.after_primary = None;
        self.notice = None;
        self.can_send = false;
        self._screen_events = None;
        self._section_events.clear();
        self._over_events = None;
        self._search_events = None;
    }

    fn refresh_room_avatar(&mut self, cx: &mut Context<Self>) {
        if self.is_character {
            return;
        }
        let avatar = self
            .rooms
            .iter()
            .find(|room| room.room_id == self.room_id)
            .and_then(|room| self.pictures.read(cx).get(&room.picture));
        self.room_header.avatar = avatar.clone();
        if let Screen::Chat(chat) = &self.screen {
            chat.update(cx, |chat, cx| {
                chat.set_identity(self.room_header.name.clone(), avatar, cx)
            });
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
        if let Some(profile) = &self.profile {
            profile.update(cx, |profile, cx| profile.set_identity(identity.clone(), cx));
        }
        if let Some(Over::Phone(phone)) = &self.over {
            phone.update(cx, |phone, cx| phone.set_identity(identity.clone(), cx));
        }
    }

    fn confirm_sign_out(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Chat(chat) = &self.screen else {
            return;
        };
        let account_view = chat.entity_id();
        let answer = window.prompt(
            PromptLevel::Warning,
            "Sign out on this computer?",
            Some("This removes the saved sign-in and cached conversations from OpenPickle. Your Pickle account and files saved to Downloads will not change."),
            &[PromptButton::cancel("Cancel"), PromptButton::new("Sign out")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if !matches!(answer.await, Ok(1)) {
                return;
            }
            this.update(cx, |this, cx| {
                if !matches!(&this.screen, Screen::Chat(chat) if chat.entity_id() == account_view) {
                    return;
                }
                this.forget_account(cx);
                this.screen = if this.commands.send(Command::SignOut).is_ok() {
                    Screen::SigningOut
                } else {
                    Screen::Failed("Could not sign out: the backend already stopped.".into())
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                ChatEvent::PhoneCalls => this.open_phone(None, window, cx),
                ChatEvent::Link(target) => this.open_link(target, window, cx),
                ChatEvent::LoadAttachments(paths) => {
                    this.commands
                        .send(Command::LoadAttachments(paths.clone()))
                        .ok();
                }
                ChatEvent::LoadHandoffCard(path) => {
                    this.commands
                        .send(Command::LoadHandoffCard(path.clone()))
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
        if self.tab == Tab::Archive && tab != Tab::Archive {
            self.commands.send(Command::WatchArchive(None)).ok();
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
                            ArchiveEvent::PreviewFile(path) | ArchiveEvent::DownloadFile(path) => {
                                this.open_archive_file(
                                    path,
                                    matches!(event, ArchiveEvent::DownloadFile(_)),
                                    window,
                                    cx,
                                );
                            }
                            ArchiveEvent::Load(paths) => {
                                this.commands
                                    .send(Command::LoadAttachments(paths.clone()))
                                    .ok();
                            }
                        },
                    ));
                    self.archive = Some(archive);
                }
                if !self.room_id.is_empty() {
                    self.commands
                        .send(Command::WatchArchive(Some(self.room_id.clone())))
                        .ok();
                }
            }
            Tab::Profile => {
                if self.profile.is_none() {
                    let (identity, pictures) = (self.identity.clone(), self.pictures.clone());
                    let profile = cx.new(|cx| ProfileView::new(identity, pictures, cx));
                    self._section_events.push(cx.subscribe_in(
                        &profile,
                        window,
                        |this, _, event, window, cx| match event {
                            ProfileEvent::Menu => this.open_drawer(window, cx),
                            ProfileEvent::Invite => this.open_invite(window, cx),
                            ProfileEvent::Open(tab) => this.open_tab(*tab, window, cx),
                            ProfileEvent::SignOut => this.confirm_sign_out(window, cx),
                            ProfileEvent::Reload => {
                                this.commands.send(Command::LoadProfile).ok();
                            }
                            ProfileEvent::Link(target) => {
                                // These are fixed destinations in the captured Profile definitions.
                                if matches!(
                                    target.as_str(),
                                    "https://pickle.com/privacy?view=app"
                                        | "https://pickle.com/help?view=app"
                                        | "mailto:contact@pickle.com"
                                ) {
                                    cx.open_url(target);
                                }
                            }
                        },
                    ));
                    self.profile = Some(profile);
                }
                self.commands.send(Command::LoadProfile).ok();
            }
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
            Tab::Profile => self.profile.clone().map(AnyView::from),
            _ => Some(chat.clone().into()),
        }
    }

    /// Puts a view over the screen, or with `None` takes down the one that is there. The
    /// backend stops reading for the view that goes: a call's transcript, a task's progress.
    fn set_over(&mut self, over: Option<Over>) {
        match self.over {
            Some(Over::Phone(_)) => {
                self.commands.send(Command::WatchPhoneCall(None)).ok();
            }
            Some(Over::Task(_)) => {
                self.commands.send(Command::WatchHandoff(None)).ok();
            }
            Some(Over::Search) if !matches!(over, Some(Over::Search)) => self.cancel_search(),
            _ => {}
        }
        self.over = over;
    }

    /// Closes what is open on top and gives the keyboard back to the section.
    fn close_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_over(None);
        self.pending_phone_task = None;
        self._over_events = None;
        let Screen::Chat(chat) = &self.screen else {
            return;
        };
        let focus = if self.tab == Tab::Profile {
            self.profile
                .as_ref()
                .map(|profile| profile.focus_handle(cx))
                .unwrap_or_else(|| chat.focus_handle(cx))
        } else {
            match (self.tab, &self.explore, &self.friends, &self.archive) {
                (Tab::Explore, Some(explore), _, _) => explore.focus_handle(cx),
                (Tab::Friends, _, Some(friends), _) => friends.focus_handle(cx),
                (Tab::Archive, _, _, Some(archive)) => archive.focus_handle(cx),
                _ => chat.focus_handle(cx),
            }
        };
        window.focus(&focus);
        self.report_reading(window);
        cx.notify();
    }

    fn open_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tab;
        let rooms = self.rooms.clone();
        let calls = self.calls.clone();
        let pictures = self.pictures.clone();
        let drawer = cx.new(|cx| {
            let mut drawer = DrawerView::new(tab, pictures, cx);
            drawer.set_rooms(rooms, cx);
            drawer.set_calls(calls, cx);
            drawer
        });
        self._over_events = Some(
            cx.subscribe_in(&drawer, window, |this, _, event, window, cx| match event {
                DrawerEvent::PhoneCalls => this.open_phone(None, window, cx),
                DrawerEvent::Phone(call_id) => this.open_phone(Some(call_id.clone()), window, cx),
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
        self.set_over(Some(Over::Drawer(drawer)));
        self.commands.send(Command::LoadPhoneCalls).ok();
        self.report_reading(window);
        cx.notify();
    }

    fn request_phone_list(&mut self, cx: &mut Context<Self>) {
        if self.commands.send(Command::LoadPhoneCalls).is_err()
            && let Some(Over::Phone(phone)) = &self.over
        {
            phone.update(cx, |phone, cx| {
                phone.set_calls(Err("OpenPickle has stopped. Reopen the app.".into()), cx)
            });
        }
    }

    fn request_phone_call(&mut self, call_id: &str, cx: &mut Context<Self>) {
        if self
            .commands
            .send(Command::WatchPhoneCall(Some(call_id.to_owned())))
            .is_err()
            && let Some(Over::Phone(phone)) = &self.over
        {
            phone.update(cx, |phone, cx| {
                phone.set_call(
                    call_id,
                    Err("OpenPickle has stopped. Reopen the app.".into()),
                    cx,
                )
            });
        }
    }

    /// The header of the character's own chat names the task the character works on. A
    /// friend's room has no such line: the task is not theirs.
    fn show_active_task(&self, cx: &mut Context<Self>) {
        if let Screen::Chat(chat) = &self.screen {
            let task = self.active_task.clone().filter(|_| self.is_character);
            chat.update(cx, |chat, cx| chat.set_active_task(task, cx));
        }
    }

    /// Opens the progress of a task over the chat, starting from what the handoff list said
    /// about it, and has the backend read the rest for as long as the sheet is open.
    fn open_task(
        &mut self,
        path: String,
        summary: openpickle_api::HandoffItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pictures = self.pictures.clone();
        let watch_path = path.clone();
        let view = cx.new(|cx| TaskView::new(path, summary, pictures, cx));
        self._over_events = Some(cx.subscribe_in(
            &view,
            window,
            |this, _, TaskEvent::Close, window, cx| this.close_over(window, cx),
        ));
        window.focus(&view.focus_handle(cx));
        self.set_over(Some(Over::Task(view)));
        self.report_reading(window);
        if self
            .commands
            .send(Command::WatchHandoff(Some(watch_path)))
            .is_err()
            && let Some(Over::Task(task)) = &self.over
        {
            let path = task.read(cx).path().to_owned();
            task.update(cx, |task, cx| {
                task.set_handoff(
                    &path,
                    Err("OpenPickle has stopped. Reopen the app.".into()),
                    cx,
                )
            });
        }
        cx.notify();
    }

    fn open_phone(&mut self, call_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_phone_task = None;
        let identity = self.identity.clone();
        let initial_call = call_id.clone();
        let view = cx.new(|cx| PhoneView::new(identity, initial_call, cx));
        if !self.calls.is_empty() {
            view.update(cx, |view, cx| view.set_calls(Ok(self.calls.clone()), cx));
        }
        self._over_events =
            Some(
                cx.subscribe_in(&view, window, |this, view, event, window, cx| match event {
                    PhoneEvent::Close => this.close_over(window, cx),
                    PhoneEvent::Open(call_id) | PhoneEvent::Refresh(call_id) => {
                        this.request_phone_call(call_id, cx)
                    }
                    PhoneEvent::Back => {
                        this.commands.send(Command::WatchPhoneCall(None)).ok();
                        this.request_phone_list(cx);
                    }
                    PhoneEvent::Reload => this.request_phone_list(cx),
                    PhoneEvent::End { call_id, action_id } => {
                        if this
                            .commands
                            .send(Command::EndPhoneCall {
                                call_id: call_id.clone(),
                                action_id: action_id.clone(),
                            })
                            .is_err()
                        {
                            view.update(cx, |view, cx| {
                                view.set_end_result(
                                    call_id,
                                    Err("OpenPickle has stopped. The end request was not sent."
                                        .into()),
                                    cx,
                                )
                            });
                        }
                    }
                }),
            );
        window.focus(&view.focus_handle(cx));
        self.set_over(Some(Over::Phone(view)));
        if let Some(call_id) = call_id {
            self.request_phone_call(&call_id, cx);
        }
        self.request_phone_list(cx);
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
        self.set_over(Some(Over::Invite(sheet)));
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
                    |this, view, event, window, cx| {
                        if !this.searching() || this.opening_room.is_some() {
                            return;
                        }
                        match event {
                            SearchEvent::Edited => this.cancel_search(),
                            SearchEvent::Query(query) => {
                                if let Err(reason) = this.start_search(|request| Command::Search {
                                    request,
                                    query: query.clone(),
                                }) {
                                    view.update(cx, |view, cx| view.found(Err(reason), cx));
                                }
                            }
                            SearchEvent::Open(seq) => {
                                this.cancel_search();
                                if this.show_message(*seq, window, cx) {
                                    return;
                                }
                                let note = match this.start_search(|request| {
                                    Command::OpenSearchResult { request, seq: *seq }
                                }) {
                                    Ok(()) => "Opening message…".into(),
                                    Err(reason) => reason.into(),
                                };
                                view.update(cx, |view, cx| view.set_opening(note, cx));
                            }
                            SearchEvent::Close => this.close_over(window, cx),
                        }
                    },
                ));
                self.search = Some(search.clone());
                search
            }
        };
        window.focus(&search.focus_handle(cx));
        self._over_events = None;
        let was_open = self.searching();
        self.set_over(Some(Over::Search));
        if !was_open {
            search.update(cx, |search, cx| search.reopen(cx));
        }
        self.report_reading(window);
        cx.notify();
    }

    fn searching(&self) -> bool {
        matches!(self.over, Some(Over::Search))
    }

    fn start_search(
        &mut self,
        command: impl FnOnce(SearchRequest) -> Command,
    ) -> Result<(), String> {
        if self.room_id.is_empty() {
            return Err("Wait for the conversation to connect, then search again.".into());
        }
        self.search_generation += 1;
        let id = self.search_generation;
        let request = SearchRequest {
            room_id: self.room_id.clone(),
            id,
        };
        self.pending_search = None;
        self.commands
            .send(command(request))
            .map_err(|_| "OpenPickle has stopped. Reopen the app to search.".to_owned())?;
        self.pending_search = Some(id);
        Ok(())
    }

    fn cancel_search(&mut self) {
        if self.pending_search.take().is_some() {
            self.commands.send(Command::CancelSearch).ok();
        }
    }

    fn accept_search(&mut self, request: &SearchRequest) -> bool {
        if self.searching()
            && self.opening_room.is_none()
            && self.room_id == request.room_id
            && self.pending_search == Some(request.id)
        {
            self.pending_search = None;
            true
        } else {
            false
        }
    }

    /// Dispatch only supported links. Attachment targets must also belong to a message in
    /// the current room; arbitrary paths are never handed to the OS or to the backend.
    fn open_link(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        match LinkTarget::parse(target) {
            Some(LinkTarget::Web(url)) => cx.open_url(&url),
            Some(LinkTarget::Room(room)) => self.open_room(&room, window, cx),
            Some(LinkTarget::PhoneTask(path)) => {
                if let Some(task) = self.tasks.get(&path).cloned() {
                    self.open_task(path, task, window, cx);
                } else {
                    self.pending_phone_task = Some(path.clone());
                    self.notice = Some("Opening phone transcript…".into());
                    if self.commands.send(Command::ResolvePhoneTask(path)).is_err() {
                        self.pending_phone_task = None;
                        self.notice = Some(
                            "OpenPickle has stopped. Reopen the app to load this call.".into(),
                        );
                    }
                    cx.notify();
                }
            }
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
                    self.open_attachment(attachment, false, window, cx);
                }
            }
            None => {
                self.notice = Some("This link is not supported in OpenPickle yet.".into());
                cx.notify();
            }
        }
    }

    fn open_archive_file(
        &mut self,
        path: &str,
        download_now: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab != Tab::Archive || LinkTarget::attachment(path).is_none() {
            return;
        }
        // Only the Archive projection for this room can supply the file. It may arrive before
        // the corresponding timeline rows; the backend still checks conversation membership.
        let attachment = self
            .archive
            .as_ref()
            .and_then(|archive| archive.read(cx).file(path))
            .cloned();
        if let Some(attachment) = attachment {
            self.open_attachment(attachment, download_now, window, cx);
        }
    }

    fn open_attachment(
        &mut self,
        attachment: Attachment,
        download_now: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_room.is_some() {
            return;
        }
        let preview = cx.new(|cx| AttachmentView::new(attachment, cx));
        let room_id = self.room_id.clone();
        self._over_events = Some(cx.subscribe_in(
            &preview,
            window,
            move |this, view, event, window, cx| {
                if this.room_id != room_id || this.opening_room.is_some() {
                    return;
                }
                let path = view.read(cx).path().to_owned();
                let stopped = "OpenPickle has stopped. Reopen the app.";
                match event {
                    AttachmentEvent::Close => this.close_over(window, cx),
                    AttachmentEvent::Preview => {
                        if this
                            .commands
                            .send(Command::PreviewAttachment(path))
                            .is_err()
                        {
                            view.update(cx, |view, cx| view.loaded(Err(stopped.into()), cx));
                        }
                    }
                    AttachmentEvent::Download | AttachmentEvent::CancelDownload => {
                        let room_id = room_id.clone();
                        let command = match event {
                            AttachmentEvent::Download => {
                                Command::DownloadAttachment { room_id, path }
                            }
                            _ => Command::CancelAttachmentDownload { room_id, path },
                        };
                        if this.commands.send(command).is_err() {
                            view.update(cx, |view, cx| view.downloaded(Err(stopped.into()), cx));
                        }
                    }
                }
            },
        ));
        window.focus(&preview.focus_handle(cx));
        self.set_over(Some(Over::Attachment(preview.clone())));
        preview.update(cx, |view, cx| {
            if download_now {
                // A direct download need not wait on a separate preview request.
                view.start_download(cx);
            } else {
                view.start_preview(cx);
            }
        });
        self.report_reading(window);
        cx.notify();
    }

    /// Keep the old conversation until the backend has opened the target successfully.
    fn open_room(&mut self, room_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Closing the drawer reports visibility; mark the switch first so the previous
        // room is never briefly acknowledged as read while it is being left.
        self.opening_room = Some(room_id.to_owned());
        self.open_tab(Tab::Chat, window, cx);
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

    /// Reveals a message already in the timeline. False means there is no row to scroll to.
    fn show_message(&mut self, seq: u64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(index) = self
            .messages
            .iter()
            .position(|message| message.seq == seq && !message.parts.is_empty())
        else {
            return false;
        };
        let (Screen::Chat(chat), Some(&start)) = (&self.screen, self.row_starts.get(index)) else {
            return false;
        };
        let rows = start..start + self.messages[index].parts.len();
        chat.update(cx, |chat, cx| chat.reveal(rows, window, cx));
        self.open_tab(Tab::Chat, window, cx);
        true
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
            }))
            .on_action(cx.listener(|this, _: &OpenProfile, window, cx| {
                this.open_tab(Tab::Profile, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenPhoneCalls, window, cx| {
                if matches!(this.screen, Screen::Chat(_)) {
                    this.open_phone(None, window, cx);
                }
            }));
        match &self.screen {
            Screen::Starting => screen,
            Screen::SigningOut => screen
                .flex()
                .items_center()
                .justify_center()
                .font_family(theme::FONT)
                .text_color(theme::color(theme::TEXT_MUTED))
                .child("Signing out…"),
            Screen::Login(login) => screen.child(login.clone()),
            Screen::Chat(_) => {
                let over: Option<AnyView> = match &self.over {
                    Some(Over::Drawer(drawer)) => Some(drawer.clone().into()),
                    Some(Over::Invite(invite)) => Some(invite.clone().into()),
                    Some(Over::Attachment(preview)) => Some(preview.clone().into()),
                    Some(Over::Phone(phone)) => Some(phone.clone().into()),
                    Some(Over::Task(task)) => Some(task.clone().into()),
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
                                        this.pending_phone_task = None;
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
    fn local_sign_out_waits_for_confirmation_and_does_not_reopen_the_old_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let old_conversation = || Update::Conversation {
            room_id: "previous".into(),
            is_character: true,
            can_send: true,
            name: "Previous Pickle".into(),
            avatar: None,
            messages: vec![message("old", 1)],
        };
        updates.unbounded_send(old_conversation()).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        shell.update_in(cx, |shell, window, cx| {
            shell.open_tab(Tab::Profile, window, cx)
        });
        let profile = shell.read_with(cx, |shell, _| shell.profile.clone().unwrap());
        profile.update(cx, |_, cx| cx.emit(ProfileEvent::SignOut));
        cx.run_until_parked();
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(
            std::iter::from_fn(|| sent.try_recv().ok())
                .all(|command| !matches!(command, Command::SignOut))
        );

        profile.update(cx, |_, cx| cx.emit(ProfileEvent::SignOut));
        cx.run_until_parked();
        cx.simulate_prompt_answer("Sign out");
        cx.run_until_parked();
        assert!(
            std::iter::from_fn(|| sent.try_recv().ok())
                .any(|command| matches!(command, Command::SignOut))
        );
        updates.unbounded_send(old_conversation()).unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert!(matches!(shell.screen, Screen::SigningOut));
            assert!(shell.profile.is_none());
            assert!(shell.messages.is_empty());
            assert_eq!(shell.identity.name.as_ref(), "Pickle");
        });
        updates.unbounded_send(Update::SignedOut).unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert!(matches!(shell.screen, Screen::Login(_)))
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
        assert_eq!(reading(cx, "secondary-2"), Some(false));
        assert_eq!(reading(cx, "secondary-3"), Some(false));
        assert_eq!(reading(cx, "secondary-1"), Some(true));
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
        cx.simulate_keystrokes("secondary-2");
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
        let commands = asked(cx, "secondary-3");
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::LoadSocial))
        );
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Friends));
        let commands = asked(cx, "secondary-2");
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
    fn an_uncached_search_result_retries_then_reveals_its_hydrated_message(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let row = |seq| Message {
            parts: vec![Part::Task(format!("Message {seq}").into())],
            ..message(&format!("message-{seq}"), seq)
        };
        let conversation = |room: &str| Update::Conversation {
            room_id: room.into(),
            is_character: room == "primary",
            can_send: true,
            name: "Pickle".into(),
            avatar: None,
            messages: (100..200).map(row).collect(),
        };
        updates.unbounded_send(conversation("primary")).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        cx.simulate_resize(gpui::size(gpui::px(440.), gpui::px(900.)));

        cx.simulate_keystrokes("secondary-f");
        cx.simulate_input("old");
        cx.simulate_keystrokes("enter enter");
        let (first_query, query) = std::iter::from_fn(|| sent.try_recv().ok())
            .find_map(|command| match command {
                Command::Search { request, query } => Some((request, query)),
                _ => None,
            })
            .unwrap();
        let (current_query, second_query) = std::iter::from_fn(|| sent.try_recv().ok())
            .find_map(|command| match command {
                Command::Search { request, query } => Some((request, query)),
                _ => None,
            })
            .unwrap();
        assert_eq!((query.as_str(), second_query.as_str()), ("old", "old"));

        let found = |seq| SearchResult {
            seq: Some(seq),
            label: "Chat".into(),
            date: "Nov 16".into(),
            snippet: "An old message".into(),
            matches: vec![3..6],
        };
        // The same query text does not identify a request. Its earlier answer cannot open a row.
        updates
            .unbounded_send(Update::Found {
                request: first_query,
                results: Ok(vec![found(121)]),
            })
            .unwrap();
        cx.run_until_parked();
        let first = gpui::point(gpui::px(120.), gpui::px(75.));
        cx.simulate_click(first, gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert!(shell.searching()));

        updates
            .unbounded_send(Update::Found {
                request: current_query,
                results: Ok(vec![found(50), found(121), found(25)]),
            })
            .unwrap();
        cx.run_until_parked();
        let lookup = |sent: &mut tokio::sync::mpsc::UnboundedReceiver<Command>, expected| {
            std::iter::from_fn(|| sent.try_recv().ok())
                .find_map(|command| match command {
                    Command::OpenSearchResult { request, seq } => {
                        assert_eq!((request.room_id.as_str(), seq), ("primary", expected));
                        Some(request)
                    }
                    _ => None,
                })
                .unwrap()
        };
        cx.simulate_click(first, gpui::Modifiers::none());
        let failed = lookup(&mut sent, 50);
        let failed_id = failed.id;
        updates
            .unbounded_send(Update::SearchResultOpened {
                request: failed,
                seq: 50,
                result: Err("This message is no longer available. Choose another result.".into()),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| assert!(shell.searching()));

        cx.simulate_click(first, gpui::Modifiers::none());
        let retried = lookup(&mut sent, 50);
        updates
            .unbounded_send(Update::SearchResultOpened {
                request: SearchRequest {
                    room_id: "primary".into(),
                    id: failed_id,
                },
                seq: 50,
                result: Ok(vec![row(50), row(51)]),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert!(shell.searching());
            assert!(shell.messages.iter().all(|message| message.seq >= 100));
        });

        updates
            .unbounded_send(Update::SearchResultOpened {
                request: retried,
                seq: 50,
                result: Ok(vec![row(52), row(50), row(48)]),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.searching());
            assert_eq!(
                shell.messages[..3]
                    .iter()
                    .map(|item| item.seq)
                    .collect::<Vec<_>>(),
                [48, 50, 52]
            );
            let Screen::Chat(chat) = &shell.screen else {
                panic!("expected chat")
            };
            let (marked, top) = chat.read(cx).search_position();
            let target = shell.row_starts[1];
            assert_eq!(marked, Some(target));
            assert!(
                top <= target && target - top <= 2,
                "top row {top}, target {target}"
            );
        });

        // Reopening the same result needs no further network work.
        cx.simulate_keystrokes("secondary-f");
        cx.simulate_click(first, gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert!(!shell.searching()));
        assert!(
            !std::iter::from_fn(|| sent.try_recv().ok())
                .any(|command| matches!(command, Command::OpenSearchResult { .. }))
        );

        cx.simulate_keystrokes("secondary-f down down down enter");
        let leaving = lookup(&mut sent, 25);
        updates.unbounded_send(conversation("peer")).unwrap();
        updates
            .unbounded_send(Update::SearchResultOpened {
                request: leaving,
                seq: 25,
                result: Ok(vec![row(25)]),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            std::iter::from_fn(|| sent.try_recv().ok())
                .any(|command| matches!(command, Command::CancelSearch))
        );
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.room_id, "peer");
            assert!(!shell.searching());
            assert!(shell.messages.iter().all(|message| message.seq >= 100));
            let Screen::Chat(chat) = &shell.screen else {
                panic!("expected chat")
            };
            assert_eq!(chat.read(cx).search_position().0, None);
        });
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
    fn room_metadata_preserves_chat_state_and_pending_room_open(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, _sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        updates
            .unbounded_send(Update::Conversation {
                room_id: "peer".into(),
                is_character: false,
                can_send: true,
                name: "Peer".into(),
                avatar: None,
                messages: vec![message("existing", 1)],
            })
            .unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));
        cx.simulate_input("unfinished draft");
        shell.update_in(cx, |shell, window, cx| {
            shell.apply(
                Update::Activity {
                    room_id: "peer".into(),
                    activity: Some(None),
                },
                window,
                cx,
            );
            shell.open_room("other", window, cx);
        });
        let metadata = |room_id: &str, can_send| {
            Update::RoomMetadata(RoomMetadata {
                room_id: room_id.into(),
                is_character: false,
                can_send,
                name: "Renamed".into(),
                avatar: None,
            })
        };
        updates.unbounded_send(metadata("peer", false)).unwrap();
        updates.unbounded_send(metadata("other", true)).unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, cx| {
            assert_eq!(&*shell.room_header.name, "Renamed");
            assert_eq!(shell.opening_room.as_deref(), Some("other"));
            assert!(!shell.can_send);
            assert_eq!(shell.activity, Some(None));
            assert_eq!(shell.messages.len(), 1);
            assert_eq!(shell.messages[0].id, "existing");
            let Screen::Chat(chat) = &shell.screen else {
                panic!("chat should remain open");
            };
            assert_eq!(chat.read(cx).draft(cx), "unfinished draft");
        });
    }

    #[gpui::test]
    fn attachment_links_open_previews_that_download_and_reject_unknown_paths(
        cx: &mut gpui::TestAppContext,
    ) {
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
        let preview = shell.read_with(cx, |shell, cx| match &shell.over {
            Some(Over::Attachment(view)) => {
                assert_eq!(view.read(cx).path(), "attachments/2026-01-01/report.pdf");
                view.clone()
            }
            _ => panic!("attachment preview must stay open"),
        });
        // The preview's Download asks for the file itself.
        preview.update(cx, |_, cx| cx.emit(AttachmentEvent::Download));
        cx.run_until_parked();
        assert!(std::iter::from_fn(|| sent.try_recv().ok()).any(|command| {
            matches!(command,
            Command::DownloadAttachment { room_id, path }
                if room_id == "primary" && path == "attachments/2026-01-01/report.pdf")
        }));
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
        // A download that ends after its preview was closed is reported in the window.
        updates
            .unbounded_send(Update::AttachmentDownloaded {
                room_id: "primary".into(),
                path: "attachments/2026-01-01/report.pdf".into(),
                result: Ok("/made-up/Downloads/Report.pdf".into()),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert_eq!(
                shell.notice.as_ref().map(|notice| notice.as_ref()),
                Some("Saved Report.pdf to your downloads.")
            )
        });
        shell.update_in(cx, |shell, window, cx| {
            shell.open_link("attachment:attachments/unknown.pdf", window, cx)
        });
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
    }

    #[gpui::test]
    fn archive_files_preview_or_download_in_place_and_can_still_jump_to_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        const PATH: &str = "attachments/2026-01-01/archive-only.pdf";
        let attachment = Attachment {
            path: PATH.into(),
            seq: 13,
            at: 1.,
            name: "Archive file.pdf".into(),
            detail: String::new(),
            kind: crate::model::AttachmentKind::File,
        };
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
        cx.simulate_resize(gpui::size(px(440.), px(900.)));
        shell.update_in(cx, |shell, window, cx| {
            shell.open_tab(Tab::Archive, window, cx)
        });
        updates
            .unbounded_send(Update::Archive {
                room_id: "primary".into(),
                complete: true,
                attachments: vec![attachment.clone()],
            })
            .unwrap();
        cx.run_until_parked();
        let click = |cx: &mut gpui::VisualTestContext, selector| {
            cx.update(|window, cx| window.draw(cx).clear());
            let bounds = cx.debug_bounds(selector).expect(selector);
            cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        };
        let mut commands = || std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<_>>();

        click(cx, "archive-Files");
        click(cx, "archive-preview-file");
        let preview = shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.tab, Tab::Archive);
            let Some(Over::Attachment(view)) = &shell.over else {
                panic!("Archive-only file should open the shared preview");
            };
            assert_eq!(view.read(cx).path(), PATH);
            view.clone()
        });
        assert!(commands().into_iter().any(|command| {
            matches!(command, Command::PreviewAttachment(path) if path == PATH)
        }));
        preview.update(cx, |_, cx| cx.emit(AttachmentEvent::Close));
        cx.run_until_parked();

        click(cx, "archive-download-file");
        let requested = commands();
        assert!(requested.iter().any(|command| {
            matches!(command, Command::DownloadAttachment { room_id, path }
                if room_id == "primary" && path == PATH)
        }));
        assert!(
            !requested
                .iter()
                .any(|command| matches!(command, Command::PreviewAttachment(_)))
        );
        let result = |room: &str| Update::AttachmentDownloaded {
            room_id: room.into(),
            path: PATH.into(),
            result: Ok(std::env::temp_dir().join("openpickle-synthetic.pdf")),
        };
        updates.unbounded_send(result("other-room")).unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("open-original").is_none());
        updates.unbounded_send(result("primary")).unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("open-original").is_some());
        cx.simulate_keystrokes("escape");

        updates
            .unbounded_send(Update::Upsert {
                room_id: "primary".into(),
                messages: vec![Message {
                    parts: vec![Part::Files(std::sync::Arc::new(crate::model::Files {
                        items: vec![attachment],
                    }))],
                    ..message("archive-message", 13)
                }],
            })
            .unwrap();
        cx.run_until_parked();
        click(cx, "archive-show-file-in-chat");
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.tab, Tab::Chat);
            assert!(shell.over.is_none());
        });
    }

    #[gpui::test]
    fn a_room_switch_hides_the_previous_rooms_archive_before_loading(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::model::{Attachment, AttachmentKind};
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
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
        cx.simulate_keystrokes("secondary-4");
        let mut archive_watches = || {
            std::iter::from_fn(|| sent.try_recv().ok())
                .filter_map(|command| match command {
                    Command::WatchArchive(room_id) => Some(room_id),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(archive_watches(), vec![Some("primary".into())]);
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
        assert_eq!(archive_watches(), vec![Some("peer".into())]);
        cx.simulate_click(gpui::point(px(60.), px(280.)), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Archive));
        // An old response cannot refill the view after it was cleared.
        updates.unbounded_send(old_archive()).unwrap();
        cx.run_until_parked();
        cx.simulate_click(gpui::point(px(60.), px(280.)), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Archive));
        cx.simulate_keystrokes("secondary-1");
        assert_eq!(archive_watches(), vec![None]);
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

    #[gpui::test]
    fn choosing_a_room_from_the_drawer_never_marks_the_hidden_chat_read(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        updates
            .unbounded_send(Update::Conversation {
                room_id: "primary".into(),
                is_character: true,
                can_send: true,
                name: "Pickle".into(),
                avatar: None,
                messages: vec![message("old", 1)],
            })
            .unwrap();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            window.activate_window();
            Shell::new(commands, received, window, cx)
        });
        shell.update_in(cx, |shell, window, cx| shell.open_drawer(window, cx));
        updates
            .unbounded_send(Update::Upsert {
                room_id: "primary".into(),
                messages: vec![message("arrived-behind-drawer", 2)],
            })
            .unwrap();
        cx.run_until_parked();
        while sent.try_recv().is_ok() {}
        let drawer = shell.read_with(cx, |shell, _| match &shell.over {
            Some(Over::Drawer(drawer)) => drawer.clone(),
            _ => panic!("drawer is open"),
        });
        drawer.update(cx, |_, cx| cx.emit(DrawerEvent::Room("peer".into())));
        cx.run_until_parked();
        let commands = std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::OpenRoom(room) if room == "peer"))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Active(false)))
        );
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Active(true))),
            "closing the drawer during a switch must not acknowledge the old room"
        );
        updates
            .unbounded_send(Update::Conversation {
                room_id: "peer".into(),
                is_character: false,
                can_send: false,
                name: "Sample room".into(),
                avatar: None,
                messages: vec![],
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            std::iter::from_fn(|| sent.try_recv().ok())
                .any(|command| matches!(command, Command::Active(true))),
            "the new room can be read once its snapshot is visible"
        );
    }

    #[gpui::test]
    fn profile_is_a_read_only_section_loaded_on_demand(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
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
        let (shell, cx) = cx.add_window_view(|window, cx| {
            window.activate_window();
            Shell::new(commands, received, window, cx)
        });
        while sent.try_recv().is_ok() {}
        cx.simulate_keystrokes("secondary-5");
        let commands = std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::LoadProfile))
        );
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Active(true)))
        );
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Send { .. }))
        );
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Profile));
        updates
            .unbounded_send(Update::Profile(Ok(crate::profile::ProfileData {
                display_name: "Sample account".into(),
                ..Default::default()
            })))
            .unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes("secondary-1");
        shell.read_with(cx, |shell, _| assert_eq!(shell.tab, Tab::Chat));
    }

    #[gpui::test]
    fn opening_or_closing_phone_views_never_ends_a_call(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
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
        let (shell, cx) = cx.add_window_view(|window, cx| {
            window.activate_window();
            Shell::new(commands, received, window, cx)
        });
        while sent.try_recv().is_ok() {}
        shell.update_in(cx, |shell, window, cx| {
            shell.open_phone(Some("sample-call".into()), window, cx)
        });
        let opened = std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<_>>();
        assert!(opened.iter().any(
            |command| matches!(command, Command::WatchPhoneCall(Some(id)) if id == "sample-call")
        ));
        assert!(
            !opened
                .iter()
                .any(|command| matches!(command, Command::EndPhoneCall { .. }))
        );
        assert!(
            !opened
                .iter()
                .any(|command| matches!(command, Command::Active(true)))
        );
        shell.update_in(cx, |shell, window, cx| shell.close_over(window, cx));
        let closed = std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            closed
                .iter()
                .any(|command| matches!(command, Command::WatchPhoneCall(None)))
        );
        assert!(
            !closed
                .iter()
                .any(|command| matches!(command, Command::EndPhoneCall { .. }))
        );
    }

    #[gpui::test]
    fn an_old_phone_task_result_cannot_open_over_the_new_request(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, _received_commands) = tokio::sync::mpsc::unbounded_channel();
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
        shell.update_in(cx, |shell, window, cx| {
            shell.open_link("handoff:handoffs/first-task", window, cx)
        });
        shell.update_in(cx, |shell, window, cx| {
            shell.open_link("handoff:handoffs/second-task", window, cx)
        });
        updates
            .unbounded_send(Update::PhoneTask {
                path: "handoffs/first-task".into(),
                result: Ok("first-call".into()),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
        updates
            .unbounded_send(Update::PhoneTask {
                path: "handoffs/second-task".into(),
                result: Ok("second-call".into()),
            })
            .unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, cx| match &shell.over {
            Some(Over::Phone(view)) => {
                assert_eq!(view.read(cx).selected_call(), Some("second-call"))
            }
            _ => panic!("the current requested call should open"),
        });
    }

    #[gpui::test]
    fn a_task_card_opens_the_progress_of_its_task_and_closing_it_stops_the_reading(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
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
        cx.simulate_resize(gpui::size(px(440.), px(900.)));
        let item: openpickle_api::HandoffItem = serde_json::from_value(serde_json::json!({
            "name":"sample-task","title":"Synthetic task","state":"running",
            "requested_at_epoch_ms":1.0,"time_text":"Now",
            "latest_log_text":"Reading a guide"
        }))
        .unwrap();
        updates
            .unbounded_send(Update::HandoffCard {
                path: "handoffs/sample-task".into(),
                card: HandoffCard::Task(item),
            })
            .unwrap();
        cx.run_until_parked();
        let open = |cx: &mut gpui::VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| {
                shell.open_link("handoff:handoffs/sample-task", window, cx)
            });
            shell.read_with(cx, |shell, _| {
                assert!(matches!(shell.over, Some(Over::Task(_))))
            });
        };
        let mut sent = move || std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<_>>();
        open(cx);
        assert!(sent().iter().any(|command| {
            matches!(command, Command::WatchHandoff(Some(path)) if path == "handoffs/sample-task")
        }));

        // A press on the sheet stays in the sheet. A press beside it closes the sheet.
        cx.simulate_click(gpui::point(px(220.), px(700.)), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| {
            assert!(matches!(shell.over, Some(Over::Task(_))))
        });
        cx.simulate_click(gpui::point(px(220.), px(200.)), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
        assert!(
            sent()
                .iter()
                .any(|command| matches!(command, Command::WatchHandoff(None)))
        );

        open(cx);
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| assert!(shell.over.is_none()));
        assert!(
            sent()
                .iter()
                .any(|command| matches!(command, Command::WatchHandoff(None)))
        );
    }

    #[gpui::test]
    fn the_phone_overlay_is_visible_above_chat_and_its_close_button_does_not_hang_up(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
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
        cx.simulate_resize(gpui::size(px(440.), px(900.)));
        cx.simulate_keystrokes("secondary-shift-p");
        shell.read_with(cx, |shell, _| {
            assert!(matches!(shell.over, Some(Over::Phone(_))))
        });
        while sent.try_recv().is_ok() {}
        cx.simulate_click(gpui::point(px(400.), px(71.)), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| {
            assert!(
                shell.over.is_none(),
                "the phone Close control must receive the click, not the chat behind it"
            )
        });
        let commands = std::iter::from_fn(|| sent.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::WatchPhoneCall(None)))
        );
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::EndPhoneCall { .. }))
        );
    }
}
