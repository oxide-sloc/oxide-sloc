// SPDX-License-Identifier: AGPL-3.0-or-later
// Coverage-boost tests targeting individual branches in the per-language
// style-guide analyzers (crates/sloc-languages/src/style/*). Each test feeds a
// crafted snippet whose shape forces a specific quote/brace/indent/naming arm.

use sloc_languages::{Language, style};

fn style_of(lang: Language, text: &str) -> style::StyleAnalysis {
    style::analyze_style(lang, text).expect("language has a style analyzer")
}

// ── Python: type hints, mixed quotes ──────────────────────────────────────────

#[test]
fn python_type_hints_and_mixed_quotes() {
    // `def f(x: int) -> int` is an annotated function (type_hints branch), and one
    // single + one double quote gives neither quote style a 70% majority → "Mixed".
    let code = concat!(
        "def f(x: int) -> int:\n",
        "    a = 'single'\n",
        "    b = \"double\"\n",
        "    return x\n",
    );
    let s = style_of(Language::Python, code);
    let quote = &s
        .signals
        .iter()
        .find(|s| s.name == "Quote Style")
        .unwrap()
        .value;
    assert_eq!(quote, "Mixed");
    let hints = &s
        .signals
        .iter()
        .find(|s| s.name == "Type Hints")
        .unwrap()
        .value;
    assert!(
        hints.contains("annotated"),
        "expected annotated function: {hints}"
    );
}

// ── Ruby: single quotes + frozen_string_literal ───────────────────────────────

#[test]
fn ruby_single_quotes_and_frozen_literal() {
    let code = concat!(
        "# frozen_string_literal: true\n",
        "a = 'one'\n",
        "b = 'two'\n",
        "c = 'three'\n",
    );
    let s = style_of(Language::Ruby, code);
    let quote = &s
        .signals
        .iter()
        .find(|s| s.name == "Quote Style")
        .unwrap()
        .value;
    assert_eq!(quote, "Single quotes");
    assert!(
        s.signals.iter().any(|s| s.name == "Frozen String Literal"),
        "frozen literal signal must be present"
    );
}

// ── JavaScript: var + double quotes ───────────────────────────────────────────

#[test]
fn js_var_present_and_double_quotes() {
    let code = concat!(
        "var x = \"hello\";\n",
        "var y = \"world\";\n",
        "var z = \"again\";\n",
    );
    let s = style_of(Language::JavaScript, code);
    let quote = &s
        .signals
        .iter()
        .find(|s| s.name == "Quote Style")
        .unwrap()
        .value;
    assert_eq!(quote, "Double quotes");
    // The var-usage signal must report legacy `var` present.
    assert!(
        s.signals
            .iter()
            .any(|sig| sig.value.contains("var present")),
        "expected a 'var present' signal: {:?}",
        s.signals
    );
}

// ── Java: Allman brace + wildcard import ───────────────────────────────────────

#[test]
fn java_allman_brace_and_wildcard_import() {
    let code = concat!(
        "import java.util.*;\n",
        "class P\n",
        "{\n",
        "    int x = 1;\n",
        "}\n",
    );
    let s = style_of(Language::Java, code);
    assert!(
        s.signals.iter().any(|sig| sig.value.contains("found")),
        "wildcard import count expected: {:?}",
        s.signals
    );
}

// ── C#: Allman brace ──────────────────────────────────────────────────────────

#[test]
fn csharp_allman_brace() {
    let code = concat!("class P\n", "{\n", "    int x = 1;\n", "}\n",);
    let _ = style_of(Language::CSharp, code); // exercises the `{`-alone allman arm
}

// ── Go: error returns ─────────────────────────────────────────────────────────

#[test]
fn go_error_returns() {
    let code = concat!(
        "func f() error {\n",
        "    x := 1\n",
        "    _ = x\n",
        "    return err\n",
        "}\n",
    );
    let s = style_of(Language::Go, code);
    let eh = &s
        .signals
        .iter()
        .find(|s| s.name == "Error Handling")
        .unwrap()
        .value;
    assert!(
        eh.contains("early return"),
        "expected early-return signal: {eh}"
    );
}

