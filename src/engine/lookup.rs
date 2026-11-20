//! 建立倉頡、粵拼、同音及動態候選，保留每次組字的查詢快照。
use super::{
    preview, Command, HomophoneAnchor, HomophoneFrame, HomophoneSelection, Match, Session,
};
use crate::dictionary::{allowed_text, stroke_key, InputMode};
use std::{borrow::Cow, collections::HashSet, sync::Arc};

impl Session {
    pub(super) fn homophone_anchor(&self) -> Option<&HomophoneAnchor> {
        self.homophone_frames.last()?.anchor.as_deref()
    }

    fn remember_homophone_selection(&mut self) {
        let selection = self.matches.get(self.selected).map(|item| match item {
            Match::Local(item) => {
                HomophoneSelection::Original(Arc::from(self.local_dictionary().original(*item)))
            }
            Match::Remote { preview, .. } => HomophoneSelection::RemotePreview(preview.clone()),
            Match::Homophone { original, .. } | Match::Generated { text: original, .. } => {
                HomophoneSelection::Original(original.clone())
            }
        });
        if let Some(frame) = self.homophone_frames.last_mut() {
            frame.selected = self.selected;
            frame.selection = selection;
        }
    }

    pub(super) fn advance_homophone(&mut self, ch: char) {
        while self.homophone_frames.len() < self.raw.len() {
            self.homophone_frames.push(HomophoneFrame::default());
        }
        if !self.raw.is_empty() {
            self.remember_homophone_selection();
        }
        let previous = self.homophone_frames.last();
        let mut next = HomophoneFrame::default();
        if self.punctuation.is_none() {
            if let Some(previous) = previous {
                let key = ch.to_ascii_lowercase();
                // 保留 xz 作倉頡碼，即使 x 已顯示補全候選。
                if key == 'z'
                    && (previous.anchor.is_some()
                        || (!self.raw.eq_ignore_ascii_case("x")
                            && self
                                .raw
                                .bytes()
                                .all(|byte| (b'a'..=b'y').contains(&byte.to_ascii_lowercase()))))
                {
                    // 只有啟動同音搜尋時才需要遠端候選的完整原文。
                    let source = match (&previous.selection, self.matches.get(self.selected)) {
                        (Some(HomophoneSelection::Original(text)), _) => Some(text.clone()),
                        (
                            Some(HomophoneSelection::RemotePreview(_)),
                            Some(Match::Remote {
                                snapshot, index, ..
                            }),
                        ) => crate::broker::resolve_original(*snapshot, *index)
                            .ok()
                            .map(Arc::from),
                        _ => None,
                    };
                    if let Some(source) = source.filter(|source| {
                        !source.is_empty()
                            && source.as_ref() != "「輸入錯誤」"
                            && source.as_ref() != "「输入错误」"
                    }) {
                        let reverse = if let Some(dictionaries) = &self.dictionaries {
                            dictionaries.cangjie.reverse(&source)
                        } else {
                            match self.matches.get(self.selected) {
                                Some(Match::Remote {
                                    snapshot, index, ..
                                }) => crate::broker::reverse_match(*snapshot, *index)
                                    .unwrap_or_default(),
                                _ => crate::broker::reverse(&source).unwrap_or_default(),
                            }
                        };
                        let command_start = previous
                            .anchor
                            .as_ref()
                            .map_or(self.raw.len(), |anchor| anchor.command_start);
                        next.anchor = Some(Arc::new(HomophoneAnchor {
                            source,
                            reverse: Arc::from(reverse),
                            command_start,
                        }));
                    }
                } else if key == 'x' {
                    if let Some(anchor) = &previous.anchor {
                        next.anchor = Some(anchor.clone());
                        next.tone_index = previous.tone_index.saturating_add(1);
                    }
                }
            }
        }
        self.homophone_frames.push(next);
    }

