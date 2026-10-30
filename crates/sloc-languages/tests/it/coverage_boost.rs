// SPDX-License-Identifier: AGPL-3.0-or-later
// Coverage-boost tests for sloc-languages.
//
// Exercises every Language variant through analyze_text (covering each
// language_scan_config arm), the detect_language extension/shebang/filename
// paths, and the C/C++ style classifier's brace / pointer / paren branches.

use std::collections::BTreeMap;
use std::path::Path;

use sloc_languages::{AnalysisOptions, Language, analyze_text, detect_language, style};

/// Every Language variant with a tiny representative snippet. Calling
/// `analyze_text` on each forces `language_scan_config` to resolve that arm.
fn all_languages() -> Vec<(Language, &'static str)> {
    vec![
        (Language::C, "// c\nint x = 1;\n"),
        (Language::Cpp, "// cpp\nint* p = nullptr;\n"),
        (Language::CSharp, "// cs\nvar x = 1;\n"),
        (Language::Go, "// go\nx := 1\n"),
        (Language::Java, "// java\nint x = 1;\n"),
        (Language::JavaScript, "// js\nconst x = 1;\n"),
        (Language::Python, "# py\nx = 1\n"),
        (Language::Rust, "// rs\nlet x = 1;\n"),
        (Language::Shell, "# sh\nx=1\n"),
        (Language::PowerShell, "# ps\n$x = 1\n"),
        (Language::TypeScript, "// ts\nconst x: number = 1;\n"),
        (Language::Assembly, "; asm\nmov eax, 1\n"),
        (Language::Clojure, ";; clj\n(def x 1)\n"),
        (Language::Css, "/* css */\n.a { color: red; }\n"),
        (Language::Dart, "// dart\nvar x = 1;\n"),
        (Language::Dockerfile, "# docker\nFROM scratch\n"),
        (Language::Elixir, "# ex\nx = 1\n"),
        (Language::Erlang, "% erl\nX = 1.\n"),
        (Language::FSharp, "// fs\nlet x = 1\n"),
        (Language::Groovy, "// groovy\ndef x = 1\n"),
        (Language::Haskell, "-- hs\nx = 1\n"),
        (Language::Html, "<!-- html -->\n<p>hi</p>\n"),
        (Language::Julia, "# jl\nx = 1\n"),
        (Language::Kotlin, "// kt\nval x = 1\n"),
        (Language::Lua, "-- lua\nlocal x = 1\n"),
        (Language::Makefile, "# make\nall:\n\techo hi\n"),
        (Language::Nim, "# nim\nvar x = 1\n"),
        (Language::ObjectiveC, "// objc\nint x = 1;\n"),
        (Language::Ocaml, "(* ml *)\nlet x = 1\n"),
        (Language::Perl, "# pl\nmy $x = 1;\n"),
        (Language::Php, "// php\n$x = 1;\n"),
        (Language::Ruby, "# rb\nx = 1\n"),
        (Language::Scala, "// scala\nval x = 1\n"),
        (Language::Scss, "// scss\n$x: 1;\n"),
        (Language::Sql, "-- sql\nSELECT 1;\n"),
        (Language::Svelte, "<!-- svelte -->\n<p>hi</p>\n"),
        (Language::Swift, "// swift\nlet x = 1\n"),
        (Language::Vue, "<!-- vue -->\n<template></template>\n"),
        (Language::Xml, "<!-- xml -->\n<root/>\n"),
        (Language::Zig, "// zig\nconst x = 1;\n"),
        (Language::Solidity, "// sol\nuint x = 1;\n"),
        (Language::Protobuf, "// proto\nmessage M {}\n"),
        (Language::Hcl, "# hcl\nx = 1\n"),
        (Language::GraphQl, "# gql\ntype Q { a: Int }\n"),
        (Language::Ada, "-- ada\nX : Integer := 1;\n"),
        (Language::Vhdl, "-- vhdl\nsignal x : bit;\n"),
        (Language::Verilog, "// v\nwire x;\n"),
        (Language::Tcl, "# tcl\nset x 1\n"),
        (Language::Pascal, "{ pas }\nx := 1;\n"),
        (Language::VisualBasic, "' vb\nDim x = 1\n"),
        (Language::Lisp, ";; lisp\n(defvar x 1)\n"),
    ]
}

