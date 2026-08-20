// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

pub mod style;
pub use style::{IndentStyle, StyleAnalysis, StyleGuideScore, StyleSignal};

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    C,
    Cpp,
    CSharp,
    Go,
    Java,
    JavaScript,
    Python,
    Rust,
    Shell,
    PowerShell,
    TypeScript,
    // --- Extended language support ---
    Assembly,
    Clojure,
    Css,
    Dart,
    Dockerfile,
    Elixir,
    Erlang,
    FSharp,
    Groovy,
    Haskell,
    Html,
    Julia,
    Kotlin,
    Lua,
    Makefile,
    Nim,
    ObjectiveC,
    Ocaml,
    Perl,
    Php,
    R,
    Ruby,
    Scala,
    Scss,
    Sql,
    Svelte,
    Swift,
    Vue,
    Xml,
    Zig,
    // --- Pass 1: modern declarative / smart-contract languages ---
    Solidity,
    Protobuf,
    Hcl,
    GraphQl,
    // --- Pass 2: legacy + embedded / hardware-description languages ---
    Ada,
    Vhdl,
    Verilog,
    Tcl,
    Pascal,
    VisualBasic,
    Lisp,
    // --- Pass 3: scientific / infra / systems / graphics ---
    Fortran,
    Nix,
    Crystal,
    D,
    Glsl,
    Cmake,
    Elm,
    Awk,
}

impl Language {
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::C => "C",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Go => "Go",
            Self::Java => "Java",
            Self::JavaScript => "JavaScript",
            Self::Python => "Python",
            Self::Rust => "Rust",
            Self::Shell => "Shell",
            Self::PowerShell => "PowerShell",
            Self::TypeScript => "TypeScript",
            Self::Assembly => "Assembly",
            Self::Clojure => "Clojure",
            Self::Css => "CSS",
            Self::Dart => "Dart",
            Self::Dockerfile => "Dockerfile",
            Self::Elixir => "Elixir",
            Self::Erlang => "Erlang",
            Self::FSharp => "F#",
            Self::Groovy => "Groovy",
            Self::Haskell => "Haskell",
            Self::Html => "HTML",
            Self::Julia => "Julia",
            Self::Kotlin => "Kotlin",
            Self::Lua => "Lua",
            Self::Makefile => "Makefile",
            Self::Nim => "Nim",
            Self::ObjectiveC => "Objective-C",
            Self::Ocaml => "OCaml",
            Self::Perl => "Perl",
            Self::Php => "PHP",
            Self::R => "R",
            Self::Ruby => "Ruby",
            Self::Scala => "Scala",
            Self::Scss => "SCSS",
            Self::Sql => "SQL",
            Self::Svelte => "Svelte",
            Self::Swift => "Swift",
            Self::Vue => "Vue",
            Self::Xml => "XML",
            Self::Zig => "Zig",
            Self::Solidity => "Solidity",
            Self::Protobuf => "Protocol Buffers",
            Self::Hcl => "HCL/Terraform",
            Self::GraphQl => "GraphQL",
            Self::Ada => "Ada",
            Self::Vhdl => "VHDL",
            Self::Verilog => "Verilog/SystemVerilog",
            Self::Tcl => "Tcl",
            Self::Pascal => "Pascal/Delphi",
            Self::VisualBasic => "Visual Basic",
            Self::Lisp => "Lisp/Scheme",
            Self::Fortran => "Fortran",
            Self::Nix => "Nix",
            Self::Crystal => "Crystal",
            Self::D => "D",
            Self::Glsl => "GLSL/HLSL",
            Self::Cmake => "CMake",
            Self::Elm => "Elm",
            Self::Awk => "Awk",
        }
    }

    #[must_use]
    pub const fn as_slug(&self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Go => "go",
            Self::Java => "java",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Shell => "shell",
            Self::PowerShell => "powershell",
            Self::TypeScript => "typescript",
            Self::Assembly => "assembly",
            Self::Clojure => "clojure",
            Self::Css => "css",
            Self::Dart => "dart",
            Self::Dockerfile => "dockerfile",
            Self::Elixir => "elixir",
            Self::Erlang => "erlang",
            Self::FSharp => "fsharp",
            Self::Groovy => "groovy",
            Self::Haskell => "haskell",
            Self::Html => "html",
            Self::Julia => "julia",
            Self::Kotlin => "kotlin",
            Self::Lua => "lua",
            Self::Makefile => "makefile",
            Self::Nim => "nim",
            Self::ObjectiveC => "objectivec",
            Self::Ocaml => "ocaml",
            Self::Perl => "perl",
            Self::Php => "php",
            Self::R => "r",
            Self::Ruby => "ruby",
            Self::Scala => "scala",
            Self::Scss => "scss",
            Self::Sql => "sql",
            Self::Svelte => "svelte",
            Self::Swift => "swift",
            Self::Vue => "vue",
            Self::Xml => "xml",
            Self::Zig => "zig",
            Self::Solidity => "solidity",
            Self::Protobuf => "protobuf",
            Self::Hcl => "hcl",
            Self::GraphQl => "graphql",
            Self::Ada => "ada",
            Self::Vhdl => "vhdl",
            Self::Verilog => "verilog",
            Self::Tcl => "tcl",
            Self::Pascal => "pascal",
            Self::VisualBasic => "visualbasic",
            Self::Lisp => "lisp",
            Self::Fortran => "fortran",
            Self::Nix => "nix",
            Self::Crystal => "crystal",
            Self::D => "d",
            Self::Glsl => "glsl",
            Self::Cmake => "cmake",
            Self::Elm => "elm",
            Self::Awk => "awk",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "c" => Some(Self::C),
            "cpp" | "c++" | "cplusplus" => Some(Self::Cpp),
            "csharp" | "c#" | "cs" => Some(Self::CSharp),
            "go" | "golang" => Some(Self::Go),
            "java" => Some(Self::Java),
            "javascript" | "js" => Some(Self::JavaScript),
            "python" | "py" => Some(Self::Python),
            "rust" | "rs" => Some(Self::Rust),
            "shell" | "sh" | "bash" => Some(Self::Shell),
            "powershell" | "pwsh" | "ps" => Some(Self::PowerShell),
            "typescript" | "ts" => Some(Self::TypeScript),
            "assembly" | "asm" => Some(Self::Assembly),
            "clojure" | "clj" => Some(Self::Clojure),
            "css" => Some(Self::Css),
            "dart" => Some(Self::Dart),
            "dockerfile" | "docker" => Some(Self::Dockerfile),
            "elixir" | "ex" => Some(Self::Elixir),
            "erlang" | "erl" => Some(Self::Erlang),
            "fsharp" | "f#" | "fs" => Some(Self::FSharp),
            "groovy" => Some(Self::Groovy),
            "haskell" | "hs" => Some(Self::Haskell),
            "html" | "htm" => Some(Self::Html),
            "julia" | "jl" => Some(Self::Julia),
            "kotlin" | "kt" => Some(Self::Kotlin),
            "lua" => Some(Self::Lua),
            "makefile" | "make" | "mk" => Some(Self::Makefile),
            "nim" => Some(Self::Nim),
            "objectivec" | "objc" | "objective-c" => Some(Self::ObjectiveC),
            "ocaml" | "ml" => Some(Self::Ocaml),
            "perl" | "pl" => Some(Self::Perl),
            "php" => Some(Self::Php),
            "r" => Some(Self::R),
            "ruby" | "rb" => Some(Self::Ruby),
            "scala" => Some(Self::Scala),
            "scss" | "sass" => Some(Self::Scss),
            "sql" => Some(Self::Sql),
            "svelte" => Some(Self::Svelte),
            "swift" => Some(Self::Swift),
            "vue" => Some(Self::Vue),
            "xml" => Some(Self::Xml),
            "zig" => Some(Self::Zig),
            "solidity" | "sol" => Some(Self::Solidity),
            "protobuf" | "proto" | "protocolbuffers" => Some(Self::Protobuf),
            "hcl" | "terraform" | "tf" => Some(Self::Hcl),
            "graphql" | "gql" => Some(Self::GraphQl),
            "ada" => Some(Self::Ada),
            "vhdl" => Some(Self::Vhdl),
            "verilog" | "systemverilog" | "sv" => Some(Self::Verilog),
            "tcl" => Some(Self::Tcl),
            "pascal" | "delphi" | "pas" => Some(Self::Pascal),
            "visualbasic" | "vb" | "vbnet" | "vb.net" => Some(Self::VisualBasic),
            "lisp" | "scheme" | "racket" | "clisp" | "elisp" => Some(Self::Lisp),
            "fortran" | "f90" | "f95" => Some(Self::Fortran),
            "nix" => Some(Self::Nix),
            "crystal" | "cr" => Some(Self::Crystal),
            "d" | "dlang" => Some(Self::D),
            "glsl" | "hlsl" | "shader" | "wgsl" => Some(Self::Glsl),
            "cmake" => Some(Self::Cmake),
            "elm" => Some(Self::Elm),
            "awk" => Some(Self::Awk),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawLineCounts {
    pub total_physical_lines: u64,
    pub blank_only_lines: u64,
    pub code_only_lines: u64,
    pub single_comment_only_lines: u64,
    pub multi_comment_only_lines: u64,
    pub mixed_code_single_comment_lines: u64,
    pub mixed_code_multi_comment_lines: u64,
    pub docstring_comment_lines: u64,
    pub skipped_unknown_lines: u64,
    /// Best-effort count of function/method definition lines detected lexically.
    #[serde(default)]
    pub functions: u64,
    /// Best-effort count of class/struct/trait/type definition lines detected lexically.
    #[serde(default)]
    pub classes: u64,
    /// Best-effort count of variable declaration lines detected lexically. Equals the sum of
    /// `variables_member + variables_local + variables_global` for C/C++ (where scope is tracked);
    /// for other languages it is the flat total with the breakdown fields left at zero.
    #[serde(default)]
    pub variables: u64,
    /// C/C++ only: variable declarations that are members of a class/struct/union body.
    #[serde(default)]
    pub variables_member: u64,
    /// C/C++ only: variable declarations local to a function or block body.
    #[serde(default)]
    pub variables_local: u64,
    /// C/C++ only: variable declarations at file / namespace scope (globals, file-statics).
    #[serde(default)]
    pub variables_global: u64,
    /// C/C++ only: object-like preprocessor macro definitions (`#define NAME value`) — named
    /// compile-time constants. Function-like macros (`#define F(x) …`) are excluded.
    #[serde(default)]
    pub macro_definitions: u64,
    /// Best-effort count of import/use/include statement lines detected lexically.
    #[serde(default)]
    pub imports: u64,
    /// Lines consisting solely of preprocessor/compiler directives (e.g. `#include`, `#define`
    /// in C/C++/Objective-C). Always a subset of `code_only_lines`. Controlled by
    /// `AnalysisConfig::count_compiler_directives`. IEEE 1045-1992 §4.2.
    #[serde(default)]
    pub compiler_directive_lines: u64,
    /// Best-effort count of test case / test function definition lines detected lexically
    /// (`GTest`, Catch2, `PyTest`, `JUnit`, etc.). Always a subset of `code_only_lines`.
    #[serde(default)]
    pub test_count: u64,
    /// Best-effort count of test assertion call lines detected lexically
    /// (`ASSERT_EQ`, `EXPECT_TRUE`, assertEquals, Assert.AreEqual, `assert_eq`!, etc.).
    #[serde(default)]
    pub test_assertion_count: u64,
    /// Best-effort count of test suite / fixture / group declaration lines detected lexically
    /// (`TEST_GROUP`, `BOOST_AUTO_TEST_SUITE`, [`TestClass`], [`TestFixture`], etc.).
    #[serde(default)]
    pub test_suite_count: u64,
    /// Cyclomatic complexity approximation: total count of branch decision keywords found on
    /// code lines (e.g. `if`, `for`, `while`, `||`, `&&`). Starts at 0; +1 per keyword hit.
    #[serde(default)]
    pub cyclomatic_complexity: u32,
    /// Logical SLOC estimate: executable statement count using a language-specific strategy.
    /// `None` when the language does not support lexical LSLOC estimation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsloc: Option<u32>,
    /// Per-code-line content hashes (trimmed) for ULOC aggregation. Never serialized — only
    /// populated during an in-process scan and consumed by `sloc-core` during aggregation.
    #[serde(skip)]
    pub code_line_hashes: Vec<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseMode {
    Lexical,
    LexicalBestEffort,
    TreeSitter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFileAnalysis {
    pub raw: RawLineCounts,
    pub parse_mode: ParseMode,
    pub warnings: Vec<String>,
    /// Lexical style-guide analysis for supported languages; `None` when no heuristics apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_analysis: Option<StyleAnalysis>,
}

/// IEEE 1045-1992 counting options passed from `sloc-core` (built from `AnalysisConfig`).
///
/// `analyze_text` accepts this struct so that the caller can control behaviour that the
/// standard defines as configurable parameters rather than fixed conventions.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisOptions {
    /// When `true` (IEEE 1045-1992 default), blank lines inside block comments count as
    /// comment lines rather than blank lines.
    pub blank_in_block_comment_as_comment: bool,
    /// When `true`, backslash-continued physical lines are collapsed into a single logical
    /// line for SLOC counting purposes (IEEE logical SLOC mode).
    pub collapse_continuation_lines: bool,
    /// When `true` (default), run lexical style-guide heuristics and populate
    /// `RawFileAnalysis::style_analysis`. Set to `false` to skip style scoring entirely.
    pub enable_style: bool,
    /// Restrict style analysis to a specific language family slug (`"all"` or `"c_family"`).
    /// When `"c_family"`, only C / C++ / Objective-C files are style-analysed.
    pub style_lang_scope: StyleLangScope,
}

/// Which language families receive style-guide heuristic analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleLangScope {
    All,
    CFamilyOnly,
}

/// Strategy for computing Logical SLOC (LSLOC) from a physical-line scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LslocStrategy {
    /// Count semicolons on code lines (C, C++, Java, C#, Go, Rust, JS/TS, Kotlin, SQL, …).
    Semicolons,
    /// Count non-blank code lines whose trimmed content does not end with a continuation
    /// character (`\`, `,`, `(`, `[`, `{`). Suitable for Python, Ruby, Shell, Elixir, Nim.
    NonContinuationNewlines,
    /// Language does not have a well-defined statement boundary detectable by simple
    /// lexical heuristics; `lsloc` will be `None` for files of this type.
    Unsupported,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            blank_in_block_comment_as_comment: true,
            collapse_continuation_lines: false,
            enable_style: true,
            style_lang_scope: StyleLangScope::All,
        }
    }
}

#[must_use]
pub fn supported_languages() -> BTreeSet<Language> {
    [
        Language::Assembly,
        Language::C,
        Language::Clojure,
        Language::Cpp,
        Language::CSharp,
        Language::Css,
        Language::Dart,
        Language::Dockerfile,
        Language::Elixir,
        Language::Erlang,
        Language::FSharp,
        Language::Go,
        Language::Groovy,
        Language::Haskell,
        Language::Html,
        Language::Java,
        Language::JavaScript,
        Language::Julia,
        Language::Kotlin,
        Language::Lua,
        Language::Makefile,
        Language::Nim,
        Language::ObjectiveC,
        Language::Ocaml,
        Language::Perl,
        Language::Php,
        Language::PowerShell,
        Language::Python,
        Language::R,
        Language::Ruby,
        Language::Rust,
        Language::Scala,
        Language::Scss,
        Language::Shell,
        Language::Sql,
        Language::Svelte,
        Language::Swift,
        Language::TypeScript,
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
    ]
    .into_iter()
    .collect()
}

/// Detect language from a shebang line (e.g. `#!/usr/bin/env python3`).
fn detect_by_shebang(line: &str) -> Option<Language> {
    let lower = line.to_ascii_lowercase();
    if !lower.starts_with("#!") {
        return None;
    }
    if lower.contains("python") {
        return Some(Language::Python);
    }
    if lower.contains("pwsh") || lower.contains("powershell") {
        return Some(Language::PowerShell);
    }
    if lower.contains("bash")
        || lower.contains("/sh")
        || lower.contains("zsh")
        || lower.contains("ksh")
    {
        return Some(Language::Shell);
    }
    if lower.contains("ruby") {
        return Some(Language::Ruby);
    }
    if lower.contains("perl") {
        return Some(Language::Perl);
    }
    if lower.contains("php") {
        return Some(Language::Php);
    }
    if lower.contains("node") || lower.contains("nodejs") {
        return Some(Language::JavaScript);
    }
    None
}

/// Detect language purely from a (lowercased) file extension.
#[allow(clippy::too_many_lines)]
fn detect_by_extension(ext: &str) -> Option<Language> {
    // Static table avoids a large match statement; each extension maps 1-to-1 to a language.
    static EXT_MAP: &[(&str, Language)] = &[
        ("c", Language::C),
        ("h", Language::C),
        ("cc", Language::Cpp),
        ("cp", Language::Cpp),
        ("cpp", Language::Cpp),
        ("cxx", Language::Cpp),
        ("hh", Language::Cpp),
        ("hpp", Language::Cpp),
        ("hxx", Language::Cpp),
        ("cs", Language::CSharp),
        ("go", Language::Go),
        ("java", Language::Java),
        ("js", Language::JavaScript),
        ("mjs", Language::JavaScript),
        ("cjs", Language::JavaScript),
        ("py", Language::Python),
        ("rs", Language::Rust),
        ("sh", Language::Shell),
        ("bash", Language::Shell),
        ("zsh", Language::Shell),
        ("ksh", Language::Shell),
        ("ps1", Language::PowerShell),
        ("psm1", Language::PowerShell),
        ("psd1", Language::PowerShell),
        ("ts", Language::TypeScript),
        ("mts", Language::TypeScript),
        ("cts", Language::TypeScript),
        ("tsx", Language::TypeScript),
        ("jsx", Language::JavaScript),
        ("asm", Language::Assembly),
        ("s", Language::Assembly),
        ("clj", Language::Clojure),
        ("cljs", Language::Clojure),
        ("cljc", Language::Clojure),
        ("edn", Language::Clojure),
        ("css", Language::Css),
        ("dart", Language::Dart),
        ("ex", Language::Elixir),
        ("exs", Language::Elixir),
        ("erl", Language::Erlang),
        ("hrl", Language::Erlang),
        ("fs", Language::FSharp),
        ("fsi", Language::FSharp),
        ("fsx", Language::FSharp),
        ("groovy", Language::Groovy),
        ("gradle", Language::Groovy),
        ("hs", Language::Haskell),
        ("lhs", Language::Haskell),
        ("html", Language::Html),
        ("htm", Language::Html),
        ("xhtml", Language::Html),
        ("jl", Language::Julia),
        ("kt", Language::Kotlin),
        ("kts", Language::Kotlin),
        ("lua", Language::Lua),
        ("mk", Language::Makefile),
        ("nim", Language::Nim),
        ("nims", Language::Nim),
        ("m", Language::ObjectiveC),
        ("mm", Language::ObjectiveC),
        ("ml", Language::Ocaml),
        ("mli", Language::Ocaml),
        ("pl", Language::Perl),
        ("pm", Language::Perl),
        ("t", Language::Perl),
        ("php", Language::Php),
        ("php3", Language::Php),
        ("php4", Language::Php),
        ("php5", Language::Php),
        ("php7", Language::Php),
        ("phtml", Language::Php),
        ("r", Language::R),
        ("rb", Language::Ruby),
        ("rake", Language::Ruby),
        ("scala", Language::Scala),
        ("sc", Language::Scala),
        ("scss", Language::Scss),
        ("sass", Language::Scss),
        ("sql", Language::Sql),
        ("svelte", Language::Svelte),
        ("swift", Language::Swift),
        ("vue", Language::Vue),
        ("xml", Language::Xml),
        ("xsd", Language::Xml),
        ("xsl", Language::Xml),
        ("xslt", Language::Xml),
        ("svg", Language::Xml),
        ("zig", Language::Zig),
        ("sol", Language::Solidity),
        ("proto", Language::Protobuf),
        ("tf", Language::Hcl),
        ("tfvars", Language::Hcl),
        ("hcl", Language::Hcl),
        ("graphql", Language::GraphQl),
        ("gql", Language::GraphQl),
        ("adb", Language::Ada),
        ("ads", Language::Ada),
        ("ada", Language::Ada),
        ("vhd", Language::Vhdl),
        ("vhdl", Language::Vhdl),
        ("v", Language::Verilog),
        ("sv", Language::Verilog),
        ("svh", Language::Verilog),
        ("vh", Language::Verilog),
        ("tcl", Language::Tcl),
        ("pas", Language::Pascal),
        ("dpr", Language::Pascal),
        ("vb", Language::VisualBasic),
        ("bas", Language::VisualBasic),
        ("lisp", Language::Lisp),
        ("lsp", Language::Lisp),
        ("el", Language::Lisp),
        ("scm", Language::Lisp),
        ("ss", Language::Lisp),
        ("rkt", Language::Lisp),
        ("f90", Language::Fortran),
        ("f95", Language::Fortran),
        ("f03", Language::Fortran),
        ("f08", Language::Fortran),
        ("f", Language::Fortran),
        ("for", Language::Fortran),
        ("nix", Language::Nix),
        ("cr", Language::Crystal),
        ("d", Language::D),
        ("glsl", Language::Glsl),
        ("vert", Language::Glsl),
        ("frag", Language::Glsl),
        ("comp", Language::Glsl),
        ("geom", Language::Glsl),
        ("tesc", Language::Glsl),
        ("tese", Language::Glsl),
        ("hlsl", Language::Glsl),
        ("wgsl", Language::Glsl),
        ("cmake", Language::Cmake),
        ("elm", Language::Elm),
        ("awk", Language::Awk),
    ];
    EXT_MAP.iter().find_map(|&(e, l)| (e == ext).then_some(l))
}

