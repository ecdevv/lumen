use super::*;

// --- block-level --------------------------------------------------

#[test]
fn paragraph_for_plain_text() {
    assert_eq!(
        parse_line("plain text"),
        Block::Paragraph(vec![Inline::Text("plain text".into())])
    );
}

#[test]
fn h1_h2_h3_recognized() {
    for (src, lvl) in [("# one", 1), ("## two", 2), ("### three", 3)] {
        let expected_text = src.trim_start_matches('#').trim_start().to_string();
        assert_eq!(
            parse_line(src),
            Block::Heading { level: lvl, inline: vec![Inline::Text(expected_text)] }
        );
    }
}

#[test]
fn h4_h5_h6_collapse_to_h3() {
    for prefix in ["####", "#####", "######"] {
        let src = format!("{prefix} deep");
        let Block::Heading { level, .. } = parse_line(&src) else {
            panic!("expected Heading for {src:?}");
        };
        assert_eq!(level, 3);
    }
}

#[test]
fn seven_hashes_is_not_a_heading() {
    // CommonMark caps at h6; we follow and treat 7+ as paragraph.
    assert!(matches!(parse_line("####### nope"), Block::Paragraph(_)));
}

#[test]
fn hash_without_space_is_not_a_heading() {
    // `#foo` is paragraph text, not a heading.
    assert!(matches!(parse_line("#foo"), Block::Paragraph(_)));
}

#[test]
fn bullet_markers() {
    for src in ["- a", "* a", "+ a"] {
        assert_eq!(
            parse_line(src),
            Block::Bullet(vec![Inline::Text("a".into())])
        );
    }
}

#[test]
fn numbered_marker_preserves_number() {
    assert_eq!(
        parse_line("42. item"),
        Block::Numbered {
            number: 42,
            inline: vec![Inline::Text("item".into())]
        }
    );
}

#[test]
fn horizontal_rule_variants() {
    for src in ["---", "***", "___", "- - -", "* * *"] {
        assert_eq!(parse_line(src), Block::Rule, "{src} should be Rule");
    }
}

#[test]
fn two_dashes_is_not_a_rule() {
    assert!(matches!(parse_line("--"), Block::Paragraph(_)));
}

// --- inline emphasis ----------------------------------------------

fn inline(src: &str) -> Vec<Inline> {
    parse_inline(src)
}

#[test]
fn bold_double_asterisk() {
    assert_eq!(
        inline("**foo**"),
        vec![Inline::Bold(vec![Inline::Text("foo".into())])]
    );
}

#[test]
fn bold_double_underscore() {
    assert_eq!(
        inline("__foo__"),
        vec![Inline::Bold(vec![Inline::Text("foo".into())])]
    );
}

#[test]
fn italic_single_asterisk() {
    assert_eq!(
        inline("*foo*"),
        vec![Inline::Italic(vec![Inline::Text("foo".into())])]
    );
}

#[test]
fn italic_single_underscore() {
    assert_eq!(
        inline("_foo_"),
        vec![Inline::Italic(vec![Inline::Text("foo".into())])]
    );
}

#[test]
fn inline_code_verbatim() {
    assert_eq!(
        inline("`x = 1`"),
        vec![Inline::Code("x = 1".into())]
    );
}

#[test]
fn code_does_not_parse_emphasis_inside() {
    assert_eq!(
        inline("`**not bold**`"),
        vec![Inline::Code("**not bold**".into())]
    );
}

#[test]
fn unclosed_emphasis_renders_as_literal() {
    assert_eq!(
        inline("*incomplete"),
        vec![Inline::Text("*incomplete".into())]
    );
}

#[test]
fn arithmetic_does_not_trigger_emphasis() {
    // `5 * 3 = 15` shouldn't become italic between the *'s.
    assert_eq!(
        inline("5 * 3 = 15"),
        vec![Inline::Text("5 * 3 = 15".into())]
    );
}

#[test]
fn bold_with_inner_italic_nests() {
    assert_eq!(
        inline("**foo *bar* baz**"),
        vec![Inline::Bold(vec![
            Inline::Text("foo ".into()),
            Inline::Italic(vec![Inline::Text("bar".into())]),
            Inline::Text(" baz".into()),
        ])]
    );
}

#[test]
fn mixed_text_and_emphasis_in_one_line() {
    assert_eq!(
        inline("call `foo()` then it is **done**"),
        vec![
            Inline::Text("call ".into()),
            Inline::Code("foo()".into()),
            Inline::Text(" then it is ".into()),
            Inline::Bold(vec![Inline::Text("done".into())]),
        ]
    );
}

#[test]
fn unclosed_inline_code_renders_as_literal_backtick() {
    assert_eq!(
        inline("oops `no close"),
        vec![Inline::Text("oops `no close".into())]
    );
}

// --- block + inline together --------------------------------------

#[test]
fn heading_with_emphasis() {
    assert_eq!(
        parse_line("## **Bold** heading"),
        Block::Heading {
            level: 2,
            inline: vec![
                Inline::Bold(vec![Inline::Text("Bold".into())]),
                Inline::Text(" heading".into()),
            ],
        }
    );
}

#[test]
fn bullet_with_inline_code() {
    assert_eq!(
        parse_line("- run `cargo test`"),
        Block::Bullet(vec![
            Inline::Text("run ".into()),
            Inline::Code("cargo test".into()),
        ])
    );
}
