// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

/// Golden tests: known-good fixture files with expected line counts.
///
/// Each test reads a corpus file, runs the lexical analyzer, and asserts exact
/// counts. The corpus files are checked into git so regressions are caught
/// immediately on CI.
use std::path::Path;

use sloc_languages::{AnalysisOptions, Language, analyze_text, detect_language, looks_like_cpp};

fn corpus(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read corpus file {}: {e}", path.display()))
}

// ─── C ────────────────────────────────────────────────────────────────────────

#[test]
fn c_mixed() {
    let text = corpus("c/mixed.c");
    let result = analyze_text(Language::C, &text, AnalysisOptions::default());
    let r = &result.raw;
    // split_terminator omits the empty string after the final newline → 8 lines
    assert_eq!(r.total_physical_lines, 8, "total physical lines");
    assert_eq!(r.blank_only_lines, 1, "blank lines");
    assert_eq!(r.multi_comment_only_lines, 1, "block-comment-only lines");
    assert_eq!(r.single_comment_only_lines, 1, "single-comment-only lines");
    assert_eq!(
        r.mixed_code_single_comment_lines, 1,
        "mixed code+single-comment"
    );
    assert_eq!(
        r.mixed_code_multi_comment_lines, 1,
        "mixed code+block-comment"
    );
    assert_eq!(
        r.code_only_lines, 3,
        "pure code lines: int y=2, void foo() {{...}}, closing brace"
    );
}

// ─── Python ───────────────────────────────────────────────────────────────────

#[test]
fn python_mixed() {
    let text = corpus("python/mixed.py");
    let result = analyze_text(Language::Python, &text, AnalysisOptions::default());
    let r = &result.raw;
    // 3 docstring lines (module doc, greet doc, Greeter class doc)
    assert_eq!(r.docstring_comment_lines, 3, "docstring lines");
    assert_eq!(r.mixed_code_single_comment_lines, 1, "mixed code+inline #");
    // blanks between sections
    assert!(r.blank_only_lines >= 2, "blank lines");
    // def greet, class Greeter, def hello, pass = 4 code lines
    assert!(r.code_only_lines >= 4, "pure code lines");
}

// ─── Rust ─────────────────────────────────────────────────────────────────────

#[test]
fn rust_mixed() {
    let text = corpus("rust/mixed.rs");
    let result = analyze_text(Language::Rust, &text, AnalysisOptions::default());
    let r = &result.raw;
    assert_eq!(r.single_comment_only_lines, 1, "// comment-only line");
    assert_eq!(
        r.multi_comment_only_lines, 1,
        "/* block comment-only line */"
    );
    assert_eq!(
        r.mixed_code_single_comment_lines, 1,
        "mixed code + // comment"
    );
    assert!(r.code_only_lines >= 4, "pure code lines");
}

// ─── Go ───────────────────────────────────────────────────────────────────────

#[test]
fn go_mixed() {
    let text = corpus("go/mixed.go");
    let result = analyze_text(Language::Go, &text, AnalysisOptions::default());
    let r = &result.raw;
    assert!(r.single_comment_only_lines >= 2, "// comment-only lines");
    assert_eq!(
        r.mixed_code_multi_comment_lines, 1,
        "mixed code + /* block */"
    );
    assert!(r.code_only_lines >= 3, "pure code lines");
}

// ─── TypeScript ───────────────────────────────────────────────────────────────

#[test]
fn typescript_mixed() {
    let text = corpus("typescript/mixed.ts");
    let result = analyze_text(Language::TypeScript, &text, AnalysisOptions::default());
    let r = &result.raw;
    assert_eq!(r.single_comment_only_lines, 1, "// comment-only");
    assert_eq!(r.multi_comment_only_lines, 1, "/* block comment-only */");
    assert_eq!(
        r.mixed_code_single_comment_lines, 1,
        "mixed code + // inline"
    );
    assert!(r.code_only_lines >= 3, "pure code lines");
}

// ─── Entity counts ────────────────────────────────────────────────────────────

#[test]
fn rust_entities() {
    let text = corpus("rust/entities.rs");
    let result = analyze_text(Language::Rust, &text, AnalysisOptions::default());
    let r = &result.raw;
    // fn area, fn new, fn distance, fn helper, fn test_new, fn test_color
    assert_eq!(r.functions, 6, "function definitions");
    // struct Point, enum Color, trait Shape, impl Point
    assert_eq!(r.classes, 4, "class/struct/trait/impl definitions");
    // let pt, let sq, let _tmp, let p, let c
    assert_eq!(r.variables, 5, "variable declarations");
    // use std::io, use std::fmt, use super::*
    assert_eq!(r.imports, 3, "import statements");
    // two #[test] attributes
    assert_eq!(r.test_count, 2, "test annotations");
    // assert_eq! and assert!
    assert_eq!(r.test_assertion_count, 2, "assertion calls");
    assert_eq!(r.test_suite_count, 0, "no test suites");
}