    pub(super) fn restore_homophone_selection(&mut self) {
        let Some(frame) = self.homophone_frames.last() else {
            return;
        };
        let selected = frame.selected;
        let Some(selection) = &frame.selection else {
            self.selected = selected.min(self.matches.len().saturating_sub(1));
            return;
        };
        let matches_source = |item: &Match| match (item, selection) {
            (Match::Remote { preview, .. }, HomophoneSelection::RemotePreview(previous)) => {
                preview == previous
            }
            (Match::Local(item), HomophoneSelection::Original(source)) => {
                self.local_dictionary().original(*item) == source.as_ref()
            }
            (
                Match::Homophone { original, .. } | Match::Generated { text: original, .. },
                HomophoneSelection::Original(source),
            ) => original == source,
            (Match::Remote { preview: value, .. }, HomophoneSelection::Original(source)) => {
                if let Some(converter) = self.converter() {
                    converter
                        .candidate_forms(source)
                        .any(|form| preview(&form, InputMode::Cangjie) == value.as_ref())
                } else {
                    preview(source, InputMode::Cangjie) == value.as_ref()
                }
            }
            _ => false,
        };
        self.selected = if self.matches.get(selected).is_some_and(matches_source) {
            selected
        } else {
            self.matches.iter().position(matches_source).unwrap_or(0)
        };
    }

    fn homophone_matches(&self, anchor: &HomophoneAnchor, tone_index: usize) -> Vec<Match> {
        let entries = if self.is_sandboxed() {
            crate::broker::homophones(&anchor.source, tone_index).unwrap_or_default()
        } else {
            let fuzzy = self
                .fuzzy
                .clone()
                .unwrap_or_else(crate::homophone::Fuzzy::standard);
            crate::homophone::lookup(&fuzzy, &anchor.source, tone_index)
        };
        self.phonetic_matches(entries)
    }

    fn phonetic_matches(&self, entries: Vec<(String, String)>) -> Vec<Match> {
        let mut seen = HashSet::<Arc<str>>::new();
        let mut result = Vec::new();
        let converter = self.converter();
        for (original, comment) in entries {
            if original == "「輸入錯誤」" && comment.is_empty() {
                continue;
            }
            if self.limit_chars && !allowed_text(&original) {
                continue;
            }
            let original: Arc<str> = Arc::from(original);
            let comment: Arc<str> = Arc::from(comment);
            let forms = converter
                .into_iter()
                .flat_map(|converter| converter.candidate_forms(&original))
                .chain(
                    converter
                        .is_none()
                        .then_some(Cow::Borrowed(original.as_ref())),
                );
            for text in forms {
                if seen.contains(text.as_ref()) {
                    continue;
                }
                let text = if converter.is_some() {
                    Arc::from(text.as_ref())
                } else {
                    original.clone()
                };
                seen.insert(text.clone());
                result.push(Match::Homophone {
                    original: original.clone(),
                    text,
                    comment: comment.clone(),
                });
            }
        }
        result
    }

    pub(super) fn match_display(&self, item: &Match) -> String {
        if let Match::Remote { preview, .. } = item {
            return preview.to_string();
        }
        let mode = if self.reverse_exact.is_some() {
            InputMode::Cangjie
        } else {
            self.input_mode
        };
        let mut text = preview(&self.match_text(item), mode);
        let comment = match item {
            Match::Homophone { comment, .. } => comment.as_ref(),
            Match::Generated { comment, .. } => comment,
            _ => "",
        };
        if !comment.is_empty() {
            text.push(' ');
            text.push_str(comment);
        }
        text
    }

