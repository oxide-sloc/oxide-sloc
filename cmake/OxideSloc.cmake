# OxideSloc.cmake - reusable CMake integration for the oxide-sloc code-metrics tool.
#
# Usage:
#   list(APPEND CMAKE_MODULE_PATH "${CMAKE_SOURCE_DIR}/path/to/cmake")
#   include(OxideSloc)
#   oxide_sloc_add_report(NAME sloc PATHS ${CMAKE_SOURCE_DIR}/src HTML JSON FAIL_BELOW 100)
#
# Then build the report on demand:
#   cmake --build build --target sloc
#
# The binary is located, in order, from:
#   1. -D OXIDE_SLOC=<path>            (CMake cache variable)
#   2. $ENV{SLOC_BIN} / $ENV{OXIDE_SLOC}
#   3. oxide-sloc on PATH
#
# This module makes no changes to the build itself: report targets are never part
# of ALL, so a plain `cmake --build` does not run them.
#
# License: AGPL-3.0-or-later. Part of the oxide-sloc project.

# Guard against double-inclusion.
if(DEFINED _OXIDE_SLOC_INCLUDED)
  return()
endif()
set(_OXIDE_SLOC_INCLUDED TRUE)

# Locate the binary. HINTS pick up the shared SLOC_BIN / OXIDE_SLOC env convention
# used by the rest of the toolchain (MCP server, etc.). find_program appends the
# platform executable suffix (.exe on Windows) automatically.
find_program(OXIDE_SLOC
  NAMES oxide-sloc
  HINTS ENV SLOC_BIN ENV OXIDE_SLOC
  DOC "Path to the oxide-sloc executable")

if(OXIDE_SLOC)
  message(STATUS "oxide-sloc: found at ${OXIDE_SLOC}")
else()
  message(WARNING
    "oxide-sloc: executable not found. Report targets will still be registered, "
    "but will fail at build time until oxide-sloc is on PATH or -D OXIDE_SLOC=<path> "
    "is set. See https://github.com/oxide-sloc/oxide-sloc for install instructions.")
endif()

# oxide_sloc_add_report(...) - register a custom target that runs an oxide-sloc scan.
#
# Options (flags):
#   HTML             Write an HTML report (see HTML_OUT for the path).
#   JSON             Write a JSON result (see JSON_OUT for the path).
#   OPEN             Open the HTML report in the default browser when done.
#   FAIL_ON_BUDGET   Fail the target if any configured SLOC budget is exceeded (exit 4).
#   FAIL_ON_WARNINGS Fail the target if any warnings are emitted (exit 2).
#   REPORT_ONLY      Never fail the target; report numbers without gating (always exit 0).
#
# One-value arguments:
#   NAME       Target name (default: sloc).
#   HTML_OUT   HTML output path (default: <binary-dir>/<name>.html).
#   JSON_OUT   JSON output path (default: <binary-dir>/<name>.json).
#   FAIL_BELOW Fail the target if code lines fall below this integer (exit 3).
#   CONFIG     Path to a .oxide-sloc.toml config file.
#
# Multi-value arguments:
#   PATHS      Directories/files to scan (default: CMAKE_SOURCE_DIR).
#   EXTRA_ARGS Extra flags passed verbatim to `oxide-sloc analyze`
#              (e.g. --per-file, --activity-window 90, --cocomo).
function(oxide_sloc_add_report)
  set(options HTML JSON OPEN FAIL_ON_BUDGET FAIL_ON_WARNINGS REPORT_ONLY)
  set(one_value_args NAME HTML_OUT JSON_OUT FAIL_BELOW CONFIG)
  set(multi_value_args PATHS EXTRA_ARGS)
  cmake_parse_arguments(OSR "${options}" "${one_value_args}" "${multi_value_args}" ${ARGN})

  if(OSR_UNPARSED_ARGUMENTS)
    message(FATAL_ERROR "oxide_sloc_add_report: unexpected arguments: ${OSR_UNPARSED_ARGUMENTS}")
  endif()

  # Defaults.
  if(NOT OSR_NAME)
    set(OSR_NAME "sloc")
  endif()
  if(NOT OSR_PATHS)
    set(OSR_PATHS "${CMAKE_SOURCE_DIR}")
  endif()

  # Build the analyze argument vector. --plain gives machine-readable stdout that
  # shows up in the build output. Using a list (not a string) keeps paths with
  # spaces intact and avoids any shell quoting.
  set(_args analyze ${OSR_PATHS} --plain)

  if(OSR_HTML)
    if(NOT OSR_HTML_OUT)
      set(OSR_HTML_OUT "${CMAKE_BINARY_DIR}/${OSR_NAME}.html")
    endif()
    list(APPEND _args --html-out "${OSR_HTML_OUT}")
  endif()

  if(OSR_JSON)
    if(NOT OSR_JSON_OUT)
      set(OSR_JSON_OUT "${CMAKE_BINARY_DIR}/${OSR_NAME}.json")
    endif()
    list(APPEND _args --json-out "${OSR_JSON_OUT}")
  endif()

  if(OSR_OPEN)
    list(APPEND _args --open)
  endif()

  if(OSR_CONFIG)
    list(APPEND _args --config "${OSR_CONFIG}")
  endif()

  # Gating flags map directly to oxide-sloc exit codes:
  #   --fail-below N     -> exit 3 when code lines < N
  #   --fail-on-budget   -> exit 4 when a configured budget is exceeded
  #   --fail-on-warnings -> exit 2 when warnings are emitted
  # A non-zero exit fails the custom-target command, which fails the build step.
  if(DEFINED OSR_FAIL_BELOW)
    list(APPEND _args --fail-below "${OSR_FAIL_BELOW}")
  endif()
  if(OSR_FAIL_ON_BUDGET)
    list(APPEND _args --fail-on-budget)
  endif()
  if(OSR_FAIL_ON_WARNINGS)
    list(APPEND _args --fail-on-warnings)
  endif()

  if(OSR_EXTRA_ARGS)
    list(APPEND _args ${OSR_EXTRA_ARGS})
  endif()

  # Resolve the command. If the binary was not found, fall back to the bare name
  # so the build-time error is a clear "command not found" rather than an empty
  # command; the configure-time WARNING above already flagged it.
  set(_bin "${OXIDE_SLOC}")
  if(NOT _bin)
    set(_bin "oxide-sloc")
  endif()

  if(OSR_REPORT_ONLY)
    # Never gate the build: run the scan but always report success. Use CMake's
    # own executable in script mode so this stays fully cross-platform (no shell).
    add_custom_target(${OSR_NAME}
      COMMAND ${CMAKE_COMMAND}
        -DOXIDE_SLOC_BIN=${_bin}
        "-DOXIDE_SLOC_ARGS=${_args}"
        -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/OxideSlocRun.cmake"
      USES_TERMINAL
      VERBATIM
      COMMENT "oxide-sloc: generating report '${OSR_NAME}' (report-only, non-gating)")
  else()
    add_custom_target(${OSR_NAME}
      COMMAND "${_bin}" ${_args}
      USES_TERMINAL
      VERBATIM
      COMMENT "oxide-sloc: generating report '${OSR_NAME}'")
  endif()
endfunction()