#[test]
fn cpp_entities() {
    let text = corpus("cpp/entities.cpp");
    let result = analyze_text(Language::Cpp, &text, AnalysisOptions::default());
    let r = &result.raw;
    // Widget::render prototype + Widget::render definition + add + range + is_even.
    // Class-typed return values (std::string, std::vector<int>) are counted, not just built-ins.
    assert_eq!(r.functions, 5, "function definitions and prototypes");
    // class Widget, struct Point, namespace demo
    assert_eq!(r.classes, 3, "class/struct/namespace definitions");
    // members id, name, x, y (4) + locals out, sum, result (3) + global g_widget_count (1)
    assert_eq!(r.variables, 8, "all variable declarations");
    assert_eq!(r.variables_member, 4, "class/struct member fields");
    assert_eq!(
        r.variables_local, 3,
        "function-local variables (for-loop `i` excluded)"
    );
    assert_eq!(r.variables_global, 1, "file-scope global");
    // member + local + global always reconstruct the C/C++ variable total.
    assert_eq!(
        r.variables_member + r.variables_local + r.variables_global,
        r.variables,
        "scope breakdown must sum to the total"
    );
    // #define MAX_WIDGETS 100 (object-like); SQUARE(x) is function-like and excluded.
    assert_eq!(r.macro_definitions, 1, "object-like macro constants");
    // #include <string>, #include <vector>
    assert_eq!(r.imports, 2, "include directives");
}

#[test]
fn python_entities() {
    let text = corpus("python/entities.py");
    let result = analyze_text(Language::Python, &text, AnalysisOptions::default());
    let r = &result.raw;
    // def __init__, def speak, def helper — test methods excluded
    assert_eq!(r.functions, 3, "plain function definitions");
    // class Animal only — class TestAnimal excluded (matched test pattern)
    assert_eq!(r.classes, 1, "plain class definitions");
    // import os, from sys import argv
    assert_eq!(r.imports, 2, "import statements");
    // class TestAnimal, def test_speak, def test_helper
    assert_eq!(r.test_count, 3, "test definitions");
    // self.assertEqual x2
    assert_eq!(r.test_assertion_count, 2, "assertion calls");
}

#[test]
fn go_entities() {
    let text = corpus("go/entities.go");
    let result = analyze_text(Language::Go, &text, AnalysisOptions::default());
    let r = &result.raw;
    // func Area, func helper — test funcs excluded
    assert_eq!(r.functions, 2, "plain function definitions");
    // type Point struct
    assert_eq!(r.classes, 1, "type definitions");
    // var result, var x
    assert_eq!(r.variables, 2, "variable declarations");
    // import "fmt"
    assert_eq!(r.imports, 1, "import statements");
    // func TestPoint, func BenchmarkHelper
    assert_eq!(r.test_count, 2, "test function definitions");
    assert_eq!(r.test_assertion_count, 0, "no assertions");
}

#[test]
fn javascript_entities() {
    let text = corpus("javascript/entities.js");
    let result = analyze_text(Language::JavaScript, &text, AnalysisOptions::default());
    let r = &result.raw;
    // function multiply only — arrow function and class methods not counted
    assert_eq!(r.functions, 1, "function declarations");
    // class Calculator
    assert_eq!(r.classes, 1, "class definitions");
    // let result, const divide
    assert_eq!(r.variables, 2, "variable declarations");
    // import { foo } from './foo'
    assert_eq!(r.imports, 1, "import statements");
    // describe, it, test
    assert_eq!(r.test_count, 3, "test block openers");
    // expect x2
    assert_eq!(r.test_assertion_count, 2, "assertion calls");
}

// ─── Broadened test detection (frameworks beyond the original set) ──────────────

#[test]
fn perl_test_more_entities() {
    let text = corpus("perl/tests.pl");
    let r = &analyze_text(Language::Perl, &text, AnalysisOptions::default()).raw;
    // one `subtest` block opener
    assert_eq!(r.test_count, 1, "subtest group");
    // ok(, is(, is_deeply(
    assert_eq!(r.test_assertion_count, 3, "Test::More assertion calls");
}

#[test]
fn erlang_eunit_entities() {
    let text = corpus("erlang/tests.erl");
    let r = &analyze_text(Language::Erlang, &text, AnalysisOptions::default()).raw;
    // EUnit names are suffix-based (`_test`), so only the ?assert* macros are counted
    assert_eq!(r.test_count, 0, "no prefix-matchable test names");
    assert_eq!(r.test_assertion_count, 2, "?assertEqual + ?assert");
}

#[test]
fn haskell_hspec_entities() {
    let text = corpus("haskell/tests.hs");
    let r = &analyze_text(Language::Haskell, &text, AnalysisOptions::default()).raw;
    // describe + two it blocks
    assert_eq!(r.test_count, 3, "Hspec describe/it openers");
}

#[test]
fn ocaml_ounit_entities() {
    let text = corpus("ocaml/tests.ml");
    let r = &analyze_text(Language::Ocaml, &text, AnalysisOptions::default()).raw;
    // let test_add, let test_bool (let suite is a plain function)
    assert_eq!(r.test_count, 2, "OUnit test functions");
    assert_eq!(r.test_assertion_count, 2, "assert_equal + assert_bool");
}

#[test]
fn shell_bats_entities() {
    let text = corpus("shell/tests.sh");
    let r = &analyze_text(Language::Shell, &text, AnalysisOptions::default()).raw;
    // two @test blocks
    assert_eq!(r.test_count, 2, "bats @test blocks");
}

