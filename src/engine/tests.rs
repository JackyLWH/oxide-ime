use super::*;
use crate::dictionary::{decode_dictionary_text, Ranking, DICTIONARY_FILES};
use std::collections::HashMap;
use std::path::Path;

fn dictionary() -> Arc<Dictionary> {
    let base = "---\n...\n日\ta\n曰\ta\n昌\taa\n昍\taa\n明\tab\n𠀀\tac\n";
    let custom = "---\n...\n第一行\\n第二行\taz\n第二個詞\taz\n";
    Arc::new(Dictionary::parse([("base", base), ("custom", custom)]).unwrap())
}

fn chinese() -> Session {
    let mut session = Session::new(dictionary());
    assert_eq!(session.handle(Key::ToggleMode).edit, Edit::None);
    session
}

#[test]
fn original_order_completion_filter_and_paging() {
    let mut session = Session::local(Dictionaries {
        cangjie: dictionary(),
        ranking: Arc::new(Ranking::parse("昍\n曰\n昌\n").unwrap()),
    });
    session.handle(Key::ToggleMode);
    assert_eq!(
        session.handle(Key::Letter('a')).edit,
        Edit::Preedit("日".into())
    );
    assert_eq!(
        session
            .candidates()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["日", "曰"]
    );
    session.handle(Key::Next);
    assert_eq!(session.page(), 0);
    session.handle(Key::Next);
    assert_eq!(session.page(), 1);
    assert_eq!(
        session
            .candidates()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["昍", "昌"]
    );
    assert_eq!(
        session.handle(Key::Select(1)).edit,
        Edit::Commit("昌".into())
    );
    assert!(!session.is_composing());
    assert!(!session
        .local_dictionary()
        .lookup("ac", true, None, &Ranking::default())
        .iter()
        .any(|&i| session.local_dictionary().text(i, None) == "𠀀"));
}

#[test]
fn backspace_escape_and_ascii_transition_preserve_raw_case() {
    let mut session = chinese();
    session.handle(Key::Letter('A'));
    session.handle(Key::Letter('b'));
    assert_eq!(
        session.handle(Key::Backspace).edit,
        Edit::Preedit("日".into())
    );
    assert_eq!(
        session.handle(Key::ToggleMode).edit,
        Edit::Commit("A".into())
    );
    assert!(session.ascii_mode);
    assert!(!session.handle(Key::Letter('x')).handled);
    session.handle(Key::ToggleMode);
    session.handle(Key::Letter('a'));
    assert_eq!(session.handle(Key::Escape).edit, Edit::Cancel);
    assert!(!session.is_composing());
}

#[test]
fn multiline_is_not_truncated_when_selected() {
    let long = format!("{}\\n第二行", "長".repeat(2000));
    let table = format!("---\n...\n{long}\tzz\n");
    let dictionary = Arc::new(Dictionary::parse([("base", "...\n"), ("custom", &table)]).unwrap());
    let mut session = Session::new(dictionary);
    session.handle(Key::ToggleMode);
    session.handle(Key::Letter('z'));
    session.handle(Key::Letter('z'));
    let view = session.candidates();
    assert_eq!(view[0].text.chars().count(), PREVIEW_LIMIT + 1);
    assert!(view[0].text.ends_with('…'));
    assert_eq!(
        session.handle(Key::Space).edit,
        Edit::CommitCandidate(format!("{}\n第二行", "長".repeat(2000)))
    );
    assert_eq!(decode_dictionary_text(r"a\\nb\nc".into()), "a\\nb\nc");
}

