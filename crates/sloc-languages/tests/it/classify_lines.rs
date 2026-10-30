// SPDX-License-Identifier: AGPL-3.0-or-later
// Tests for `classify_physical_lines` — the physical-line-aligned coarse classifier that
// powers per-author code-ownership attribution in sloc-core. It must return exactly one
// category per physical line so the result zips one-to-one with `git blame` output.

use sloc_languages::{Language, LineCategory, classify_physical_lines};

fn counts(cats: &[LineCategory]) -> (usize, usize, usize) {
    let code = cats.iter().filter(|c| **c == LineCategory::Code).count();
    let comment = cats.iter().filter(|c| **c == LineCategory::Comment).count();
    let blank = cats.iter().filter(|c| **c == LineCategory::Blank).count();
    (code, comment, blank)
}

#[test]
fn one_category_per_physical_line() {
    let src = "fn main() {\n    // a comment\n\n    let x = 1;\n}\n";
    let cats = classify_physical_lines(Language::Rust, src);
    assert_eq!(cats.len(), 5, "one entry per physical line");
    assert_eq!(
        cats,
        vec![
            LineCategory::Code,    // fn main() {
            LineCategory::Comment, // // a comment
            LineCategory::Blank,   // (empty)
            LineCategory::Code,    // let x = 1;
            LineCategory::Code,    // }
        ]
    );
}

#[test]
fn block_comment_spans_including_blank_interior() {
    let src = "/*\n\n comment body\n*/\ncode();\n";
    let cats = classify_physical_lines(Language::C, src);
    let (code, comment, blank) = counts(&cats);
    assert_eq!(code, 1, "only the code() line is code");
    assert_eq!(
        blank, 0,
        "blank line inside a block comment counts as comment"
    );
    assert_eq!(comment, 4);
}

#[test]
fn mixed_code_and_trailing_comment_is_code() {
    let src = "let y = 2; // trailing\n";
    let cats = classify_physical_lines(Language::Rust, src);
    assert_eq!(cats, vec![LineCategory::Code]);
}

#[test]
fn empty_input_yields_no_lines() {
    assert!(classify_physical_lines(Language::Rust, "").is_empty());
}

#[test]
fn python_docstring_lines_are_comments() {
    let src = "def f():\n    \"\"\"\n    doc\n    \"\"\"\n    return 1\n";
    let cats = classify_physical_lines(Language::Python, src);
    assert_eq!(cats.len(), 5);
    assert_eq!(cats[0], LineCategory::Code);
    assert_eq!(cats[1], LineCategory::Comment);
    assert_eq!(cats[2], LineCategory::Comment);
    assert_eq!(cats[3], LineCategory::Comment);
    assert_eq!(cats[4], LineCategory::Code);
}