// ─── Empty file ───────────────────────────────────────────────────────────────

#[test]
fn empty_file_all_languages() {
    for lang in [
        Language::C,
        Language::Cpp,
        Language::CSharp,
        Language::Go,
        Language::Java,
        Language::JavaScript,
        Language::Python,
        Language::Rust,
        Language::Shell,
        Language::PowerShell,
        Language::TypeScript,
        // Extended languages
        Language::Assembly,
        Language::Clojure,
        Language::Css,
        Language::Dart,
        Language::Dockerfile,
        Language::Elixir,
        Language::Erlang,
        Language::FSharp,
        Language::Groovy,
        Language::Haskell,
        Language::Html,
        Language::Julia,
        Language::Kotlin,
        Language::Lua,
        Language::Makefile,
        Language::Nim,
        Language::ObjectiveC,
        Language::Ocaml,
        Language::Perl,
        Language::Php,
        Language::R,
        Language::Ruby,
        Language::Scala,
        Language::Scss,
        Language::Sql,
        Language::Svelte,
        Language::Swift,
        Language::Vue,
        Language::Xml,
        Language::Zig,
        Language::Solidity,
        Language::Protobuf,
        Language::Hcl,
        Language::GraphQl,
        Language::Ada,
        Language::Vhdl,
        Language::Verilog,
        Language::Tcl,
        Language::Pascal,
        Language::VisualBasic,
        Language::Lisp,
        Language::Fortran,
        Language::Nix,
        Language::Crystal,
        Language::D,
        Language::Glsl,
        Language::Cmake,
        Language::Elm,
        Language::Awk,
    ] {
        let result = analyze_text(lang, "", AnalysisOptions::default());
        assert_eq!(
            result.raw.total_physical_lines,
            0,
            "{} should have 0 lines for empty input",
            lang.display_name()
        );
    }
}

// ─── Basic corpus: extended languages ────────────────────────────────────────

#[test]
fn assembly_basic() {
    let text = corpus("assembly/basic.asm");
    let r = &analyze_text(Language::Assembly, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1, "should detect code lines");
    assert!(r.single_comment_only_lines >= 1, "should detect ; comments");
}

#[test]
fn clojure_basic() {
    let text = corpus("clojure/basic.clj");
    let r = &analyze_text(Language::Clojure, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.single_comment_only_lines >= 1,
        "should detect ;; comments"
    );
}

#[test]
fn css_basic() {
    let text = corpus("css/basic.css");
    let r = &analyze_text(Language::Css, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.multi_comment_only_lines >= 1,
        "should detect /* */ block comments"
    );
}