#[test]
fn real_dictionary_keeps_exact_matches_before_ranked_completions() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data/dictionaries")
        .join(DICTIONARY_FILES[0]);
    let base = std::fs::read_to_string(path).unwrap();
    let dictionary = Dictionary::parse([(DICTIONARY_FILES[0], base.as_str())]).unwrap();
    let expected: HashMap<&str, [&str; 2]> = HashMap::from([
        ("a", ["日", "曰"]),
        ("aa", ["昌", "昍"]),
        ("ab", ["明", "冐"]),
        ("b", ["月", "同"]),
    ]);
    let ranking = Ranking::parse(crate::public_data::INITIAL_RANKING).unwrap();
    for (code, texts) in expected {
        let found = dictionary
            .lookup(code, true, None, &ranking)
            .iter()
            .take(2)
            .map(|&i| dictionary.text(i, None))
            .collect::<Vec<_>>();
        assert_eq!(found, texts);
    }
}

#[test]
fn lookup_keeps_order_when_duplicates_come_from_different_codes_and_tables() {
    let base = "---\n...\n甲\ta\n乙\tab\n甲\tac\n髮\tad\n发\tae\n";
    let custom = "---\n...\n乙\ta\n";
    let dictionary = Dictionary::parse([("base", base), ("custom", custom)]).unwrap();
    let converter = crate::simplify::current();
    let results = |simplified: bool| {
        let simplified = simplified.then_some(converter.as_ref());
        dictionary
            .lookup("a", true, simplified, &Ranking::default())
            .into_iter()
            .map(|index| dictionary.text(index, simplified))
            .collect::<Vec<_>>()
    };
    assert_eq!(results(false), ["甲", "乙", "髮", "发"]);
    assert_eq!(results(true), ["甲", "乙", "发"]);
}

#[test]
fn simplified_candidates_keep_multiple_forms_and_their_original_source() {
    let table = "---\n...\n钁\ta\n鐝\tab\n";
    let dictionary = Dictionary::parse([("test", table)]).unwrap();
    let converter = crate::simplify::current();
    let matches = dictionary.lookup("a", true, Some(&converter), &Ranking::default());
    assert_eq!(
        matches
            .iter()
            .map(|&item| dictionary.text(item, Some(&converter)))
            .collect::<Vec<_>>(),
        ["镢", "䦆"]
    );
    assert!(matches
        .iter()
        .all(|&item| dictionary.original(item) == "钁"));
}

#[test]
fn f4_switches_filter_and_only_converts_in_simplified_mode() {
    let table = "---\n...\n髮\ta\n发\ta\n𠀀\ta\n";
    let dictionary = Arc::new(Dictionary::parse([("test", table)]).unwrap());
    let mut session = Session::new(dictionary);
    session.handle(Key::ToggleMode);
    session.handle(Key::Letter('a'));
    assert_eq!(
        session
            .candidates()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["髮", "发"]
    );
    session.handle(Key::Menu);
    assert!(session.menu_open);
    assert_eq!(
        session
            .switch_items()
            .iter()
            .map(|item| item.name)
            .collect::<Vec<_>>(),
        ["字集", "繁簡", "預設輸入模式", "字形", "標點"]
    );
    assert_eq!(session.ui_count(), 5);
    assert!(session.ui_set_selection(5).is_none());
    session.handle(Key::Select(2));
    assert!(!session.startup_ascii);
    assert!(!session.ascii_mode);
    session.handle(Key::Select(1)); // 繁 → 簡；重複的「发」合併。
    assert_eq!(session.candidates()[0].text, "发");
    assert_eq!(session.current_count(), 1);
    session.handle(Key::Select(0)); // 香港字 → 全字。
    assert_eq!(
        session
            .candidates()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["发", "𠀀"]
    );
    session.handle(Key::Select(1)); // 回到繁體時原文直出，無簡轉繁。
    assert_eq!(
        session
            .candidates()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["髮", "发"]
    );
    session.handle(Key::Escape);
    assert!(!session.menu_open);
    assert_eq!(
        session.handle(Key::Select(1)).edit,
        Edit::Commit("发".into())
    );
}

