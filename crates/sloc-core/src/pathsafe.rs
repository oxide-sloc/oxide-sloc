// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! Filesystem-path traversal barrier.
//!
//! Filesystem paths in this workbench are frequently assembled from operator-supplied
//! values — CLI output flags, the web output directory, run identifiers, and git repo
//! locations. Before such a path reaches a filesystem operation it is passed through
//! [`reject_traversal`], which refuses any path that contains a parent-directory (`..`)
//! segment and returns a fresh owned path rebuilt from the validated string. Callers
//! must use the returned value for the filesystem call so the check dominates the
//! operation and no `..` sequence can walk outside the intended location.

use std::path::{Path, PathBuf};

/// Reject a path that contains a parent-directory (`..`) segment.
///
/// Returns a freshly-owned [`PathBuf`] rebuilt from the validated string when the path
/// is free of `..` sequences. Callers must use the returned value (not the original) for
/// the subsequent filesystem operation.
///
/// # Errors
/// Returns an [`std::io::Error`] with kind [`InvalidInput`](std::io::ErrorKind::InvalidInput)
/// when the path contains a `..` component.
pub fn reject_traversal(path: &Path) -> std::io::Result<PathBuf> {
    let text = path.to_string_lossy().into_owned();
    if text.contains("..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing filesystem path with a parent-directory (`..`) segment",
        ));
    }
    Ok(PathBuf::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_relative_path() {
        let p = Path::new("out/web/report.html");
        let got = reject_traversal(p).expect("plain path is accepted");
        assert_eq!(got, PathBuf::from("out/web/report.html"));
    }

    #[test]
    fn accepts_absolute_path() {
        let p = Path::new("/var/lib/oxide-sloc/run.json");
        let got = reject_traversal(p).expect("absolute path is accepted");
        assert_eq!(got, PathBuf::from("/var/lib/oxide-sloc/run.json"));
    }

    #[test]
    fn accepts_name_with_non_adjacent_dots() {
        // Dots separated by other characters are not a traversal segment.
        let p = Path::new("out/web/report.final.html");
        assert!(reject_traversal(p).is_ok());
    }

    #[test]
    fn rejects_unix_traversal() {
        let p = Path::new("out/../../etc/passwd");
        assert!(reject_traversal(p).is_err());
    }

    #[test]
    fn rejects_windows_traversal() {
        let p = Path::new(r"out\..\..\secret");
        assert!(reject_traversal(p).is_err());
    }

    #[test]
    fn rejects_leading_traversal() {
        assert!(reject_traversal(Path::new("../escape")).is_err());
    }

    #[test]
    fn error_kind_is_invalid_input() {
        let err = reject_traversal(Path::new("a/../b")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