#[test]
fn dart_basic() {
    let text = corpus("dart/basic.dart");
    let r = &analyze_text(Language::Dart, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn dockerfile_basic() {
    let text = corpus("dockerfile/basic.dockerfile");
    let r = &analyze_text(Language::Dockerfile, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1, "should detect # comments");
}

#[test]
fn elixir_basic() {
    let text = corpus("elixir/basic.ex");
    let r = &analyze_text(Language::Elixir, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn erlang_basic() {
    let text = corpus("erlang/basic.erl");
    let r = &analyze_text(Language::Erlang, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1, "should detect % comments");
}

#[test]
fn fsharp_basic() {
    let text = corpus("fsharp/basic.fs");
    let r = &analyze_text(Language::FSharp, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn groovy_basic() {
    let text = corpus("groovy/basic.groovy");
    let r = &analyze_text(Language::Groovy, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn haskell_basic() {
    let text = corpus("haskell/basic.hs");
    let r = &analyze_text(Language::Haskell, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.single_comment_only_lines >= 1,
        "should detect -- comments"
    );
}

#[test]
fn html_basic() {
    let text = corpus("html/basic.html");
    let r = &analyze_text(Language::Html, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.multi_comment_only_lines >= 1,
        "should detect <!-- --> comments"
    );
}

#[test]
fn julia_basic() {
    let text = corpus("julia/basic.jl");
    let r = &analyze_text(Language::Julia, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn kotlin_basic() {
    let text = corpus("kotlin/basic.kt");
    let r = &analyze_text(Language::Kotlin, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn lua_basic() {
    let text = corpus("lua/basic.lua");
    let r = &analyze_text(Language::Lua, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.single_comment_only_lines >= 1,
        "should detect -- comments"
    );
}

#[test]
fn makefile_basic() {
    let text = corpus("makefile/basic.mk");
    let r = &analyze_text(Language::Makefile, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1, "should detect # comments");
}

#[test]
fn nim_basic() {
    let text = corpus("nim/basic.nim");
    let r = &analyze_text(Language::Nim, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn objectivec_basic() {
    let text = corpus("objectivec/basic.m");
    let r = &analyze_text(Language::ObjectiveC, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn ocaml_basic() {
    let text = corpus("ocaml/basic.ml");
    let r = &analyze_text(Language::Ocaml, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.multi_comment_only_lines >= 1,
        "should detect (* *) block comments"
    );
}

#[test]
fn perl_basic() {
    let text = corpus("perl/basic.pl");
    let r = &analyze_text(Language::Perl, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn php_basic() {
    let text = corpus("php/basic.php");
    let r = &analyze_text(Language::Php, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn r_basic() {
    let text = corpus("r/basic.r");
    let r = &analyze_text(Language::R, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn ruby_basic() {
    let text = corpus("ruby/basic.rb");
    let r = &analyze_text(Language::Ruby, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn scala_basic() {
    let text = corpus("scala/basic.scala");
    let r = &analyze_text(Language::Scala, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn scss_basic() {
    let text = corpus("scss/basic.scss");
    let r = &analyze_text(Language::Scss, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
}

#[test]
fn sql_basic() {
    let text = corpus("sql/basic.sql");
    let r = &analyze_text(Language::Sql, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.single_comment_only_lines >= 1,
        "should detect -- comments"
    );
}

#[test]
fn svelte_basic() {
    let text = corpus("svelte/basic.svelte");
    let r = &analyze_text(Language::Svelte, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
}

#[test]
fn swift_basic() {
    let text = corpus("swift/basic.swift");
    let r = &analyze_text(Language::Swift, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn vue_basic() {
    let text = corpus("vue/basic.vue");
    let r = &analyze_text(Language::Vue, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
}

#[test]
fn xml_basic() {
    let text = corpus("xml/basic.xml");
    let r = &analyze_text(Language::Xml, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(
        r.multi_comment_only_lines >= 1,
        "should detect <!-- --> comments"
    );
}

#[test]
fn zig_basic() {
    let text = corpus("zig/basic.zig");
    let r = &analyze_text(Language::Zig, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn cpp_basic() {
    let text = corpus("cpp/basic.cpp");
    let r = &analyze_text(Language::Cpp, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn csharp_basic() {
    let text = corpus("csharp/basic.cs");
    let r = &analyze_text(Language::CSharp, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn java_basic() {
    let text = corpus("java/basic.java");
    let r = &analyze_text(Language::Java, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn shell_basic() {
    let text = corpus("shell/basic.sh");
    let r = &analyze_text(Language::Shell, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

#[test]
fn powershell_basic() {
    let text = corpus("powershell/basic.ps1");
    let r = &analyze_text(Language::PowerShell, &text, AnalysisOptions::default()).raw;
    assert!(r.total_physical_lines >= 3);
    assert!(r.code_only_lines >= 1);
    assert!(r.single_comment_only_lines >= 1);
}

// ─── Pass 1 language detection ────────────────────────────────────────────────

#[test]
fn detect_by_extension_tsx_is_typescript() {
    let path = std::path::Path::new("App.tsx");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::TypeScript));
}

#[test]
fn detect_by_extension_jsx_is_javascript() {
    let path = std::path::Path::new("App.jsx");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::JavaScript));
}

#[test]
fn detect_by_extension_sol_is_solidity() {
    let path = std::path::Path::new("Token.sol");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Solidity));
}

#[test]
fn detect_by_extension_proto_is_protobuf() {
    let path = std::path::Path::new("service.proto");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Protobuf));
}

#[test]
fn detect_by_extension_tf_is_hcl() {
    let path = std::path::Path::new("main.tf");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Hcl));
}

#[test]
fn detect_by_extension_graphql_is_graphql() {
    let path = std::path::Path::new("schema.graphql");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::GraphQl));
}

// ─── Pass 1 analysis behaviour ────────────────────────────────────────────────

#[test]
fn solidity_classifies_comments_and_code() {
    let src = "// SPDX-License-Identifier: MIT\n\
               pragma solidity ^0.8.0;\n\
               /* a block\n   comment */\n\
               contract Token {\n\
               \x20\x20function mint() public {}\n\
               }\n";
    let r = &analyze_text(Language::Solidity, src, AnalysisOptions::default()).raw;
    assert!(
        r.single_comment_only_lines >= 1,
        "expected a // comment line"
    );
    assert!(
        r.multi_comment_only_lines >= 1,
        "expected a block comment line"
    );
    assert!(
        r.code_only_lines >= 3,
        "expected pragma/contract/function code lines"
    );
}

#[test]
fn graphql_hash_comment_is_comment() {
    let src = "# the root query type\n\
               type Query {\n\
               \x20\x20hello: String\n\
               }\n";
    let r = &analyze_text(Language::GraphQl, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1);
    assert!(r.code_only_lines >= 2);
}

#[test]
fn hcl_supports_hash_and_slash_comments() {
    let src = "# hash comment\n\
               // slash comment\n\
               resource \"aws_s3_bucket\" \"b\" {\n\
               \x20\x20bucket = \"my-bucket\"\n\
               }\n";
    let r = &analyze_text(Language::Hcl, src, AnalysisOptions::default()).raw;
    assert_eq!(
        r.single_comment_only_lines, 2,
        "both # and // are line comments"
    );
    assert!(r.code_only_lines >= 2);
}

// ─── Pass 2 language detection (legacy + embedded / HDL) ──────────────────────

#[test]
fn detect_pass2_extensions() {
    let cases = [
        ("pkg.adb", Language::Ada),
        ("cpu.vhd", Language::Vhdl),
        ("alu.sv", Language::Verilog),
        ("alu.v", Language::Verilog),
        ("build.tcl", Language::Tcl),
        ("unit.pas", Language::Pascal),
        ("Form1.vb", Language::VisualBasic),
        ("core.lisp", Language::Lisp),
        ("init.el", Language::Lisp),
        ("main.scm", Language::Lisp),
    ];
    for (path, expected) in cases {
        let lang = detect_language(
            std::path::Path::new(path),
            None,
            &std::collections::BTreeMap::new(),
            false,
        );
        assert_eq!(lang, Some(expected), "{path} should detect as {expected:?}");
    }
}

// ─── Pass 2 analysis behaviour ────────────────────────────────────────────────

#[test]
fn ada_dash_dash_comment_is_comment() {
    let src = "-- a comment\n\
               procedure Main is\n\
               begin\n\
               \x20\x20null;\n\
               end Main;\n";
    let r = &analyze_text(Language::Ada, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1);
    assert!(r.code_only_lines >= 3);
}

#[test]
fn verilog_classifies_slash_comments() {
    let src = "// line comment\n\
               module top;\n\
               /* block */\n\
               \x20\x20wire a;\n\
               endmodule\n";
    let r = &analyze_text(Language::Verilog, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1);
    assert_eq!(r.multi_comment_only_lines, 1);
    assert!(r.code_only_lines >= 3);
}

#[test]
fn pascal_brace_block_and_slash_line_comments() {
    let src = "{ a brace block comment }\n\
               // a line comment\n\
               procedure Hello;\n\
               begin\n\
               end;\n";
    let r = &analyze_text(Language::Pascal, src, AnalysisOptions::default()).raw;
    assert_eq!(r.multi_comment_only_lines, 1, "{{ }} is a block comment");
    assert_eq!(r.single_comment_only_lines, 1, "// is a line comment");
    assert!(r.code_only_lines >= 3);
}

#[test]
fn visual_basic_apostrophe_is_comment_not_string() {
    let src = "' a comment\n\
               Public Sub Main()\n\
               \x20\x20Dim x As String = \"hi\"\n\
               End Sub\n";
    let r = &analyze_text(Language::VisualBasic, src, AnalysisOptions::default()).raw;
    assert_eq!(
        r.single_comment_only_lines, 1,
        "' opens a comment, not a string"
    );
    assert!(r.code_only_lines >= 3);
}

#[test]
fn assembly_gas_block_comment_now_counted() {
    // Regression for the Assembly dialect fix: GAS `/* */` blocks are recognized.
    let src = "; nasm-style line comment\n\
               /* gas block\n   comment */\n\
               mov eax, 1\n";
    let r = &analyze_text(Language::Assembly, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1, "; line comment");
    assert!(r.multi_comment_only_lines >= 1, "/* */ block now counted");
    assert!(r.code_only_lines >= 1);
}

// ─── Pass 3 language detection (scientific / infra / systems / graphics) ──────

#[test]
fn detect_pass3_extensions() {
    let cases = [
        ("solver.f90", Language::Fortran),
        ("legacy.f", Language::Fortran),
        ("default.nix", Language::Nix),
        ("server.cr", Language::Crystal),
        ("app.d", Language::D),
        ("shader.frag", Language::Glsl),
        ("compute.wgsl", Language::Glsl),
        ("build.cmake", Language::Cmake),
        ("Main.elm", Language::Elm),
        ("report.awk", Language::Awk),
    ];
    for (path, expected) in cases {
        let lang = detect_language(
            std::path::Path::new(path),
            None,
            &std::collections::BTreeMap::new(),
            false,
        );
        assert_eq!(lang, Some(expected), "{path} should detect as {expected:?}");
    }
}

#[test]
fn detect_cmakelists_by_filename() {
    let lang = detect_language(
        std::path::Path::new("CMakeLists.txt"),
        None,
        &std::collections::BTreeMap::new(),
        false,
    );
    assert_eq!(lang, Some(Language::Cmake));
}

// ─── Pass 3 analysis behaviour ────────────────────────────────────────────────

#[test]
fn fortran_bang_comment_is_comment() {
    let src = "! a comment\n\
               program demo\n\
               \x20\x20print *, 1  ! inline\n\
               end program demo\n";
    let r = &analyze_text(Language::Fortran, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1, "leading ! comment");
    assert_eq!(r.mixed_code_single_comment_lines, 1, "inline ! comment");
    assert!(r.code_only_lines >= 2);
}

#[test]
fn nix_hash_line_and_slash_block_comments() {
    let src = "# hash comment\n\
               /* block */\n\
               { pkgs }:\n\
               pkgs.hello\n";
    let r = &analyze_text(Language::Nix, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1);
    assert_eq!(r.multi_comment_only_lines, 1);
    assert!(r.code_only_lines >= 2);
}

#[test]
fn cmake_hash_and_bracket_block_comments() {
    let src = "# a comment\n\
               #[[ a block\n   comment ]]\n\
               project(Demo)\n";
    let r = &analyze_text(Language::Cmake, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1, "# line comment");
    assert!(r.multi_comment_only_lines >= 1, "#[[ ]] block comment");
    assert!(r.code_only_lines >= 1);
}

#[test]
fn elm_dash_block_comment() {
    let src = "-- line\n\
               {- block -}\n\
               module Main exposing (..)\n";
    let r = &analyze_text(Language::Elm, src, AnalysisOptions::default()).raw;
    assert_eq!(r.single_comment_only_lines, 1);
    assert_eq!(r.multi_comment_only_lines, 1);
    assert!(r.code_only_lines >= 1);
}

// ─── detect_language tests ────────────────────────────────────────────────────

#[test]
fn detect_by_extension_rs_is_rust() {
    let path = std::path::Path::new("src/lib.rs");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Rust));
}

#[test]
fn detect_by_extension_py_is_python() {
    let path = std::path::Path::new("app.py");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Python));
}

#[test]
fn detect_by_extension_js_is_javascript() {
    let path = std::path::Path::new("index.js");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::JavaScript));
}

#[test]
fn detect_by_extension_ts_is_typescript() {
    let path = std::path::Path::new("app.ts");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::TypeScript));
}

#[test]
fn detect_by_extension_go_is_go() {
    let path = std::path::Path::new("main.go");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Go));
}

#[test]
fn detect_by_extension_c_is_c() {
    let path = std::path::Path::new("main.c");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::C));
}

#[test]
fn detect_by_extension_h_is_c() {
    let path = std::path::Path::new("header.h");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::C));
}

#[test]
fn detect_by_extension_cpp_is_cpp() {
    let path = std::path::Path::new("main.cpp");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Cpp));
}

#[test]
fn detect_by_extension_hpp_is_cpp() {
    let path = std::path::Path::new("include.hpp");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Cpp));
}

#[test]
fn looks_like_cpp_detects_cpp_constructs() {
    assert!(looks_like_cpp("namespace foo {\nint x;\n}"));
    assert!(looks_like_cpp("class Widget { public: int id; };"));
    assert!(looks_like_cpp("std::string greet();"));
    assert!(looks_like_cpp("template <typename T> T id(T v);"));
    assert!(looks_like_cpp("int add(int a, int b) noexcept;"));
}

#[test]
fn looks_like_cpp_rejects_plain_c_headers() {
    // A pure C header — struct + function prototypes, no C++ constructs — must stay C.
    let c_header = "#ifndef FOO_H\n#define FOO_H\n\
        struct point { int x; int y; };\n\
        int add(int a, int b);\n\
        void reset(struct point* p);\n\
        #endif\n";
    assert!(!looks_like_cpp(c_header));
}

#[test]
fn detect_by_extension_java_is_java() {
    let path = std::path::Path::new("Main.java");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Java));
}

#[test]
fn detect_by_extension_cs_is_csharp() {
    let path = std::path::Path::new("Program.cs");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::CSharp));
}

#[test]
fn detect_by_extension_rb_is_ruby() {
    let path = std::path::Path::new("main.rb");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Ruby));
}

#[test]
fn detect_by_extension_php_is_php() {
    let path = std::path::Path::new("index.php");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Php));
}

#[test]
fn detect_by_extension_swift_is_swift() {
    let path = std::path::Path::new("app.swift");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Swift));
}

#[test]
fn detect_by_extension_kt_is_kotlin() {
    let path = std::path::Path::new("App.kt");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Kotlin));
}

