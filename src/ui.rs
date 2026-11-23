mod demo_actions;
#[cfg(test)]
mod demo_tests;
mod events;
mod message_media;
mod time;
mod voice_panel;

use crate::{
    auth,
    model::{Channel, Guild, Message, User},
    network::{Command, Network},
    search::{Query, Scope, Search},
    store::{Direction, Store, HISTORY_LIMIT},
};
use rune::{
    col, el, row, text, text_input,
    tok::{self, type_},
    AppSpec, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, Waker,
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
use zeroize::Zeroizing;

const SIDEBAR_PAGE: usize = 50;

fn sidebar_channels<'a>(
    channels: &[&'a Channel],
    limit: usize,
    selected: Option<&str>,
) -> Vec<&'a str> {
    channels
        .iter()
        .filter(|c| c.is_text() || c.kind == 2)
        .enumerate()
        .filter(|(index, c)| *index < limit || Some(c.id.as_str()) == selected)
        .map(|(_, c)| c.id.as_str())
        .collect()
}

pub struct State {
    token: TextEdit,
    draft: TextEdit,
    drafts: HashMap<String, String>,
    remember: bool,
    network: Option<Network>,
    waker: Option<Waker>,
    store: Store,
    status: String,
    error: String,
    guild: Option<String>,
    channel: Option<String>,
    loading: HashSet<String>,
    pending_present: HashSet<String>,
    sidebar_limit: usize,
    demo: bool,
    sending: bool,
    scroll_top: Rc<Cell<bool>>,
    scroll_bottom: Rc<Cell<bool>>,
    voice: crate::voice::Snapshot,
    search_draft: TextEdit,
    search_here: bool,
    search: Option<Search>,
    search_error: Option<String>,
    scrolled_up: Rc<Cell<bool>>,
    scrolled_up_changed: Rc<Cell<bool>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            token: TextEdit::new("").secure(),
            draft: TextEdit::default(),
            drafts: HashMap::new(),
            remember: false,
            network: None,
            waker: None,
            store: Store::default(),
            status: "Offline".into(),
            error: String::new(),
            guild: None,
            channel: None,
            loading: HashSet::new(),
            pending_present: HashSet::new(),
            sidebar_limit: SIDEBAR_PAGE,
            demo: false,
            sending: false,
            scroll_top: Rc::new(Cell::new(false)),
            scroll_bottom: Rc::new(Cell::new(false)),
            voice: crate::voice::Snapshot::default(),
            search_draft: TextEdit::default(),
            search_here: false,
            search: None,
            search_error: None,
            scrolled_up: Rc::new(Cell::new(false)),
            scrolled_up_changed: Rc::new(Cell::new(false)),
        }
    }
}
impl State {
    fn finish_sent(&mut self, message: Message) -> bool {
        self.sending = false;
        self.draft.clear();
        if self
            .store
            .windows
            .get(&message.channel_id)
            .is_some_and(|w| w.has_newer)
        {
            self.pending_present.insert(message.channel_id.clone());
            self.load_pending_present(&message.channel_id);
            false
        } else {
            self.store.insert(message);
            true
        }
    }
    fn load_pending_present(&mut self, channel: &str) {
        if !self.loading.contains(channel) && self.pending_present.remove(channel) {
            self.history(channel.to_owned(), Direction::Latest);
        }
    }
    fn finish_history(
        &mut self,
        channel: &str,
        direction: Direction,
        result: Result<Vec<Message>, String>,
    ) -> bool {
        self.loading.remove(channel);
        if self.pending_present.contains(channel) {
            self.load_pending_present(channel);
            return false;
        }
        match result {
            Ok(messages) => {
                self.store
                    .history(channel, direction, messages, HISTORY_LIMIT);
                direction == Direction::Latest && self.channel.as_deref() == Some(channel)
            }
            Err(error) => {
                self.error = error;
                false
            }
        }
    }
    fn voice_command(&mut self, command: crate::voice::Command) {
        if self.demo {
            demo_actions::voice(self, command);
            return;
        }
        if !self
            .network
            .as_ref()
            .is_some_and(|n| n.voice.try_send(command).is_ok())
        {
            self.error = "Voice queue unavailable; try again.".into();
        }
    }
    fn open_dm(&mut self, user: User, tx: &mut Tx) {
        if self.store.user.as_ref().is_some_and(|me| me.id == user.id) {
            return;
        }
        self.store.users.insert(user.id.clone(), user.clone());
        if let Some(id) = self.store.dm_for(&user.id) {
            if self.guild.is_some() {
                self.sidebar_limit = SIDEBAR_PAGE;
            }
            self.guild = None;
            self.select(id, tx);
        } else if !self.demo {
            self.command(Command::OpenDm(user.id));
        }
    }
    fn command(&mut self, command: Command) -> bool {
        if self
            .network
            .as_ref()
            .is_some_and(|n| n.tx.try_send(command).is_ok())
        {
            true
        } else {
            self.error = "Network queue busy; please try again.".into();
            false
        }
    }
    fn history(&mut self, channel: String, direction: Direction) {
        if self.demo
            || !self
                .store
                .can_page(&channel, direction, self.loading.contains(&channel))
        {
            return;
        }
        let cursor = if direction != Direction::Latest {
            self.store
                .messages
                .get(&channel)
                .and_then(|ms| {
                    if direction == Direction::Older {
                        ms.front()
                    } else {
                        ms.back()
                    }
                })
                .map(|m| m.id.clone())
        } else {
            None
        };
        if self.command(Command::History {
            channel: channel.clone(),
            cursor,
            direction,
        }) {
            self.loading.insert(channel);
        }
    }
    fn search_scope(&self) -> Option<Scope> {
        match (&self.guild, &self.channel) {
            (Some(guild), channel) => Some(Scope::Guild {
                guild: guild.clone(),
                channel: channel.clone().filter(|_| self.search_here),
            }),
            (None, Some(channel)) => Some(Scope::Channel(channel.clone())),
            (None, None) => None,
        }
    }
    fn search_page(&mut self, offset: usize) {
        let Some(search) = &mut self.search else {
            return;
        };
        if self.demo {
            let query = search.query.clone();
            let mut hits: Vec<Message> = self
                .store
                .messages
                .values()
                .flatten()
                .filter(|m| query.matches(m))
                .cloned()
                .collect();
            hits.sort_by_key(|m| std::cmp::Reverse(m.id.parse::<u64>().unwrap_or(0)));
            search.apply(offset, Ok((hits.len(), hits)));
            return;
        }
        let query = search.query.clone();
        search.loading = true;
        if !self.command(Command::Search { query, offset }) {
            if let Some(search) = &mut self.search {
                search.loading = false;
            }
        }
    }
    fn close_search(&mut self) {
        self.search = None;
        self.search_error = None;
    }
    fn escape_key(&mut self, key: &Key) -> bool {
        if *key == Key::Escape && self.search.is_some() {
            self.close_search();
            return true;
        }
        false
    }
    fn reset_search(&mut self) {
        self.close_search();
        self.search_draft.clear();
    }
    fn select(&mut self, channel: String, tx: &mut Tx) {
        self.scroll_top.set(false);
        self.scroll_bottom.set(false);
        if self.guild.is_none() && self.channel.as_ref() != Some(&channel) {
            self.reset_search();
        } else {
            self.close_search();
        }
        if let Some(current) = self.channel.take() {
            if self.draft.text.is_empty() {
                self.drafts.remove(&current);
            } else {
                self.drafts.insert(current, self.draft.text.clone());
            }
        }
        self.channel = Some(channel.clone());
        self.store.pin(Some(channel.clone()));
        if let Some(saved) = self.drafts.get(&channel) {
            self.draft.replace_text(saved.clone());
        } else {
            self.draft.clear();
        }
        if !self.store.messages.contains_key(&channel) {
            self.history(channel, Direction::Latest);
        }
        tx.scroll_to_bottom("transcript");
    }
}
fn connect(s: &mut State, _: &mut Tx) {
    if s.token.text.trim().is_empty() {
        s.error = "Enter your Discord token to connect.".into();
        return;
    }
    let Some(waker) = s.waker.clone() else {
        return;
    };
    let token = Zeroizing::new(s.token.text.trim().to_owned());
    let result = if s.remember {
        auth::save(&token)
    } else {
        auth::clear()
    };
    if result.is_err() && s.remember {
        s.error = "Cannot save to OS keychain. Disable Remember me to continue.".into();
        return;
    }
    s.network = Some(Network::start(token, waker));
    s.token.clear();
    s.error.clear();
    s.status = "Connecting…".into();
}
fn logout(s: &mut State, _: &mut Tx) {
    s.network = None;
    s.store = Store::default();
    s.guild = None;
    s.channel = None;
    s.token.clear();
    s.draft.clear();
    s.drafts.clear();
    s.loading.clear();
    s.pending_present.clear();
    s.sidebar_limit = SIDEBAR_PAGE;
    s.sending = false;
    s.demo = false;
    s.reset_search();
    s.status = "Offline".into();
    s.voice = crate::voice::Snapshot::default();
    s.error = if auth::clear().is_err() {
        "Could not clear keychain. Remove rustcord/discord-token manually from your OS keychain."
            .into()
    } else {
        String::new()
    };
}
fn search(s: &mut State, _: &mut Tx) {
    start_search(s);
}
fn start_search(s: &mut State) {
    let Some(scope) = s.search_scope() else {
        return;
    };
    match Query::new(scope, &s.search_draft.text) {
        Ok(query) => {
            if s.search
                .as_ref()
                .is_some_and(|x| x.query == query && x.loading)
            {
                return;
            }
            s.search_error = None;
            s.search = Some(Search::new(query));
            s.search_page(0);
        }
        Err(error) => {
            s.search = None;
            s.search_error = Some(error);
        }
    }
}
fn send(s: &mut State, tx: &mut Tx) {
    if s.sending || s.draft.text.trim().is_empty() {
        return;
    }
    if s.draft.text.chars().count() > 2000 {
        s.error = "Messages must be 2,000 characters or fewer.".into();
        return;
    }
    if let Some(channel) = s.channel.clone() {
        if s.demo {
            demo_actions::send(s, channel);
            tx.scroll_to_bottom("transcript");
            return;
        }
        s.sending = s.command(Command::Send {
            channel,
            content: s.draft.text.clone(),
        });
    }
}
fn send_typing(s: &mut State, _: &mut Tx) {
    if s.demo || s.sending {
        return;
    }
    let now = std::time::Instant::now();
    if let Some(channel) = s.channel.clone() {
        if s.store.typing.can_send(&channel, &s.draft.text, now)
            && s.network
                .as_ref()
                .is_some_and(|n| n.tx.try_send(Command::Typing(channel.clone())).is_ok())
        {
            s.store.typing.sent(channel, now);
        }
    }
}
fn label(value: impl Into<String>) -> El {
    text(value.into())
        .font(type_::BODY)
        .color(tok::theme(true).text)
}
fn button(
    key: impl Into<String>,
    value: impl Into<String>,
    selected: bool,
    action: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    button_content(key, label(value), selected, action)
}
fn button_content(
    key: impl Into<String>,
    content: El,
    selected: bool,
    action: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    let t = tok::theme(true);
    el().key(key.into())
        .role("button")
        .focusable(true)
        .w_full()
        .min_h(36.0)
        .px(12.0)
        .py(8.0)
        .radius(7.0)
        .fill(if selected { t.control_off } else { t.card })
        .hover(t.hover)
        .on_press_tx(action)
        .child(content)
}
fn login(s: &State) -> El {
    let t = tok::theme(true);
    let field = text_input(&s.token, type_::BODY)
        .key("token")
        .single_line()
        .secure()
        .h(44.0)
        .w_full()
        .padding(12.0)
        .fill(t.bg)
        .radius(7.0)
        .color(t.text)
        .placeholder("Your Discord user token", t.text_secondary)
        .on_edit(|s: &mut State| &mut s.token)
        .on_submit(connect);
    col().grow(1.0).w_full().center().gap(16.0)
        .child(text("Rustcord").font(rune::TextStyle::new(30.0, 600.0)).color(t.text))
        .child(label("A quieter place for your conversations."))
        .child(col().w(420.0).padding(28.0).gap(18.0).radius(14.0).fill(t.card).border(1.0, t.card_border)
            .child(label("Sign in with your Discord token"))
            .child(field)
            .child(button("remember", if s.remember { "[✓] Remember me in the OS keychain" } else { "[ ] Remember me in the OS keychain" }, s.remember, |s, _| s.remember = !s.remember))
            .child(button("connect", "Connect →", true, connect))
            .child(text("Third-party user-token clients violate Discord’s terms. Your account may be restricted or terminated.").font(type_::BODY).color(t.text_secondary).wrap().w_full()))
        .child(text("No telemetry · No message or audio files · Links open in your browser").font(type_::BODY).color(t.text_secondary).wrap().w_full())
}
fn chat(s: &State) -> El {
    let t = tok::theme(true);
    let mut servers = col()
        .w(185.0)
        .h_full()
        .padding(14.0)
        .gap(10.0)
        .fill(t.bg)
        .child(text("Rustcord").font(type_::TITLE).color(t.text))
        .child(button(
            "dms",
            "Direct messages",
            s.guild.is_none(),
            |s, _| {
                s.guild = None;
                s.channel = None;
                s.sidebar_limit = SIDEBAR_PAGE;
                s.reset_search();
                s.store.pin(None);
            },
        ));
    let mut guilds: Vec<_> = s.store.guilds.values().collect();
    guilds.sort_by(|a, b| a.name.cmp(&b.name));
    let mut list = col().gap(6.0).w_full();
    for guild in guilds {
        let id = guild.id.clone();
        list = list.child(button(
            format!("guild-{id}"),
            &guild.name,
            s.guild.as_ref() == Some(&id),
            move |s, _| {
                s.guild = Some(id.clone());
                s.channel = None;
                s.sidebar_limit = SIDEBAR_PAGE;
                s.reset_search();
                s.store.pin(None);
                if s.store.channels_for(Some(&id)).is_empty() {
                    s.command(Command::Channels(id.clone()));
                }
            },
        ));
    }
    servers = servers.child(
        col()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(list),
    );
    let channels = s.store.channels_for(s.guild.as_deref());
    let visible = sidebar_channels(&channels, s.sidebar_limit, s.channel.as_deref());
    let mut channel_list = col().w_full().gap(6.0);
    let channel_button = |c: &Channel| {
        let id = c.id.clone();
        let guild = c.guild_id.clone();
        let voice = c.kind == 2;
        button_content(
            format!("channel-{id}"),
            if voice {
                voice_panel::speaker_label(c.label())
            } else {
                label(format!(
                    "{} {}",
                    if c.guild_id.is_none() {
                        if c.kind == 3 {
                            "◆"
                        } else {
                            "@"
                        }
                    } else {
                        "#"
                    },
                    s.store.channel_label(c)
                ))
            },
            s.channel.as_ref() == Some(&id)
                || s.voice.target.as_ref().is_some_and(|t| t.channel == id),
            move |s, tx| {
                if voice {
                    s.voice_command(crate::voice::Command::Join(
                        crate::voice::handshake::Target {
                            guild: guild.clone(),
                            channel: id.clone(),
                        },
                    ));
                } else {
                    s.select(id.clone(), tx);
                }
            },
        )
    };
    for c in channels
        .iter()
        .filter(|c| {
            (c.is_text() || c.kind == 2)
                && !channels.iter().any(|category| {
                    category.kind == 4 && c.parent_id.as_ref() == Some(&category.id)
                })
        })
        .filter(|c| visible.contains(&c.id.as_str()))
    {
        channel_list = channel_list.child(channel_button(c));
        if c.kind == 2 {
            channel_list = channel_list.child(voice_panel::members(s, &c.id));
        }
    }
    for category in channels.iter().filter(|c| c.kind == 4) {
        if !channels
            .iter()
            .any(|c| c.parent_id.as_ref() == Some(&category.id) && visible.contains(&c.id.as_str()))
        {
            continue;
        }
        channel_list = channel_list.child(
            text(category.label().to_uppercase())
                .font(type_::BODY)
                .color(t.text_secondary)
                .pt(20.0),
        );
        for c in channels
            .iter()
            .filter(|c| (c.is_text() || c.kind == 2) && c.parent_id.as_ref() == Some(&category.id))
            .filter(|c| visible.contains(&c.id.as_str()))
        {
            channel_list = channel_list.child(channel_button(c));
            if c.kind == 2 {
                channel_list = channel_list.child(voice_panel::members(s, &c.id));
            }
        }
    }
    if channels
        .iter()
        .filter(|c| c.is_text() || c.kind == 2)
        .count()
        > visible.len()
    {
        channel_list = channel_list.child(button("more-channels", "Show more", false, |s, _| {
            s.sidebar_limit = s.sidebar_limit.saturating_add(SIDEBAR_PAGE);
        }));
    }
    let channels = col()
        .w(220.0)
        .h_full()
        .padding(16.0)
        .gap(16.0)
        .fill(t.card)
        .child(label(if s.guild.is_some() {
            "CHANNELS"
        } else {
            "DIRECT MESSAGES"
        }))
        .child(
            col()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(channel_list),
        );
    let mut main = col()
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .h_full()
        .padding(24.0)
        .gap(18.0);
    if let Some(channel) = s.channel.as_ref().filter(|id| {
        s.store
            .channels
            .get(*id)
            .is_some_and(|c| matches!(c.kind, 1 | 3))
    }) {
        let id = channel.clone();
        main = main.child(button("dm-call", "Join call", false, move |s, _| {
            s.voice_command(crate::voice::Command::Join(
                crate::voice::handshake::Target {
                    guild: None,
                    channel: id.clone(),
                },
            ));
        }));
    }
    if s.search_scope().is_some() {
        main = main.child(header(s));
    }
    if let Some(error) = &s.search_error {
        main = main.child(
            text(error)
                .font(type_::BODY)
                .color(rune::oklch(0.75, 0.13, 25.0))
                .wrap()
                .w_full(),
        );
    }
    if let Some(search) = &s.search {
        main = main.child(results(s, search));
    } else if let Some(channel) = &s.channel {
        let mut messages = col().w_full().gap(18.0).py(12.0);
        if s.loading.contains(channel) {
            messages = messages.child(label("Loading messages…"));
        } else if !s
            .store
            .windows
            .get(channel)
            .is_some_and(|w| w.oldest_exhausted)
        {
            messages = messages.child(button("older", "Load older messages", false, |s, _| {
                if let Some(id) = s.channel.clone() {
                    s.history(id, Direction::Older);
                }
            }));
        }
        if let Some(items) = s.store.messages.get(channel) {
            for m in items {
                let author = m.author.clone();
                let mut item = col()
                    .key(format!("message-{}", m.id))
                    .w_full()
                    .gap(6.0)
                    .child(
                        row()
                            .gap(12.0)
                            .child(
                                text(m.author.name())
                                    .font(type_::LABEL)
                                    .color(tok::accent(0, true))
                                    .key(format!("author-{}", m.id))
                                    .role("button")
                                    .focusable(true)
                                    .on_press_tx(move |s: &mut State, tx| {
                                        s.open_dm(author.clone(), tx)
                                    }),
                            )
                            .child(
                                text(time::format_timestamp(&m.timestamp))
                                    .font(type_::BODY)
                                    .color(t.text_secondary),
                            ),
                    );
                if !m.content.is_empty() {
                    item = item.child(label(&m.content).wrap_anywhere().w_full());
                }
                for (index, embed) in m.embeds.iter().enumerate() {
                    item = item.child(message_media::embed_card(index, embed));
                }
                for attachment in &m.attachments {
                    item = item.child(message_media::attachment_card(attachment));
                }
                messages = messages.child(item);
            }
        }
        let detached = s.store.windows.get(channel).is_some_and(|w| w.has_newer);
        if detached {
            if !s.loading.contains(channel) {
                messages = messages.child(button("newer", "Load newer messages", false, |s, _| {
                    if let Some(id) = s.channel.clone() {
                        s.history(id, Direction::Newer);
                    }
                }));
            }
            main = main.child(button("present", "Jump to present", false, |s, _| {
                if let Some(id) = s.channel.clone() {
                    s.history(id, Direction::Latest);
                }
            }));
        }
        let top = s.scroll_top.clone();
        let bottom = s.scroll_bottom.clone();
        let up = s.scrolled_up.clone();
        let up_changed = s.scrolled_up_changed.clone();
        let waker = s.waker.clone();
        main = main.child(
            col()
                .key("transcript")
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .scroll(ScrollSpec {
                    stick_bottom: !detached,
                })
                .on_scroll_offset(move |pos| {
                    if pos.offset[1] < 1.0 && pos.max[1] > 1.0 {
                        top.set(true);
                        if let Some(waker) = &waker {
                            waker();
                        }
                    }
                    if detached && pos.max[1] > 1.0 && pos.offset[1] >= pos.max[1] - 1.0 {
                        bottom.set(true);
                        if let Some(waker) = &waker {
                            waker();
                        }
                    }
                    let away = pos.max[1] - pos.offset[1] > 1.0;
                    if up.replace(away) != away {
                        up_changed.set(true);
                        if let Some(waker) = &waker {
                            waker();
                        }
                    }
                })
                .child(messages),
        );
        let typing = s.store.typing.label(channel, std::time::Instant::now());
        if !typing.is_empty() {
            main = main.child(text(typing).font(type_::BODY).color(t.text_secondary));
        }
        main = main.child(
            row()
                .gap(8.0)
                .w_full()
                .child(
                    text_input(&s.draft, type_::BODY)
                        .key("composer")
                        .single_line()
                        .grow(1.0)
                        .min_w(0.0)
                        .h(46.0)
                        .padding(12.0)
                        .radius(8.0)
                        .fill(t.card)
                        .color(t.text)
                        .placeholder("Write a message…", t.text_secondary)
                        .on_edit(|s: &mut State| &mut s.draft)
                        .on_input_change(send_typing)
                        .on_input_key(|s: &mut State, ev: &KeyEvent, _: &mut Tx| {
                            s.escape_key(&ev.key)
                        })
                        .on_submit(send),
                )
                .child(el().w(90.0).child(button(
                    "send",
                    if s.sending { "Sending…" } else { "Send" },
                    true,
                    send,
                ))),
        );
        if s.scrolled_up.get() {
            main = main.child(button("bottom", "↓ Scroll to bottom", false, |_, tx| {
                tx.scroll_to_bottom("transcript")
            }));
        }
    } else {
        main = main.child(
            col()
                .grow(1.0)
                .w_full()
                .center()
                .child(label("Choose a conversation to begin.")),
        );
    }
    row()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .child(servers)
        .child(channels)
        .child(main)
}
fn header(s: &State) -> El {
    let t = tok::theme(true);
    let title = match (&s.channel, &s.guild) {
        (Some(channel), _) => s
            .store
            .channels
            .get(channel)
            .map(|c| s.store.channel_label(c))
            .unwrap_or_default(),
        (None, Some(guild)) => s
            .store
            .guilds
            .get(guild)
            .map(|g| g.name.clone())
            .unwrap_or_default(),
        (None, None) => String::new(),
    };
    let mut bar = row()
        .gap(8.0)
        .w_full()
        .center_y()
        .child(
            text(title)
                .font(type_::TITLE)
                .color(t.text)
                .wrap_anywhere()
                .min_w(0.0),
        )
        .child(
            text_input(&s.search_draft, type_::BODY)
                .key("search")
                .single_line()
                .grow(1.0)
                .min_w(0.0)
                .h(38.0)
                .padding(10.0)
                .radius(8.0)
                .fill(t.card)
                .color(t.text)
                .placeholder(
                    if s.guild.is_some() {
                        "Search this server…"
                    } else {
                        "Search this conversation…"
                    },
                    t.text_secondary,
                )
                .on_edit(|s: &mut State| &mut s.search_draft)
                .on_input_key(|s: &mut State, ev: &KeyEvent, _: &mut Tx| s.escape_key(&ev.key))
                .on_submit(search),
        );
    if s.guild.is_some() && s.channel.is_some() {
        bar = bar.child(el().w(150.0).child(button(
            "search-here",
            if s.search_here {
                "[✓] This channel"
            } else {
                "[ ] This channel"
            },
            s.search_here,
            |s, _| s.search_here = !s.search_here,
        )));
    }
    bar
}
fn results(s: &State, search: &Search) -> El {
    let t = tok::theme(true);
    let mut list = col().w_full().gap(10.0).py(4.0);
    for m in &search.results {
        let channel = s
            .store
            .channels
            .get(&m.channel_id)
            .map(|c| s.store.channel_label(c))
            .unwrap_or_else(|| "unknown channel".into());
        let id = m.channel_id.clone();
        list = list.child(
            col()
                .key(format!("result-{}", m.id))
                .role("button")
                .focusable(true)
                .w_full()
                .gap(6.0)
                .padding(12.0)
                .radius(8.0)
                .fill(t.card)
                .hover(t.hover)
                .on_press_tx(move |s: &mut State, tx| s.select(id.clone(), tx))
                .child(
                    row()
                        .gap(12.0)
                        .child(
                            text(m.author.name())
                                .font(type_::LABEL)
                                .color(tok::accent(0, true)),
                        )
                        .child(
                            text(format!("# {channel}"))
                                .font(type_::BODY)
                                .color(t.text_secondary),
                        )
                        .child(
                            text(time::format_timestamp(&m.timestamp))
                                .font(type_::BODY)
                                .color(t.text_secondary),
                        ),
                )
                .child(label(&m.content).wrap_anywhere().w_full()),
        );
    }
    if search.loading && !search.results.is_empty() {
        list = list.child(label("Loading more results…"));
    } else if search.has_more() {
        list = list.child(button(
            "more-results",
            "Load more results",
            false,
            |s, _| {
                if let Some(offset) = s.search.as_ref().map(|x| x.next_offset) {
                    s.search_page(offset);
                }
            },
        ));
    }
    if let Some(error) = &search.error {
        list = list.child(
            text(error)
                .font(type_::BODY)
                .color(rune::oklch(0.75, 0.13, 25.0))
                .wrap()
                .w_full(),
        );
    }
    col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .gap(12.0)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(
                    text(search.summary())
                        .font(type_::LABEL)
                        .color(t.text)
                        .grow(1.0),
                )
                .child(el().w(130.0).child(button(
                    "close-search",
                    "Close search",
                    false,
                    |s, _| s.close_search(),
                ))),
        )
        .child(
            col()
                .key("search-results")
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
}
fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let mut root = col()
        .w_full()
        .h_full()
        .fill(t.bg)
        .child(if s.network.is_some() || s.demo {
            chat(s)
        } else {
            login(s)
        });
    if let Some(panel) = voice_panel::panel(s) {
        root = root.child(panel);
    }
    if !s.error.is_empty() {
        root = root.child(
            text(&s.error)
                .font(type_::BODY)
                .color(rune::oklch(0.75, 0.13, 25.0))
                .wrap()
                .w_full()
                .padding(10.0),
        );
    }
    root.child(
        row()
            .w_full()
            .h(44.0)
            .px(16.0)
            .center_y()
            .gap(16.0)
            .fill(t.card)
            .child(
                text(format!("● {}", s.status))
                    .font(type_::BODY)
                    .color(t.text_secondary)
                    .grow(1.0),
            )
            .child(
                el().w(100.0)
                    .child(button("logout", "Log out", false, logout)),
            ),
    )
}
fn on_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    s.escape_key(&ev.key)
}
fn demo(s: &mut State) {
    s.demo = true;
    s.status = "Demo · no network".into();
    s.guild = Some("100".into());
    s.channel = Some("102".into());
    let guild: Guild = serde_json::from_value(serde_json::json!({"id":"100","name":"Rust community","channels":[{"id":"101","type":4,"name":"Community","position":0},{"id":"102","type":0,"name":"general","parent_id":"101","position":1},{"id":"103","type":0,"name":"projects","parent_id":"101","position":2},{"id":"104","type":2,"name":"Lounge","parent_id":"101","position":3}]})).unwrap();
    s.store.guild(guild);
    s.store.user = Some(User {
        id: "1".into(),
        username: "you".into(),
        global_name: Some("You".into()),
    });
    for dm in [
        serde_json::json!({"id":"200","type":1,"last_message_id":"50","recipients":[{"id":"3","username":"River"}]}),
        serde_json::json!({"id":"201","type":1,"last_message_id":"80","recipients":[{"id":"4","username":"Kai"}]}),
        serde_json::json!({"id":"202","type":3,"last_message_id":"60","recipients":[{"id":"2","username":"Ada"},{"id":"3","username":"River"}]}),
    ] {
        let dm: Channel = serde_json::from_value(dm).unwrap();
        s.store.channels.insert(dm.id.clone(), dm);
    }
    let now = jiff::Timestamp::now();
    for (i, author, name, content, timestamp) in [
        (
            1,
            "2",
            "Ada",
            "Welcome to Rustcord. A little less noise, a little more conversation.",
            (now - jiff::Span::new().hours(49)).to_string(),
        ),
        (
            2,
            "3",
            "River",
            "Text chat is here. Messages stay in memory, not on disk.",
            (now - jiff::Span::new().minutes(4)).to_string(),
        ),
        (
            3,
            "1",
            "You",
            "The gateway and REST client run off the UI thread. Voice can join later.",
            now.to_string(),
        ),
    ] {
        s.store.insert(Message {
            id: i.to_string(),
            channel_id: "102".into(),
            author: User {
                id: author.into(),
                username: name.into(),
                global_name: None,
            },
            content: content.into(),
            timestamp,
            embeds: Vec::new(),
            attachments: Vec::new(),
        });
    }
    let media: Message = serde_json::from_value(serde_json::json!({
        "id":"4","channel_id":"102","author":{"id":"1","username":"Ada"},
        "timestamp":"2026-11-23T10:04:00Z",
        "embeds":[{
            "title":"The Rust Book","description":"A guide to learning Rust.",
            "url":"https://doc.rust-lang.org/book/","color":6667018,
            "fields":[{"name":"Chapter 1","value":"Getting Started"}],
            "footer":{"text":"Rust documentation"}
        }],
        "attachments":[{
            "id":"8","filename":"reading-list.pdf","size":24576,"content_type":"application/pdf",
            "url":"https://cdn.discordapp.com/attachments/102/8/reading-list.pdf"
        }]
    }))
    .unwrap();
    s.store.insert(media);
    s.store
        .windows
        .entry("102".into())
        .or_default()
        .oldest_exhausted = true;
}
fn demo_search(s: &mut State) {
    demo(s);
    s.search_draft.replace_text("rustcord");
    start_search(s);
}
pub fn run() {
    rune::run(AppSpec {
        setup: Some(|s: &mut State, waker| {
            s.waker = Some(waker);
            // Headless scenes must never read credentials.
            if !std::env::args()
                .any(|a| matches!(a.as_str(), "--snap" | "--tree" | "--scene" | "--script"))
            {
                match auth::load() {
                    Ok(Some(token)) => {
                        s.token.replace_text(token.as_str());
                        s.remember = true;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        s.error = "OS keychain unavailable; sign in without Remember me.".into()
                    }
                }
            }
        }),
        poll: Some(events::poll),
        on_key: Some(on_key),
        scenes: &[
            ("main", demo),
            ("voice", voice_panel::demo),
            ("search", demo_search),
            ("dms", |s| {
                demo(s);
                s.guild = None;
                s.channel = Some("200".into());
            }),
        ],
        min_size: Some((800.0, 540.0)),
        ..AppSpec::new("Rustcord", (1120.0, 760.0), State::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_send_reloads_latest_and_scrolls_when_it_arrives() {
        for loading in [false, true] {
            let mut state = State::default();
            let (network, mut commands) = Network::mock();
            state.network = Some(network);
            state.channel = Some("1".into());
            state.store.windows.entry("1".into()).or_default().has_newer = true;
            state.draft.replace_text("sent");
            state.sending = true;
            if loading {
                state.loading.insert("1".into());
            }
            let message = Message {
                id: "100".into(),
                channel_id: "1".into(),
                author: User::default(),
                content: "sent".into(),
                timestamp: "now".into(),
                embeds: Vec::new(),
                attachments: Vec::new(),
            };
            assert!(!state.finish_sent(message.clone()));
            assert!(!state.sending);
            assert!(state.draft.text.is_empty());
            assert!(!state.store.messages.contains_key("1"));
            if loading {
                assert!(commands.try_recv().is_err());
                assert!(!state.finish_history("1", Direction::Older, Ok(vec![])));
            }
            assert!(matches!(commands.try_recv().unwrap(), Command::History {
                channel, direction: Direction::Latest, cursor: None
            } if channel == "1"));
            assert!(state.loading.contains("1"));
            assert!(state.finish_history("1", Direction::Latest, Ok(vec![message.clone()])));
            assert!(!state.store.windows["1"].has_newer);
            assert_eq!(state.store.messages["1"][0].id, "100");
            assert!(state.finish_sent(message));
            assert!(commands.try_recv().is_err());
            assert_eq!(state.store.messages["1"].len(), 1);
        }
    }

    #[test]
    fn sidebar_limits_and_keeps_selection() {
        let channels: Vec<Channel> = (0..120)
            .map(|id| {
                serde_json::from_value(serde_json::json!({"id":id.to_string(),"type":0})).unwrap()
            })
            .collect();
        let refs: Vec<_> = channels.iter().collect();
        assert_eq!(sidebar_channels(&[], SIDEBAR_PAGE, None).len(), 0);
        assert_eq!(sidebar_channels(&refs, SIDEBAR_PAGE, None).len(), 50);
        let visible = sidebar_channels(&refs, SIDEBAR_PAGE, Some("110"));
        assert_eq!(visible.len(), 51);
        assert!(visible.contains(&"110"));
        assert_eq!(sidebar_channels(&refs, 100, Some("10")).len(), 100);
        assert_eq!(sidebar_channels(&refs, 150, None).len(), 120);
    }

    #[test]
    fn dm_sidebar_batches_follow_activity_and_keep_selected_dm() {
        let mut store = Store::default();
        for id in 1..=120 {
            let channel: Channel = serde_json::from_value(serde_json::json!({
                "id": id.to_string(), "type": if id % 2 == 0 { 3 } else { 1 },
                "last_message_id": id.to_string(), "recipient_ids": ["999"]
            }))
            .unwrap();
            store.channels.insert(channel.id.clone(), channel);
        }
        let channels = store.channels_for(None);
        let visible = sidebar_channels(&channels, SIDEBAR_PAGE, Some("1"));
        assert_eq!(visible.len(), SIDEBAR_PAGE + 1);
        assert_eq!(visible[0], "120");
        assert_eq!(visible[49], "71");
        assert_eq!(visible[50], "1");
        assert_eq!(sidebar_channels(&channels, 100, None).len(), 100);
        assert_eq!(sidebar_channels(&channels, 150, None).len(), 120);
        let message: Message = serde_json::from_value(serde_json::json!({
            "id":"121", "channel_id":"1", "author":{"id":"999"}, "timestamp":"now"
        }))
        .unwrap();
        store.activity(&message);
        let channels = store.channels_for(None);
        let visible = sidebar_channels(&channels, SIDEBAR_PAGE, Some("1"));
        assert_eq!(visible.len(), SIDEBAR_PAGE);
        assert_eq!(visible[0], "1");
    }

    #[test]
    fn sidebar_categories_do_not_consume_budget_and_dms_are_supported() {
        let channels: Vec<Channel> = [
            serde_json::json!({"id":"1","type":4}),
            serde_json::json!({"id":"2","type":0,"parent_id":"1"}),
            serde_json::json!({"id":"3","type":1}),
            serde_json::json!({"id":"4","type":2,"parent_id":"1"}),
        ]
        .into_iter()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
        let refs: Vec<_> = channels.iter().collect();
        assert_eq!(sidebar_channels(&refs, 1, None), ["2"]);
        assert_eq!(sidebar_channels(&refs, 1, Some("3")), ["2", "3"]);
        assert_eq!(sidebar_channels(&refs, 1, Some("4")), ["2", "4"]);
        assert_eq!(sidebar_channels(&refs, 3, None), ["2", "3", "4"]);
    }

    #[test]
    fn history_failure_preserves_window_and_allows_retry() {
        let mut state = State::default();
        state.store.windows.entry("1".into()).or_default().has_newer = true;
        state.loading.insert("1".into());
        assert!(!state.finish_history("1", Direction::Newer, Err("network failed".into())));
        assert_eq!(state.error, "network failed");
        assert!(state
            .store
            .can_page("1", Direction::Newer, state.loading.contains("1")));
        state.channel = Some("1".into());
        assert!(state.finish_history("1", Direction::Latest, Ok(vec![])));
        assert!(!state.store.windows["1"].has_newer);
        assert!(state.store.windows["1"].oldest_exhausted);
    }
    #[test]
    fn demo_search_scopes_and_invalid_queries() {
        let mut s = State::default();
        demo(&mut s);
        s.search_draft.replace_text("RUST");
        start_search(&mut s);
        let search = s.search.as_ref().unwrap();
        assert_eq!(search.results.len(), 1);
        assert_eq!(search.summary(), "1 result for “RUST”");
        s.search_draft.replace_text("   ");
        start_search(&mut s);
        assert!(s.search.is_none());
        assert_eq!(
            s.search_error.as_deref(),
            Some("Type something to search for.")
        );
        s.close_search();
        assert!(s.search_error.is_none());
        s.channel = Some("103".into());
        s.search_here = true;
        s.search_draft.replace_text("rust");
        start_search(&mut s);
        assert_eq!(s.search.as_ref().unwrap().total, 0);
        s.reset_search();
        assert!(s.search.is_none() && s.search_draft.text.is_empty());
        s.guild = None;
        s.channel = Some("200".into());
        assert_eq!(s.search_scope(), Some(Scope::Channel("200".into())));
    }
    #[test]
    fn escape_closes_search_only_when_open() {
        let mut s = State::default();
        demo(&mut s);
        s.search_draft.replace_text("rust");
        start_search(&mut s);
        assert!(s.search.is_some());
        assert!(s.escape_key(&Key::Escape));
        assert!(s.search.is_none() && s.search_error.is_none());
        assert!(!s.escape_key(&Key::Escape));
        s.search_draft.replace_text("rust");
        start_search(&mut s);
        assert!(!s.escape_key(&Key::Tab));
        assert!(s.search.is_some());
    }
}