/// Detect language from an exact filename (no extension) or well-known filename patterns.
fn detect_by_filename(filename: &str, filename_lower: &str) -> Option<Language> {
    // Dockerfile: exact name or Dockerfile.* variant
    if filename == "Dockerfile"
        || filename.starts_with("Dockerfile.")
        || filename_lower == "dockerfile"
    {
        return Some(Language::Dockerfile);
    }
    // Makefile variants
    if matches!(
        filename,
        "Makefile" | "GNUmakefile" | "makefile" | "BSDmakefile"
    ) {
        return Some(Language::Makefile);
    }
    // Ruby ecosystem files that have no extension
    if matches!(
        filename,
        "Rakefile" | "Gemfile" | "Guardfile" | "Vagrantfile" | "Fastfile" | "Podfile"
    ) {
        return Some(Language::Ruby);
    }
    // CMake build scripts: `CMakeLists.txt` has a `.txt` extension, so it must be
    // matched by exact name before extension-based detection.
    if filename == "CMakeLists.txt" || filename_lower == "cmakelists.txt" {
        return Some(Language::Cmake);
    }
    None
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn detect_language(
    path: &Path,
    first_line: Option<&str>,
    extension_overrides: &BTreeMap<String, String>,
    shebang_detection: bool,
) -> Option<Language> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);

    // Extension override check (user-configured mappings win over everything)
    if let Some(ext) = extension.as_ref()
        && let Some(override_name) = extension_overrides.get(ext.as_str())
        && let Some(lang) = Language::from_name(override_name)
    {
        return Some(lang);
    }

    // Filename-based detection for files that have no extension or use exact names
    let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let filename_lower = filename.to_ascii_lowercase();

    if let Some(lang) = detect_by_filename(filename, &filename_lower) {
        return Some(lang);
    }

    // Extension-based detection
    if let Some(lang) = extension.as_deref().and_then(detect_by_extension) {
        return Some(lang);
    }

    // Shebang detection (last resort when filename/extension detection did not match)
    if shebang_detection
        && let Some(line) = first_line
        && let Some(lang) = detect_by_shebang(line)
    {
        return Some(lang);
    }

    None
}

/// Best-effort test: does this source text use C++-only constructs?
///
/// The `.h` extension is shared by C and C++ headers. When a `.h` file (detected as C by
/// extension) contains any unambiguously C++ construct, callers should reclassify it as C++ so
/// namespaces, classes, templates, and class-typed function signatures are counted correctly.
/// Markers chosen to not appear in valid C: `namespace`, `template`, `class`, access specifiers,
/// `::` scope resolution, and `std::`.
#[must_use]
pub fn looks_like_cpp(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        "namespace ",
        "template<",
        "template <",
        "class ",
        "public:",
        "private:",
        "protected:",
        "std::",
        "::",
        "nullptr",
        "constexpr ",
        "noexcept",
    ];
    MARKERS.iter().any(|m| text.contains(m))
}

/// tree-sitter fast-path for languages that have an adapter. Returns `Some` only
/// when an adapter exists AND no IEEE 1045-1992 counting policy is engaged — the
/// adapters don't implement those policies, so honoring them here would make counts
/// depend on whether the `tree-sitter` feature is compiled in. Takes
/// `&AnalysisOptions` to avoid assuming `AnalysisOptions: Copy`.
#[cfg(feature = "tree-sitter")]
fn tree_sitter_fast_path(
    language: Language,
    text: &str,
    options: AnalysisOptions,
) -> Option<RawFileAnalysis> {
    if options.blank_in_block_comment_as_comment || options.collapse_continuation_lines {
        return None;
    }
    match language {
        Language::C | Language::Cpp => {
            let mut result = ts::analyze_c(text)?;
            if options.enable_style && should_style_analyse(language, options.style_lang_scope) {
                result.style_analysis = style::analyze_style(language, text);
            }
            Some(result)
        }
        Language::Python => ts::analyze_python(text),
        _ => None,
    }
}

#[must_use]
pub fn analyze_text(language: Language, text: &str, options: AnalysisOptions) -> RawFileAnalysis {
    // tree-sitter fast-path (compiled out when the feature is disabled).
    #[cfg(feature = "tree-sitter")]
    if let Some(result) = tree_sitter_fast_path(language, text, options) {
        return result;
    }

    let (mut config, has_preprocessor) = language_scan_config(language);

    // Python docstring lines are computed from the text and cannot be a static constant.
    if language == Language::Python {
        config.skip_lines = detect_python_docstring_lines(text);
    }

    // C, C++, and Objective-C have a preprocessor whose directive lines are tracked separately
    // per IEEE 1045-1992 §4.2; every other language uses base flags.
    let flags = IeeeFlags {
        has_preprocessor_directives: has_preprocessor,
        blank_in_block_comment_as_comment: options.blank_in_block_comment_as_comment,
        collapse_continuation_lines: options.collapse_continuation_lines,
    };
    let mut result = analyze_generic(text, config, flags);
    if options.enable_style && should_style_analyse(language, options.style_lang_scope) {
        result.style_analysis = style::analyze_style(language, text);
    }
    result
}

/// Returns `true` when `language` should be style-analysed under `scope`.
const fn should_style_analyse(language: Language, scope: StyleLangScope) -> bool {
    match scope {
        StyleLangScope::CFamilyOnly => {
            matches!(language, Language::C | Language::Cpp | Language::ObjectiveC)
        }
        StyleLangScope::All => true,
    }
}

/// Returns the lexical scan configuration for `language` and whether it uses a C preprocessor.
/// All fields are static constants except `skip_lines`, which is always empty here; callers that
/// need non-empty skip sets (currently only Python) must populate the field after this call.
///
/// The implementation delegates to `LANG_SCAN_TABLE` (a static `&[(Language, StaticLangConfig)]`)
/// defined below the `SP_*` symbol-pattern constants.  Each language appears exactly once in the
/// table, so the linear scan is O(|languages|) but avoids a 41-arm `match` statement.
fn language_scan_config(language: Language) -> (ScanConfig, bool) {
    let cfg = LANG_SCAN_TABLE
        .iter()
        .find_map(|&(l, c)| (l == language).then_some(c))
        .unwrap_or_else(|| panic!("language_scan_config: no entry for {language:?}"));
    let (branch_keywords, lsloc_strategy) = language_complexity_config(language);
    (
        ScanConfig {
            line_comments: cfg.line_comments,
            block_comment: cfg.block_comment,
            allow_single_quote_strings: cfg.allow_single_quote_strings,
            allow_double_quote_strings: cfg.allow_double_quote_strings,
            allow_triple_quote_strings: cfg.allow_triple_quote_strings,
            allow_csharp_verbatim_strings: cfg.allow_csharp_verbatim_strings,
            allow_raw_strings: cfg.allow_raw_strings,
            skip_lines: HashSet::new(),
            symbol_patterns: cfg.symbol_patterns,
            branch_keywords,
            lsloc_strategy,
        },
        cfg.has_preprocessor,
    )
}

// ── Cyclomatic complexity branch-keyword lists ────────────────────────────────
// Alphabetic tokens are matched word-bounded; operator tokens (||, &&, ?) are
// matched as raw substrings.  Each list covers one language family.

const BRANCH_C_FAMILY: &[&str] = &[
    "if", "else", "for", "while", "switch", "case", "catch", "||", "&&",
];
const BRANCH_C_TERNARY: &[&str] = &[
    "if", "else", "for", "while", "switch", "case", "catch", "||", "&&", "?",
];
const BRANCH_GO: &[&str] = &["if", "else", "for", "switch", "case", "select", "||", "&&"];
const BRANCH_RUST: &[&str] = &["if", "else", "for", "while", "match", "||", "&&"];
const BRANCH_ZIG: &[&str] = &["if", "else", "for", "while", "switch", "catch", "||", "&&"];
const BRANCH_FSHARP: &[&str] = &["if", "then", "else", "elif", "match", "when", "||", "&&"];
const BRANCH_LUA: &[&str] = &[
    "if", "elseif", "else", "for", "while", "repeat", "and", "or",
];
const BRANCH_HASKELL: &[&str] = &["if", "then", "else", "case", "otherwise"];
const BRANCH_SQL: &[&str] = &["CASE", "WHEN", "IF", "ELSE", "case", "when", "if", "else"];
const BRANCH_OCAML: &[&str] = &["if", "then", "else", "match", "when", "||", "&&"];
const BRANCH_CLOJURE: &[&str] = &["if", "when", "cond", "case", "and", "or"];
const BRANCH_PHP: &[&str] = &[
    "if", "elseif", "else", "for", "while", "switch", "case", "catch", "match", "||", "&&", "?",
];
const BRANCH_JULIA: &[&str] = &["if", "elseif", "else", "for", "while", "catch", "||", "&&"];
const BRANCH_PYTHON: &[&str] = &["if", "elif", "else", "for", "while", "except", "or", "and"];
const BRANCH_RUBY: &[&str] = &[
    "if", "elsif", "else", "unless", "until", "while", "case", "when", "rescue", "||", "&&",
];
const BRANCH_SHELL: &[&str] = &["if", "elif", "else", "while", "until", "case", "||", "&&"];
const BRANCH_ELIXIR: &[&str] = &[
    "if", "else", "cond", "case", "when", "rescue", "||", "&&", "and", "or",
];
const BRANCH_POWERSHELL: &[&str] = &[
    "if", "elseif", "else", "for", "while", "switch", "foreach", "||", "&&",
];
const BRANCH_NIM: &[&str] = &[
    "if", "elif", "else", "for", "while", "case", "of", "except", "and", "or",
];
const BRANCH_PERL: &[&str] = &[
    "if", "elsif", "else", "unless", "until", "for", "while", "foreach", "||", "&&",
];
const BRANCH_R: &[&str] = &["if", "else", "for", "while", "repeat", "||", "&&"];
// Pass 2 branch-keyword lists (legacy + embedded / HDL).
const BRANCH_ADA: &[&str] = &[
    "if", "elsif", "else", "case", "when", "loop", "while", "for", "and", "or",
];
const BRANCH_VHDL: &[&str] = &[
    "if", "elsif", "else", "case", "when", "loop", "while", "for", "and", "or", "nand", "nor",
    "xor",
];
const BRANCH_VERILOG: &[&str] = &[
    "if", "else", "case", "casex", "casez", "for", "while", "&&", "||",
];
const BRANCH_TCL: &[&str] = &["if", "elseif", "else", "switch", "while", "for", "foreach"];
const BRANCH_PASCAL: &[&str] = &[
    "if", "then", "else", "case", "while", "for", "repeat", "until", "and", "or",
];
const BRANCH_VB: &[&str] = &[
    "If", "Then", "ElseIf", "Else", "Select", "Case", "While", "For", "Do", "And", "Or",
];
const BRANCH_LISP: &[&str] = &["if", "when", "unless", "cond", "case", "and", "or"];
// Pass 3 branch-keyword lists (scientific / infra / systems / graphics).
const BRANCH_FORTRAN: &[&str] = &[
    "if", "then", "else", "elseif", "case", "do", "while", "where",
];
const BRANCH_NIX: &[&str] = &["if", "then", "else"];
const BRANCH_CMAKE: &[&str] = &["if(", "elseif(", "else(", "while(", "foreach("];
const BRANCH_ELM: &[&str] = &["if", "then", "else", "case", "of"];
const BRANCH_AWK: &[&str] = &["if", "else", "while", "for", "do"];

/// Returns (`branch_keywords`, `lsloc_strategy`) for the given language.
/// Kept separate from `LANG_SCAN_TABLE` to avoid touching that large table.
const fn language_complexity_config(
    language: Language,
) -> (&'static [&'static str], LslocStrategy) {
    match language {
        // ── C-ternary family (ternary operator counted as branch) ─────────────
        Language::C
        | Language::Cpp
        | Language::ObjectiveC
        | Language::CSharp
        | Language::JavaScript
        | Language::TypeScript
        | Language::Svelte
        | Language::Vue
        | Language::Dart
        | Language::Groovy
        | Language::Swift
        | Language::Solidity => (BRANCH_C_TERNARY, LslocStrategy::Semicolons),
        // ── C-family (no ternary keyword) ────────────────────────────────────
        Language::Java | Language::Kotlin | Language::Scala | Language::D | Language::Glsl => {
            (BRANCH_C_FAMILY, LslocStrategy::Semicolons)
        }
        Language::Go => (BRANCH_GO, LslocStrategy::Semicolons),
        Language::Rust => (BRANCH_RUST, LslocStrategy::Semicolons),
        Language::Zig => (BRANCH_ZIG, LslocStrategy::Semicolons),
        Language::FSharp => (BRANCH_FSHARP, LslocStrategy::Unsupported),
        // ── Hash-comment family ───────────────────────────────────────────────
        Language::Shell => (BRANCH_SHELL, LslocStrategy::NonContinuationNewlines),
        Language::Elixir => (BRANCH_ELIXIR, LslocStrategy::NonContinuationNewlines),
        Language::Perl => (BRANCH_PERL, LslocStrategy::Semicolons),
        Language::R => (BRANCH_R, LslocStrategy::NonContinuationNewlines),
        Language::Ruby | Language::Crystal => (BRANCH_RUBY, LslocStrategy::NonContinuationNewlines),
        Language::Python => (BRANCH_PYTHON, LslocStrategy::NonContinuationNewlines),
        Language::PowerShell => (BRANCH_POWERSHELL, LslocStrategy::Unsupported),
        Language::Nim => (BRANCH_NIM, LslocStrategy::NonContinuationNewlines),
        // ── Unique comment styles ─────────────────────────────────────────────
        Language::Lua => (BRANCH_LUA, LslocStrategy::Unsupported),
        Language::Haskell => (BRANCH_HASKELL, LslocStrategy::Unsupported),
        Language::Sql => (BRANCH_SQL, LslocStrategy::Semicolons),
        Language::Ocaml => (BRANCH_OCAML, LslocStrategy::Semicolons),
        Language::Clojure => (BRANCH_CLOJURE, LslocStrategy::Unsupported),
        Language::Php => (BRANCH_PHP, LslocStrategy::Semicolons),
        Language::Julia => (BRANCH_JULIA, LslocStrategy::NonContinuationNewlines),
        Language::Protobuf => (&[], LslocStrategy::Semicolons),
        Language::Hcl => (&[], LslocStrategy::NonContinuationNewlines),
        // ── Legacy / embedded / HDL ───────────────────────────────────────────
        Language::Ada => (BRANCH_ADA, LslocStrategy::Semicolons),
        Language::Vhdl => (BRANCH_VHDL, LslocStrategy::Semicolons),
        Language::Verilog => (BRANCH_VERILOG, LslocStrategy::Semicolons),
        Language::Tcl => (BRANCH_TCL, LslocStrategy::NonContinuationNewlines),
        Language::Pascal => (BRANCH_PASCAL, LslocStrategy::Semicolons),
        Language::VisualBasic => (BRANCH_VB, LslocStrategy::NonContinuationNewlines),
        Language::Lisp => (BRANCH_LISP, LslocStrategy::Unsupported),
        // ── Scientific / infra / systems / graphics ───────────────────────────
        Language::Fortran => (BRANCH_FORTRAN, LslocStrategy::NonContinuationNewlines),
        Language::Nix => (BRANCH_NIX, LslocStrategy::Unsupported),
        Language::Cmake => (BRANCH_CMAKE, LslocStrategy::Unsupported),
        Language::Elm => (BRANCH_ELM, LslocStrategy::Unsupported),
        Language::Awk => (BRANCH_AWK, LslocStrategy::NonContinuationNewlines),
        // ── No branch detection / syntax unsupported ──────────────────────────
        Language::Makefile
        | Language::Dockerfile
        | Language::Css
        | Language::Html
        | Language::Xml
        | Language::Assembly
        | Language::Erlang
        | Language::GraphQl
        | Language::Scss => (&[], LslocStrategy::Unsupported),
    }
}

/// Per-language keyword prefixes used for best-effort structural symbol detection.
/// Each slice lists line prefixes (after leading whitespace is stripped) that indicate
/// a definition of that category. Empty slice = detection disabled for that category.
#[derive(Debug, Clone, Copy)]
struct SymbolPatterns {
    functions: &'static [&'static str],
    /// Line prefixes that classify as a function only when the line ALSO contains `(`
    /// AND there is no `=` between the prefix and the first `(`.  Used for C/C++ where
    /// function definitions are led by the return type (`void`, `int`, `bool`, …) with
    /// no dedicated keyword, so the paren guard distinguishes `void f(x)` from
    /// `void* p = malloc(n)`.
    functions_prefix_paren: &'static [&'static str],
    classes: &'static [&'static str],
    variables: &'static [&'static str],
    imports: &'static [&'static str],
    /// Line prefixes (after stripping leading whitespace) that indicate a test case or test
    /// function definition. Matched against code lines only, same as other symbol categories.
    tests: &'static [&'static str],
    /// Line prefixes that indicate a test assertion call (`ASSERT_EQ`, assertEquals, `assert_eq`!,
    /// Assert.AreEqual, etc.). Matched against code lines only.
    assertions: &'static [&'static str],
    /// Line prefixes that indicate a test suite / fixture / group declaration
    /// (`TEST_GROUP`, `BOOST_AUTO_TEST_SUITE`, [`TestClass`], [`TestFixture`], etc.).
    test_suites: &'static [&'static str],
    /// Type-keyword prefixes (e.g. `"int "`, `"const "`) that classify a line as a
    /// variable declaration when the line ALSO satisfies the complement of the
    /// `functions_prefix_paren` condition: either no `(` is present, or a `=` appears
    /// before the first `(`.  Used for C/C++ where both functions and variables are
    /// led by the same return / value type keywords; the paren guard splits them.
    variables_prefix_no_paren: &'static [&'static str],
}