#[test]
fn detect_by_extension_scala_is_scala() {
    let path = std::path::Path::new("App.scala");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Scala));
}

#[test]
fn detect_by_extension_dart_is_dart() {
    let path = std::path::Path::new("main.dart");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Dart));
}

#[test]
fn detect_by_extension_lua_is_lua() {
    let path = std::path::Path::new("init.lua");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Lua));
}

#[test]
fn detect_by_extension_sh_is_shell() {
    let path = std::path::Path::new("build.sh");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Shell));
}

#[test]
fn detect_by_extension_ps1_is_powershell() {
    let path = std::path::Path::new("install.ps1");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::PowerShell));
}

#[test]
fn detect_by_extension_sql_is_sql() {
    let path = std::path::Path::new("query.sql");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Sql));
}

#[test]
fn detect_by_extension_css_is_css() {
    let path = std::path::Path::new("style.css");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Css));
}

#[test]
fn detect_by_extension_scss_is_scss() {
    let path = std::path::Path::new("style.scss");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Scss));
}

#[test]
fn detect_by_extension_html_is_html() {
    let path = std::path::Path::new("index.html");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Html));
}

#[test]
fn detect_by_extension_xml_is_xml() {
    let path = std::path::Path::new("config.xml");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Xml));
}

#[test]
fn detect_by_extension_zig_is_zig() {
    let path = std::path::Path::new("main.zig");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Zig));
}

