# OxideSlocRun.cmake - internal helper for oxide_sloc_add_report(REPORT_ONLY).
#
# Runs oxide-sloc but always exits 0, so a report-only target never gates the
# build. Invoked via `cmake -P` from OxideSloc.cmake; not meant to be used
# directly. Expects:
#   -DOXIDE_SLOC_BIN=<binary>
#   -DOXIDE_SLOC_ARGS=<semicolon-separated arg list>
#
# License: AGPL-3.0-or-later. Part of the oxide-sloc project.

if(NOT DEFINED OXIDE_SLOC_BIN)
  message(FATAL_ERROR "OxideSlocRun: OXIDE_SLOC_BIN not set")
endif()

execute_process(
  COMMAND "${OXIDE_SLOC_BIN}" ${OXIDE_SLOC_ARGS}
  RESULT_VARIABLE _code)

if(NOT _code EQUAL 0)
  message(STATUS "oxide-sloc: exited ${_code} (ignored; report-only target)")
endif()