#[test]
fn analyze_text_covers_every_language_arm() {
    for (lang, text) in all_languages() {
        // Default options.
        let r = analyze_text(lang, text, AnalysisOptions::default());
        assert!(
            r.raw.total_physical_lines >= 1,
            "{lang:?}: expected at least one physical line"
        );
        // Re-run with the IEEE-1045 flags toggled to exercise alternate branches.
        let opts = AnalysisOptions {
            collapse_continuation_lines: true,
            blank_in_block_comment_as_comment: true,
            ..AnalysisOptions::default()
        };
        let _ = analyze_text(lang, text, opts);
    }
}

#[test]
fn analyze_text_handles_block_comments_and_strings() {
    // C/C++ block comments spanning lines + string with comment-like content.
    let c = "/* a\n   b */\nconst char* s = \"/* not a comment */\";\nint x = 1; // trailing\n";
    let r = analyze_text(Language::Cpp, c, AnalysisOptions::default());
    assert!(r.raw.multi_comment_only_lines >= 1);

    // Python triple-quoted docstring classification.
    let py = "def f():\n    \"\"\"doc\n    line2\n    \"\"\"\n    return 1\n";
    let r = analyze_text(Language::Python, py, AnalysisOptions::default());
    assert!(r.raw.total_physical_lines >= 5);
}

#[test]
fn detect_language_by_extension_and_overrides() {
    let no_overrides = BTreeMap::new();
    let cases = [
        ("a.rs", Language::Rust),
        ("a.py", Language::Python),
        ("a.cpp", Language::Cpp),
        ("a.ts", Language::TypeScript),
        ("a.go", Language::Go),
        ("a.sol", Language::Solidity),
        ("a.vhd", Language::Vhdl),
        ("a.v", Language::Verilog),
        ("a.tcl", Language::Tcl),
        ("a.pas", Language::Pascal),
        ("a.lisp", Language::Lisp),
    ];
    for (name, _expected) in cases {
        let got = detect_language(Path::new(name), None, &no_overrides, true);
        assert!(got.is_some(), "{name}: expected a detected language");
    }

    // Extension override wins over built-in detection.
    let mut overrides = BTreeMap::new();
    overrides.insert("rs".to_string(), "Python".to_string());
    let got = detect_language(Path::new("x.rs"), None, &overrides, true);
    assert_eq!(got, Some(Language::Python), "override should win");

    // Unknown extension → None.
    assert_eq!(
        detect_language(Path::new("x.unknownext"), None, &no_overrides, true),
        None
    );
}

#[test]
fn detect_language_by_shebang() {
    let no = BTreeMap::new();
    let cases = [
        ("#!/usr/bin/env python3", Language::Python),
        ("#!/bin/bash", Language::Shell),
        ("#!/usr/bin/pwsh", Language::PowerShell),
        ("#!/usr/bin/env ruby", Language::Ruby),
        ("#!/usr/bin/perl", Language::Perl),
        ("#!/usr/bin/php", Language::Php),
        ("#!/usr/bin/env node", Language::JavaScript),
    ];
    for (line, expected) in cases {
        let got = detect_language(Path::new("script"), Some(line), &no, true);
        assert_eq!(got, Some(expected), "shebang {line:?}");
    }
    // Shebang detection disabled → None for an extensionless script.
    assert_eq!(
        detect_language(Path::new("script"), Some("#!/bin/bash"), &no, false),
        None
    );
}

#[test]
fn detect_language_by_filename() {
    let no = BTreeMap::new();
    for name in ["Dockerfile", "Makefile", "makefile"] {
        assert!(
            detect_language(Path::new(name), None, &no, true).is_some(),
            "{name} should be detected by filename"
        );
    }
}