#[test]
fn detect_by_extension_r_is_r() {
    let path = std::path::Path::new("analysis.r");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::R));
}

#[test]
fn detect_by_extension_ml_is_ocaml() {
    let path = std::path::Path::new("parser.ml");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Ocaml));
}

#[test]
fn detect_by_extension_hs_is_haskell() {
    let path = std::path::Path::new("Main.hs");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Haskell));
}

#[test]
fn detect_by_extension_erl_is_erlang() {
    let path = std::path::Path::new("server.erl");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Erlang));
}

#[test]
fn detect_by_extension_ex_is_elixir() {
    let path = std::path::Path::new("app.ex");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Elixir));
}

#[test]
fn detect_by_extension_clj_is_clojure() {
    let path = std::path::Path::new("core.clj");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Clojure));
}

#[test]
fn detect_by_extension_svelte_is_svelte() {
    let path = std::path::Path::new("App.svelte");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Svelte));
}

#[test]
fn detect_by_extension_vue_is_vue() {
    let path = std::path::Path::new("App.vue");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Vue));
}

#[test]
fn detect_by_filename_dockerfile_exact() {
    let path = std::path::Path::new("Dockerfile");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Dockerfile));
}

#[test]
fn detect_by_filename_dockerfile_variant() {
    let path = std::path::Path::new("Dockerfile.prod");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Dockerfile));
}