// ── Rust: CamelCase fn naming ─────────────────────────────────────────────────

#[test]
fn rust_camelcase_fn_naming() {
    let code = concat!("fn CamelName() {\n", "    let x = 1;\n", "}\n",);
    let s = style_of(Language::Rust, code);
    let naming = &s
        .signals
        .iter()
        .find(|s| s.name == "Function Naming")
        .unwrap()
        .value;
    assert!(
        naming.contains("CamelCase"),
        "expected CamelCase naming: {naming}"
    );
}

#[test]
fn rust_trailing_comma_mid_ratio() {
    // ~1 trailing comma across ~100 lines → ratio in (0.005, 0.02], the middle
    // trailing-comma scoring bucket.
    let mut code = String::from("fn f() {\n");
    for i in 0..100 {
        if i == 0 {
            code.push_str("    let a = vec![1, 2, 3],\n"); // one trailing comma
        } else {
            code.push_str("    let b = 1;\n");
        }
    }
    code.push_str("}\n");
    let _ = style_of(Language::Rust, &code);
}

// ── C/C++: brace styles, char literals, unclassified pointer, array init ───────

#[test]
fn cpp_allman_dominant_and_array_init() {
    // Two `{`-alone lines dominate → Allman; `int arr[] = {` is a non-block-head
    // brace opener (is_block_head → false).
    let code = concat!(
        "int arr[] = {\n",
        "    1, 2, 3,\n",
        "};\n",
        "int main()\n",
        "{\n",
        "    return 0;\n",
        "}\n",
        "void g()\n",
        "{\n",
        "    return;\n",
        "}\n",
    );
    let _ = style_of(Language::Cpp, code);
}

#[test]
fn cpp_mixed_brace_char_literal_and_multiply() {
    // One attach + one allman brace → Mixed; a char literal exercises the `'`
    // quote-state toggle; `a * b` is an unclassified `*` token.
    let code = concat!(
        "void f() {\n",
        "    char c = 'x';\n",
        "    int y = a * b;\n",
        "    if (z)\n",
        "    {\n",
        "        g();\n",
        "    }\n",
        "}\n",
    );
    let _ = style_of(Language::Cpp, code);
}

// ── common::classify_indent tie-break arms ────────────────────────────────────

#[test]
fn indent_spaces4_tiebreak() {
    // sp4=3, sp2=1, tabs=2 → no style ≥60%, but sp4 > sp2*2 and sp4 > tabs.
    let code = concat!(
        "fn f() {\n",
        "    let a = 1;\n", // 4 spaces
        "    let b = 2;\n", // 4 spaces
        "    let c = 3;\n", // 4 spaces
        "  let d = 4;\n",   // 2 spaces
        "\tlet e = 5;\n",   // tab
        "\tlet g = 6;\n",   // tab
        "}\n",
    );
    let _ = style_of(Language::Rust, code);
}

#[test]
fn indent_spaces2_tiebreak() {
    // sp2=5, sp4=3, tabs=3 → no style ≥60%, but sp2 > sp4 and sp2 > tabs.
    let code = concat!(
        "fn f() {\n",
        "  let a = 1;\n", // 2 spaces ×5
        "  let b = 2;\n",
        "  let c = 3;\n",
        "  let d = 4;\n",
        "  let e = 5;\n",
        "    let f1 = 1;\n", // 4 spaces ×3
        "    let f2 = 2;\n",
        "    let f3 = 3;\n",
        "\tlet g1 = 1;\n", // tab ×3
        "\tlet g2 = 2;\n",
        "\tlet g3 = 3;\n",
        "}\n",
    );
    let _ = style_of(Language::Rust, code);
}

#[test]
fn indent_mixed_scores_indent4_mixed_arm() {
    // tabs=1, sp2=1, sp4=1 → Mixed, then score_indent_4(Mixed) = 0.35.
    let code = concat!(
        "fn f() {\n",
        "\tlet a = 1;\n",   // tab
        "  let b = 2;\n",   // 2 spaces
        "    let c = 3;\n", // 4 spaces
        "}\n",
    );
    let _ = style_of(Language::Rust, code);
}