#[test]
fn simplification_shortcut_preserves_composition_and_homophone_source() {
    let table = "---\n...\n國\ta\n";
    let dictionary = Arc::new(Dictionary::parse([("test", table)]).unwrap());
    let mut session = Session::new(dictionary);
    session.handle(Key::ToggleMode);
    session.handle(Key::Letter('a'));
    session.handle(Key::Letter('z'));
    assert!(session.ui_text(0).unwrap().starts_with("國 "));
    assert_eq!(
        session.handle(Key::ToggleSimplification).edit,
        Edit::Preedit("日z".into())
    );
    assert_eq!(session.raw_text(), "az");
    assert!(session.ui_text(0).unwrap().starts_with("国 "));
    session.handle(Key::ToggleSimplification);
    assert!(session.ui_text(0).unwrap().starts_with("國 "));
    assert_eq!(session.handle(Key::Space).edit, Edit::Commit("國".into()));
    session.handle(Key::ToggleSimplification);
    session.handle(Key::Letter('a'));
    assert_eq!(session.handle(Key::Space).edit, Edit::Commit("国".into()));
}

#[test]
fn punctuation_candidates_and_full_width_follow_switches() {
    let mut session = chinese();
    assert!(!session.handle(Key::Symbol('.')).handled);
    session.handle(Key::ToggleOption(Switch::AsciiPunct));
    assert_eq!(
        session.handle(Key::Symbol('.')).edit,
        Edit::Insert("。".into())
    );
    assert_eq!(
        session.handle(Key::Symbol('\'')).edit,
        Edit::Insert("‘".into())
    );
    assert_eq!(
        session.handle(Key::Symbol('\'')).edit,
        Edit::Insert("’".into())
    );
    assert_eq!(
        session.handle(Key::Symbol('[')).edit,
        Edit::Preedit("「".into())
    );
    assert_eq!(
        session
            .candidates()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["「", "【"]
    );
    assert_eq!(session.handle(Key::Next).edit, Edit::Preedit("【".into()));
    assert_eq!(
        session.handle(Key::Letter('a')).edit,
        Edit::Preedit("【日".into())
    );
    assert_eq!(
        session.handle(Key::Backspace).edit,
        Edit::Preedit("【".into())
    );
    assert_eq!(session.handle(Key::Space).edit, Edit::Commit("【".into()));
    session.handle(Key::ToggleOption(Switch::FullShape));
    assert_eq!(
        session.handle(Key::Symbol('1')).edit,
        Edit::Insert("１".into())
    );
    session.handle(Key::ToggleMode);
    assert_eq!(
        session.handle(Key::Letter('A')).edit,
        Edit::Insert("Ａ".into())
    );
    assert_eq!(
        session.handle(Key::Symbol('.')).edit,
        Edit::Insert("．".into())
    );
}

#[test]
fn homophone_sessions_keep_source_comments_and_check_exact_tones() {
    let table = "---\n...\n日\ta\n";
    let make_session = |code: &str| {
        let mut session = Session::new(Arc::new(Dictionary::parse([("test", table)]).unwrap()));
        session.ascii_mode = false;
        for ch in code.chars() {
            session.handle(Key::Letter(ch));
        }
        session
    };
    let common = make_session("az");
    assert!(common
        .ui_text(0)
        .unwrap()
        .starts_with("日 \"jat6, jat2, mik6\""));
    let exact = make_session("azx");
    assert!(exact.ui_text(0).unwrap().starts_with("日 \"jat6\""));
    assert!(!exact
        .matches
        .iter()
        .any(|item| matches!(item, Match::Homophone { text, .. } if &**text == "一")));
    assert_eq!(make_session("azxxxx").candidates()[0].text, "「輸入錯誤」");
}