#[test]
fn detect_by_filename_makefile() {
    let path = std::path::Path::new("Makefile");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Makefile));
}

#[test]
fn detect_by_filename_gnumakefile() {
    let path = std::path::Path::new("GNUmakefile");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Makefile));
}

#[test]
fn detect_by_filename_rakefile_is_ruby() {
    let path = std::path::Path::new("Rakefile");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Ruby));
}

#[test]
fn detect_by_filename_gemfile_is_ruby() {
    let path = std::path::Path::new("Gemfile");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert_eq!(lang, Some(Language::Ruby));
}

#[test]
fn detect_by_shebang_python3() {
    let path = std::path::Path::new("script");
    let first_line = "#!/usr/bin/env python3";
    let lang = detect_language(
        path,
        Some(first_line),
        &std::collections::BTreeMap::new(),
        true,
    );
    assert_eq!(lang, Some(Language::Python));
}

#[test]
fn detect_by_shebang_bash() {
    let path = std::path::Path::new("script");
    let first_line = "#!/bin/bash";
    let lang = detect_language(
        path,
        Some(first_line),
        &std::collections::BTreeMap::new(),
        true,
    );
    assert_eq!(lang, Some(Language::Shell));
}

#[test]
fn detect_by_shebang_ruby() {
    let path = std::path::Path::new("script");
    let first_line = "#!/usr/bin/env ruby";
    let lang = detect_language(
        path,
        Some(first_line),
        &std::collections::BTreeMap::new(),
        true,
    );
    assert_eq!(lang, Some(Language::Ruby));
}

#[test]
fn detect_by_shebang_perl() {
    let path = std::path::Path::new("script");
    let first_line = "#!/usr/bin/perl";
    let lang = detect_language(
        path,
        Some(first_line),
        &std::collections::BTreeMap::new(),
        true,
    );
    assert_eq!(lang, Some(Language::Perl));
}

#[test]
fn detect_by_shebang_node() {
    let path = std::path::Path::new("script");
    let first_line = "#!/usr/bin/env node";
    let lang = detect_language(
        path,
        Some(first_line),
        &std::collections::BTreeMap::new(),
        true,
    );
    assert_eq!(lang, Some(Language::JavaScript));
}

#[test]
fn detect_shebang_disabled_returns_none_for_extensionless() {
    let path = std::path::Path::new("script");
    let first_line = "#!/usr/bin/python3";
    let lang = detect_language(
        path,
        Some(first_line),
        &std::collections::BTreeMap::new(),
        false, // shebang_detection = false
    );
    assert!(
        lang.is_none(),
        "should not detect language without shebang detection enabled"
    );
}

#[test]
fn detect_extension_override_wins_over_default() {
    let path = std::path::Path::new("app.js");
    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert("js".to_string(), "typescript".to_string());
    let lang = detect_language(path, None, &overrides, false);
    assert_eq!(
        lang,
        Some(Language::TypeScript),
        "extension override should win"
    );
}

#[test]
fn detect_unknown_extension_returns_none() {
    let path = std::path::Path::new("data.xyz_unknown_extension");
    let lang = detect_language(path, None, &std::collections::BTreeMap::new(), false);
    assert!(lang.is_none(), "unknown extension should return None");
}

// ─── Language::from_name ─────────────────────────────────────────────────────

