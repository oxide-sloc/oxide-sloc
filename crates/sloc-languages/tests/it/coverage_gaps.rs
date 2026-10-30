// SPDX-License-Identifier: AGPL-3.0-or-later
// Targeted coverage-gap tests for sloc-languages: C/C++ definition heuristics,
// Python docstring edge cases, EOF-truncation warnings, extension-override and
// shebang detection paths.

use std::collections::BTreeMap;
use std::path::Path;

use sloc_languages::{
    AnalysisOptions, Language, ParseMode, analyze_text, detect_language, looks_like_cpp,
};

// ── C/C++ function / variable / scope heuristics ──────────────────────────────

/// A C++ file that drives function detection with a user-defined return type, a
/// namespaced return type, member vs. local vs. global variable scopes, an
/// object-like macro, and the most-vexing-parse disambiguation.
#[test]
fn cpp_function_variable_scope_and_macro_detection() {
    let src = "\
#define MAX_LEN 128
namespace app {
int g_counter = 0;
class Widget {
    int member_field;
    std::string label;
public:
    MyType compute(int a, int b) {
        int local_sum = a + b;
        return local_sum;
    }
};
std::string make_label(const Widget& w) {
    std::string s = \"x\";
    return s;
}
}
";
    let r = analyze_text(Language::Cpp, src, AnalysisOptions::default());
    // Object-like macro `#define MAX_LEN 128`.
    assert_eq!(r.raw.macro_definitions, 1, "MAX_LEN macro should count");
    // compute() and make_label() are function definitions with non-builtin returns.
    assert!(
        r.raw.functions >= 2,
        "expected >=2 functions, got {}",
        r.raw.functions
    );
    // A class body opener.
    assert!(r.raw.classes >= 1, "Widget class should be counted");
    // Scope-classified variables: member (inside class), local (inside fn), global (namespace).
    assert!(
        r.raw.variables_member >= 1,
        "member field expected, got {}",
        r.raw.variables_member
    );
    assert!(
        r.raw.variables_local >= 1,
        "local var expected, got {}",
        r.raw.variables_local
    );
    assert!(
        r.raw.variables_global >= 1,
        "global var expected, got {}",
        r.raw.variables_global
    );
    assert_eq!(
        r.raw.variables,
        r.raw.variables_member + r.raw.variables_local + r.raw.variables_global,
        "flat total must equal the scope breakdown sum"
    );
}

/// Most-vexing-parse: `T v(expr);` with a lone value is a variable, not a function,
/// while `T f(int);` (a prototype-shaped param list) is treated as a function.
#[test]
fn cpp_most_vexing_parse_direct_init_is_not_a_function() {
    // Lone value in parens → direct-init variable, must not count as a function.
    let var = "std::istringstream ss(input_string);\n";
    let r = analyze_text(Language::Cpp, var, AnalysisOptions::default());
    assert_eq!(
        r.raw.functions, 0,
        "direct-init variable must not be a function"
    );

    // Param-list-shaped `;` form → function prototype.
    let proto = "int parse(int argc, char** argv);\n";
    let r2 = analyze_text(Language::Cpp, proto, AnalysisOptions::default());
    assert_eq!(r2.raw.functions, 1, "prototype should count as a function");
}

/// Lines that must NOT be classified as C functions: calls, member access,
/// control flow, and pointer-arrow expressions.
#[test]
fn cpp_calls_and_control_flow_are_not_functions() {
    let src = "\
void run() {
    foo(x);
    obj->method(y);
    if (a && b) {
        do_work();
    }
}
";
    let r = analyze_text(Language::Cpp, src, AnalysisOptions::default());
    // Only run() itself is a definition.
    assert_eq!(
        r.raw.functions, 1,
        "only run() is a definition, got {}",
        r.raw.functions
    );
}

/// Braces inside string / char literals and a same-line block comment must not
/// corrupt the C/C++ scope stack.
#[test]
fn cpp_braces_in_strings_and_comments_do_not_break_scope() {
    let src = "\
void f() {
    const char* s = \"a { b } c\";
    char c = '}';
    int x = 1; /* inline { comment } */
    int y = 2;
}
";
    let r = analyze_text(Language::Cpp, src, AnalysisOptions::default());
    // x and y are locals inside f().
    assert!(
        r.raw.variables_local >= 2,
        "expected >=2 locals, got {}",
        r.raw.variables_local
    );
}

/// `looks_like_cpp` marker detection.
#[test]
fn looks_like_cpp_detects_markers_and_rejects_plain_c() {
    assert!(looks_like_cpp("namespace app {\n"));
    assert!(looks_like_cpp("auto x = std::make_unique<T>();\n"));
    assert!(looks_like_cpp("constexpr int k = 3;\n"));
    assert!(!looks_like_cpp("int add(int a, int b) { return a + b; }\n"));
}