// ── C/C++ style classifier branches ──────────────────────────────────────────

#[test]
fn cpp_style_with_type_pointers_allman_braces_space_parens() {
    // Allman braces (brace alone on a line), `Type* name` pointers, spaced parens,
    // and a variety of attach-brace block heads to exercise is_block_head.
    let src = "#pragma once\n\
class Foo\n{\n\
    int* a;\n\
    char* b;\n\
    void method() const\n    {\n        if (a)\n        {\n            return;\n        }\n    }\n\
};\n\
namespace ns {\n}\n\
struct S {\n};\n\
enum E {\n};\n\
extern \"C\" {\n}\n\
void g() noexcept {\n}\n\
void h() override {\n}\n\
void run()\n{\n    try {\n    } catch (int e) {\n    }\n    do {\n    } while (a);\n}\n";
    let st = style::analyze_style(Language::Cpp, src).expect("cpp style should be Some");
    assert!(!st.signals.is_empty(), "expected style signals");
}

#[test]
fn cpp_style_with_name_pointers_attach_braces_nospace_parens() {
    // `Type *name` pointers, attach braces, and parens with no space.
    let src = "void f() {\n\
    int *a;\n    char *b;\n    long *c;\n    short *d;\n\
    if(a){\n    }\n    while(b){\n    }\n    for(;;){\n    }\n\
}\n";
    let st = style::analyze_style(Language::Cpp, src).expect("cpp style should be Some");
    assert!(!st.signals.is_empty());
}

#[test]
fn cpp_style_mixed_pointers() {
    // Roughly even split of `Type* name` and `Type *name` → Mixed classification.
    let src = "void f() {\n    int* a;\n    int *b;\n    char* c;\n    char *d;\n}\n";
    let _ = style::analyze_style(Language::Cpp, src);
}

#[test]
fn other_language_style_analyzers_run() {
    // Each of these languages has a dedicated style analyzer module.
    let samples = [
        (Language::Rust, "fn main() {\n    let x = 1;\n}\n"),
        (Language::Java, "class A {\n    int x = 1;\n}\n"),
        (Language::Python, "def f():\n    return 1\n"),
        (Language::Go, "func main() {\n}\n"),
        (Language::CSharp, "class A {\n    int X = 1;\n}\n"),
        (Language::JavaScript, "function f() {\n    return 1;\n}\n"),
        (Language::Ruby, "def f\n  1\nend\n"),
    ];
    for (lang, src) in samples {
        // May be Some or None depending on heuristics; just must not panic.
        let _ = style::analyze_style(lang, src);
    }
}

// ── Pass 3 language variants (not in all_languages above) ────────────────────

#[test]
fn analyze_text_pass3_new_languages() {
    // Fortran, Nix, Crystal, D, GLSL, CMake, Elm, Awk — added in pass 3
    let cases = [
        (
            Language::Fortran,
            "subroutine foo()\n  x = 1\nend subroutine\n",
        ),
        (
            Language::Nix,
            "{ pkgs ? import <nixpkgs> {} }:\npkgs.mkShell { buildInputs = [ pkgs.git ]; }\n",
        ),
        (
            Language::Crystal,
            "# crystal\ndef hello\n  puts \"hi\"\nend\n",
        ),
        (
            Language::D,
            "// d\nimport std.stdio;\nvoid main() { writeln(\"hi\"); }\n",
        ),
        (
            Language::Glsl,
            "// glsl\nvoid main() { gl_Position = vec4(0.0); }\n",
        ),
        (
            Language::Cmake,
            "# cmake\ncmake_minimum_required(VERSION 3.0)\nfunction(my_func)\nendfunction()\n",
        ),
        (
            Language::Elm,
            "-- elm\nmodule Main exposing (..)\nimport Html exposing (text)\n",
        ),
        (
            Language::Awk,
            "# awk\nBEGIN { print \"start\" }\n{ print $0 }\nEND { print \"end\" }\n",
        ),
    ];
    for (lang, text) in cases {
        let r = analyze_text(lang, text, AnalysisOptions::default());
        assert!(
            r.raw.total_physical_lines >= 1,
            "{lang:?}: expected at least one physical line"
        );
        // Also test with IEEE flags toggled
        let opts = AnalysisOptions {
            collapse_continuation_lines: true,
            blank_in_block_comment_as_comment: true,
            ..AnalysisOptions::default()
        };
        let _ = analyze_text(lang, text, opts);
    }
}

