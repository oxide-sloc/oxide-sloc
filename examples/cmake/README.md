# oxide-sloc + CMake example

A minimal, self-contained project that wires [oxide-sloc](https://github.com/oxide-sloc/oxide-sloc)
into a CMake build using the reusable `OxideSloc` module in [`cmake/OxideSloc.cmake`](../../cmake/OxideSloc.cmake).

## Prerequisites

- CMake 3.16 or newer.
- A C++ compiler (only needed to build the sample `app` executable; the report targets do not need it).
- The `oxide-sloc` executable on `PATH`, or its path passed via `-D OXIDE_SLOC=<path>`.

## Configure and run

```sh
cmake -S . -B build
cmake --build build --target sloc
```

The `sloc` target scans `src/`, prints a machine-readable summary, and writes:

- `build/sloc.html` - HTML report
- `build/sloc.json` - JSON result (re-render later with `oxide-sloc report`)

A second, non-gating target reports numbers without ever failing the build:

```sh
cmake --build build --target sloc-report
```

If `oxide-sloc` is not on `PATH`:

```sh
cmake -S . -B build -D OXIDE_SLOC=/path/to/oxide-sloc
```

## Using it in your own project

Copy `cmake/OxideSloc.cmake` (and its `OxideSlocRun.cmake` helper) into your project, then:

```cmake
list(APPEND CMAKE_MODULE_PATH "${CMAKE_SOURCE_DIR}/cmake")
include(OxideSloc)

oxide_sloc_add_report(
  NAME sloc
  PATHS ${CMAKE_SOURCE_DIR}/src ${CMAKE_SOURCE_DIR}/include
  HTML JSON
  FAIL_BELOW 100)
```

See the header comment in [`OxideSloc.cmake`](../../cmake/OxideSloc.cmake) for every option.

## Build gating and exit codes

Report targets propagate the `oxide-sloc` exit code, so a failing gate fails the build step:

| Exit | Meaning | Triggered by |
|------|---------|--------------|
| 0 | Success | - |
| 1 | Generic error | Bad path, missing binary, etc. |
| 2 | Warnings gate | `FAIL_ON_WARNINGS` |
| 3 | Below code-line threshold | `FAIL_BELOW <n>` |
| 4 | SLOC budget exceeded | `FAIL_ON_BUDGET` (budget defined in `.oxide-sloc.toml`) |
| 6 | Complexity threshold exceeded | `EXTRA_ARGS --max-complexity <n>` |

To see gating in action, raise `FAIL_BELOW` in [`CMakeLists.txt`](CMakeLists.txt) above the real code-line
count and rebuild `sloc` - the build step will fail with exit 3. Use the `REPORT_ONLY` option (as the
`sloc-report` target does) to always exit 0.