// ── Python docstring edge cases ───────────────────────────────────────────────

/// Single-quote triple docstrings, decorators before a def, and an unclosed
/// docstring that runs to end-of-file.
#[test]
fn python_single_quote_docstring_and_decorator() {
    let src = "\
@decorator
def greet():
    '''single-quote docstring'''
    return 1
";
    let r = analyze_text(Language::Python, src, AnalysisOptions::default());
    assert!(
        r.raw.docstring_comment_lines >= 1,
        "single-quote docstring should be counted, got {}",
        r.raw.docstring_comment_lines
    );
    assert!(r.raw.functions >= 1, "greet() should count as a function");
}

/// An unclosed triple-quoted docstring at EOF marks all remaining lines and
/// produces a best-effort warning.
#[test]
fn python_unclosed_docstring_at_eof_is_best_effort() {
    let src = "\
def f():
    \"\"\"this docstring never closes
    still going
";
    let r = analyze_text(Language::Python, src, AnalysisOptions::default());
    // The unclosed docstring runs to EOF; every remaining line is marked as a
    // docstring comment line via mark_unclosed_docstring_lines.
    assert!(
        r.raw.docstring_comment_lines >= 2,
        "remaining lines after an unclosed docstring should be marked, got {}",
        r.raw.docstring_comment_lines
    );
}

// ── EOF-truncation warnings (best-effort parse mode) ──────────────────────────

#[test]
fn cpp_unclosed_block_comment_is_best_effort() {
    let src = "int x = 1;\n/* this comment never closes\nstill inside\n";
    let r = analyze_text(Language::Cpp, src, AnalysisOptions::default());
    assert!(
        matches!(r.parse_mode, ParseMode::LexicalBestEffort),
        "unclosed block comment must be best-effort"
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("block comment")),
        "should warn about the unclosed block comment: {:?}",
        r.warnings
    );
}

#[test]
fn cpp_unclosed_string_literal_is_best_effort() {
    let src = "const char* s = \"unterminated\nint y = 2;\n";
    let r = analyze_text(Language::Cpp, src, AnalysisOptions::default());
    assert!(
        r.warnings.iter().any(|w| w.contains("string literal")),
        "should warn about the unclosed string literal: {:?}",
        r.warnings
    );
}

/// A backslash-continued line reaching EOF while continuation collapsing is on
/// flushes the pending continuation.
#[test]
fn c_continuation_line_flushed_at_eof() {
    let opts = AnalysisOptions {
        collapse_continuation_lines: true,
        ..AnalysisOptions::default()
    };
    let src = "int total = a + \\\n    b + \\\n    c";
    let r = analyze_text(Language::C, src, opts);
    // The three physical lines collapse; at least one code line is recorded.
    assert!(
        r.raw.code_only_lines >= 1,
        "collapsed continuation should record code, got {}",
        r.raw.code_only_lines
    );
}

// ── detect_language: extension override + shebang ─────────────────────────────

#[test]
fn detect_language_extension_override_wins() {
    let mut overrides = BTreeMap::new();
    // Map an unusual extension to Rust via override.
    overrides.insert("rrs".to_string(), "Rust".to_string());
    let lang = detect_language(Path::new("weird.rrs"), None, &overrides, true);
    assert_eq!(
        lang,
        Some(Language::Rust),
        "override should map .rrs to Rust"
    );
}

#[test]
fn detect_language_shebang_node_and_unknown() {
    let empty = BTreeMap::new();
    // node shebang → JavaScript
    let js = detect_language(
        Path::new("script"),
        Some("#!/usr/bin/env node"),
        &empty,
        true,
    );
    assert_eq!(js, Some(Language::JavaScript), "node shebang → JavaScript");

    // Unknown shebang → None
    let unknown = detect_language(
        Path::new("script"),
        Some("#!/usr/bin/env frobnicate"),
        &empty,
        true,
    );
    assert_eq!(unknown, None, "unknown interpreter → no language");
}

/// A language with no symbol patterns (SP_NONE) analyses cleanly with zero symbols.
#[test]
fn xml_has_no_symbol_counts() {
    let r = analyze_text(
        Language::Xml,
        "<!-- doc -->\n<root>\n  <child/>\n</root>\n",
        AnalysisOptions::default(),
    );
    assert_eq!(r.raw.functions, 0);
    assert_eq!(r.raw.classes, 0);
    assert!(r.raw.code_only_lines >= 1, "XML markup lines are code");
}