#[test]
fn homophone_history_restores_highlights_and_nested_sources() {
    let table = "---\n...\n日\ta\n曰\ta\n曰\txa\n一\tm\n";
    let mut session = Session::new(Arc::new(Dictionary::parse([("test", table)]).unwrap()));
    session.ascii_mode = false;
    session.handle(Key::Letter('a'));
    session.handle(Key::Next);
    assert_eq!(
        session.handle(Key::Letter('z')).edit,
        Edit::Preedit("日 難日z".into())
    );
    assert!(session
        .ui_text(0)
        .unwrap()
        .starts_with("曰 \"joek6, jyut6\""));
    session.handle(Key::Backspace);
    assert_eq!(session.ui_selection(), Some(1));

    session.handle(Key::Previous);
    session.handle(Key::Letter('z'));
    assert_eq!(session.preedit(), "日z");
    session.handle(Key::Next);
    let nested = session
        .match_text(&session.matches[session.selected])
        .into_owned();
    assert_ne!(nested, "日");
    session.handle(Key::Letter('z'));
    assert!(session.preedit().ends_with("zz"));
    assert_eq!(&*session.homophone_anchor().unwrap().source, nested);
    assert!(session
        .ui_text(0)
        .unwrap()
        .starts_with(&format!("{nested} ")));
    session.handle(Key::Backspace);
    assert_eq!(session.ui_selection(), Some(1));
    session.handle(Key::Letter('x'));
    assert!(session.ui_text(0).unwrap().starts_with("日 \"jat6\""));
    session.handle(Key::Letter('x'));
    assert!(session.ui_text(0).unwrap().starts_with("日 \"jat2\""));
    session.handle(Key::Letter('x'));
    assert!(session.ui_text(0).unwrap().starts_with("日 \"mik6\""));
    assert_eq!(
        session.handle(Key::Letter('x')).edit,
        Edit::Preedit("日重難難難難".into())
    );
    assert_eq!(session.candidates()[0].text, "「輸入錯誤」");
    session.handle(Key::Backspace);
    assert!(session.ui_text(0).unwrap().starts_with("日 \"mik6\""));
    session.handle(Key::Escape);
    assert!(!session.is_composing());
    assert!(session.homophone_frames.is_empty());
}

#[test]
fn homophone_history_restores_remote_previews_after_reordering() {
    let remote = |preview: &str, index| Match::Remote {
        preview: Arc::from(preview),
        snapshot: 0,
        index,
    };
    let truncated = "字".repeat(PREVIEW_LIMIT) + "…";
    for displayed in ["短候選", &truncated] {
        let mut session = Session::sandboxed();
        session.raw.push('a');
        session.matches = vec![remote("甲", 0), remote(displayed, 1)].into();
        session.selected = 1;
        session.advance_homophone('b');

        session.homophone_frames.pop();
        session.matches = vec![remote("甲", 0), remote("乙", 1), remote(displayed, 2)].into();
        session.restore_homophone_selection();
        assert_eq!(session.selected, 2);
    }
}

#[test]
fn xz_keeps_cangjie_completions_without_starting_homophone_search() {
    let table = "---\n...\n曰\txa\n難題\txza\n";
    let mut session = Session::new(Arc::new(Dictionary::parse([("test", table)]).unwrap()));
    session.ascii_mode = false;
    for code in ["xz", "XZ"] {
        for letter in code.chars() {
            session.handle(Key::Letter(letter));
        }
        assert!(session.homophone_anchor().is_none());
        assert_eq!(session.ui_text(0).as_deref(), Some("難題"));
        session.handle(Key::Escape);
    }
    for letter in "xaz".chars() {
        session.handle(Key::Letter(letter));
    }
    assert_eq!(session.homophone_anchor().unwrap().source.as_ref(), "曰");
}