impl SymbolPatterns {
    const fn none() -> Self {
        Self {
            functions: &[],
            functions_prefix_paren: &[],
            classes: &[],
            variables: &[],
            imports: &[],
            tests: &[],
            assertions: &[],
            test_suites: &[],
            variables_prefix_no_paren: &[],
        }
    }
}

const SP_NONE: SymbolPatterns = SymbolPatterns::none(); // all fields are &[]

// Solidity: `function`/`modifier`/`constructor` definitions; `contract`/`interface`/
// `library` are the structural units (mapped to classes alongside struct/enum).
const SP_SOLIDITY: SymbolPatterns = SymbolPatterns {
    functions: &[
        "function ",
        "modifier ",
        "constructor",
        "receive ",
        "fallback ",
    ],
    functions_prefix_paren: &[],
    classes: &["contract ", "interface ", "library ", "struct ", "enum "],
    variables: &[],
    imports: &["import "],
    // Foundry / DSTest / Forge-std: test functions are `function test...`, fuzz
    // tests `function testFuzz...`, and assertions are the `assert*`/`expect*` cheats.
    tests: &["function test", "function testFuzz", "function invariant"],
    assertions: &[
        "assertEq(",
        "assertEq0(",
        "assertTrue(",
        "assertFalse(",
        "assertGt(",
        "assertLt(",
        "assertGe(",
        "assertLe(",
        "assertApproxEq",
        "vm.expectRevert(",
        "vm.expectEmit(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

// Protocol Buffers: `message`/`service`/`enum` declarations are the structural units;
// `rpc` entries are the closest thing to functions.
const SP_PROTOBUF: SymbolPatterns = SymbolPatterns {
    functions: &["rpc "],
    functions_prefix_paren: &[],
    classes: &["message ", "service ", "enum "],
    variables: &[],
    imports: &["import "],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

// ── Pass 2 symbol patterns (legacy + embedded / HDL) ──────────────────────────
const SP_ADA: SymbolPatterns = SymbolPatterns {
    functions: &["procedure ", "function "],
    functions_prefix_paren: &[],
    classes: &["package ", "type ", "task ", "protected "],
    variables: &[],
    imports: &["with ", "use "],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_VHDL: SymbolPatterns = SymbolPatterns {
    functions: &["function ", "procedure ", "process "],
    functions_prefix_paren: &[],
    classes: &["entity ", "architecture ", "package ", "component "],
    variables: &[],
    imports: &["library ", "use "],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_VERILOG: SymbolPatterns = SymbolPatterns {
    functions: &["function ", "task "],
    functions_prefix_paren: &[],
    classes: &["module ", "interface ", "class ", "package "],
    variables: &[],
    imports: &["import ", "`include"],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_TCL: SymbolPatterns = SymbolPatterns {
    functions: &["proc "],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &[],
    imports: &["source ", "package require "],
    // tcltest: each case is introduced by the `test` command.
    tests: &["test "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_PASCAL: SymbolPatterns = SymbolPatterns {
    functions: &["procedure ", "function "],
    functions_prefix_paren: &[],
    classes: &["type ", "class ", "record "],
    variables: &[],
    imports: &["uses "],
    // DUnit / FPCUnit: test methods are `procedure Test...`; checks are the assertions.
    tests: &["procedure Test"],
    assertions: &[
        "Check(",
        "CheckEquals(",
        "CheckTrue(",
        "CheckFalse(",
        "CheckNotNull(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_VB: SymbolPatterns = SymbolPatterns {
    functions: &[
        "Sub ",
        "Function ",
        "Private Sub ",
        "Public Sub ",
        "Private Function ",
        "Public Function ",
    ],
    functions_prefix_paren: &[],
    classes: &["Class ", "Module ", "Structure "],
    variables: &[],
    imports: &["Imports "],
    // MSTest attributes on their own line; Assert.* calls for assertions.
    tests: &["<TestMethod>", "<TestMethod("],
    assertions: &["Assert.", "CollectionAssert.", "StringAssert."],
    test_suites: &["<TestClass>", "<TestClass("],
    variables_prefix_no_paren: &[],
};

const SP_LISP: SymbolPatterns = SymbolPatterns {
    functions: &["(defun ", "(defmacro ", "(define ", "(defmethod ", "(defn "],
    functions_prefix_paren: &[],
    classes: &["(defclass ", "(defstruct "],
    variables: &[],
    imports: &["(require ", "(import ", "(use-package "],
    // FiveAM (Common Lisp): `(test name ...)` cases with `(is ...)` checks.
    tests: &["(test ", "(deftest "],
    assertions: &["(is ", "(is-true ", "(is-false ", "(signals "],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

// ── Pass 3 symbol patterns (scientific / infra / systems / graphics) ──────────
const SP_FORTRAN: SymbolPatterns = SymbolPatterns {
    functions: &["subroutine ", "function "],
    functions_prefix_paren: &[],
    classes: &["module ", "program ", "type "],
    variables: &[],
    imports: &["use ", "include "],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_CRYSTAL: SymbolPatterns = SymbolPatterns {
    functions: &["def "],
    functions_prefix_paren: &[],
    classes: &["class ", "module ", "struct ", "enum "],
    variables: &[],
    imports: &["require "],
    // Crystal Spec (RSpec-style): describe/it/context groups, pending stubs.
    tests: &["it ", "it(", "describe ", "context ", "pending "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_D: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &["class ", "struct ", "interface ", "enum ", "template "],
    variables: &[],
    imports: &["import "],
    // D built-in unittest blocks; `assert` is the in-language check.
    tests: &["unittest"],
    assertions: &["assert(", "assertThrown", "assertNotThrown"],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_CMAKE: SymbolPatterns = SymbolPatterns {
    functions: &["function(", "macro("],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &[],
    imports: &["include(", "add_subdirectory("],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_ELM: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &["type "],
    variables: &[],
    imports: &["import "],
    // elm-test: test/describe/fuzz cases, with `Expect.*` checks.
    tests: &["test ", "describe ", "fuzz "],
    assertions: &["Expect."],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_AWK: SymbolPatterns = SymbolPatterns {
    functions: &["function "],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &[],
    imports: &[],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_RUST: SymbolPatterns = SymbolPatterns {
    functions: &[
        "fn ",
        "pub fn ",
        "pub(crate) fn ",
        "pub(super) fn ",
        "async fn ",
        "pub async fn ",
        "pub(crate) async fn ",
        "unsafe fn ",
        "pub unsafe fn ",
        "pub(crate) unsafe fn ",
        "const fn ",
        "pub const fn ",
        "pub(crate) const fn ",
        "extern fn ",
        "pub extern fn ",
    ],
    functions_prefix_paren: &[],
    classes: &[
        "struct ",
        "pub struct ",
        "pub(crate) struct ",
        "enum ",
        "pub enum ",
        "pub(crate) enum ",
        "trait ",
        "pub trait ",
        "pub(crate) trait ",
        "impl ",
        "impl<",
        "type ",
        "pub type ",
        "pub(crate) type ",
    ],
    variables: &["let ", "let mut "],
    imports: &["use ", "pub use ", "pub(crate) use ", "extern crate "],
    // Built-in #[test], tokio/actix async test attributes, rstest
    tests: &[
        "#[test]",
        "#[tokio::test]",
        "#[actix_web::test]",
        "#[rstest]",
        "#[test_case",
    ],
    assertions: &[
        "assert_eq!(",
        "assert_ne!(",
        "assert!(",
        "assert_matches!(",
        "assert_err!(",
        "assert_ok!(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_PYTHON: SymbolPatterns = SymbolPatterns {
    functions: &["def ", "async def "],
    functions_prefix_paren: &[],
    classes: &["class "],
    variables: &[],
    imports: &["import ", "from "],
    // pytest: test_ prefix functions and Test* classes; unittest: test_ methods
    tests: &["def test_", "async def test_", "class Test"],
    assertions: &[
        "self.assertEqual(",
        "self.assertNotEqual(",
        "self.assertTrue(",
        "self.assertFalse(",
        "self.assertIsNone(",
        "self.assertIsNotNone(",
        "self.assertIn(",
        "self.assertNotIn(",
        "self.assertRaises(",
        "self.assertAlmostEqual(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_JS: SymbolPatterns = SymbolPatterns {
    functions: &[
        "function ",
        "async function ",
        "export function ",
        "export async function ",
        "export default function ",
    ],
    functions_prefix_paren: &[],
    classes: &["class ", "export class ", "export default class "],
    variables: &[
        "var ",
        "let ",
        "const ",
        "export var ",
        "export let ",
        "export const ",
    ],
    imports: &["import "],
    // Jest/Mocha/Jasmine: describe/it/test block openers
    tests: &[
        "describe(",
        "it(",
        "test(",
        "it.each(",
        "test.each(",
        "describe.each(",
    ],
    assertions: &["expect("],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_TS: SymbolPatterns = SymbolPatterns {
    functions: &[
        "function ",
        "async function ",
        "export function ",
        "export async function ",
        "export default function ",
    ],
    functions_prefix_paren: &[],
    classes: &[
        "class ",
        "export class ",
        "export default class ",
        "abstract class ",
        "export abstract class ",
        "interface ",
        "export interface ",
        "declare class ",
        "declare interface ",
    ],
    variables: &[
        "var ",
        "let ",
        "const ",
        "export var ",
        "export let ",
        "export const ",
    ],
    imports: &["import "],
    // Jest/Mocha/Jasmine/Vitest: describe/it/test block openers
    tests: &[
        "describe(",
        "it(",
        "test(",
        "it.each(",
        "test.each(",
        "describe.each(",
    ],
    assertions: &["expect("],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_GO: SymbolPatterns = SymbolPatterns {
    functions: &["func "],
    functions_prefix_paren: &[],
    classes: &["type "],
    variables: &["var "],
    imports: &["import "],
    // Go standard testing: Test* functions (convention is practically exclusive to _test.go files)
    tests: &["func Test", "func Benchmark", "func Fuzz"],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_JAVA: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &[
        "class ",
        "public class ",
        "private class ",
        "protected class ",
        "abstract class ",
        "final class ",
        "public abstract class ",
        "public final class ",
        "interface ",
        "public interface ",
        "enum ",
        "public enum ",
        "record ",
        "public record ",
        "@interface ",
    ],
    variables: &[],
    imports: &["import "],
    // JUnit 4 & 5, TestNG — annotations appear on their own line before the method
    tests: &[
        "@Test",
        "@ParameterizedTest",
        "@RepeatedTest",
        "@TestFactory",
        "@TestTemplate",
    ],
    assertions: &[
        "assertEquals(",
        "assertNotEquals(",
        "assertTrue(",
        "assertFalse(",
        "assertNull(",
        "assertNotNull(",
        "assertThat(",
        "assertThrows(",
        "assertAll(",
        "assertArrayEquals(",
        "assertIterableEquals(",
        "assertLinesMatch(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_CSHARP: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &[
        "class ",
        "public class ",
        "private class ",
        "protected class ",
        "internal class ",
        "abstract class ",
        "sealed class ",
        "static class ",
        "partial class ",
        "public abstract class ",
        "public sealed class ",
        "public static class ",
        "interface ",
        "public interface ",
        "internal interface ",
        "enum ",
        "public enum ",
        "struct ",
        "public struct ",
        "record ",
        "public record ",
    ],
    variables: &["var "],
    imports: &["using "],
    // MSTest, NUnit, xUnit — attributes on their own line before the method
    tests: &[
        "[TestMethod]",
        "[Test]",
        "[Fact]",
        "[Theory]",
        "[TestCase(",
        "[DataRow(",
        "[InlineData(",
        "[MemberData(",
    ],
    assertions: &[
        "Assert.AreEqual(",
        "Assert.AreNotEqual(",
        "Assert.IsTrue(",
        "Assert.IsFalse(",
        "Assert.IsNull(",
        "Assert.IsNotNull(",
        "Assert.Equal(",
        "Assert.NotEqual(",
        "Assert.True(",
        "Assert.False(",
        "Assert.That(",
        "Assert.Contains(",
        "Assert.Throws(",
        "Assert.ThrowsAsync(",
        "Assert.IsInstanceOfType(",
    ],
    test_suites: &["[TestClass]", "[TestFixture]", "[SetUpFixture]"],
    variables_prefix_no_paren: &[],
};

// GTest, Catch2/doctest, Boost.Test, Unity, Check, CMocka, CppUTest patterns for C and C++.
const TEST_PATTERNS_C_CPP: &[&str] = &[
    // Google Test
    "TEST(",
    "TEST_F(",
    "TEST_P(",
    "TYPED_TEST(",
    "TYPED_TEST_P(",
    "INSTANTIATE_TEST_SUITE_P(",
    "INSTANTIATE_TYPED_TEST_SUITE_P(",
    // Catch2 / doctest
    "TEST_CASE(",
    "SECTION(",
    "SCENARIO(",
    "SCENARIO_METHOD(",
    "TEST_CASE_METHOD(",
    // Boost.Test
    "BOOST_AUTO_TEST_CASE(",
    "BOOST_FIXTURE_TEST_CASE(",
    "BOOST_AUTO_TEST_SUITE(",
    "BOOST_PARAM_TEST_CASE(",
    // CppUnit
    "CPPUNIT_TEST(",
    "CPPUNIT_TEST_SUITE(",
    // Unity (embedded C)
    "RUN_TEST(",
    "TEST_IGNORE(",
    "TEST_FAIL(",
    // Check (libcheck — embedded C)
    "START_TEST(",
    "tcase_add_test(",
    "suite_create(",
    // CMocka (embedded C)
    "cmocka_unit_test(",
    "cmocka_run_group_tests(",
    // CppUTest
    "IGNORE_TEST(",
    "TEST_GROUP(",
    "TEST_GROUP_BASE(",
];

// Test assertion patterns shared by C and C++.
const ASSERT_PATTERNS_C_CPP: &[&str] = &[
    // Google Test ASSERT_* (test-stopping failures)
    "ASSERT_EQ(",
    "ASSERT_NE(",
    "ASSERT_LT(",
    "ASSERT_LE(",
    "ASSERT_GT(",
    "ASSERT_GE(",
    "ASSERT_TRUE(",
    "ASSERT_FALSE(",
    "ASSERT_STREQ(",
    "ASSERT_STRNE(",
    "ASSERT_FLOAT_EQ(",
    "ASSERT_DOUBLE_EQ(",
    "ASSERT_NEAR(",
    "ASSERT_THROW(",
    "ASSERT_NO_THROW(",
    "ASSERT_ANY_THROW(",
    // Google Test EXPECT_* (non-stopping failures)
    "EXPECT_EQ(",
    "EXPECT_NE(",
    "EXPECT_LT(",
    "EXPECT_LE(",
    "EXPECT_GT(",
    "EXPECT_GE(",
    "EXPECT_TRUE(",
    "EXPECT_FALSE(",
    "EXPECT_STREQ(",
    "EXPECT_STRNE(",
    "EXPECT_FLOAT_EQ(",
    "EXPECT_DOUBLE_EQ(",
    "EXPECT_NEAR(",
    "EXPECT_THROW(",
    "EXPECT_NO_THROW(",
    "EXPECT_ANY_THROW(",
    // Catch2 / doctest assertions
    "REQUIRE(",
    "CHECK(",
    "REQUIRE_FALSE(",
    "CHECK_FALSE(",
    "REQUIRE_NOTHROW(",
    "CHECK_NOTHROW(",
    "REQUIRE_THROWS(",
    "CHECK_THROWS(",
    "REQUIRE_THAT(",
    "CHECK_THAT(",
    // Unity assertions (embedded C)
    "TEST_ASSERT_EQUAL(",
    "TEST_ASSERT_EQUAL_INT(",
    "TEST_ASSERT_EQUAL_STRING(",
    "TEST_ASSERT_EQUAL_FLOAT(",
    "TEST_ASSERT_EQUAL_DOUBLE(",
    "TEST_ASSERT_EQUAL_PTR(",
    "TEST_ASSERT_TRUE(",
    "TEST_ASSERT_FALSE(",
    "TEST_ASSERT_NULL(",
    "TEST_ASSERT_NOT_NULL(",
    "TEST_ASSERT_BITS_HIGH(",
    "TEST_ASSERT_BITS_LOW(",
    // CMocka assertions (embedded C)
    "assert_int_equal(",
    "assert_int_not_equal(",
    "assert_string_equal(",
    "assert_string_not_equal(",
    "assert_true(",
    "assert_false(",
    "assert_null(",
    "assert_non_null(",
    "assert_ptr_equal(",
    "assert_memory_equal(",
    "assert_return_code(",
];

// Test suite/group declaration patterns for C and C++.
const SUITE_PATTERNS_C_CPP: &[&str] = &[
    "TEST_GROUP(",
    "TEST_GROUP_BASE(",
    "BOOST_AUTO_TEST_SUITE(",
    "CPPUNIT_TEST_SUITE(",
    "CPPUNIT_TEST_SUITE_END(",
];

const SP_C: SymbolPatterns = SymbolPatterns {
    // C has no function keyword; detect by common return types that precede `(` with no `=`.
    functions: &[],
    functions_prefix_paren: &[
        "void ",
        "int ",
        "char ",
        "float ",
        "double ",
        "long ",
        "unsigned ",
        "size_t ",
        "static ",
        "inline ",
        "const ",
        "extern ",
    ],
    classes: &[
        "struct ",
        "typedef struct ",
        "union ",
        "typedef union ",
        "typedef enum ",
    ],
    variables: &[],
    imports: &["#include "],
    tests: TEST_PATTERNS_C_CPP,
    assertions: ASSERT_PATTERNS_C_CPP,
    test_suites: SUITE_PATTERNS_C_CPP,
    // Same type keywords as functions_prefix_paren; the complement paren guard (no unguarded `(`
    // in the line) distinguishes `int x;` / `int x = 5;` (variable) from `int foo()` (function).
    variables_prefix_no_paren: &[
        "void ",
        "int ",
        "char ",
        "float ",
        "double ",
        "long ",
        "unsigned ",
        "size_t ",
        "static ",
        "inline ",
        "const ",
        "extern ",
    ],
};

const SP_CPP: SymbolPatterns = SymbolPatterns {
    // C++ specific function keyword-prefixes; return-type-led patterns use functions_prefix_paren.
    functions: &[
        "virtual ",  // virtual method declaration/definition
        "explicit ", // explicit constructor modifier
        "~",         // destructor (e.g. ~MyClass())
        "operator",  // operator overload (operator==, operator+, …)
    ],
    functions_prefix_paren: &[
        "void ",
        "bool ",
        "int ",
        "char ",
        "float ",
        "double ",
        "long ",
        "unsigned ",
        "size_t ",
        "auto ",
        "static ",
        "inline ",
        "constexpr ",
        "const ",
        "extern ",
    ],
    // `template<` (no space) is the dominant modern style alongside `template ` (with space).
    classes: &["class ", "struct ", "namespace ", "template ", "template<"],
    variables: &[],
    imports: &["#include "],
    tests: TEST_PATTERNS_C_CPP,
    assertions: ASSERT_PATTERNS_C_CPP,
    test_suites: SUITE_PATTERNS_C_CPP,
    // Mirror of functions_prefix_paren; complement paren guard splits variables from functions.
    variables_prefix_no_paren: &[
        "void ",
        "bool ",
        "int ",
        "char ",
        "float ",
        "double ",
        "long ",
        "unsigned ",
        "size_t ",
        "auto ",
        "static ",
        "inline ",
        "constexpr ",
        "const ",
        "extern ",
    ],
};

const SP_SHELL: SymbolPatterns = SymbolPatterns {
    functions: &["function "],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &["declare ", "local ", "export "],
    imports: &["source ", ". "],
    // Bats (Bash Automated Testing System): each case is a `@test "name" {` block.
    tests: &["@test "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_POWERSHELL: SymbolPatterns = SymbolPatterns {
    functions: &["function ", "Function "],
    functions_prefix_paren: &[],
    classes: &["class "],
    variables: &[],
    imports: &["Import-Module ", "using "],
    // Pester test framework
    tests: &["Describe ", "It ", "Context "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_KOTLIN: SymbolPatterns = SymbolPatterns {
    functions: &[
        "fun ",
        "private fun ",
        "public fun ",
        "protected fun ",
        "internal fun ",
        "override fun ",
        "suspend fun ",
        "abstract fun ",
        "open fun ",
        "private suspend fun ",
        "public suspend fun ",
    ],
    functions_prefix_paren: &[],
    classes: &[
        "class ",
        "data class ",
        "sealed class ",
        "abstract class ",
        "open class ",
        "object ",
        "companion object",
        "interface ",
        "enum class ",
        "annotation class ",
    ],
    variables: &["val ", "var ", "private val ", "private var ", "const val "],
    imports: &["import "],
    // JUnit 4/5, KotlinTest, Kotest
    tests: &[
        "@Test",
        "@ParameterizedTest",
        "@RepeatedTest",
        "\"should ",
        "\"it ",
    ],
    assertions: &[
        "assertEquals(",
        "assertNotEquals(",
        "assertTrue(",
        "assertFalse(",
        "assertNull(",
        "assertNotNull(",
        "assertThat(",
        "assertThrows(",
        "shouldBe(",
        "shouldNotBe(",
        "shouldThrow(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_SWIFT: SymbolPatterns = SymbolPatterns {
    functions: &[
        "func ",
        "private func ",
        "public func ",
        "internal func ",
        "override func ",
        "open func ",
        "static func ",
        "class func ",
        "mutating func ",
        "private static func ",
        "public static func ",
    ],
    functions_prefix_paren: &[],
    classes: &[
        "class ",
        "struct ",
        "protocol ",
        "enum ",
        "extension ",
        "actor ",
        "public class ",
        "private class ",
        "open class ",
        "final class ",
        "public struct ",
        "private struct ",
        "public protocol ",
    ],
    variables: &[
        "var ",
        "let ",
        "private var ",
        "private let ",
        "static var ",
        "static let ",
    ],
    imports: &["import "],
    // XCTest: test functions are named test* by convention; Swift Testing: @Test attribute
    tests: &["func test", "func Test", "@Test"],
    assertions: &[
        "XCTAssertEqual(",
        "XCTAssertNotEqual(",
        "XCTAssertTrue(",
        "XCTAssertFalse(",
        "XCTAssertNil(",
        "XCTAssertNotNil(",
        "XCTAssertGreaterThan(",
        "XCTAssertLessThan(",
        "XCTAssertThrowsError(",
        "XCTAssertNoThrow(",
        "#expect(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_RUBY: SymbolPatterns = SymbolPatterns {
    functions: &["def ", "private def ", "protected def "],
    functions_prefix_paren: &[],
    classes: &["class ", "module "],
    variables: &[],
    imports: &["require ", "require_relative "],
    // RSpec / minitest
    tests: &["it ", "it(", "describe ", "context ", "test "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_SCALA: SymbolPatterns = SymbolPatterns {
    functions: &["def ", "private def ", "protected def ", "override def "],
    functions_prefix_paren: &[],
    classes: &[
        "class ",
        "case class ",
        "abstract class ",
        "sealed class ",
        "object ",
        "trait ",
    ],
    variables: &["val ", "var ", "lazy val "],
    imports: &["import "],
    // ScalaTest / MUnit: FunSuite test("..."), FlatSpec it("..."), AnyWordSpec "..." should
    tests: &["test(", "it(", "describe("],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_PHP: SymbolPatterns = SymbolPatterns {
    functions: &[
        "function ",
        "public function ",
        "private function ",
        "protected function ",
        "static function ",
        "abstract function ",
        "final function ",
        "public static function ",
        "private static function ",
        "protected static function ",
    ],
    functions_prefix_paren: &[],
    classes: &[
        "class ",
        "abstract class ",
        "final class ",
        "interface ",
        "trait ",
        "enum ",
    ],
    variables: &[],
    imports: &[
        "use ",
        "require ",
        "require_once ",
        "include ",
        "include_once ",
    ],
    // PHPUnit: test methods start with test, or use @test annotation
    tests: &[
        "public function test",
        "function test",
        "#[Test]",
        "#[DataProvider(",
    ],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_ELIXIR: SymbolPatterns = SymbolPatterns {
    functions: &[
        "def ",
        "defp ",
        "defmacro ",
        "defmacrop ",
        "defguard ",
        "defguardp ",
    ],
    functions_prefix_paren: &[],
    classes: &["defmodule ", "defprotocol ", "defimpl "],
    variables: &[],
    imports: &["import ", "alias ", "use ", "require "],
    // ExUnit
    tests: &["test ", "describe "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_ERLANG: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &["-module("],
    variables: &[],
    imports: &["-import(", "-include(", "-include_lib("],
    // EUnit: test names end in `_test`/`_test_` (suffix — not prefix-matchable), so we
    // only count the `?assert*` macro family, which is line-prefixable.
    tests: &[],
    assertions: &[
        "?assert(",
        "?assertEqual(",
        "?assertNotEqual(",
        "?assertMatch(",
        "?assertError(",
        "?assertThrow(",
        "?assertException(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_FSHARP: SymbolPatterns = SymbolPatterns {
    functions: &[
        "let ",
        "let rec ",
        "member ",
        "override ",
        "abstract member ",
    ],
    functions_prefix_paren: &[],
    classes: &["type "],
    variables: &["let mutable "],
    imports: &["open "],
    // NUnit / xUnit attributes on their own line; FsUnit uses [<Test>] / [<Fact>]
    tests: &["[<Test>]", "[<Fact>]", "[<Theory>]", "[<TestCase("],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_GROOVY: SymbolPatterns = SymbolPatterns {
    functions: &["def ", "private def ", "public def ", "protected def "],
    functions_prefix_paren: &[],
    classes: &["class ", "abstract class ", "interface ", "enum ", "trait "],
    variables: &[],
    imports: &["import "],
    // Spock framework: feature methods; JUnit annotations
    tests: &["def \"", "@Test", "given:", "when:", "then:", "expect:"],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_HASKELL: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &["class ", "data ", "newtype ", "type "],
    variables: &[],
    imports: &["import "],
    // Hspec (describe/it) and QuickCheck (prop_) conventions. Hspec expectations
    // (`x `shouldBe` y`) are infix/mid-line, so they are not prefix-countable here.
    tests: &["describe ", "it ", "prop_"],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_LUA: SymbolPatterns = SymbolPatterns {
    functions: &["function ", "local function "],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &["local "],
    imports: &[],
    // busted test framework
    tests: &["it(", "describe(", "pending("],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_NIM: SymbolPatterns = SymbolPatterns {
    functions: &[
        "proc ",
        "func ",
        "method ",
        "iterator ",
        "converter ",
        "template ",
        "macro ",
    ],
    functions_prefix_paren: &[],
    classes: &["type "],
    variables: &["var ", "let ", "const "],
    imports: &["import ", "from "],
    // unittest module
    tests: &["test "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_OBJECTIVEC: SymbolPatterns = SymbolPatterns {
    functions: &["- (", "+ ("],
    functions_prefix_paren: &[],
    classes: &["@interface ", "@implementation ", "@protocol "],
    variables: &[],
    imports: &["#import ", "#include "],
    // XCTest: test methods start with - (void)test
    tests: &["- (void)test"],
    assertions: &[
        "XCTAssertEqual(",
        "XCTAssertNotEqual(",
        "XCTAssertTrue(",
        "XCTAssertFalse(",
        "XCTAssertNil(",
        "XCTAssertNotNil(",
        "XCTAssertGreaterThan(",
        "XCTAssertLessThan(",
        "XCTAssertThrowsError(",
        "XCTAssertNoThrow(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_OCAML: SymbolPatterns = SymbolPatterns {
    functions: &["let ", "let rec "],
    functions_prefix_paren: &[],
    classes: &["type ", "module ", "class "],
    variables: &[],
    imports: &["open "],
    // OUnit (`let test_... >:: `, `assert_*`) and Alcotest (`test_case`) conventions.
    tests: &["let test_", "test_case "],
    assertions: &[
        "assert_equal ",
        "assert_bool ",
        "assert_raises ",
        "assert_failure ",
        "OUnit.assert",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_PERL: SymbolPatterns = SymbolPatterns {
    functions: &["sub "],
    functions_prefix_paren: &[],
    classes: &["package "],
    variables: &["my ", "our ", "local "],
    imports: &["use ", "require "],
    // Test::More / Test2: subtests group cases; ok/is/like/etc. are the assertions.
    tests: &["subtest "],
    assertions: &[
        "ok(",
        "is(",
        "isnt(",
        "like(",
        "unlike(",
        "cmp_ok(",
        "is_deeply(",
        "isa_ok(",
        "can_ok(",
    ],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_CLOJURE: SymbolPatterns = SymbolPatterns {
    functions: &["(defn ", "(defn- ", "(defmacro ", "(defmulti "],
    functions_prefix_paren: &[],
    classes: &[
        "(defrecord ",
        "(defprotocol ",
        "(deftype ",
        "(definterface ",
    ],
    variables: &["(def ", "(defonce "],
    imports: &["(ns ", "(require "],
    // clojure.test
    tests: &["(deftest ", "(testing "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_JULIA: SymbolPatterns = SymbolPatterns {
    functions: &["function ", "macro "],
    functions_prefix_paren: &[],
    classes: &[
        "struct ",
        "mutable struct ",
        "abstract type ",
        "primitive type ",
    ],
    variables: &["const "],
    imports: &["import ", "using "],
    // Test.jl standard library
    tests: &["@test ", "@testset "],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_DART: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &["class ", "abstract class ", "mixin ", "extension ", "enum "],
    variables: &["var ", "final ", "const ", "late "],
    imports: &["import "],
    // flutter_test / test package
    tests: &["test(", "testWidgets(", "group("],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_R: SymbolPatterns = SymbolPatterns {
    functions: &[],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &[],
    imports: &["library(", "source("],
    // testthat
    tests: &["test_that(", "it(", "describe(", "expect_"],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_SQL: SymbolPatterns = SymbolPatterns {
    functions: &[
        "create function ",
        "create or replace function ",
        "create procedure ",
        "create or replace procedure ",
        "CREATE FUNCTION ",
        "CREATE OR REPLACE FUNCTION ",
        "CREATE PROCEDURE ",
        "CREATE OR REPLACE PROCEDURE ",
    ],
    functions_prefix_paren: &[],
    classes: &[
        "create table ",
        "create view ",
        "create schema ",
        "CREATE TABLE ",
        "CREATE VIEW ",
        "CREATE SCHEMA ",
    ],
    variables: &["declare ", "DECLARE "],
    imports: &[],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_ASSEMBLY: SymbolPatterns = SymbolPatterns {
    functions: &["proc ", "PROC "],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &[],
    imports: &["include ", "INCLUDE ", "%include "],
    tests: &[],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

const SP_ZIG: SymbolPatterns = SymbolPatterns {
    functions: &[
        "fn ",
        "pub fn ",
        "export fn ",
        "inline fn ",
        "pub inline fn ",
    ],
    functions_prefix_paren: &[],
    classes: &[],
    variables: &["var ", "pub var "],
    imports: &[],
    // Zig built-in test blocks
    tests: &["test \"", "test{"],
    assertions: &[],
    test_suites: &[],
    variables_prefix_no_paren: &[],
};

/// Static (non-heap) language scanning parameters.  All fields are `'static` so this struct
/// can be stored in a `static` array.  The dynamic `skip_lines` set (used only for Python
/// docstring detection) is kept in `ScanConfig` and populated by the caller after lookup.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy)]
struct StaticLangConfig {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    allow_single_quote_strings: bool,
    allow_double_quote_strings: bool,
    allow_triple_quote_strings: bool,
    allow_csharp_verbatim_strings: bool,
    /// `true` for Rust: `r"…"`, `r#"…"#`, `br#"…"#` raw strings where inner `"` do not close
    /// the literal. Prevents branch keywords in embedded templates (HTML/JS) from being counted.
    allow_raw_strings: bool,
    symbol_patterns: SymbolPatterns,
    /// `true` for C, C++, and Objective-C (languages that have a C preprocessor).
    has_preprocessor: bool,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
struct ScanConfig {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    allow_single_quote_strings: bool,
    allow_double_quote_strings: bool,
    allow_triple_quote_strings: bool,
    allow_csharp_verbatim_strings: bool,
    allow_raw_strings: bool,
    skip_lines: HashSet<usize>,
    symbol_patterns: SymbolPatterns,
    /// Branch keywords used to approximate cyclomatic complexity.
    branch_keywords: &'static [&'static str],
    /// Strategy for computing Logical SLOC.
    lsloc_strategy: LslocStrategy,
}

// ── Per-family base configurations ───────────────────────────────────────────
//
// Most languages share one of two comment styles.  Define a base `const` for
// each family; table entries override only the fields that differ (symbol
// patterns, preprocessor flag, verbatim-string flag, etc.).
//
// C-slash family: `//` line, `/* */` block, single + double quotes.
// Covers C, C++, Obj-C, C#, Go, Java, JS/TS/Svelte/Vue, Dart, Groovy, Kotlin,
// Scala, SCSS, Swift, Rust, and Zig (Zig has no block comment → overridden).
const C_SLASH_BASE: StaticLangConfig = StaticLangConfig {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    allow_single_quote_strings: true,
    allow_double_quote_strings: true,
    allow_triple_quote_strings: false,
    allow_csharp_verbatim_strings: false,
    allow_raw_strings: false,
    symbol_patterns: SP_NONE,
    has_preprocessor: false,
};

// Hash-comment family: `#` line comment, no block comment, single + double
// quotes.  Covers Shell, Ruby, R, Perl, Elixir (each overrides only SP_*);
// Python overrides triple-quote; PowerShell and Nim override block_comment.
const HASH_BASE: StaticLangConfig = StaticLangConfig {
    line_comments: &["#"],
    block_comment: None,
    allow_single_quote_strings: true,
    allow_double_quote_strings: true,
    allow_triple_quote_strings: false,
    allow_csharp_verbatim_strings: false,
    allow_raw_strings: false,
    symbol_patterns: SP_NONE,
    has_preprocessor: false,
};

/// Static language-scan configuration table — one entry per supported language.
/// Used by `language_scan_config` to avoid a 41-arm match.  All `SP_*` constants
/// referenced here are defined above in the same module.
static LANG_SCAN_TABLE: &[(Language, StaticLangConfig)] = &[
    // ── C preprocessor family ─────────────────────────────────────────────────
    (
        Language::C,
        StaticLangConfig {
            symbol_patterns: SP_C,
            has_preprocessor: true,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Cpp,
        StaticLangConfig {
            symbol_patterns: SP_CPP,
            has_preprocessor: true,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::ObjectiveC,
        StaticLangConfig {
            symbol_patterns: SP_OBJECTIVEC,
            has_preprocessor: true,
            ..C_SLASH_BASE
        },
    ),
    // ── C-slash family ────────────────────────────────────────────────────────
    (
        Language::CSharp,
        StaticLangConfig {
            symbol_patterns: SP_CSHARP,
            allow_csharp_verbatim_strings: true,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Go,
        StaticLangConfig {
            symbol_patterns: SP_GO,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Java,
        StaticLangConfig {
            symbol_patterns: SP_JAVA,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::JavaScript,
        StaticLangConfig {
            symbol_patterns: SP_JS,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::TypeScript,
        StaticLangConfig {
            symbol_patterns: SP_TS,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Svelte,
        StaticLangConfig {
            symbol_patterns: SP_JS,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Vue,
        StaticLangConfig {
            symbol_patterns: SP_JS,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Dart,
        StaticLangConfig {
            symbol_patterns: SP_DART,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Groovy,
        StaticLangConfig {
            symbol_patterns: SP_GROOVY,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Kotlin,
        StaticLangConfig {
            symbol_patterns: SP_KOTLIN,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Scala,
        StaticLangConfig {
            symbol_patterns: SP_SCALA,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Scss,
        StaticLangConfig {
            symbol_patterns: SP_NONE,
            ..C_SLASH_BASE
        },
    ),
    // Rust: no single-quote char literals (they're lifetime annotations)
    (
        Language::Rust,
        StaticLangConfig {
            symbol_patterns: SP_RUST,
            allow_single_quote_strings: false,
            allow_raw_strings: true,
            ..C_SLASH_BASE
        },
    ),
    // Swift: no single-quote strings
    (
        Language::Swift,
        StaticLangConfig {
            symbol_patterns: SP_SWIFT,
            allow_single_quote_strings: false,
            ..C_SLASH_BASE
        },
    ),
    // Zig: no block comment
    (
        Language::Zig,
        StaticLangConfig {
            symbol_patterns: SP_ZIG,
            block_comment: None,
            ..C_SLASH_BASE
        },
    ),
    // F#: `(*` … `*)` block comment, no single-quote strings
    (
        Language::FSharp,
        StaticLangConfig {
            line_comments: &["//"],
            block_comment: Some(("(*", "*)")),
            allow_single_quote_strings: false,
            allow_double_quote_strings: true,
            symbol_patterns: SP_FSHARP,
            ..C_SLASH_BASE
        },
    ),
    // ── Hash-comment family ───────────────────────────────────────────────────
    (
        Language::Shell,
        StaticLangConfig {
            symbol_patterns: SP_SHELL,
            ..HASH_BASE
        },
    ),
    (
        Language::Elixir,
        StaticLangConfig {
            symbol_patterns: SP_ELIXIR,
            ..HASH_BASE
        },
    ),
    (
        Language::Perl,
        StaticLangConfig {
            symbol_patterns: SP_PERL,
            ..HASH_BASE
        },
    ),
    (
        Language::R,
        StaticLangConfig {
            symbol_patterns: SP_R,
            ..HASH_BASE
        },
    ),
    (
        Language::Ruby,
        StaticLangConfig {
            symbol_patterns: SP_RUBY,
            ..HASH_BASE
        },
    ),
    // Python: triple-quote string literals
    (
        Language::Python,
        StaticLangConfig {
            symbol_patterns: SP_PYTHON,
            allow_triple_quote_strings: true,
            ..HASH_BASE
        },
    ),
    // PowerShell: `<# … #>` block comment
    (
        Language::PowerShell,
        StaticLangConfig {
            symbol_patterns: SP_POWERSHELL,
            block_comment: Some(("<#", "#>")),
            ..HASH_BASE
        },
    ),
    // Nim: `#[` … `]#` block comment
    (
        Language::Nim,
        StaticLangConfig {
            symbol_patterns: SP_NIM,
            block_comment: Some(("#[", "]#")),
            ..HASH_BASE
        },
    ),
    // Makefile / Dockerfile: `#` only, no string literals
    (
        Language::Makefile,
        StaticLangConfig {
            symbol_patterns: SP_NONE,
            allow_single_quote_strings: false,
            allow_double_quote_strings: false,
            ..HASH_BASE
        },
    ),
    (
        Language::Dockerfile,
        StaticLangConfig {
            symbol_patterns: SP_NONE,
            allow_single_quote_strings: false,
            allow_double_quote_strings: false,
            ..HASH_BASE
        },
    ),
    // ── Other unique comment styles ───────────────────────────────────────────
    // CSS / SCSS: only `/* */` block, no line comment
    (
        Language::Css,
        StaticLangConfig {
            line_comments: &[],
            block_comment: Some(("/*", "*/")),
            symbol_patterns: SP_NONE,
            ..C_SLASH_BASE
        },
    ),
    // HTML / XML: `<!-- -->` block, no line comment, no string literals
    (
        Language::Html,
        StaticLangConfig {
            line_comments: &[],
            block_comment: Some(("<!--", "-->")),
            allow_single_quote_strings: false,
            allow_double_quote_strings: false,
            symbol_patterns: SP_NONE,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Xml,
        StaticLangConfig {
            line_comments: &[],
            block_comment: Some(("<!--", "-->")),
            allow_single_quote_strings: false,
            allow_double_quote_strings: false,
            symbol_patterns: SP_NONE,
            ..C_SLASH_BASE
        },
    ),
    // Lua: `--` line, `--[[ ]]` block
    (
        Language::Lua,
        StaticLangConfig {
            line_comments: &["--"],
            block_comment: Some(("--[[", "]]")),
            symbol_patterns: SP_LUA,
            ..C_SLASH_BASE
        },
    ),
    // Haskell: `--` line, `{- -}` block
    (
        Language::Haskell,
        StaticLangConfig {
            line_comments: &["--"],
            block_comment: Some(("{-", "-}")),
            symbol_patterns: SP_HASKELL,
            ..C_SLASH_BASE
        },
    ),
    // SQL: `--` line, `/* */` block, single quote only
    (
        Language::Sql,
        StaticLangConfig {
            line_comments: &["--"],
            block_comment: Some(("/*", "*/")),
            allow_single_quote_strings: true,
            allow_double_quote_strings: false,
            symbol_patterns: SP_SQL,
            ..C_SLASH_BASE
        },
    ),
    // OCaml: `(*` … `*)` only, no line comment, no single-quote strings
    (
        Language::Ocaml,
        StaticLangConfig {
            line_comments: &[],
            block_comment: Some(("(*", "*)")),
            allow_single_quote_strings: false,
            symbol_patterns: SP_OCAML,
            ..C_SLASH_BASE
        },
    ),
    // Assembly: `;` line comment (NASM/MASM) + `/* */` block (GAS), double-quote
    // strings for `.ascii`/`.string` directives. `#` (GAS x86) and `@` (ARM) line
    // comments are intentionally NOT added: `#` is an immediate prefix in ARM
    // (`mov r0, #5`) and `@` appears in x86 symbol versioning (`memcpy@plt`), so a
    // universal superset would mis-count one dialect or the other.
    (
        Language::Assembly,
        StaticLangConfig {
            line_comments: &[";"],
            block_comment: Some(("/*", "*/")),
            allow_single_quote_strings: false,
            allow_double_quote_strings: true,
            symbol_patterns: SP_ASSEMBLY,
            ..C_SLASH_BASE
        },
    ),
    (
        Language::Clojure,
        StaticLangConfig {
            line_comments: &[";"],
            block_comment: None,
            allow_single_quote_strings: false,
            symbol_patterns: SP_CLOJURE,
            ..C_SLASH_BASE
        },
    ),
    // Erlang: `%` line comment, no block, no single-quote strings
    (
        Language::Erlang,
        StaticLangConfig {
            line_comments: &["%"],
            block_comment: None,
            allow_single_quote_strings: false,
            symbol_patterns: SP_ERLANG,
            ..C_SLASH_BASE
        },
    ),
    // PHP: `//` or `#` line, `/* */` block
    (
        Language::Php,
        StaticLangConfig {
            line_comments: &["//", "#"],
            block_comment: Some(("/*", "*/")),
            symbol_patterns: SP_PHP,
            ..C_SLASH_BASE
        },
    ),
    // Julia: `#` line, `#= =#` block, double + triple quotes, no single
    (
        Language::Julia,
        StaticLangConfig {
            line_comments: &["#"],
            block_comment: Some(("#=", "=#")),
            allow_single_quote_strings: false,
            allow_triple_quote_strings: true,
            symbol_patterns: SP_JULIA,
            ..C_SLASH_BASE
        },
    ),
    // ── Pass 1 additions ──────────────────────────────────────────────────────
    // Solidity: C-slash family (`//`, `/* */`, single + double quotes).
    (
        Language::Solidity,
        StaticLangConfig {
            symbol_patterns: SP_SOLIDITY,
            ..C_SLASH_BASE
        },
    ),
    // Protocol Buffers: C-slash family, statements terminated by `;`.
    (
        Language::Protobuf,
        StaticLangConfig {
            symbol_patterns: SP_PROTOBUF,
            ..C_SLASH_BASE
        },
    ),
    // HCL / Terraform: `#` or `//` line, `/* */` block, double-quote strings only.
    (
        Language::Hcl,
        StaticLangConfig {
            line_comments: &["#", "//"],
            allow_single_quote_strings: false,
            symbol_patterns: SP_NONE,
            ..C_SLASH_BASE
        },
    ),
    // GraphQL: `#` line comment, no block; `"""` block-string descriptions, no single quotes.
    (
        Language::GraphQl,
        StaticLangConfig {
            allow_single_quote_strings: false,
            allow_triple_quote_strings: true,
            symbol_patterns: SP_NONE,
            ..HASH_BASE
        },
    ),
    // ── Pass 2 additions (legacy + embedded / HDL) ────────────────────────────
    // Ada: `--` line comment, no block; `'` is a char/attribute tick, not a string.
    (
        Language::Ada,
        StaticLangConfig {
            line_comments: &["--"],
            block_comment: None,
            allow_single_quote_strings: false,
            symbol_patterns: SP_ADA,
            ..C_SLASH_BASE
        },
    ),
    // VHDL: `--` line comment, no block; `'` is a bit/char literal, not a string.
    (
        Language::Vhdl,
        StaticLangConfig {
            line_comments: &["--"],
            block_comment: None,
            allow_single_quote_strings: false,
            symbol_patterns: SP_VHDL,
            ..C_SLASH_BASE
        },
    ),
    // Verilog / SystemVerilog: C-slash family; `'` is a sized-literal base, not a string.
    (
        Language::Verilog,
        StaticLangConfig {
            allow_single_quote_strings: false,
            symbol_patterns: SP_VERILOG,
            ..C_SLASH_BASE
        },
    ),
    // Tcl: `#` line comment, no block; `"` strings only.
    (
        Language::Tcl,
        StaticLangConfig {
            allow_single_quote_strings: false,
            symbol_patterns: SP_TCL,
            ..HASH_BASE
        },
    ),
    // Pascal / Delphi: `//` line, `{ }` block; strings are single-quoted.
    (
        Language::Pascal,
        StaticLangConfig {
            line_comments: &["//"],
            block_comment: Some(("{", "}")),
            allow_single_quote_strings: true,
            allow_double_quote_strings: false,
            symbol_patterns: SP_PASCAL,
            ..C_SLASH_BASE
        },
    ),
    // Visual Basic: `'` line comment, no block; `"` strings only.
    (
        Language::VisualBasic,
        StaticLangConfig {
            line_comments: &["'"],
            block_comment: None,
            allow_single_quote_strings: false,
            allow_double_quote_strings: true,
            symbol_patterns: SP_VB,
            ..C_SLASH_BASE
        },
    ),
    // Lisp / Scheme: `;` line comment, `#| |#` block; `"` strings, `'` is the quote operator.
    (
        Language::Lisp,
        StaticLangConfig {
            line_comments: &[";"],
            block_comment: Some(("#|", "|#")),
            allow_single_quote_strings: false,
            symbol_patterns: SP_LISP,
            ..C_SLASH_BASE
        },
    ),
    // ── Pass 3 additions (scientific / infra / systems / graphics) ────────────
    // Fortran: `!` line comment (free-form), no block; single + double strings.
    (
        Language::Fortran,
        StaticLangConfig {
            line_comments: &["!"],
            block_comment: None,
            symbol_patterns: SP_FORTRAN,
            ..C_SLASH_BASE
        },
    ),
    // Nix: `#` line, `/* */` block; double-quote strings (and `''` multi-line).
    (
        Language::Nix,
        StaticLangConfig {
            block_comment: Some(("/*", "*/")),
            allow_single_quote_strings: false,
            symbol_patterns: SP_NONE,
            ..HASH_BASE
        },
    ),
    // Crystal: `#` line comment, no block; Ruby-like single + double strings.
    (
        Language::Crystal,
        StaticLangConfig {
            symbol_patterns: SP_CRYSTAL,
            ..HASH_BASE
        },
    ),
    // D: C-slash family (`//`, `/* */`); single-quote char literals + double strings.
    (
        Language::D,
        StaticLangConfig {
            symbol_patterns: SP_D,
            ..C_SLASH_BASE
        },
    ),
    // GLSL / HLSL / WGSL shaders: C-slash family; no char literals.
    (
        Language::Glsl,
        StaticLangConfig {
            allow_single_quote_strings: false,
            symbol_patterns: SP_NONE,
            ..C_SLASH_BASE
        },
    ),
    // CMake: `#` line, `#[[ ]]` block; double-quote strings only.
    (
        Language::Cmake,
        StaticLangConfig {
            block_comment: Some(("#[[", "]]")),
            allow_single_quote_strings: false,
            symbol_patterns: SP_CMAKE,
            ..HASH_BASE
        },
    ),
    // Elm: `--` line, `{- -}` block; double-quote strings only.
    (
        Language::Elm,
        StaticLangConfig {
            line_comments: &["--"],
            block_comment: Some(("{-", "-}")),
            allow_single_quote_strings: false,
            symbol_patterns: SP_ELM,
            ..C_SLASH_BASE
        },
    ),
    // Awk: `#` line comment, no block; double-quote strings only.
    (
        Language::Awk,
        StaticLangConfig {
            allow_single_quote_strings: false,
            symbol_patterns: SP_AWK,
            ..HASH_BASE
        },
    ),
];

/// Per-call IEEE 1045-1992 flags derived from `AnalysisOptions` plus per-language properties.
/// Private to this crate; constructed inside `analyze_text`.
#[derive(Debug, Clone, Copy)]
struct IeeeFlags {
    /// True for C, C++, and Objective-C — languages with a C preprocessor.
    has_preprocessor_directives: bool,
    /// Mirrors `AnalysisOptions::blank_in_block_comment_as_comment`.
    blank_in_block_comment_as_comment: bool,
    /// Mirrors `AnalysisOptions::collapse_continuation_lines`.
    collapse_continuation_lines: bool,
}

#[derive(Debug, Clone, Copy)]
enum StringState {
    Single(char),
    Triple(&'static str),
    VerbatimDouble,
    /// Rust raw string `r#…"…"#…` with the given number of `#` hashes. Closed only by a `"`
    /// followed by exactly that many `#`; inner `"` and `\` are literal (no escaping).
    RawHash(usize),
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
struct LineFacts {
    has_code: bool,
    has_single_comment: bool,
    has_multi_comment: bool,
    has_docstring: bool,
}

/// Process one character while the lexer is inside a string literal.
///
/// Returns `(new_string_state, advance)` where `advance` is the number of chars to skip.
fn process_string_char(
    state: StringState,
    chars: &[char],
    i: usize,
) -> (Option<StringState>, usize) {
    match state {
        StringState::Single(delim) => step_single(state, delim, chars, i),
        StringState::Triple(delim) => step_triple(state, delim, chars, i),
        StringState::VerbatimDouble => step_verbatim(state, chars, i),
        StringState::RawHash(hashes) => step_raw_hash(state, hashes, chars, i),
    }
}

/// One step inside a single-char-delimited string (`'…'` or `"…"`), honouring `\` escapes.
fn step_single(
    state: StringState,
    delim: char,
    chars: &[char],
    i: usize,
) -> (Option<StringState>, usize) {
    if chars[i] == '\\' {
        return (Some(state), 2); // skip escaped character
    }
    if chars[i] == delim {
        (None, 1)
    } else {
        (Some(state), 1)
    }
}

/// One step inside a triple-quoted string (`"""…"""` / `'''…'''`).
fn step_triple(
    state: StringState,
    delim: &'static str,
    chars: &[char],
    i: usize,
) -> (Option<StringState>, usize) {
    if starts_with(chars, i, delim) {
        (None, delim.len())
    } else {
        (Some(state), 1)
    }
}

/// One step inside a C# verbatim string (`@"…"`), where `""` is an escaped quote.
fn step_verbatim(state: StringState, chars: &[char], i: usize) -> (Option<StringState>, usize) {
    if starts_with(chars, i, "\"\"") {
        return (Some(state), 2); // escaped quote-quote inside verbatim string
    }
    if chars[i] == '"' {
        (None, 1)
    } else {
        (Some(state), 1)
    }
}

/// One step inside a Rust raw string (`r#"…"#`); closes on `"` + at least `hashes` `#`, no escapes.
fn step_raw_hash(
    state: StringState,
    hashes: usize,
    chars: &[char],
    i: usize,
) -> (Option<StringState>, usize) {
    if chars[i] == '"' && count_leading_hashes(chars, i + 1) >= hashes {
        (None, 1 + hashes)
    } else {
        (Some(state), 1)
    }
}

/// Count consecutive `#` characters starting at `index`.
fn count_leading_hashes(chars: &[char], index: usize) -> usize {
    let mut n = 0;
    while chars.get(index + n) == Some(&'#') {
        n += 1;
    }
    n
}

/// Detect a Rust raw-string opener at `i`: optional `b`, `r`, zero or more `#`, then `"`.
///
/// Returns `Some((hashes, advance))` where `advance` is the opener length. Requires a preceding
/// non-word boundary so `r`/`br` inside an identifier is not misread as a raw string.
fn try_open_raw_string(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let word_before = i
        .checked_sub(1)
        .and_then(|p| chars.get(p))
        .is_some_and(|c| c.is_alphanumeric() || *c == '_');
    if word_before {
        return None;
    }
    let mut j = i;
    if chars.get(j) == Some(&'b') {
        j += 1; // byte raw string: br"…"
    }
    if chars.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let hashes = count_leading_hashes(chars, j);
    j += hashes;
    if chars.get(j) != Some(&'"') {
        return None;
    }
    Some((hashes, j + 1 - i))
}

/// Process one character while the lexer is inside a block comment.
///
/// Returns `(still_in_block_comment, advance)`.
fn process_block_comment_char(chars: &[char], i: usize, close: &str) -> (bool, usize) {
    if starts_with(chars, i, close) {
        (false, close.len())
    } else {
        (true, 1)
    }
}

/// Attempt to begin a new string literal at position `i`.
///
/// Returns `Some((new_state, advance))` when a string opener is detected, else `None`.
fn try_open_string(chars: &[char], i: usize, config: &ScanConfig) -> Option<(StringState, usize)> {
    if config.allow_raw_strings
        && let Some((hashes, advance)) = try_open_raw_string(chars, i)
    {
        return Some((StringState::RawHash(hashes), advance));
    }
    if config.allow_csharp_verbatim_strings && starts_with(chars, i, "@\"") {
        return Some((StringState::VerbatimDouble, 2));
    }
    if config.allow_triple_quote_strings {
        if starts_with(chars, i, "\"\"\"") {
            return Some((StringState::Triple("\"\"\""), 3));
        }
        if starts_with(chars, i, "'''") {
            return Some((StringState::Triple("'''"), 3));
        }
    }
    if config.allow_single_quote_strings && chars[i] == '\'' {
        return Some((StringState::Single('\''), 1));
    }
    if config.allow_double_quote_strings && chars[i] == '"' {
        return Some((StringState::Single('"'), 1));
    }
    None
}

/// Advance past one character position while inside a block comment.
///
/// Updates `in_block_comment` if the closing delimiter is found and returns the
/// number of characters consumed. Returns 0 when no block-comment config is set
/// (preserving the caller's `continue`-without-advance behaviour for that impossible state).
fn step_through_block_comment(
    chars: &[char],
    i: usize,
    block_comment: Option<(&'static str, &'static str)>,
    in_block_comment: &mut bool,
) -> usize {
    if let Some((_, close)) = block_comment {
        let (still_in, advance) = process_block_comment_char(chars, i, close);
        *in_block_comment = still_in;
        return advance;
    }
    0
}

/// If the character at `i` starts a block comment, return the length of the opening
/// delimiter so the caller can advance past it. Returns `None` if no match.
fn try_open_block_comment(
    chars: &[char],
    i: usize,
    block_comment: Option<(&'static str, &'static str)>,
) -> Option<usize> {
    let (open, _) = block_comment?;
    starts_with(chars, i, open).then_some(open.len())
}

/// When the scanner is already inside a string literal or block comment, consume the character at
/// `i`, update the running state/mask, and return how many chars were advanced. Returns `None`
/// when the scanner is not currently inside any such span.
fn advance_inside_span(
    chars: &[char],
    i: usize,
    config: &ScanConfig,
    facts: &mut LineFacts,
    in_block_comment: &mut bool,
    string_state: &mut Option<StringState>,
    code: &mut Vec<u8>,
) -> Option<usize> {
    // Inside a string literal — string content is not code, so blank it out of the mask.
    if let Some(state) = *string_state {
        facts.has_code = true;
        let (new_state, advance) = process_string_char(state, chars, i);
        *string_state = new_state;
        blank_mask(code, advance);
        return Some(advance);
    }

    // Inside a block comment — advance until the closing delimiter.
    if *in_block_comment {
        facts.has_multi_comment = true;
        let advance = step_through_block_comment(chars, i, config.block_comment, in_block_comment);
        blank_mask(code, advance);
        return Some(advance);
    }

    None
}

/// Scan a single physical line and update `facts`, `in_block_comment`, and `string_state`.
///
/// Returns `true` when the caller should break out of the per-line loop early (line comment hit).
fn scan_line(
    chars: &[char],
    config: &ScanConfig,
    facts: &mut LineFacts,
    in_block_comment: &mut bool,
    string_state: &mut Option<StringState>,
    code: &mut Vec<u8>,
) {
    let mut i = 0usize;
    while i < chars.len() {
        // Already inside a string literal or block comment — advance until its closing delimiter.
        if let Some(advance) = advance_inside_span(
            chars,
            i,
            config,
            facts,
            in_block_comment,
            string_state,
            code,
        ) {
            i += advance;
            continue;
        }

        // Whitespace outside any string/comment — preserve as a boundary in the mask.
        if chars[i].is_whitespace() {
            code.push(b' ');
            i += 1;
            continue;
        }

        // Attempt to open a string literal — the opening delimiter is not code.
        if let Some((new_state, advance)) = try_open_string(chars, i, config) {
            facts.has_code = true;
            *string_state = Some(new_state);
            blank_mask(code, advance);
            i += advance;
            continue;
        }

        // Attempt to open a block comment.
        if let Some(advance) = try_open_block_comment(chars, i, config.block_comment) {
            facts.has_multi_comment = true;
            *in_block_comment = true;
            blank_mask(code, advance);
            i += advance;
            continue;
        }

        // Line comment — rest of the line is a comment; stop scanning.
        if config
            .line_comments
            .iter()
            .any(|prefix| starts_with(chars, i, prefix))
        {
            facts.has_single_comment = true;
            break;
        }

        // Plain code character — copy it into the mask (ASCII bytes only; branch keywords are
        // ASCII, so non-ASCII code is blanked without affecting the count).
        facts.has_code = true;
        let ch = chars[i];
        code.push(if ch.is_ascii() { ch as u8 } else { b' ' });
        i += 1;
    }
}

/// Append `n` blank (space) bytes to the code mask, preserving positions/word boundaries while
/// excluding non-code (string/comment) regions from branch counting.
fn blank_mask(code: &mut Vec<u8>, n: usize) {
    code.resize(code.len() + n, b' ');
}

/// Apply IEEE 1045-1992 §4.2 preprocessor-directive tracking and continuation-line merging,
/// then emit the finalized `LineFacts` for this physical line.
///
/// Returns `None` when the line is part of a continuation sequence and should be deferred.
fn finalize_line_facts(
    facts: LineFacts,
    trimmed: &str,
    raw: &mut RawLineCounts,
    ieee: IeeeFlags,
    in_block_comment: bool,
    string_state: Option<StringState>,
    pending_continuation: &mut Option<LineFacts>,
) -> Option<LineFacts> {
    // IEEE 1045-1992 §4.2: track preprocessor/compiler directive lines (C/C++/ObjC).
    // A directive line is a pure code line (no comment on the same physical line) whose
    // trimmed content starts with '#'.
    if ieee.has_preprocessor_directives
        && facts.has_code
        && !facts.has_single_comment
        && !facts.has_multi_comment
        && trimmed.starts_with('#')
    {
        raw.compiler_directive_lines += 1;
    }

    // IEEE 1045-1992 continuation-line handling.
    // A line is a continuation starter when it ends with '\' outside any comment or string.
    let is_continuation = ieee.collapse_continuation_lines
        && !in_block_comment
        && string_state.is_none()
        && trimmed.ends_with('\\');

    if is_continuation {
        let pending = pending_continuation.get_or_insert_with(LineFacts::default);
        pending.has_code |= facts.has_code;
        pending.has_single_comment |= facts.has_single_comment;
        pending.has_multi_comment |= facts.has_multi_comment;
        pending.has_docstring |= facts.has_docstring;
        return None; // defer classification until the sequence ends
    }

    // Merge any accumulated continuation facts into the final line.
    let emit = if let Some(pending) = pending_continuation.take() {
        LineFacts {
            has_code: pending.has_code | facts.has_code,
            has_single_comment: pending.has_single_comment | facts.has_single_comment,
            has_multi_comment: pending.has_multi_comment | facts.has_multi_comment,
            has_docstring: pending.has_docstring | facts.has_docstring,
        }
    } else {
        facts
    };
    Some(emit)
}

/// Scan and classify one physical line, updating all running state in place.
///
/// Pre-classified lines (present in `config.skip_lines`) are counted as docstring-comment
/// lines and returned early without further analysis.
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::many_single_char_names)] // destructuring return from count_symbols; names match field roles
fn process_physical_line(
    line: &str,
    line_idx: usize,
    config: &ScanConfig,
    raw: &mut RawLineCounts,
    in_block_comment: &mut bool,
    string_state: &mut Option<StringState>,
    pending_continuation: &mut Option<LineFacts>,
    ieee: IeeeFlags,
    scope: &mut CScopeState,
) {
    raw.total_physical_lines += 1;

    if config.skip_lines.contains(&line_idx) {
        raw.docstring_comment_lines += 1;
        return;
    }

    let trimmed = line.trim();
    let mut facts = LineFacts::default();

    // IEEE 1045-1992: blank lines inside block comments are comment lines by default.
    // When blank_in_block_comment_as_comment is false, blank lines keep their blank
    // classification even while inside a block comment.
    if *in_block_comment && (ieee.blank_in_block_comment_as_comment || !trimmed.is_empty()) {
        facts.has_multi_comment = true;
    }

    let chars: Vec<char> = line.chars().collect();
    // `code_mask` receives only the line's actual code bytes; string-literal and comment
    // regions are blanked to spaces (positions preserved for word-boundary matching) so that
    // branch keywords embedded in string constants — e.g. `&&`, `||`, `?`, `=>` inside an
    // HTML/JS template literal — are not miscounted as control-flow branches.
    let mut code_mask: Vec<u8> = Vec::with_capacity(chars.len());
    scan_line(
        &chars,
        config,
        &mut facts,
        in_block_comment,
        string_state,
        &mut code_mask,
    );

    let Some(emit) = finalize_line_facts(
        facts,
        trimmed,
        raw,
        ieee,
        *in_block_comment,
        *string_state,
        pending_continuation,
    ) else {
        return;
    };

    classify_line(raw, &emit, trimmed);

    if emit.has_code {
        accumulate_code_line(raw, config, trimmed, scope, &code_mask);
    }
}

/// Accumulate all per-line code metrics (symbols, C/C++ scope breakdown, cyclomatic complexity,
/// logical SLOC, and the ULOC hash) for a physical line already classified as containing code.
#[allow(clippy::many_single_char_names)] // destructuring return from count_symbols; names match roles
fn accumulate_code_line(
    raw: &mut RawLineCounts,
    config: &ScanConfig,
    trimmed: &str,
    scope: &mut CScopeState,
    code_mask: &[u8],
) {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let (f, c, v, i, t, a, s) = count_symbols(&config.symbol_patterns, trimmed);
    raw.functions += f;
    raw.classes += c;
    raw.variables += v;
    raw.imports += i;
    raw.test_count += t;
    raw.test_assertion_count += a;
    raw.test_suite_count += s;

    // C/C++ only: split variables by scope (member/local/global), count object-like macro
    // constants, and advance the brace-scope tracker. Gated on the C/C++ marker (non-empty
    // `functions_prefix_paren`); other languages leave the breakdown fields at zero.
    if !config.symbol_patterns.functions_prefix_paren.is_empty() {
        accumulate_c_family(raw, v, trimmed, scope);
    }

    // Cyclomatic complexity: count branch decision keywords in real code only (the
    // masked line excludes string-literal and comment content).
    raw.cyclomatic_complexity += count_branch_in_line(code_mask, config.branch_keywords);

    // Logical SLOC (language-specific strategy).
    accumulate_lsloc(raw, trimmed, config.lsloc_strategy);

    // ULOC: hash each trimmed code line for cross-file unique-line counting.
    let mut h = DefaultHasher::new();
    trimmed.hash(&mut h);
    raw.code_line_hashes.push(h.finish());
}

/// C/C++ per-line breakdown: bucket a single variable declaration by enclosing scope, count
/// object-like macro constants, and advance the brace-scope tracker.
fn accumulate_c_family(
    raw: &mut RawLineCounts,
    var_count: u64,
    trimmed: &str,
    scope: &mut CScopeState,
) {
    if var_count == 1 {
        match scope.current_var_kind() {
            VarKind::Member => raw.variables_member += 1,
            VarKind::Local => raw.variables_local += 1,
            VarKind::Global => raw.variables_global += 1,
        }
    }
    if is_object_like_macro(trimmed) {
        raw.macro_definitions += 1;
    }
    scope.update(trimmed);
}

/// Apply the language-specific logical-SLOC counting strategy for one code line.
fn accumulate_lsloc(raw: &mut RawLineCounts, trimmed: &str, strategy: LslocStrategy) {
    match strategy {
        LslocStrategy::Semicolons => {
            let semi =
                u32::try_from(trimmed.bytes().filter(|&b| b == b';').count()).unwrap_or(u32::MAX);
            *raw.lsloc.get_or_insert(0) += semi;
        }
        LslocStrategy::NonContinuationNewlines => {
            let cont = trimmed.ends_with('\\')
                || trimmed.ends_with(',')
                || trimmed.ends_with('(')
                || trimmed.ends_with('[')
                || trimmed.ends_with('{');
            if !cont {
                *raw.lsloc.get_or_insert(0) += 1;
            }
        }
        LslocStrategy::Unsupported => {}
    }
}

#[allow(clippy::needless_pass_by_value)]
fn analyze_generic(text: &str, config: ScanConfig, ieee: IeeeFlags) -> RawFileAnalysis {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split_terminator('\n').collect();

    let mut raw = RawLineCounts::default();
    let mut warnings = Vec::new();

    let mut in_block_comment = false;
    let mut string_state: Option<StringState> = None;
    // IEEE continuation-line state: accumulates facts across a backslash-continued sequence.
    let mut pending_continuation: Option<LineFacts> = None;
    // C/C++ brace-scope tracker for member/local/global variable classification.
    let mut scope = CScopeState::default();

    for (line_idx, line) in lines.iter().enumerate() {
        process_physical_line(
            line,
            line_idx,
            &config,
            &mut raw,
            &mut in_block_comment,
            &mut string_state,
            &mut pending_continuation,
            ieee,
            &mut scope,
        );
    }

    // Flush any pending continuation that reaches end-of-file without a closing line.
    if let Some(pending) = pending_continuation.take() {
        classify_line(&mut raw, &pending, "");
    }

    if in_block_comment {
        warnings.push("unclosed block comment detected; result is best effort".into());
    }
    if string_state.is_some() {
        warnings.push("unclosed string literal detected; result is best effort".into());
    }

    RawFileAnalysis {
        raw,
        parse_mode: if warnings.is_empty() {
            ParseMode::Lexical
        } else {
            ParseMode::LexicalBestEffort
        },
        warnings,
        style_analysis: None,
    }
}

/// Coarse per-physical-line classification used by the code-ownership attribution pass in
/// `sloc-core`. Deliberately three-way (blame is physical-line based, and the ownership view
/// only needs code / comment / blank) so that per-author tallies sum exactly to a file's
/// physical line count. This is *not* the IEEE policy classifier — it does not collapse
/// continuation lines or split the fine-grained mixed/docstring buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineCategory {
    Code,
    Comment,
    Blank,
}

/// Classify every physical line of `text` (for `language`) into exactly one [`LineCategory`],
/// returning one entry per physical line in source order. Reuses the same lexical scanner as
/// [`analyze_text`] so block-comment / string state is tracked identically, but assigns a
/// category to each physical line without continuation collapsing — keeping the result aligned
/// one-to-one with `git blame`'s per-line output. Blank lines inside a block comment are
/// reported as `Comment`, matching the IEEE default.
pub fn classify_physical_lines(language: Language, text: &str) -> Vec<LineCategory> {
    #[cfg(feature = "tree-sitter")]
    let _ = (); // classification always uses the lexical scanner for stable line alignment.

    let (mut config, _has_preprocessor) = language_scan_config(language);
    if language == Language::Python {
        config.skip_lines = detect_python_docstring_lines(text);
    }

    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split_terminator('\n').collect();

    let mut out = Vec::with_capacity(lines.len());
    let mut in_block_comment = false;
    let mut string_state: Option<StringState> = None;

    for (idx, line) in lines.iter().enumerate() {
        if config.skip_lines.contains(&idx) {
            out.push(LineCategory::Comment); // pre-detected docstring line
            continue;
        }

        let trimmed = line.trim();
        let mut facts = LineFacts::default();
        let opened_in_block = in_block_comment;
        if in_block_comment {
            facts.has_multi_comment = true;
        }

        let chars: Vec<char> = line.chars().collect();
        let mut code_mask: Vec<u8> = Vec::with_capacity(chars.len());
        scan_line(
            &chars,
            &config,
            &mut facts,
            &mut in_block_comment,
            &mut string_state,
            &mut code_mask,
        );

        let category = if facts.has_code {
            LineCategory::Code
        } else if facts.has_single_comment || facts.has_multi_comment || facts.has_docstring {
            LineCategory::Comment
        } else if trimmed.is_empty() {
            // A blank line spanned by an open block comment counts as a comment line.
            if opened_in_block {
                LineCategory::Comment
            } else {
                LineCategory::Blank
            }
        } else {
            // Non-empty, non-comment, non-code (rare "skipped/unknown") — attribute as code so
            // ownership totals never silently drop physical lines.
            LineCategory::Code
        };
        out.push(category);
    }

    out
}

const fn classify_line(raw: &mut RawLineCounts, facts: &LineFacts, trimmed: &str) {
    if facts.has_docstring {
        raw.docstring_comment_lines += 1;
    } else if !facts.has_code
        && !facts.has_single_comment
        && !facts.has_multi_comment
        && trimmed.is_empty()
    {
        raw.blank_only_lines += 1;
    } else if facts.has_code && facts.has_single_comment {
        raw.mixed_code_single_comment_lines += 1;
    } else if facts.has_code && facts.has_multi_comment {
        raw.mixed_code_multi_comment_lines += 1;
    } else if facts.has_code {
        raw.code_only_lines += 1;
    } else if facts.has_single_comment {
        raw.single_comment_only_lines += 1;
    } else if facts.has_multi_comment {
        raw.multi_comment_only_lines += 1;
    } else if trimmed.is_empty() {
        raw.blank_only_lines += 1;
    } else {
        raw.skipped_unknown_lines += 1;
    }
}

/// True (as 0/1) when `trimmed` starts with any of the prefixes in `pats`.
fn prefix_hit(pats: &[&str], trimmed: &str) -> u64 {
    u64::from(pats.iter().any(|p| trimmed.starts_with(p)))
}

/// Match a return-type-led function prefix (C/C++): prefix AND `(` present AND no `=` sits
/// between the prefix start and the first `(` (guards against `void* p = malloc(n)`).
fn fn_prefix_paren_hit(patterns: &SymbolPatterns, trimmed: &str) -> u64 {
    if patterns.functions_prefix_paren.is_empty() {
        return 0;
    }
    let Some(paren_pos) = trimmed.find('(') else {
        return 0;
    };
    if trimmed[..paren_pos].contains('=') {
        0
    } else {
        prefix_hit(patterns.functions_prefix_paren, trimmed)
    }
}

/// Complement of `functions_prefix_paren`: same type keywords, but triggered when there is no
/// unguarded `(` on the line (i.e. not a function definition).
fn var_prefix_no_paren_hit(patterns: &SymbolPatterns, trimmed: &str) -> u64 {
    if patterns.variables_prefix_no_paren.is_empty()
        || prefix_hit(patterns.variables_prefix_no_paren, trimmed) == 0
    {
        return 0;
    }
    trimmed
        .find('(')
        .map_or(1, |pp| u64::from(trimmed[..pp].contains('=')))
}

/// Statement/expression keywords that can legally precede `(` or a declarator but are NOT a
/// function or variable definition. Used to reject false positives in the C/C++ heuristics.
const C_STMT_KEYWORDS: &[&str] = &[
    "if",
    "for",
    "while",
    "switch",
    "return",
    "catch",
    "sizeof",
    "do",
    "else",
    "case",
    "throw",
    "goto",
    "using",
    "namespace",
    "typedef",
    "friend",
    "decltype",
    "alignof",
    "new",
    "delete",
    "static_assert",
    "template",
    "co_await",
    "co_return",
    "co_yield",
    "assert",
    "default",
    "class",
    "struct",
    "union",
    "enum",
    "public",
    "private",
    "protected",
    "try",
];

/// True when `c` may appear inside a C/C++ return type or declarator (identifier chars, pointer
/// / reference markers, template brackets, scope resolution, qualifiers with spaces).
const fn c_type_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '_' | ':' | '<' | '>' | '*' | '&' | '~' | ' ' | '\t' | ','
        )
}

/// True when `name` (the token immediately before `(` or the assignment/terminator) is a
/// plausible C/C++ identifier — allowing leading `*`/`&`/`~` and a `Scope::` qualifier.
fn c_name_is_identifier(name: &str) -> bool {
    let core = name.trim_start_matches(['*', '&', '~']);
    let seg = core.rsplit("::").next().unwrap_or(core);
    let mut chars = seg.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Heuristic: does `trimmed` look like a C/C++ function definition or prototype?
///
/// Recognises `<return-type> <name>(...)` regardless of the return type, so functions returning
/// user-defined or namespaced types (`std::string foo(...)`, `MyClass bar(...)`) are counted —
/// the fixed keyword list in `functions_prefix_paren` only caught built-in return types. Rejects
/// calls (`foo(x)`, `obj.m(x)`, `std::sort(v)`), control flow (`if (...)`), and initialisers
/// (`T x = f(y)`).
fn looks_like_c_function(trimmed: &str) -> u64 {
    let Some(paren) = trimmed.find('(') else {
        return 0;
    };
    let pre = trimmed[..paren].trim();
    // The segment before `(` must be a clean `<type> <name>` — reject anything containing
    // assignment, statement terminators, member access, indexing, or arithmetic/logical
    // operators, all of which indicate an expression or call rather than a definition.
    if pre.is_empty() || pre.contains("->") || !pre.chars().all(c_type_char_ok) {
        return 0;
    }
    // Need at least "<return type> <name>": two whitespace-separated tokens.
    let mut toks = pre.split_whitespace();
    let Some(first) = toks.next() else {
        return 0;
    };
    if C_STMT_KEYWORDS.contains(&first) || toks.next().is_none() {
        return 0;
    }
    let Some(name) = pre.split_whitespace().next_back() else {
        return 0;
    };
    if !c_name_is_identifier(name) {
        return 0;
    }
    // Disambiguate the "most vexing parse": `T v(expr);` is a variable direct-initialisation, not
    // a function. Only the `;`-terminated form is ambiguous — a definition ends with `{` (or a
    // continued signature). Treat it as a function only when the parentheses hold a parameter
    // list (empty, comma-separated, or containing type markers) rather than a lone value.
    if trimmed.ends_with(';') {
        let args = trimmed[paren + 1..]
            .rsplit_once(')')
            .map_or("", |(a, _)| a)
            .trim();
        let looks_like_params = args.is_empty()
            || args.contains(',')
            || args.contains('&')
            || args.contains('*')
            || args.contains("::")
            || args.split_whitespace().count() >= 2;
        if !looks_like_params {
            return 0;
        }
    }
    1
}

/// Heuristic: does `trimmed` look like a C/C++ variable declaration?
///
/// Recognises `<type> <name>;`, `<type> <name> = …;`, and `<type> <name>{…};` for any type,
/// including user-defined / namespaced / templated types. Rejects function definitions and calls
/// (declarator immediately followed by `(`), labels, control flow, and bare expressions.
fn looks_like_c_variable(trimmed: &str) -> u64 {
    // Locate the first declarator-terminating delimiter. A leading `(` covers both calls
    // (`foo(x);`) and direct-init variables (`std::istringstream ss(s);`); the `<type> <name>`
    // shape test below rejects calls, while `count_symbols` only consults this heuristic when the
    // line was not already classified as a function, so real prototypes are not double-counted.
    let Some(delim_pos) = trimmed.find(['=', ';', '{', '(']) else {
        return 0;
    };
    let head = trimmed[..delim_pos].trim();
    if head.is_empty() || head.contains("->") || !head.chars().all(c_type_char_ok) {
        return 0;
    }
    let mut toks = head.split_whitespace();
    let Some(first) = toks.next() else {
        return 0;
    };
    if C_STMT_KEYWORDS.contains(&first) || toks.next().is_none() {
        return 0;
    }
    let Some(name) = head.split_whitespace().next_back() else {
        return 0;
    };
    u64::from(c_name_is_identifier(name))
}

/// The kind of brace-delimited scope currently open, tracked to classify variable declarations
/// as member / local / global in C and C++.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CScope {
    /// `class` / `struct` / `union` body — declarations inside are member variables.
    Aggregate,
    /// Function body — declarations inside are local variables.
    Function,
    /// `namespace` body — declarations inside are global-scope variables.
    Namespace,
    /// Control / other block (`if`/`for`/`{ … }`) — treated as local (blocks live in functions).
    Block,
}

/// Which bucket a detected variable declaration belongs to, from the enclosing scope.
#[derive(Clone, Copy)]
enum VarKind {
    Member,
    Local,
    Global,
}

/// Best-effort brace-based scope tracker for C/C++, threaded across the lines of one file.
#[derive(Default)]
struct CScopeState {
    stack: Vec<CScope>,
    /// Scope kind established by an opener line whose `{` has not yet appeared (handles
    /// Allman/K&R style where the brace is on the following line).
    pending: Option<CScope>,
}

impl CScopeState {
    /// Classify a variable declaration by the current innermost scope.
    fn current_var_kind(&self) -> VarKind {
        match self.stack.last() {
            Some(CScope::Aggregate) => VarKind::Member,
            Some(CScope::Function | CScope::Block) => VarKind::Local,
            _ => VarKind::Global, // namespace or file scope
        }
    }

    /// Update the brace stack for one C/C++ code line. Braces inside string / char literals and
    /// comments (`//` and same-line `/* … */`) are skipped so they cannot corrupt the stack.
    fn update(&mut self, trimmed: &str) {
        if let Some(kind) = c_line_scope_kind(trimmed) {
            self.pending = Some(kind);
        }
        let bytes = trimmed.as_bytes();
        let mut i = 0;
        let mut in_str: Option<u8> = None;
        while i < bytes.len() {
            // While inside a string/char literal, consume the byte and skip the brace logic.
            if let Some(next) = skip_string_literal(bytes, i, &mut in_str) {
                i = next;
                continue;
            }
            let b = bytes[i];
            match b {
                b'"' | b'\'' => in_str = Some(b),
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => break, // line comment
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                    // Skip a same-line block comment; if unterminated, stop scanning the line.
                    match trimmed[i + 2..].find("*/") {
                        Some(off) => i += 2 + off + 2,
                        None => break,
                    }
                    continue;
                }
                b'{' => self
                    .stack
                    .push(self.pending.take().unwrap_or(CScope::Block)),
                b'}' => {
                    self.stack.pop();
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// While the brace scanner is inside a C/C++ string or char literal, consume the byte at `i` and
/// return the next index to visit, honouring `\` escapes and clearing `in_str` on the closing
/// quote. Returns `None` when the scanner is not currently inside a literal.
fn skip_string_literal(bytes: &[u8], i: usize, in_str: &mut Option<u8>) -> Option<usize> {
    let q = (*in_str)?;
    let b = bytes[i];
    if b == b'\\' {
        return Some(i + 2); // skip escaped character
    }
    if b == q {
        *in_str = None;
    }
    Some(i + 1)
}

/// Determine whether a C/C++ code line opens a named scope, to label the `{` it introduces.
/// Returns `None` for lines that merely declare (`struct Foo f;`), forward-declare (`struct Foo;`),
/// or prototype (`int f(int);`) — none of which open a body.
fn c_line_scope_kind(trimmed: &str) -> Option<CScope> {
    // A `;` with no `{` on the line is a declaration/prototype, not a body opener.
    if trimmed.contains(';') && !trimmed.contains('{') {
        return None;
    }
    if trimmed.starts_with("namespace") {
        return Some(CScope::Namespace);
    }
    if is_c_aggregate_opener(trimmed) {
        return Some(CScope::Aggregate);
    }
    if looks_like_c_function(trimmed) == 1 {
        return Some(CScope::Function);
    }
    None
}

/// True when the line is a `class` / `struct` / `union` body definition (keyword present as a
/// standalone token and no `(`, which would make it a function returning that aggregate type).
fn is_c_aggregate_opener(trimmed: &str) -> bool {
    if trimmed.contains('(') {
        return false;
    }
    trimmed
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|tok| matches!(tok, "class" | "struct" | "union"))
}

/// True when `trimmed` is an object-like preprocessor macro definition (`#define NAME value`).
/// Function-like macros (`#define F(x) …`) and value-less defines (include guards) are excluded.
fn is_object_like_macro(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix('#') else {
        return false;
    };
    let Some(rest) = rest.trim_start().strip_prefix("define") else {
        return false;
    };
    if !rest.starts_with(char::is_whitespace) {
        return false; // `#defineFOO` is not a define
    }
    let rest = rest.trim_start();
    let name_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    if name_end == 0 {
        return false; // no macro name
    }
    let after = &rest[name_end..];
    // Function-like macro: name immediately followed by `(`. Require a non-empty replacement
    // (skips bare include-guard defines like `#define FOO_H`).
    !after.starts_with('(') && !after.trim().is_empty()
}

fn count_symbols(patterns: &SymbolPatterns, trimmed: &str) -> (u64, u64, u64, u64, u64, u64, u64) {
    let hit = |pats: &[&str]| prefix_hit(pats, trimmed);
    // C and C++ are the only languages with a non-empty `functions_prefix_paren` list. For them
    // the generic `looks_like_c_*` heuristics fully replace the fixed keyword lists (which only
    // caught built-in return types like `int`/`void` and mis-fired on prototype continuation
    // lines); every other language keeps its prefix-based detection.
    let c_style = !patterns.functions_prefix_paren.is_empty();
    let fn_extra = if c_style {
        looks_like_c_function(trimmed)
    } else {
        fn_prefix_paren_hit(patterns, trimmed)
    };
    let test_hit = hit(patterns.tests);
    // Lines matching a test pattern count as tests, not as plain functions or classes.
    // This prevents double-counting in Python (`def test_` / `class Test`) and Go
    // (`func Test` / `func Benchmark` / `func Fuzz`) where the same line satisfies both
    // a function/class prefix and a test pattern. Rust is unaffected: `#[test]` is a
    // standalone attribute line; the `fn` declaration on the next line does not match any
    // test pattern and still increments functions correctly.
    let fn_hit = if test_hit == 0 {
        hit(patterns.functions) | fn_extra
    } else {
        0
    };
    let class_hit = if test_hit == 0 {
        hit(patterns.classes)
    } else {
        0
    };
    let var_hit = if c_style {
        // For C/C++, use only the generic heuristic, and only when the line is not already a
        // test, function, or class definition (avoids double-counting).
        if test_hit == 0 && fn_hit == 0 && class_hit == 0 {
            looks_like_c_variable(trimmed)
        } else {
            0
        }
    } else {
        hit(patterns.variables) | var_prefix_no_paren_hit(patterns, trimmed)
    };
    (
        fn_hit,
        class_hit,
        var_hit,
        hit(patterns.imports),
        test_hit,
        hit(patterns.assertions),
        hit(patterns.test_suites),
    )
}

/// True when `line[start..end]` is surrounded by non-identifier characters.
fn is_word_boundary(line: &[u8], start: usize, end: usize) -> bool {
    let before_ok =
        start == 0 || (!line[start - 1].is_ascii_alphanumeric() && line[start - 1] != b'_');
    let after_ok = end >= line.len() || (!line[end].is_ascii_alphanumeric() && line[end] != b'_');
    before_ok && after_ok
}

/// True when `kw_bytes` appears at `line[i..]`, respecting word boundaries when `word_kw` is set.
fn keyword_matches_at(line: &[u8], i: usize, kw_bytes: &[u8], word_kw: bool) -> bool {
    if &line[i..i + kw_bytes.len()] != kw_bytes {
        return false;
    }
    !word_kw || is_word_boundary(line, i, i + kw_bytes.len())
}

/// Count branch keyword occurrences in `line` (ASCII bytes of a trimmed code line).
///
/// Alphabetic keywords are matched word-bounded (not as substrings of longer identifiers).
/// Operator tokens (`||`, `&&`, `?`) are matched as raw substrings.
fn count_branch_in_line(line: &[u8], keywords: &[&str]) -> u32 {
    if keywords.is_empty() || line.is_empty() {
        return 0;
    }
    let mut total = 0u32;
    for &kw in keywords {
        let kw_bytes = kw.as_bytes();
        let word_kw = kw.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_');
        let mut i = 0usize;
        while i + kw_bytes.len() <= line.len() {
            if keyword_matches_at(line, i, kw_bytes, word_kw) {
                total += 1;
                i += kw_bytes.len();
            } else {
                i += 1;
            }
        }
    }
    total
}

fn starts_with(chars: &[char], index: usize, needle: &str) -> bool {
    let Some(tail) = chars.get(index..) else {
        return false;
    };

    for (i, needle_char) in needle.chars().enumerate() {
        if tail.get(i).copied() != Some(needle_char) {
            return false;
        }
    }

    true
}

#[derive(Debug, Clone)]
struct PyContext {
    indent: usize,
    expect_docstring: bool,
}

/// Update `contexts` to pop any scopes that the current `indent` has outdented past.
fn py_pop_outdented_contexts(contexts: &mut Vec<PyContext>, indent: usize) {
    while contexts.len() > 1 && indent < contexts.last().map_or(0, |c| c.indent) {
        contexts.pop();
    }
}

/// Handle `pending_block_indent` transition: push a new docstring-expecting context when we
/// detect the first indented line of a new block, or cancel the pending state otherwise.
fn py_handle_pending_indent(
    pending_block_indent: &mut Option<usize>,
    contexts: &mut Vec<PyContext>,
    indent: usize,
    trimmed: &str,
) {
    let Some(base_indent) = *pending_block_indent else {
        return;
    };
    if indent > base_indent {
        contexts.push(PyContext {
            indent,
            expect_docstring: true,
        });
        *pending_block_indent = None;
    } else if !trimmed.starts_with('@') {
        *pending_block_indent = None;
    }
}

/// Check whether the current line is a docstring opener in the current context.
///
/// If it is, records the line, adjusts `ctx.expect_docstring`, and optionally sets
/// `active_docstring` for multi-line docstrings. Returns `true` when the caller should
/// `continue` to the next line.
fn py_try_record_docstring(
    ctx: &mut PyContext,
    trimmed: &str,
    idx: usize,
    docstring_lines: &mut HashSet<usize>,
    active_docstring: &mut Option<(&'static str, usize)>,
) -> bool {
    if !ctx.expect_docstring {
        return false;
    }
    if let Some(delim) = docstring_delimiter(trimmed) {
        docstring_lines.insert(idx);
        ctx.expect_docstring = false;
        if !closes_triple_docstring(trimmed, delim, true) {
            *active_docstring = Some((delim, idx));
        }
        return true;
    }
    ctx.expect_docstring = false;
    false
}

/// Advance through an active multi-line docstring: marks the current line and clears
/// `active_docstring` when the closing delimiter is found. Returns `true` when the caller
/// should `continue` to the next line (i.e. we were inside a docstring).
fn track_active_docstring(
    active_docstring: &mut Option<(&'static str, usize)>,
    docstring_lines: &mut HashSet<usize>,
    idx: usize,
    trimmed: &str,
) -> bool {
    let Some((delim, start_line)) = *active_docstring else {
        return false;
    };
    docstring_lines.insert(idx);
    if closes_triple_docstring(trimmed, delim, idx == start_line) {
        *active_docstring = None;
    }
    true
}

/// Attempt to record a docstring opener using the top of the context stack.
/// Returns `true` when the caller should `continue` to the next line.
fn try_record_docstring_if_context(
    contexts: &mut [PyContext],
    trimmed: &str,
    idx: usize,
    docstring_lines: &mut HashSet<usize>,
    active_docstring: &mut Option<(&'static str, usize)>,
) -> bool {
    let Some(ctx) = contexts.last_mut() else {
        return false;
    };
    py_try_record_docstring(ctx, trimmed, idx, docstring_lines, active_docstring)
}

/// If an unclosed docstring is still active at end-of-file, mark all remaining lines.
fn mark_unclosed_docstring_lines(
    active_docstring: Option<&(&'static str, usize)>,
    docstring_lines: &mut HashSet<usize>,
    num_lines: usize,
) {
    if let Some(&(_, start_line)) = active_docstring {
        for idx in start_line..num_lines {
            docstring_lines.insert(idx);
        }
    }
}

fn detect_python_docstring_lines(text: &str) -> HashSet<usize> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split_terminator('\n').collect();

    let mut docstring_lines = HashSet::new();
    let mut contexts = vec![PyContext {
        indent: 0,
        expect_docstring: true,
    }];
    let mut pending_block_indent: Option<usize> = None;
    let mut active_docstring: Option<(&'static str, usize)> = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let indent = leading_indent(line);

        if track_active_docstring(&mut active_docstring, &mut docstring_lines, idx, trimmed) {
            continue;
        }

        // Blank lines and comment lines don't affect docstring detection.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        py_pop_outdented_contexts(&mut contexts, indent);
        py_handle_pending_indent(&mut pending_block_indent, &mut contexts, indent, trimmed);

        if try_record_docstring_if_context(
            &mut contexts,
            trimmed,
            idx,
            &mut docstring_lines,
            &mut active_docstring,
        ) {
            continue;
        }

        if is_python_block_header(trimmed) {
            pending_block_indent = Some(indent);
        }
    }

    mark_unclosed_docstring_lines(active_docstring.as_ref(), &mut docstring_lines, lines.len());

    docstring_lines
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

fn is_python_block_header(trimmed: &str) -> bool {
    (trimmed.starts_with("def ")
        || trimmed.starts_with("async def ")
        || trimmed.starts_with("class "))
        && trimmed.ends_with(':')
}

fn docstring_delimiter(trimmed: &str) -> Option<&'static str> {
    let mut idx = 0usize;
    let bytes = trimmed.as_bytes();
    while idx < bytes.len() {
        let c = bytes[idx] as char;
        if matches!(c, 'r' | 'R' | 'u' | 'U' | 'b' | 'B' | 'f' | 'F') {
            idx += 1;
            continue;
        }
        break;
    }

    let rest = &trimmed[idx..];
    if rest.starts_with("\"\"\"") {
        Some("\"\"\"")
    } else if rest.starts_with("'''") {
        Some("'''")
    } else {
        None
    }
}

fn closes_triple_docstring(trimmed: &str, delim: &str, same_line_as_start: bool) -> bool {
    let mut occurrences = 0usize;
    let mut search = trimmed;
    while let Some(index) = search.find(delim) {
        occurrences += 1;
        search = &search[index + delim.len()..];
    }

    if same_line_as_start {
        occurrences >= 2
    } else {
        occurrences >= 1
    }
}

/// Tree-sitter-backed adapters (compiled only when the `tree-sitter` feature is enabled).
///
/// When parsing succeeds the result is used directly; on any failure the caller falls back
/// to the lexical state machine.
#[cfg(feature = "tree-sitter")]
pub mod ts {
    use tree_sitter::Node;

    use super::{ParseMode, RawFileAnalysis, RawLineCounts};

    /// Configuration for which AST node kinds map to symbols in this grammar.
    struct SymbolKinds {
        /// Node kind name for function definitions (e.g. `"function_definition"`).
        function_def: &'static str,
        /// Node kind name for class definitions (e.g. `"class_definition"`).
        class_def: &'static str,
        /// Name field of a function node that, when it starts with this prefix, marks a test.
        /// Empty string disables test-prefix detection.
        test_fn_prefix: &'static str,
        /// Name field of a class node that, when it starts with this prefix, marks a test.
        /// Empty string disables test-prefix detection.
        test_class_prefix: &'static str,
        /// When non-empty, `call` nodes whose `function` is an `attribute` access and whose
        /// attribute identifier starts with this prefix are counted as test assertions.
        /// Used for Python `self.assertXxx(...)` detection.
        assertion_attr_prefix: &'static str,
    }

    impl SymbolKinds {
        const fn none() -> Self {
            Self {
                function_def: "",
                class_def: "",
                test_fn_prefix: "",
                test_class_prefix: "",
                assertion_attr_prefix: "",
            }
        }
    }

    /// Classify every line of `text` using a tree-sitter grammar.
    ///
    /// `comment_node_kinds` — node type names that represent comments in this grammar
    /// `docstring_stmt_kind` — optional parent node type whose direct `string` child is a docstring
    /// `symbols` — AST node kinds used to populate symbol counters
    fn analyze_lines(
        text: &str,
        ts_language: &tree_sitter::Language,
        comment_node_kinds: &[&str],
        docstring_stmt_kind: Option<&str>,
        symbols: &SymbolKinds,
    ) -> Option<RawFileAnalysis> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(ts_language).ok()?;
        let tree = parser.parse(text, None)?;

        let lines: Vec<&str> = text.split_terminator('\n').collect();
        let n = lines.len();

        let mut has_code = vec![false; n];
        let mut has_comment = vec![false; n];
        let mut comment_is_block = vec![false; n];
        let mut has_docstring = vec![false; n];

        // Walk every node in the tree and mark line arrays.
        let mut ctx = VisitCtx {
            source: text.as_bytes(),
            comment_kinds: comment_node_kinds,
            docstring_stmt_kind,
            has_code: &mut has_code,
            has_comment: &mut has_comment,
            comment_is_block: &mut comment_is_block,
            has_docstring: &mut has_docstring,
        };
        visit(tree.root_node(), &mut ctx);

        let mut raw = RawLineCounts::default();
        classify_ts_lines(
            &lines,
            &has_code,
            &has_comment,
            &comment_is_block,
            &has_docstring,
            &mut raw,
        );

        // Symbol counting: walk the AST a second time to collect function/class/test counts.
        if !symbols.function_def.is_empty() || !symbols.class_def.is_empty() {
            count_symbols(tree.root_node(), text.as_bytes(), symbols, &mut raw);
        }

        Some(RawFileAnalysis {
            raw,
            parse_mode: ParseMode::TreeSitter,
            warnings: Vec::new(),
            style_analysis: None,
        })
    }

    /// Recurse into every direct child of `node`.
    fn recurse_children(node: Node, source: &[u8], kinds: &SymbolKinds, raw: &mut RawLineCounts) {
        for i in 0..node.child_count() {
            #[allow(clippy::cast_possible_truncation)]
            if let Some(child) = node.child(i as u32) {
                count_symbols(child, source, kinds, raw);
            }
        }
    }

    /// Handle a function-definition node. Returns `true` if the node matched.
    fn try_count_function(
        node: Node,
        source: &[u8],
        kinds: &SymbolKinds,
        raw: &mut RawLineCounts,
    ) -> bool {
        if kinds.function_def.is_empty() || node.kind() != kinds.function_def {
            return false;
        }
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .unwrap_or("");
        if !kinds.test_fn_prefix.is_empty() && name.starts_with(kinds.test_fn_prefix) {
            raw.test_count += 1;
        } else {
            raw.functions += 1;
        }
        recurse_children(node, source, kinds, raw);
        true
    }

    /// Handle a class-definition node. Returns `true` if the node matched.
    fn try_count_class(
        node: Node,
        source: &[u8],
        kinds: &SymbolKinds,
        raw: &mut RawLineCounts,
    ) -> bool {
        if kinds.class_def.is_empty() || node.kind() != kinds.class_def {
            return false;
        }
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .unwrap_or("");
        if !kinds.test_class_prefix.is_empty() && name.starts_with(kinds.test_class_prefix) {
            raw.test_count += 1;
        } else {
            raw.classes += 1;
        }
        recurse_children(node, source, kinds, raw);
        true
    }

    /// Handle an assertion call node. Returns `true` if the node matched (skips recursion
    /// into arguments, preserving "don't double-count test bodies" semantics).
    fn try_count_assertion(
        node: Node,
        source: &[u8],
        kinds: &SymbolKinds,
        raw: &mut RawLineCounts,
    ) -> bool {
        if kinds.assertion_attr_prefix.is_empty() || node.kind() != "call" {
            return false;
        }
        let Some(func) = node.child_by_field_name("function") else {
            return false;
        };
        if func.kind() != "attribute" {
            return false;
        }
        let attr_text = func
            .child_by_field_name("attribute")
            .and_then(|n| n.utf8_text(source).ok())
            .unwrap_or("");
        if !attr_text.starts_with(kinds.assertion_attr_prefix) {
            return false;
        }
        raw.test_assertion_count += 1;
        true
    }

    /// Walk the AST and populate `raw.functions`, `raw.classes`, `raw.test_count`,
    /// and `raw.test_assertion_count`.
    fn count_symbols(node: Node, source: &[u8], kinds: &SymbolKinds, raw: &mut RawLineCounts) {
        if try_count_function(node, source, kinds, raw) {
            return;
        }
        if try_count_class(node, source, kinds, raw) {
            return;
        }
        if try_count_assertion(node, source, kinds, raw) {
            return;
        }
        recurse_children(node, source, kinds, raw);
    }

    /// Flags describing what kinds of content appear on a single line.
    // Four bools are the natural representation for these four independent properties.
    #[allow(clippy::struct_excessive_bools)]
    #[derive(Clone, Copy)]
    struct TsLineFlags {
        has_code: bool,
        has_comment: bool,
        comment_is_block: bool,
        has_docstring: bool,
    }

    /// Classify a single tree-sitter-annotated line and accumulate into `raw`.
    const fn classify_ts_line(trimmed: &str, flags: TsLineFlags, raw: &mut RawLineCounts) {
        if trimmed.is_empty() {
            raw.blank_only_lines += 1;
        } else if flags.has_docstring && !flags.has_code {
            raw.docstring_comment_lines += 1;
        } else if flags.has_code && flags.has_comment {
            // Classify the mixed line as single or multi based on what kind of comment is on it.
            if flags.comment_is_block {
                raw.mixed_code_multi_comment_lines += 1;
            } else {
                raw.mixed_code_single_comment_lines += 1;
            }
        } else if flags.has_comment {
            if flags.comment_is_block {
                raw.multi_comment_only_lines += 1;
            } else {
                raw.single_comment_only_lines += 1;
            }
        } else {
            raw.code_only_lines += 1;
        }
    }

    /// Classify each tree-sitter-annotated line and accumulate counts into `raw`.
    fn classify_ts_lines(
        lines: &[&str],
        has_code: &[bool],
        has_comment: &[bool],
        comment_is_block: &[bool],
        has_docstring: &[bool],
        raw: &mut RawLineCounts,
    ) {
        for i in 0..lines.len() {
            raw.total_physical_lines += 1;
            classify_ts_line(
                lines[i].trim(),
                TsLineFlags {
                    has_code: has_code[i],
                    has_comment: has_comment[i],
                    comment_is_block: comment_is_block[i],
                    has_docstring: has_docstring[i],
                },
                raw,
            );
        }
    }

    struct VisitCtx<'a> {
        source: &'a [u8],
        comment_kinds: &'a [&'a str],
        docstring_stmt_kind: Option<&'a str>,
        has_code: &'a mut Vec<bool>,
        has_comment: &'a mut Vec<bool>,
        comment_is_block: &'a mut Vec<bool>,
        has_docstring: &'a mut Vec<bool>,
    }

    /// Mark all rows of a comment node and detect whether it is a block comment.
    fn visit_comment_node(node: Node, ctx: &mut VisitCtx<'_>) {
        let start_row = node.start_position().row;
        let end_row = node.end_position().row;
        let first_two = node
            .utf8_text(ctx.source)
            .unwrap_or("")
            .get(..2)
            .unwrap_or("");
        let is_block = first_two == "/*" || first_two == "<#";
        for row in start_row..=end_row {
            if row < ctx.has_comment.len() {
                ctx.has_comment[row] = true;
                if is_block {
                    ctx.comment_is_block[row] = true;
                }
            }
        }
    }

    /// If `node` is an `expression_statement` whose sole named child is a string literal,
    /// mark those rows as docstring and return `true`.
    fn visit_maybe_docstring(node: Node, kind: &str, ctx: &mut VisitCtx<'_>) -> bool {
        let Some(stmt_kind) = ctx.docstring_stmt_kind else {
            return false;
        };
        if kind != stmt_kind || node.named_child_count() != 1 {
            return false;
        }
        let Some(child) = node.named_child(0) else {
            return false;
        };
        if child.kind() != "string" {
            return false;
        }
        let child_start = child.start_position().row;
        let child_end = child.end_position().row;
        for row in child_start..=child_end {
            if row < ctx.has_docstring.len() {
                ctx.has_docstring[row] = true;
            }
        }
        true
    }

    /// Mark all rows of a leaf (non-comment, non-extra) node as code.
    fn visit_leaf_code(node: Node, ctx: &mut VisitCtx<'_>) {
        let start_row = node.start_position().row;
        let end_row = node.end_position().row;
        for row in start_row..=end_row {
            if row < ctx.has_code.len() {
                ctx.has_code[row] = true;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn visit(node: Node, ctx: &mut VisitCtx<'_>) {
        let kind = node.kind();

        // Comment node — mark rows as comment, detect block vs. line comment.
        if ctx.comment_kinds.contains(&kind) {
            visit_comment_node(node, ctx);
            return;
        }

        // Python docstring: expression_statement whose only named child is a string literal.
        if visit_maybe_docstring(node, kind, ctx) {
            return;
        }

        // Leaf non-comment node: mark as code.
        if node.child_count() == 0 && !node.is_extra() {
            visit_leaf_code(node, ctx);
            return;
        }

        for i in 0..node.child_count() {
            #[allow(clippy::cast_possible_truncation)]
            // child_count bounded by tree-sitter u32 capacity
            if let Some(child) = node.child(i as u32) {
                visit(child, ctx);
            }
        }
    }

    const C_SYMBOLS: SymbolKinds = SymbolKinds::none();

    const PYTHON_SYMBOLS: SymbolKinds = SymbolKinds {
        function_def: "function_definition",
        class_def: "class_definition",
        test_fn_prefix: "test_",
        test_class_prefix: "Test",
        assertion_attr_prefix: "assert",
    };

    /// Parse C or C++ source with tree-sitter-c.
    #[must_use]
    pub fn analyze_c(text: &str) -> Option<RawFileAnalysis> {
        let lang: tree_sitter::Language = tree_sitter_c::LANGUAGE.into();
        analyze_lines(text, &lang, &["comment"], None, &C_SYMBOLS)
    }

    /// Parse Python source with tree-sitter-python.
    #[must_use]
    pub fn analyze_python(text: &str) -> Option<RawFileAnalysis> {
        let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
        analyze_lines(
            text,
            &lang,
            &["comment"],
            Some("expression_statement"),
            &PYTHON_SYMBOLS,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_docstrings_are_separated() {
        let input = r#""""module docs"""


def fn_a():
    """function docs"""
    value = 1  # trailing comment
    return value
"#;

        let result = analyze_text(Language::Python, input, AnalysisOptions::default());
        assert_eq!(result.raw.docstring_comment_lines, 2);
        assert_eq!(result.raw.mixed_code_single_comment_lines, 1);
        assert_eq!(result.raw.code_only_lines, 2);
    }

    #[test]
    fn c_style_mixed_lines_are_captured() {
        let input = "int x = 1; // note\n/* block */\n";
        let result = analyze_text(Language::C, input, AnalysisOptions::default());
        assert_eq!(result.raw.mixed_code_single_comment_lines, 1);
        assert_eq!(result.raw.multi_comment_only_lines, 1);
    }

    #[test]
    fn branch_keywords_inside_strings_are_not_counted() {
        // Branch operators inside a normal string literal are not control flow → 0.
        let s = analyze_text(
            Language::Rust,
            "let s = \"if a && b || c ? d : e\";\n",
            AnalysisOptions::default(),
        );
        assert_eq!(s.raw.cyclomatic_complexity, 0);

        // Same, inside a Rust raw string whose inner `\"` must not end the literal
        // (the HTML/JS-template shape that previously inflated cyclomatic complexity).
        let raw = analyze_text(
            Language::Rust,
            "let h = r#\"<a href=\"x\">a && b ? c : d</a>\"#;\n",
            AnalysisOptions::default(),
        );
        assert_eq!(raw.raw.cyclomatic_complexity, 0);

        // Real control flow outside string literals is still counted.
        let code = analyze_text(
            Language::Rust,
            "if a && b { c } else { d }\n",
            AnalysisOptions::default(),
        );
        assert!(code.raw.cyclomatic_complexity >= 2);
    }

    #[test]
    fn multiline_raw_string_does_not_swallow_following_code() {
        // Regression: a multi-line r##"..."## template with inner quotes must close cleanly so
        // the code after it stays classified as code (it was previously swallowed as string).
        let input = concat!(
            "let cfg = r##\"\n",
            "# looks like a comment but is string content\n",
            "key = \"value with \"\" inner quotes\"\n",
            "\"##;\n",
            "let x = 1;\n",
        );
        let r = analyze_text(Language::Rust, input, AnalysisOptions::default());
        assert!(
            r.raw.code_only_lines >= 1,
            "code after the raw string was swallowed"
        );
        assert_eq!(r.raw.single_comment_only_lines, 0);
        assert_eq!(r.raw.cyclomatic_complexity, 0);
    }

    #[test]
    fn detect_language_by_shebang() {
        let language = detect_language(
            Path::new("script"),
            Some("#!/usr/bin/env bash"),
            &BTreeMap::new(),
            true,
        );
        assert_eq!(language, Some(Language::Shell));
    }

    // ── count_symbols: no double-counting of test functions ──────────────────

    fn sym(lang: Language, line: &str) -> (u64, u64, u64, u64, u64, u64, u64) {
        let result = analyze_text(lang, &format!("{line}\n"), AnalysisOptions::default());
        let r = &result.raw;
        (
            r.functions,
            r.classes,
            r.variables,
            r.imports,
            r.test_count,
            r.test_assertion_count,
            r.test_suite_count,
        )
    }

    #[test]
    fn python_test_fn_not_double_counted() {
        // def test_ lines count as tests only, NOT as functions
        let (f, c, _, _, t, _, _) = sym(Language::Python, "def test_foo():");
        assert_eq!(f, 0, "test fn must not also increment functions");
        assert_eq!(t, 1, "must be counted as a test");
        assert_eq!(c, 0);
    }

    #[test]
    fn python_test_class_not_double_counted() {
        // class Test* lines count as tests only, NOT as classes
        let (f, c, _, _, t, _, _) = sym(Language::Python, "class TestFoo:");
        assert_eq!(c, 0, "test class must not also increment classes");
        assert_eq!(t, 1, "must be counted as a test");
        assert_eq!(f, 0);
    }

    #[test]
    fn python_regular_fn_counts_as_function() {
        let (f, c, _, _, t, _, _) = sym(Language::Python, "def regular():");
        assert_eq!(f, 1, "regular function must be counted");
        assert_eq!(t, 0);
        assert_eq!(c, 0);
    }

    #[test]
    fn python_regular_class_counts_as_class() {
        let (f, c, _, _, t, _, _) = sym(Language::Python, "class Regular:");
        assert_eq!(c, 1, "regular class must be counted");
        assert_eq!(t, 0);
        assert_eq!(f, 0);
    }

    #[test]
    fn go_test_fn_not_double_counted() {
        let (f, _, _, _, t, _, _) = sym(Language::Go, "func TestFoo(t *testing.T) {");
        assert_eq!(f, 0, "Go test func must not also increment functions");
        assert_eq!(t, 1, "must be counted as a test");
    }

    #[test]
    fn go_benchmark_fn_not_double_counted() {
        let (f, _, _, _, t, _, _) = sym(Language::Go, "func BenchmarkBar(b *testing.B) {");
        assert_eq!(f, 0, "Go benchmark func must not also increment functions");
        assert_eq!(t, 1, "must be counted as a test");
    }

    #[test]
    fn go_regular_fn_counts_as_function() {
        let (f, _, _, _, t, _, _) = sym(Language::Go, "func doSomething() {");
        assert_eq!(f, 1, "regular Go func must be counted");
        assert_eq!(t, 0);
    }

    #[test]
    fn rust_test_attr_counts_as_test_not_function() {
        // #[test] is a standalone attribute line — counted as a test, never as a function
        let (f, _, _, _, t, _, _) = sym(Language::Rust, "#[test]");
        assert_eq!(t, 1, "#[test] must be counted as a test");
        assert_eq!(f, 0, "#[test] attribute must not be counted as a function");
    }

    #[test]
    fn rust_fn_line_counts_as_function_not_test() {
        // The fn declaration after #[test] does NOT match any test pattern
        let (f, _, _, _, t, _, _) = sym(Language::Rust, "fn test_something() {");
        assert_eq!(f, 1, "fn declaration must count as a function");
        assert_eq!(
            t, 0,
            "fn declaration line must not be double-counted as a test"
        );
    }

    #[test]
    fn js_describe_counts_as_test_not_function() {
        let (f, _, _, _, t, _, _) = sym(Language::JavaScript, "describe('suite', () => {");
        assert_eq!(t, 1, "describe must be counted as a test");
        assert_eq!(f, 0, "describe must not be counted as a function");
    }

    #[test]
    fn js_regular_fn_counts_as_function() {
        let (f, _, _, _, t, _, _) = sym(Language::JavaScript, "function doWork() {");
        assert_eq!(f, 1, "JS function declaration must be counted");
        assert_eq!(t, 0);
    }

    // ── Language detection tests ─────────────────────────────────────────────

    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn detect_language_rs_extension() {
        let lang = detect_language(Path::new("foo.rs"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Rust));
    }

    #[test]
    fn detect_language_py_extension() {
        let lang = detect_language(Path::new("foo.py"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Python));
    }

    #[test]
    fn detect_language_ts_extension() {
        let lang = detect_language(Path::new("app.ts"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::TypeScript));
    }

    #[test]
    fn detect_language_js_extension() {
        let lang = detect_language(Path::new("app.js"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::JavaScript));
    }

    #[test]
    fn detect_language_go_extension() {
        let lang = detect_language(Path::new("main.go"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Go));
    }

    #[test]
    fn detect_language_c_extension() {
        let lang = detect_language(Path::new("main.c"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::C));
    }

    #[test]
    fn detect_language_cpp_extension() {
        let lang = detect_language(Path::new("main.cpp"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Cpp));
    }

    #[test]
    fn detect_language_java_extension() {
        let lang = detect_language(Path::new("Main.java"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Java));
    }

    #[test]
    fn detect_language_makefile_exact_name() {
        let lang = detect_language(Path::new("Makefile"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Makefile));
    }

    #[test]
    fn detect_language_dockerfile_exact_name() {
        let lang = detect_language(Path::new("Dockerfile"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Dockerfile));
    }

    #[test]
    fn detect_language_rakefile() {
        let lang = detect_language(Path::new("Rakefile"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Ruby));
    }

    #[test]
    fn detect_language_gemfile() {
        let lang = detect_language(Path::new("Gemfile"), None, &BTreeMap::new(), false);
        assert_eq!(lang, Some(Language::Ruby));
    }

    #[test]
    fn detect_language_unknown_extension_returns_none() {
        let lang = detect_language(Path::new("foo.xyz123"), None, &BTreeMap::new(), false);
        assert_eq!(lang, None);
    }

    #[test]
    fn detect_language_extension_override() {
        let mut overrides = BTreeMap::new();
        overrides.insert("h".into(), "cpp".into());
        let lang = detect_language(Path::new("header.h"), None, &overrides, false);
        assert_eq!(lang, Some(Language::Cpp));
    }

    #[test]
    fn detect_language_shebang_python() {
        let lang = detect_language(
            Path::new("script"),
            Some("#!/usr/bin/env python3"),
            &BTreeMap::new(),
            true,
        );
        assert_eq!(lang, Some(Language::Python));
    }

    #[test]
    fn detect_language_shebang_bash() {
        let lang = detect_language(
            Path::new("script"),
            Some("#!/bin/bash"),
            &BTreeMap::new(),
            true,
        );
        assert_eq!(lang, Some(Language::Shell));
    }

    #[test]
    fn detect_language_shebang_ruby() {
        let lang = detect_language(
            Path::new("script"),
            Some("#!/usr/bin/env ruby"),
            &BTreeMap::new(),
            true,
        );
        assert_eq!(lang, Some(Language::Ruby));
    }

    #[test]
    fn detect_language_shebang_disabled() {
        // When shebang_detection=false, shebang is ignored
        let lang = detect_language(
            Path::new("script"),
            Some("#!/usr/bin/env python3"),
            &BTreeMap::new(),
            false,
        );
        assert_eq!(lang, None);
    }

    #[test]
    fn from_name_rust() {
        assert_eq!(Language::from_name("rust"), Some(Language::Rust));
    }

    #[test]
    fn from_name_python() {
        assert_eq!(Language::from_name("python"), Some(Language::Python));
    }

    #[test]
    fn from_name_unknown() {
        assert_eq!(Language::from_name("brainfuck"), None);
    }

    #[test]
    fn from_name_roundtrip_all() {
        // Every language's slug should round-trip through from_name
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
            let slug = lang.as_slug();
            let roundtripped = Language::from_name(slug);
            assert_eq!(
                roundtripped,
                Some(lang),
                "from_name({slug:?}) should return {lang:?}"
            );
        }
    }

    // ── blank_in_block_comment_policy behavioral tests ───────────────────────

    #[test]
    fn blank_in_block_comment_defaults_to_comment() {
        // Default: blank lines inside /* */ count as multi-comment lines (IEEE-aligned).
        let input = "/*\n\n*/";
        let opts = AnalysisOptions {
            blank_in_block_comment_as_comment: true,
            ..Default::default()
        };
        let result = analyze_text(Language::C, input, opts);
        assert_eq!(
            result.raw.multi_comment_only_lines, 3,
            "all 3 block-comment lines must count as multi-comment with CountAsComment policy"
        );
        assert_eq!(
            result.raw.blank_only_lines, 0,
            "no blank lines expected with CountAsComment policy"
        );
    }

    #[test]
    fn blank_in_block_comment_counted_as_blank_when_policy_false() {
        // CountAsBlank: blank lines inside /* */ count as blank, not comment.
        let input = "/*\n\n*/";
        let opts = AnalysisOptions {
            blank_in_block_comment_as_comment: false,
            ..Default::default()
        };
        let result = analyze_text(Language::C, input, opts);
        assert_eq!(
            result.raw.multi_comment_only_lines, 2,
            "opener and closer must count as multi-comment with CountAsBlank policy"
        );
        assert_eq!(
            result.raw.blank_only_lines, 1,
            "the blank line inside the block comment must count as blank with CountAsBlank policy"
        );
    }

    // ── continuation_line_policy behavioral tests ────────────────────────────

    #[test]
    fn continuation_lines_each_physical_default() {
        // Default (EachPhysicalLine): every physical line counted separately.
        let input = "#define FOO \\\n  1 \\\n  + 2\n";
        let opts = AnalysisOptions {
            collapse_continuation_lines: false,
            ..Default::default()
        };
        let result = analyze_text(Language::C, input, opts);
        assert_eq!(
            result.raw.total_physical_lines, 3,
            "3 physical lines expected"
        );
        assert_eq!(
            result.raw.code_only_lines, 3,
            "each physical line must count as code with EachPhysicalLine policy"
        );
    }

    #[test]
    fn continuation_lines_collapse_to_logical() {
        // CollapseToLogical: 3 backslash-continued lines collapse to 1 logical code line.
        let input = "#define FOO \\\n  1 \\\n  + 2\n";
        let opts = AnalysisOptions {
            collapse_continuation_lines: true,
            ..Default::default()
        };
        let result = analyze_text(Language::C, input, opts);
        assert_eq!(
            result.raw.total_physical_lines, 3,
            "physical line count is always 3 regardless of policy"
        );
        assert_eq!(
            result.raw.code_only_lines, 1,
            "3 continuation lines must collapse to 1 logical code line"
        );
    }
}