#[test]
fn analyze_text_r_language() {
    let r_code = "# R\nx <- 1\ny <- x + 2\ncat(y)\n";
    let result = analyze_text(Language::R, r_code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 3);
}

// ── Language::from_name coverage for Pass 3 languages ────────────────────────

#[test]
fn from_name_pass3_languages() {
    let cases = [
        ("fortran", Language::Fortran),
        ("f90", Language::Fortran),
        ("nix", Language::Nix),
        ("crystal", Language::Crystal),
        ("cr", Language::Crystal),
        ("d", Language::D),
        ("dlang", Language::D),
        ("glsl", Language::Glsl),
        ("hlsl", Language::Glsl),
        ("cmake", Language::Cmake),
        ("elm", Language::Elm),
        ("awk", Language::Awk),
        ("r", Language::R),
    ];
    for (name, expected) in cases {
        let got = Language::from_name(name);
        assert_eq!(
            got,
            Some(expected),
            "from_name({name:?}) should be {expected:?}"
        );
    }
}

#[test]
fn from_name_returns_none_for_unknown() {
    assert_eq!(Language::from_name("thislanguagedoesnotexist"), None);
}

// ── Language display_name and as_slug for Pass 3 ─────────────────────────────

#[test]
fn display_name_and_slug_pass3() {
    let cases = [
        (Language::Fortran, "Fortran", "fortran"),
        (Language::Nix, "Nix", "nix"),
        (Language::Crystal, "Crystal", "crystal"),
        (Language::D, "D", "d"),
        (Language::Glsl, "GLSL/HLSL", "glsl"),
        (Language::Cmake, "CMake", "cmake"),
        (Language::Elm, "Elm", "elm"),
        (Language::Awk, "Awk", "awk"),
        (Language::R, "R", "r"),
    ];
    for (lang, expected_name, expected_slug) in cases {
        assert_eq!(lang.display_name(), expected_name, "{lang:?}");
        assert_eq!(lang.as_slug(), expected_slug, "{lang:?}");
    }
}

// ── Fortran block comment (continuation lines) ────────────────────────────────

#[test]
fn analyze_text_fortran_with_comments_and_blank_lines() {
    let code = "! This is a Fortran comment\nprogram test\n  integer :: x\n  x = 42\nend program\n";
    let result = analyze_text(Language::Fortran, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 4);
    assert!(result.raw.code_only_lines >= 1);
    assert!(result.raw.single_comment_only_lines >= 1);
}

// ── Nix with block comments ───────────────────────────────────────────────────

#[test]
fn analyze_text_nix_with_block_comments() {
    let code = "/* This is a block comment\n   spanning multiple lines */\n{ x = 1; }\n";
    let result = analyze_text(Language::Nix, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 2);
}

// ── Crystal with class and method definitions ─────────────────────────────────

#[test]
fn analyze_text_crystal_class_and_methods() {
    let code = "class Greeter\n  def initialize(@name : String)\n  end\n  def greet\n    puts \"Hello, #{@name}!\"\n  end\nend\n";
    let result = analyze_text(Language::Crystal, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 5);
    assert!(result.raw.classes >= 1, "class keyword should be detected");
    assert!(result.raw.functions >= 1, "def keyword should be detected");
}

// ── CMake with functions and macros ──────────────────────────────────────────

