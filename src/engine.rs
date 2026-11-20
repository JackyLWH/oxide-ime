//! 組字狀態、按鍵及候選，供原生 TSF 與隔離測試共用。
mod lookup;
mod options;

use options::SWITCHES;
pub(crate) use options::{Switch, SwitchView};

use crate::commands::{self, Command, CommandProvider};
use crate::dictionary::{stroke_key, Dictionaries, Dictionary, DictionaryMatch, InputMode};
use crate::punctuation::{self, Mapping};
use std::sync::Arc;

const PREVIEW_LIMIT: usize = 20;
const RADICALS: [char; 26] = [
    '日', '月', '金', '木', '水', '火', '土', '竹', '戈', '十', '大', '中', '一', '弓', '人', '心',
    '手', '口', '尸', '廿', '山', '女', '田', '難', '卜', '重',
];

/// 候選框只取得縮略文字；實際提交仍使用未截斷的字串。
pub(crate) fn preview(text: &str, mode: InputMode) -> String {
    let (limit, suffix) = match mode {
        InputMode::Cangjie => (PREVIEW_LIMIT, "…"),
        InputMode::Quick | InputMode::Stroke => (4, "..."),
    };
    let mut chars = text.chars();
    let mut result = String::new();
    for ch in chars.by_ref().take(limit) {
        result.push(match ch {
            '\n' | '\r' => '↵',
            _ => ch,
        });
    }
    if chars.next().is_some() {
        result.push_str(suffix);
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Menu,
    ToggleJyutping,
    ToggleReverseExact,
    ToggleMode,
    ToggleSimplification,
    ToggleOption(Switch),
    Letter(char),
    Numpad(char),
    Symbol(char),
    Backspace,
    Escape,
    Enter,
    Space,
    Select(usize),
    Previous,
    Next,
    Block,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    None,
    Inspect,
    Preedit(String),
    Commit(String),
    CommitCandidate(String),
    /// TSF 在獨立編輯工作階段結束預編輯，供下一個工作階段插入多行文字。
    PrepareMultiline,
    ConfirmMultilineCaret,
    /// 多行提交失敗時，以宿主原有的 UTF-16 還原組字前的選取內容。
    RestoreSelection(Arc<[u16]>),
    Insert(String),
    Cancel,
    CancelExternal,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub handled: bool,
    pub edit: Edit,
}

impl Outcome {
    fn pass() -> Self {
        Self {
            handled: false,
            edit: Edit::None,
        }
    }
    fn handled(edit: Edit) -> Self {
        Self {
            handled: true,
            edit,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateView {
    pub text: String,
    pub selected: bool,
}

#[derive(Clone)]
struct PunctuationState {
    raw: String,
    choices: &'static [&'static str],
    selected: usize,
}

enum Match {
    Local(DictionaryMatch),
    Remote {
        preview: Arc<str>,
        snapshot: u64,
        index: u32,
    },
    Homophone {
        original: Arc<str>,
        text: Arc<str>,
        comment: Arc<str>,
    },
    Generated {
        text: Arc<str>,
        comment: &'static str,
        command: Command,
    },
}

#[derive(Clone)]
struct HomophoneAnchor {
    source: Arc<str>,
    reverse: Arc<str>,
    command_start: usize,
}

#[derive(Clone)]
enum HomophoneSelection {
    Original(Arc<str>),
    RemotePreview(Arc<str>),
}

#[derive(Clone, Default)]
struct HomophoneFrame {
    anchor: Option<Arc<HomophoneAnchor>>,
    tone_index: usize,
    selected: usize,
    selection: Option<HomophoneSelection>,
}

#[derive(Clone)]
pub(crate) struct Session {
    dictionaries: Option<Dictionaries>,
    pub input_mode: InputMode,
    converter: Arc<crate::simplify::Converter>,
    pub ascii_mode: bool,
    pub limit_chars: bool,
    pub simplification: bool,
    pub full_shape: bool,
    pub ascii_punct: bool,
    pub menu_open: bool,
    menu_selected: usize,
    pub startup_ascii: bool,
    reverse_exact: Option<bool>,
    fuzzy: Option<Arc<crate::homophone::Fuzzy>>,
    punctuation: Option<PunctuationState>,
    single_quote_close: bool,
    double_quote_close: bool,
    raw: String,
    matches: Arc<[Match]>,
    lookup_failed: bool,
    selected: usize,
    homophone_frames: Vec<HomophoneFrame>,
    command_provider: Arc<dyn CommandProvider>,
    clipboard_snapshot: Option<Option<Arc<commands::ClipboardSnapshot>>>,
}

impl Session {
    #[cfg(test)]
    pub(crate) fn new(dictionary: Arc<Dictionary>) -> Self {
        Self::local(Dictionaries {
            cangjie: dictionary,
            stroke: None,
            ranking: Arc::default(),
        })
    }

    pub(crate) fn local(dictionaries: Dictionaries) -> Self {
        Self::with_dictionaries(Some(dictionaries))
    }

    pub(crate) fn sandboxed() -> Self {
        Self::with_dictionaries(None)
    }

    fn with_dictionaries(dictionaries: Option<Dictionaries>) -> Self {
        let sandboxed = dictionaries.is_none();
        Self {
            dictionaries,
            input_mode: InputMode::Cangjie,
            converter: crate::simplify::current(),
            ascii_mode: true,
            limit_chars: true,
            simplification: false,
            full_shape: false,
            ascii_punct: true,
            menu_open: false,
            menu_selected: 0,
            startup_ascii: true,
            reverse_exact: None,
            fuzzy: None,
            punctuation: None,
            single_quote_close: false,
            double_quote_close: false,
            raw: String::new(),
            matches: Arc::default(),
            lookup_failed: false,
            selected: 0,
            homophone_frames: Vec::new(),
            command_provider: Arc::new(commands::WindowsCommands { sandboxed }),
            clipboard_snapshot: None,
        }
    }

    pub(crate) fn update_dictionaries(&mut self, dictionaries: Dictionaries) {
        let Some(current) = &mut self.dictionaries else {
            return;
        };
        let same_table = match (
            current.table(self.input_mode),
            dictionaries.table(self.input_mode),
        ) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        let changed = !same_table || !Arc::ptr_eq(&current.ranking, &dictionaries.ranking);
        *current = dictionaries;
        if changed && !self.raw.is_empty() {
            self.refresh();
        }
    }

    pub(crate) fn is_sandboxed(&self) -> bool {
        self.dictionaries.is_none()
    }

    pub(crate) fn refresh_converter(&mut self) {
        self.converter = crate::simplify::current();
    }

    fn converter(&self) -> Option<&crate::simplify::Converter> {
        self.simplification.then_some(self.converter.as_ref())
    }

    fn local_dictionary(&self) -> &Dictionary {
        self.dictionaries
            .as_ref()
            .and_then(|dictionaries| dictionaries.table(self.input_mode))
            .map(Arc::as_ref)
            .expect("本機候選需要本機字庫")
    }

    pub(crate) fn reverse_exact(&self) -> Option<bool> {
        self.reverse_exact
    }

    pub(crate) fn stroke_input(&self) -> bool {
        !self.ascii_mode && self.reverse_exact.is_none() && self.input_mode == InputMode::Stroke
    }

    pub(crate) fn needs_local_phonetics(&self, key: Key) -> bool {
        !self.is_sandboxed()
            && (self.reverse_exact.is_some()
                || self.homophone_anchor().is_some()
                || matches!(key, Key::ToggleJyutping)
                || (self.input_mode == InputMode::Cangjie && matches!(key, Key::Letter('z' | 'Z'))))
    }

    pub(crate) fn set_fuzzy(&mut self, fuzzy: Arc<crate::homophone::Fuzzy>) {
        self.fuzzy = Some(fuzzy);
    }

    pub(crate) fn set_modes(&mut self, modes: crate::broker::Modes) {
        self.ascii_mode = modes.ascii;
        self.startup_ascii = modes.startup_ascii;
        if !self.is_composing() {
            self.input_mode = modes.input_mode;
            self.reverse_exact = modes.reverse_exact;
        }
        if self.simplification != modes.simplified {
            self.simplification = modes.simplified;
            if self.is_composing() {
                self.refresh();
            }
        }
    }

    pub(crate) fn is_composing(&self) -> bool {
        !self.raw.is_empty() || self.punctuation.is_some()
    }
    pub(crate) fn raw_text(&self) -> String {
        let mut text = self
            .punctuation
            .as_ref()
            .map_or_else(String::new, |p| p.raw.clone());
        text.push_str(&self.raw);
        text
    }

    pub(crate) fn close_menu(&mut self) -> bool {
        std::mem::take(&mut self.menu_open)
    }
    pub(crate) fn page(&self) -> usize {
        self.current_selection() / self.page_size()
    }

    pub(crate) fn page_size(&self) -> usize {
        if self.reverse_exact.is_none()
            && self.input_mode != InputMode::Cangjie
            && self.punctuation_candidates().is_none()
        {
            9
        } else {
            2
        }
    }

    pub(crate) fn ui_count(&self) -> usize {
        if self.menu_open {
            SWITCHES.len()
        } else if self.shows_lookup_message() {
            1
        } else {
            self.current_count()
        }
    }

    pub(crate) fn ui_selection(&self) -> Option<usize> {
        if self.menu_open {
            Some(self.menu_selected)
        } else if self.current_count() > 0 {
            Some(self.current_selection())
        } else {
            None
        }
    }

    pub(crate) fn ui_text(&self, index: usize) -> Option<String> {
        if self.menu_open {
            let (name, state) = self.switch_labels(*SWITCHES.get(index)?);
            return Some(format!("{name}　{state}"));
        }
        if index == 0 && self.shows_lookup_message() {
            return Some(self.lookup_message().to_owned());
        }
        self.candidate_text(index)
    }

    pub(crate) fn ui_set_selection(&mut self, index: usize) -> Option<Edit> {
        if self.menu_open {
            if index >= SWITCHES.len() {
                return None;
            }
            self.menu_selected = index;
            return Some(Edit::None);
        }
        if index >= self.current_count() {
            return None;
        }
        self.set_current_selection(index);
        Some(self.selection_changed().edit)
    }

    pub(crate) fn candidates(&self) -> Vec<CandidateView> {
        if !self.is_composing() {
            return Vec::new();
        }
        if self.shows_lookup_message() {
            return vec![CandidateView {
                text: self.lookup_message().to_owned(),
                selected: true,
            }];
        }
        let size = self.page_size();
        let first = self.page() * size;
        let selected = self.current_selection();
        (first..(first + size).min(self.current_count()))
            .filter_map(|index| {
                self.candidate_text(index).map(|text| CandidateView {
                    text,
                    selected: index == selected,
                })
            })
            .collect()
    }

    fn punctuation_candidates(&self) -> Option<&PunctuationState> {
        if self.raw.is_empty() {
            self.punctuation.as_ref()
        } else {
            None
        }
    }

    fn candidate_text(&self, index: usize) -> Option<String> {
        if let Some(punctuation) = self.punctuation_candidates() {
            punctuation
                .choices
                .get(index)
                .map(|text| (*text).to_owned())
        } else {
            self.matches
                .get(index)
                .map(|entry| self.match_display(entry))
        }
    }

    fn shows_lookup_message(&self) -> bool {
        self.current_count() == 0
            && self.is_composing()
            && (self.raw.len() <= 6
                || self.homophone_anchor().is_some()
                || self.reverse_exact.is_some()
                || self.input_mode == InputMode::Stroke)
    }

    fn current_selection(&self) -> usize {
        self.punctuation_candidates()
            .map_or(self.selected, |punctuation| punctuation.selected)
    }

    fn current_count(&self) -> usize {
        self.punctuation_candidates()
            .map_or(self.matches.len(), |punctuation| punctuation.choices.len())
    }

    fn set_current_selection(&mut self, value: usize) {
        match &mut self.punctuation {
            Some(punctuation) if self.raw.is_empty() => punctuation.selected = value,
            _ => self.selected = value,
        }
    }

    fn selected_punctuation(&self) -> &str {
        self.punctuation
            .as_ref()
            .map_or("", |p| p.choices[p.selected])
    }

    pub(crate) fn accepts(&self, key: Key) -> bool {
        if matches!(
            key,
            Key::ToggleMode | Key::ToggleSimplification | Key::ToggleOption(_) | Key::Menu
        ) || self.menu_open
        {
            return true;
        }
        if key == Key::ToggleJyutping {
            return !self.ascii_mode
                || self.is_composing()
                || punctuation::mapping('`', self.ascii_mode, self.full_shape, self.ascii_punct)
                    .is_some();
        }
        if key == Key::ToggleReverseExact {
            return self.reverse_exact.is_some() || self.is_composing();
        }
        if self.is_composing() || self.reverse_exact.is_some() {
            return true;
        }
        match key {
            Key::Letter(ch) | Key::Numpad(ch) if self.stroke_input() => stroke_key(ch).is_some(),
            Key::Letter(ch) => ch.is_ascii_alphabetic() && (!self.ascii_mode || self.full_shape),
            Key::Symbol(ch) => {
                punctuation::mapping(ch, self.ascii_mode, self.full_shape, self.ascii_punct)
                    .is_some()
            }
            Key::Space => {
                punctuation::mapping(' ', self.ascii_mode, self.full_shape, self.ascii_punct)
                    .is_some()
            }
            _ => false,
        }
    }

    pub(crate) fn handle(&mut self, key: Key) -> Outcome {
        if !self.accepts(key) {
            return Outcome::pass();
        }
        if key == Key::Menu {
            self.menu_open = !self.menu_open;
            return Outcome::handled(Edit::None);
        }
        if key == Key::ToggleSimplification {
            return self.toggle_option(Switch::Simplification);
        }
        if let Key::ToggleOption(index) = key {
            return self.toggle_option(index);
        }
        if self.menu_open {
            return self.handle_menu(key);
        }
        match key {
            Key::ToggleMode => self.toggle_ascii(),
            Key::ToggleSimplification => unreachable!(),
            Key::ToggleOption(_) => unreachable!(),
            Key::Menu => unreachable!(),
            Key::ToggleJyutping => {
                if self.reverse_exact.is_some() {
                    return self.cancel_composition();
                }
                if self.is_composing() {
                    return Outcome::handled(Edit::None);
                }
                if self.ascii_mode {
                    return self.insert_symbol('`');
                }
                self.reverse_exact = Some(false);
                Outcome::handled(Edit::None)
            }
            Key::ToggleReverseExact => {
                if let Some(exact) = self.reverse_exact {
                    self.reverse_exact = Some(!exact);
                    self.refresh();
                }
                Outcome::handled(Edit::None)
            }
            Key::Letter(ch) | Key::Numpad(ch) => {
                if self.stroke_input() {
                    if stroke_key(ch).is_none() {
                        return Outcome::handled(Edit::None);
                    }
                } else if matches!(key, Key::Numpad(_)) {
                    return Outcome::handled(Edit::None);
                }
                if self.ascii_mode {
                    return self.insert_symbol(ch);
                }
                if self.reverse_exact.is_none() && self.input_mode == InputMode::Cangjie {
                    self.advance_homophone(ch);
                }
                self.raw.push(ch);
                self.refresh();
                Outcome::handled(Edit::Preedit(self.preedit()))
            }
            Key::Backspace => {
                if self.raw.is_empty() {
                    return self.cancel_composition();
                }
                self.raw.pop();
                self.homophone_frames.pop();
                self.refresh();
                self.restore_homophone_selection();
                if self.is_composing() {
                    Outcome::handled(Edit::Preedit(self.preedit()))
                } else {
                    Outcome::handled(Edit::Cancel)
                }
            }
            Key::Escape => self.cancel_composition(),
            Key::Enter => {
                let raw = self.raw_text();
                self.reset_composition();
                Outcome::handled(if raw.is_empty() {
                    Edit::None
                } else {
                    Edit::Commit(raw)
                })
            }
            Key::Space => {
                if !self.is_composing() {
                    if self.reverse_exact.take().is_some() {
                        Outcome::handled(Edit::None)
                    } else {
                        self.insert_symbol(' ')
                    }
                } else if self.current_count() == 0 {
                    self.reset_composition();
                    Outcome::handled(Edit::Cancel)
                } else {
                    self.commit_index(self.current_selection())
                }
            }
            Key::Select(position) => {
                let size = self.page_size();
                let index = self.page() * size + position;
                if position < size && index < self.current_count() {
                    self.commit_index(index)
                } else {
                    Outcome::handled(Edit::None)
                }
            }
            Key::Previous => {
                self.set_current_selection(self.current_selection().saturating_sub(1));
                self.selection_changed()
            }
            Key::Next => {
                self.set_current_selection(
                    (self.current_selection() + 1).min(self.current_count().saturating_sub(1)),
                );
                self.selection_changed()
            }
            Key::Symbol(ch) => {
                if self.is_composing() || self.reverse_exact.is_some() {
                    Outcome::handled(Edit::None)
                } else {
                    self.insert_symbol(ch)
                }
            }
            Key::Block => Outcome::handled(Edit::None),
        }
    }

    fn toggle_ascii(&mut self) -> Outcome {
        self.ascii_mode = !self.ascii_mode;
        if self.ascii_mode && self.is_composing() {
            let raw = self.raw_text();
            self.reset_composition();
            Outcome::handled(Edit::Commit(raw))
        } else {
            self.reverse_exact = None;
            Outcome::handled(Edit::None)
        }
    }

    fn insert_symbol(&mut self, ch: char) -> Outcome {
        match punctuation::mapping(ch, self.ascii_mode, self.full_shape, self.ascii_punct) {
            Some(Mapping::Insert(text)) => Outcome::handled(Edit::Insert(text.to_owned())),
            Some(Mapping::FullWidth(ch)) => Outcome::handled(Edit::Insert(ch.to_string())),
            Some(Mapping::Quote(open, close, double)) => {
                let close_next = if double {
                    &mut self.double_quote_close
                } else {
                    &mut self.single_quote_close
                };
                let text = if *close_next { close } else { open };
                *close_next = !*close_next;
                Outcome::handled(Edit::Insert(text.to_owned()))
            }
            Some(Mapping::Choices(choices)) => {
                self.punctuation = Some(PunctuationState {
                    raw: ch.to_string(),
                    choices,
                    selected: 0,
                });
                Outcome::handled(Edit::Preedit(choices[0].to_owned()))
            }
            None => Outcome::pass(),
        }
    }

    fn selection_changed(&self) -> Outcome {
        if self.raw.is_empty() && self.punctuation.is_some() {
            Outcome::handled(Edit::Preedit(self.preedit()))
        } else {
            Outcome::handled(Edit::None)
        }
    }

    fn commit_index(&mut self, index: usize) -> Outcome {
        let mut text = self.selected_punctuation().to_owned();
        let mut batch_candidate = false;
        if let Some(punctuation) = self.punctuation_candidates() {
            text = punctuation.choices[index].to_owned();
        } else {
            let item = &self.matches[index];
            batch_candidate = match item {
                Match::Local(item) => self.local_dictionary().is_custom(*item),
                Match::Generated {
                    command: Command::ClipboardRaw | Command::ClipboardConverted,
                    ..
                } => true,
                _ => false,
            };
            match item {
                Match::Local(_) | Match::Homophone { .. } | Match::Generated { .. } => {
                    text.push_str(&self.match_text(item))
                }
                Match::Remote {
                    snapshot, index, ..
                } => match crate::broker::resolve(*snapshot, *index) {
                    Ok(selected) => text.push_str(&selected),
                    Err(_) => {
                        self.matches = Arc::default();
                        self.lookup_failed = true;
                        return Outcome::handled(Edit::None);
                    }
                },
            }
        }
        self.reset_composition();
        Outcome::handled(if batch_candidate {
            Edit::CommitCandidate(text)
        } else {
            Edit::Commit(text)
        })
    }

    fn lookup_message(&self) -> &'static str {
        if self.lookup_failed {
            "「字庫未連線」"
        } else {
            "「輸入錯誤」"
        }
    }

    fn cancel_composition(&mut self) -> Outcome {
        let edit = if self.is_composing() {
            Edit::Cancel
        } else {
            Edit::None
        };
        self.reset_composition();
        Outcome::handled(edit)
    }

    fn reset_composition(&mut self) {
        self.reverse_exact = None;
        self.reset_preedit();
    }

    pub(crate) fn reset_preedit(&mut self) {
        self.raw.clear();
        self.punctuation = None;
        self.matches = Arc::default();
        self.lookup_failed = false;
        self.selected = 0;
        self.homophone_frames.clear();
        self.clipboard_snapshot = None;
    }

    fn preedit(&self) -> String {
        let prefix = self.selected_punctuation();
        if self.reverse_exact.is_some() {
            return self.raw.to_ascii_lowercase();
        }
        if self.raw.is_empty() {
            return prefix.to_owned();
        }
        if self.input_mode == InputMode::Stroke {
            return prefix
                .chars()
                .chain(
                    self.raw
                        .chars()
                        .filter_map(|key| stroke_key(key).map(|(_, display)| display)),
                )
                .collect();
        }
        if let Some(anchor) = self.homophone_anchor() {
            if !self.matches.is_empty() || self.raw.len() > 6 {
                let suffix = self.raw[anchor.command_start..].to_ascii_lowercase();
                return format!("{prefix}{}{suffix}", radicals(&anchor.reverse));
            }
        }
        if self.matches.is_empty() && self.raw.len() > 6 {
            return format!("{prefix}{}", self.raw);
        }
        format!("{prefix}{}", radicals(&self.raw))
    }
}

fn radicals(code: &str) -> String {
    code.bytes()
        .map(|ch| {
            let ch = ch.to_ascii_lowercase();
            if ch.is_ascii_lowercase() {
                RADICALS[(ch - b'a') as usize]
            } else {
                ch as char
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