#[test]
fn dynamic_commands_keep_snapshot_order_and_full_clipboard_text() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeCommands(AtomicUsize);
    impl CommandProvider for FakeCommands {
        fn clipboard(&self) -> Option<commands::ClipboardSnapshot> {
            let count = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            Some(commands::ClipboardSnapshot::new(
                format!(
                    "合成简体\\n字面反斜線\n第二行🙂{}第{count}次",
                    "長".repeat(40)
                )
                .into(),
                Some(format!("合成繁體\n第二行🙂第{count}次").into()),
            ))
        }
        fn time(&self) -> String {
            "04:05:06".into()
        }
        fn date(&self) -> String {
            "07/08/2026".into()
        }
    }

    let table = "---\n...\n其他\tz\n__rime_clipboard_raw__\tzv\n__rime_clipboard_s2t__\tzv\n__rime_time__\tztime\n__rime_date__\tzdate\n";
    let provider = Arc::new(FakeCommands(AtomicUsize::new(0)));
    let mut session = Session::new(Arc::new(Dictionary::parse([("test", table)]).unwrap()));
    session.ascii_mode = false;
    session.command_provider = provider.clone();
    session.handle(Key::Letter('z'));
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    assert_eq!(session.ui_text(0).unwrap(), "其他");
    assert!(session.ui_text(1).unwrap().starts_with("合成简体"));
    assert_eq!(session.ui_text(3).unwrap(), "04:05:06 時間");
    assert_eq!(session.ui_text(4).unwrap(), "07/08/2026 日期");
    assert!(session.ui_text(1).unwrap().ends_with('…'));
    session.handle(Key::Letter('v'));
    session.handle(Key::Backspace);
    session.handle(Key::Letter('v'));
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    assert!(session.ui_text(1).unwrap().starts_with("合成繁體"));
    session.handle(Key::ToggleOption(Switch::Simplification));
    assert!(session.ui_text(0).unwrap().starts_with("合成简体"));
    assert!(session.ui_text(1).unwrap().starts_with("合成繁體"));
    let Match::Generated {
        text: simplified, ..
    } = &session.matches[0]
    else {
        panic!("缺少剪貼簿候選")
    };
    let simplified = simplified.clone();
    session.handle(Key::Backspace);
    session.handle(Key::Letter('v'));
    let Match::Generated { text, .. } = &session.matches[0] else {
        panic!("缺少剪貼簿候選")
    };
    assert!(Arc::ptr_eq(text, &simplified));
    session.handle(Key::ToggleOption(Switch::Simplification));
    let expected = format!("合成简体\\n字面反斜線\n第二行🙂{}第1次", "長".repeat(40));
    assert_eq!(
        session.handle(Key::Space).edit,
        Edit::CommitCandidate(expected)
    );
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    session.handle(Key::Letter('z'));
    session.handle(Key::Letter('v'));
    assert_eq!(provider.0.load(Ordering::SeqCst), 2);
    assert_eq!(
        session.handle(Key::Select(1)).edit,
        Edit::CommitCandidate("合成繁體\n第二行🙂第2次".into())
    );

    for (code, expected) in [("ztime", "04:05:06"), ("zdate", "07/08/2026")] {
        for ch in code.chars() {
            session.handle(Key::Letter(ch));
        }
        assert_eq!(
            session.handle(Key::Space).edit,
            Edit::Commit(expected.into())
        );
    }
}

#[test]
fn unavailable_or_identical_clipboard_candidates_are_handled_like_rime() {
    struct FakeCommands(bool);
    impl CommandProvider for FakeCommands {
        fn clipboard(&self) -> Option<commands::ClipboardSnapshot> {
            self.0
                .then(|| commands::ClipboardSnapshot::new("合成🙂".into(), Some("合成🙂".into())))
        }
        fn time(&self) -> String {
            String::new()
        }
        fn date(&self) -> String {
            String::new()
        }
    }
    let table = "---\n...\n__rime_clipboard_raw__\tzv\n__rime_clipboard_s2t__\tzv\n";
    let dictionary = Arc::new(Dictionary::parse([("test", table)]).unwrap());
    let mut session = Session::new(dictionary.clone());
    session.ascii_mode = false;
    session.command_provider = Arc::new(FakeCommands(true));
    session.handle(Key::Letter('z'));
    session.handle(Key::Letter('v'));
    assert_eq!(session.ui_count(), 2);
    assert_eq!(session.ui_text(0), session.ui_text(1));
    assert_eq!(
        session.handle(Key::Select(1)).edit,
        Edit::CommitCandidate("合成🙂".into())
    );

    let mut session = Session::new(dictionary);
    session.ascii_mode = false;
    session.command_provider = Arc::new(FakeCommands(false));
    session.handle(Key::Letter('z'));
    session.handle(Key::Letter('v'));
    assert_eq!(session.candidates()[0].text, "「輸入錯誤」");
    assert_eq!(session.handle(Key::Space).edit, Edit::Cancel);
}