#[test]
fn analyze_text_cmake_with_functions_and_macros() {
    let code = "cmake_minimum_required(VERSION 3.10)\nproject(MyProject)\n\nfunction(my_function arg1)\n  message(STATUS \"${arg1}\")\nendfunction()\n\nmacro(my_macro)\n  add_executable(app main.cpp)\nendmacro()\n\ninclude(GNUInstallDirs)\nadd_subdirectory(src)\n";
    let result = analyze_text(Language::Cmake, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 5);
    assert!(result.raw.functions >= 1, "function() should be detected");
}

// ── Elm imports ───────────────────────────────────────────────────────────────

#[test]
fn analyze_text_elm_with_imports_and_types() {
    let code = "module Main exposing (main)\nimport Html exposing (text)\nimport Html.Attributes exposing (class)\ntype alias Model = { count : Int }\n";
    let result = analyze_text(Language::Elm, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 3);
    assert!(result.raw.imports >= 1, "import keyword should be detected");
    assert!(result.raw.classes >= 1, "type keyword should be detected");
}

// ── Awk with functions ────────────────────────────────────────────────────────

#[test]
fn analyze_text_awk_with_function() {
    let code = "#!/usr/bin/awk -f\n# Process input\nfunction double(x) {\n  return x * 2\n}\nBEGIN { print double(5) }\n";
    let result = analyze_text(Language::Awk, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 4);
    assert!(
        result.raw.functions >= 1,
        "function keyword should be detected"
    );
}

// ── D language tests ──────────────────────────────────────────────────────────

#[test]
fn analyze_text_d_with_classes_and_imports() {
    let code = "import std.stdio;\nimport std.algorithm;\n\nclass MyClass {\n  int x;\n  void doSomething() {\n    writeln(x);\n  }\n}\n\nstruct Point {\n  double x, y;\n}\n";
    let result = analyze_text(Language::D, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 5);
    assert!(result.raw.imports >= 1, "import should be detected");
    assert!(result.raw.classes >= 1, "class/struct should be detected");
}

// ── GLSL tests ────────────────────────────────────────────────────────────────

#[test]
fn analyze_text_glsl_with_uniforms_and_varyings() {
    let code = "// vertex shader\nuniform mat4 modelMatrix;\nuniform mat4 viewMatrix;\nvarying vec3 vNormal;\n\nvoid main() {\n    vNormal = normal;\n    gl_Position = viewMatrix * modelMatrix * vec4(position, 1.0);\n}\n";
    let result = analyze_text(Language::Glsl, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 5);
    assert!(result.raw.code_only_lines >= 1);
}

// ── analyze_text with style enabled for previously untested langs ────────────

#[test]
fn analyze_text_with_style_enabled_for_all_pass3() {
    let cases = [
        (
            Language::Fortran,
            "subroutine foo()\n    x = 1\nend subroutine\n",
        ),
        (Language::Crystal, "def hello\n    puts \"hi\"\nend\n"),
        (Language::D, "void main() {\n    writeln(1);\n}\n"),
        (
            Language::Glsl,
            "void main() {\n    gl_Position = vec4(0.0);\n}\n",
        ),
        (
            Language::Cmake,
            "function(foo)\n    message(STATUS \"hi\")\nendfunction()\n",
        ),
        (Language::Elm, "module Main exposing (..)\nimport Html\n"),
        (Language::Awk, "function f(x) {\n    return x\n}\n"),
        (Language::R, "f <- function(x) {\n    x + 1\n}\n"),
        (Language::Nix, "{ pkgs }:\n  pkgs.mkShell {}\n"),
    ];
    for (lang, text) in cases {
        let opts = AnalysisOptions {
            enable_style: true,
            ..AnalysisOptions::default()
        };
        let result = analyze_text(lang, text, opts);
        // Must not panic; style_analysis may be None for languages without a style module
        let _ = result.style_analysis;
    }
}

// ── Detect language: Pass 3 extensions ───────────────────────────────────────

