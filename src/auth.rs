use crate::model::Role;

pub const MIN_KEY_LEN: usize = 43;

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Health,
    Mcp { role: Role, key: String },
    Status { key: String },
    Other,
}

pub fn route(path: &str) -> Route {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        ["health"] => Route::Health,
        ["orch", key, "mcp"] => Route::Mcp {
            role: Role::Orchestrator,
            key: key.to_string(),
        },
        ["work", key, "mcp"] => Route::Mcp {
            role: Role::Worker,
            key: key.to_string(),
        },
        ["status", key] => Route::Status {
            key: key.to_string(),
        },
        _ => Route::Other,
    }
}

/// Compares every byte of the longer input so timing reveals little about where a mismatch sits.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    let span = a.len().max(b.len());
    for i in 0..span {
        let x = if i < a.len() { a[i] } else { 0 };
        let y = if i < b.len() { b[i] } else { 0 };
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

/// A configured key shorter than MIN_KEY_LEN counts as unset, so a blank secret fails every comparison.
pub fn key_matches(expected: &str, given: &str) -> bool {
    expected.len() >= MIN_KEY_LEN
        && !expected.starts_with("replace-with-")
        && expected
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && constant_time_eq(expected.as_bytes(), given.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_known_paths() {
        assert_eq!(route("/health"), Route::Health);
        assert_eq!(
            route("/orch/abc/mcp"),
            Route::Mcp {
                role: Role::Orchestrator,
                key: "abc".to_string()
            }
        );
        assert_eq!(
            route("/work/xyz/mcp/"),
            Route::Mcp {
                role: Role::Worker,
                key: "xyz".to_string()
            }
        );
        assert_eq!(
            route("/status/abc"),
            Route::Status {
                key: "abc".to_string()
            }
        );
    }

    #[test]
    fn unknown_paths_fall_through() {
        assert_eq!(route("/"), Route::Other);
        assert_eq!(route("/orch/abc"), Route::Other);
        assert_eq!(route("/work/abc/mcp/extra"), Route::Other);
    }

    #[test]
    fn compares_keys() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(key_matches(key, key));
        assert!(!key_matches(key, "0123456789abcdef0124"));
        assert!(!key_matches(key, "short"));
        assert!(!key_matches("", ""));
        assert!(!key_matches("tooshort", "tooshort"));
    }

    #[test]
    fn constant_time_eq_handles_length_gaps() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
    }
}