#[test]
fn clipboard_text_can_equal_the_error_label_or_contain_supplementary_characters() {
    struct FakeCommands(&'static str);
    impl CommandProvider for FakeCommands {
        fn clipboard(&self) -> Option<commands::ClipboardSnapshot> {
            Some(commands::ClipboardSnapshot::new(self.0.into(), None))
        }
        fn time(&self) -> String {
            String::new()
        }
        fn date(&self) -> String {
            String::new()
        }
    }
    let table = "---\n...\n__rime_clipboard_raw__\tzv\n";
    let dictionary = Arc::new(Dictionary::parse([("test", table)]).unwrap());
    for raw in ["「輸入錯誤」", "𠀀🙂\n第二行"] {
        let mut session = Session::new(dictionary.clone());
        session.ascii_mode = false;
        session.limit_chars = true;
        session.command_provider = Arc::new(FakeCommands(raw));
        session.handle(Key::Letter('z'));
        session.handle(Key::Letter('v'));
        assert_eq!(
            session.handle(Key::Space).edit,
            Edit::CommitCandidate(raw.into())
        );
    }
}

#[test]
fn dynamic_command_candidates_match_isolated_rime() {
    struct FakeCommands;
    impl CommandProvider for FakeCommands {
        fn clipboard(&self) -> Option<commands::ClipboardSnapshot> {
            Some(commands::ClipboardSnapshot::new(
                "合成简体\n第二行🙂".into(),
                Some("合成繁體\n第二行🙂".into()),
            ))
        }
        fn time(&self) -> String {
            "04:05:06".into()
        }
        fn date(&self) -> String {
            "07/08/2026".into()
        }
    }
    let base = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data/dictionaries")
            .join(DICTIONARY_FILES[0]),
    )
    .unwrap();
    let custom = "---\n...\n__rime_clipboard_raw__\tzv\n__rime_clipboard_s2t__\tzv\n__rime_time__\tztime\n__rime_date__\tzdate\n";
    let dictionary =
        Arc::new(Dictionary::parse([("base", base.as_str()), ("custom", custom)]).unwrap());
    let mut case = None;
    let mut expected = Vec::<String>::new();
    let check = |case: Option<&str>, expected: &[String]| {
        let Some(code) = case else {
            return;
        };
        let mut session = Session::new(dictionary.clone());
        session.ascii_mode = false;
        session.command_provider = Arc::new(FakeCommands);
        for ch in code.chars() {
            session.handle(Key::Letter(ch));
        }
        let actual: Vec<_> = session
            .matches
            .iter()
            // 公用碼表在 zd 另有片假名。細分碼只比對動態候選的順序。
            .filter(|item| code == "z" || matches!(item, Match::Generated { .. }))
            .map(|item| session.match_text(item).into_owned())
            .collect();
        if code == "z" {
            // 字碼補全改按常用字及原碼表次序，候選內容維持一致。
            let mut actual = actual;
            let mut expected = expected.to_vec();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "參考碼 {code}");
        } else {
            assert_eq!(actual, expected, "參考碼 {code}");
        }
    };
    for line in include_str!("../../tests/fixtures/commands-rime.tsv").lines() {
        if let Some(code) = line.strip_prefix("@\t") {
            check(case, &expected);
            expected.clear();
            case = Some(code);
        } else {
            let (text, _) = line.split_once('\t').unwrap();
            expected.push(decode_dictionary_text(text.into()).into_owned());
        }
    }
    check(case, &expected);
}