#[test]
fn detect_language_pass3_extensions() {
    let no = BTreeMap::new();
    let cases = [
        ("shader.glsl", Language::Glsl),
        ("shader.hlsl", Language::Glsl),
        ("hello.cr", Language::Crystal),
        ("main.d", Language::D),
        ("script.awk", Language::Awk),
        ("script.r", Language::R),
        ("default.nix", Language::Nix),
        ("program.f90", Language::Fortran),
    ];
    for (name, expected) in cases {
        let got = detect_language(Path::new(name), None, &no, true);
        assert_eq!(got, Some(expected), "{name}: expected {expected:?}");
    }
}

// ── Edge cases in the state machine ──────────────────────────────────────────

#[test]
fn analyze_text_python_docstring_count_as_comment() {
    // Test docstrings counted as comments (default)
    let code = "def f():\n    \"\"\"This is a docstring.\"\"\"\n    return 1\n";
    let result = analyze_text(Language::Python, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 3);
    // The docstring line should be classified as docstring_comment
    assert!(
        result.raw.docstring_comment_lines >= 1,
        "docstring should be classified"
    );
}

#[test]
fn analyze_text_python_multiline_docstring() {
    let code = "def greet(name):\n    \"\"\"\n    Greet someone.\n    \n    Args:\n        name: the name\n    \"\"\"\n    return f\"Hello, {name}\"\n";
    let result = analyze_text(Language::Python, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 8);
    assert!(
        result.raw.docstring_comment_lines >= 2,
        "multi-line docstring lines should be classified"
    );
}

#[test]
fn analyze_text_shell_with_inline_comments() {
    let code = "#!/bin/bash\n# Comment\nx=1  # inline comment\nif [ $x -eq 1 ]; then\n    echo \"yes\"\nfi\n";
    let result = analyze_text(Language::Shell, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 5);
    assert!(
        result.raw.mixed_code_single_comment_lines >= 1,
        "inline comment line should be mixed"
    );
}

#[test]
fn analyze_text_rust_block_comment() {
    let code = "fn main() {\n    /* This is a\n       block comment */\n    let x = 1;\n}\n";
    let result = analyze_text(Language::Rust, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 5);
    assert!(result.raw.multi_comment_only_lines >= 1);
}

#[test]
fn analyze_text_c_mixed_code_and_comment() {
    let code = "int x = 1; /* value */\nint y = 2; // another\nint z = x + y;\n";
    let result = analyze_text(Language::C, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 3);
    // At least some mixed lines
    let mixed =
        result.raw.mixed_code_single_comment_lines + result.raw.mixed_code_multi_comment_lines;
    assert!(mixed >= 1, "should have at least one mixed line");
}

// ── string-literal lexer state machine: verbatim + triple-quote branches ──────

#[test]
fn analyze_text_csharp_verbatim_string_with_escaped_quotes() {
    // C# verbatim string (@"...") with a "" escape and trailing content forces the
    // VerbatimDouble state through its escape, continuation, and close branches.
    let code = concat!(
        "class P {\n",
        "    string path = @\"plain verbatim text\";\n",
        "    string quoted = @\"he said \"\"hi\"\" loudly\";\n",
        "    int x = 1;\n",
        "}\n",
    );
    let result = analyze_text(Language::CSharp, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 5);
    assert!(result.raw.code_only_lines >= 3);
}

#[test]
fn analyze_text_julia_triple_quote_string_spans_content() {
    // Julia """ ... """ docstring/string: opener + Triple continuation + close.
    let code = concat!(
        "\"\"\"\n",
        "A multi-line\n",
        "triple-quoted string body\n",
        "\"\"\"\n",
        "function f()\n",
        "    return 1\n",
        "end\n",
    );
    let result = analyze_text(Language::Julia, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 7);
}

#[test]
fn analyze_text_python_single_triple_quote_string() {
    // Python ''' ... ''' triple-quoted string opener branch.
    let code = concat!(
        "x = '''\n",
        "single-quote triple block\n",
        "'''\n",
        "y = 2\n",
    );
    let result = analyze_text(Language::Python, code, AnalysisOptions::default());
    assert!(result.raw.total_physical_lines >= 4);
}
