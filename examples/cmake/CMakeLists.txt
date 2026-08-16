# Standalone example showing how to wire oxide-sloc into a CMake build.
#
# Configure and build the report target:
#   cmake -S . -B build
#   cmake --build build --target sloc
#
# Requires the oxide-sloc executable on PATH, or pass its location explicitly:
#   cmake -S . -B build -D OXIDE_SLOC=/path/to/oxide-sloc

cmake_minimum_required(VERSION 3.16)
project(sloc_example CXX)

# Make the OxideSloc module discoverable (it lives two directories up, in cmake/).
list(APPEND CMAKE_MODULE_PATH "${CMAKE_SOURCE_DIR}/../../cmake")
include(OxideSloc)

# A trivial program so the scan has real source to count.
add_executable(app src/main.cpp)

# Register a `sloc` target that scans src/, writes HTML + JSON reports into the
# build directory, and fails the build if fewer than 10 code lines are found
# (demonstrates exit-code gating via FAIL_BELOW -> oxide-sloc exit 3).
oxide_sloc_add_report(
  NAME sloc
  PATHS ${CMAKE_SOURCE_DIR}/src
  HTML
  JSON
  FAIL_BELOW 10)

# A second, non-gating target that always succeeds - handy in CI when you want
# the numbers reported without blocking the pipeline.
oxide_sloc_add_report(
  NAME sloc-report
  PATHS ${CMAKE_SOURCE_DIR}/src
  HTML
  REPORT_ONLY)