    pub(super) fn refresh(&mut self) {
        let raw = self.raw.to_ascii_lowercase();
        if !"zv".starts_with(&raw) || raw.is_empty() || self.punctuation.is_some() {
            self.clipboard_snapshot = None;
        }
        self.lookup_failed = false;
        self.matches = (if let Some(exact) = self.reverse_exact {
            let entries = if raw.is_empty() {
                Vec::new()
            } else if self.is_sandboxed() {
                match crate::broker::jyutping(&raw, exact) {
                    Ok(entries) => entries,
                    Err(_) => {
                        self.lookup_failed = true;
                        Vec::new()
                    }
                }
            } else {
                let fuzzy = self
                    .fuzzy
                    .clone()
                    .unwrap_or_else(crate::homophone::Fuzzy::standard);
                crate::homophone::reverse(&fuzzy, &raw, exact)
            };
            self.phonetic_matches(entries)
        } else if let Some((anchor, tone)) = self.homophone_frames.last().and_then(|frame| {
            frame
                .anchor
                .as_deref()
                .map(|anchor| (anchor, frame.tone_index))
        }) {
            self.homophone_matches(anchor, tone)
        } else {
            if self.input_mode == InputMode::Stroke {
                let code: String = raw
                    .chars()
                    .filter_map(|key| stroke_key(key).map(|(code, _)| code))
                    .collect();
                self.lookup_dictionary(&code)
            } else {
                self.lookup_dictionary(&raw)
            }
        })
        .into();
        self.selected = 0;
    }

    fn lookup_dictionary(&mut self, raw: &str) -> Vec<Match> {
        if raw.is_empty() {
            return Vec::new();
        }
        let mode = self.input_mode;
        if let Some(dictionaries) = &self.dictionaries {
            let Some(dictionary) = dictionaries.table(mode) else {
                self.lookup_failed = true;
                return Vec::new();
            };
            let entries = dictionary.lookup(
                mode,
                raw,
                self.limit_chars,
                self.converter(),
                &dictionaries.ranking,
            );
            return entries
                .into_iter()
                .filter_map(|item| match self.local_dictionary().command(item) {
                    Some(command) => self.generated_command(command, raw),
                    None => Some(Match::Local(item)),
                })
                .collect();
        }
        let Ok(entries) = crate::broker::lookup(mode, raw, self.limit_chars, self.simplification)
        else {
            self.lookup_failed = true;
            return Vec::new();
        };
        entries
            .previews
            .into_iter()
            .enumerate()
            .filter_map(|(index, preview)| match Command::from_marker(&preview) {
                Some(command) => self.generated_command(command, raw),
                None => Some(Match::Remote {
                    preview,
                    snapshot: entries.snapshot,
                    index: index as u32,
                }),
            })
            .collect()
    }

    fn generated_command(&mut self, command: Command, raw: &str) -> Option<Match> {
        if raw.is_empty() || !command.code().starts_with(raw) {
            return None;
        }
        let (text, comment) = match command {
            Command::Time => (Arc::from(self.command_provider.time()), "時間"),
            Command::Date => (Arc::from(self.command_provider.date()), "日期"),
            Command::ClipboardRaw | Command::ClipboardConverted => {
                if self.punctuation.is_some() {
                    return None;
                }
                let snapshot = self
                    .clipboard_snapshot
                    .get_or_insert_with(|| self.command_provider.clipboard().map(Arc::new))
                    .as_ref()?;
                let text = match command {
                    Command::ClipboardRaw if self.simplification => {
                        snapshot.simplified(&self.converter)
                    }
                    Command::ClipboardRaw => snapshot.raw.clone(),
                    Command::ClipboardConverted => snapshot.converted.clone()?,
                    _ => unreachable!(),
                };
                (text, "")
            }
        };
        Some(Match::Generated {
            text,
            comment,
            command,
        })
    }

    pub(super) fn match_text<'a>(&'a self, entry: &'a Match) -> Cow<'a, str> {
        match entry {
            Match::Local(index) => self.local_dictionary().text(*index, self.converter()),
            Match::Remote { preview, .. } => Cow::Borrowed(preview),
            Match::Homophone { text, .. } => Cow::Borrowed(text),
            Match::Generated { text, .. } => Cow::Borrowed(text),
        }
    }
}