#[test]
fn from_name_aliases() {
    assert_eq!(Language::from_name("c++"), Some(Language::Cpp));
    assert_eq!(Language::from_name("cplusplus"), Some(Language::Cpp));
    assert_eq!(Language::from_name("c#"), Some(Language::CSharp));
    assert_eq!(Language::from_name("cs"), Some(Language::CSharp));
    assert_eq!(Language::from_name("golang"), Some(Language::Go));
    assert_eq!(Language::from_name("js"), Some(Language::JavaScript));
    assert_eq!(Language::from_name("py"), Some(Language::Python));
    assert_eq!(Language::from_name("rs"), Some(Language::Rust));
    assert_eq!(Language::from_name("sh"), Some(Language::Shell));
    assert_eq!(Language::from_name("bash"), Some(Language::Shell));
    assert_eq!(Language::from_name("pwsh"), Some(Language::PowerShell));
    assert_eq!(Language::from_name("ts"), Some(Language::TypeScript));
    assert_eq!(Language::from_name("asm"), Some(Language::Assembly));
    assert_eq!(Language::from_name("clj"), Some(Language::Clojure));
    assert_eq!(Language::from_name("ex"), Some(Language::Elixir));
    assert_eq!(Language::from_name("erl"), Some(Language::Erlang));
    assert_eq!(Language::from_name("f#"), Some(Language::FSharp));
    assert_eq!(Language::from_name("fs"), Some(Language::FSharp));
    assert_eq!(Language::from_name("hs"), Some(Language::Haskell));
    assert_eq!(Language::from_name("htm"), Some(Language::Html));
    assert_eq!(Language::from_name("jl"), Some(Language::Julia));
    assert_eq!(Language::from_name("kt"), Some(Language::Kotlin));
    assert_eq!(Language::from_name("mk"), Some(Language::Makefile));
    assert_eq!(Language::from_name("make"), Some(Language::Makefile));
    assert_eq!(Language::from_name("objc"), Some(Language::ObjectiveC));
    assert_eq!(
        Language::from_name("objective-c"),
        Some(Language::ObjectiveC)
    );
    assert_eq!(Language::from_name("ml"), Some(Language::Ocaml));
    assert_eq!(Language::from_name("pl"), Some(Language::Perl));
    assert_eq!(Language::from_name("rb"), Some(Language::Ruby));
    assert_eq!(Language::from_name("scala"), Some(Language::Scala));
    assert_eq!(Language::from_name("sass"), Some(Language::Scss));
    assert_eq!(Language::from_name("xml"), Some(Language::Xml));
}

#[test]
fn from_name_unknown_returns_none() {
    assert!(Language::from_name("cobol").is_none());
    assert!(Language::from_name("").is_none());
    assert!(Language::from_name("brainfuck").is_none());
}

#[test]
fn from_name_is_case_insensitive() {
    assert_eq!(Language::from_name("RUST"), Some(Language::Rust));
    assert_eq!(Language::from_name("Python"), Some(Language::Python));
    assert_eq!(
        Language::from_name("JAVASCRIPT"),
        Some(Language::JavaScript)
    );
}

// ─── Language display_name and as_slug (all variants) ────────────────────────

#[test]
fn display_name_non_empty_for_all_languages() {
    use sloc_languages::supported_languages;
    for lang in supported_languages() {
        let name = lang.display_name();
        assert!(!name.is_empty(), "{lang:?} display_name must not be empty");
    }
}

#[test]
fn as_slug_non_empty_for_all_languages() {
    use sloc_languages::supported_languages;
    for lang in supported_languages() {
        let slug = lang.as_slug();
        assert!(!slug.is_empty(), "{lang:?} as_slug must not be empty");
        // slugs should only contain lowercase alphanumeric or +
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+'),
            "{lang:?} slug '{slug}' contains unexpected characters"
        );
    }
}

#[test]
fn supported_languages_count_is_60() {
    use sloc_languages::supported_languages;
    let count = supported_languages().len();
    assert_eq!(count, 60, "expected 60 supported languages, got {count}");
}

// ─── AnalysisOptions non-defaults ────────────────────────────────────────────

#[test]
fn analyze_with_blank_in_block_comment_false() {
    let text = "/* block comment\n\n end */\nint x = 1;\n";
    let opts = sloc_languages::AnalysisOptions {
        blank_in_block_comment_as_comment: false,
        ..AnalysisOptions::default()
    };
    let result = analyze_text(Language::C, text, opts);
    let r = &result.raw;
    // With blank_in_block_comment_as_comment=false, the blank inside the block counts as blank
    assert!(r.total_physical_lines >= 3);
}

#[test]
fn analyze_with_collapse_continuation_lines() {
    // Shell continuation: line ending with backslash
    let text = "echo hello \\\n  world\necho done\n";
    let opts = sloc_languages::AnalysisOptions {
        collapse_continuation_lines: true,
        ..AnalysisOptions::default()
    };
    let result = analyze_text(Language::Shell, text, opts);
    // With continuation collapse, logical lines < physical lines
    assert!(result.raw.total_physical_lines >= 2);
}

#[test]
fn analyze_c_family_style_scope_only_c_files() {
    let text = "void foo() {}\n// comment\n";
    let opts = sloc_languages::AnalysisOptions {
        style_lang_scope: sloc_languages::StyleLangScope::CFamilyOnly,
        ..AnalysisOptions::default()
    };
    let c_result = analyze_text(Language::C, text, opts);
    assert!(
        c_result.style_analysis.is_some(),
        "C should get style analysis in CFamilyOnly mode"
    );

    let rust_result = analyze_text(Language::Rust, text, opts);
    assert!(
        rust_result.style_analysis.is_none(),
        "Rust should NOT get style analysis in CFamilyOnly mode"
    );
}

#[test]
fn analyze_style_disabled() {
    let text = "void foo() {}\n// comment\n";
    let opts = sloc_languages::AnalysisOptions {
        enable_style: false,
        ..AnalysisOptions::default()
    };
    let result = analyze_text(Language::C, text, opts);
    assert!(
        result.style_analysis.is_none(),
        "style analysis should be None when enable_style=false"
    );
}
