use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const MAX_FILES: usize = 16;
pub const MAX_BYTES: usize = 200_000;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Text,
    Base64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub path: String,
    pub encoding: Encoding,
    pub content: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

pub fn schema() -> Value {
    json!({"type":"array","maxItems":MAX_FILES,"description":"Complete files, up to 200000 decoded bytes combined. Text is UTF-8, binary is standard padded base64. Supply sha256 to verify integrity.","items":{
        "type":"object","required":["path","encoding","content"],"additionalProperties":false,
        "properties":{"path":{"type":"string","description":"Relative project path with forward slashes."},
        "encoding":{"type":"string","enum":["text","base64"]},"content":{"type":"string"},"sha256":{"type":"string","pattern":"^[0-9a-fA-F]{64}$"}}}})
}

fn valid_segment(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let device_number = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    let numbered_device = device_number
        .is_some_and(|n| ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&n));
    !part.is_empty()
        && !part.ends_with([' ', '.'])
        && !part.eq_ignore_ascii_case(".git")
        && !part
            .chars()
            .any(|c| c.is_control() || "<>:\"\\|?*~".contains(c))
        && !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        && !numbered_device
}

pub fn validate(files: &mut [Attachment]) -> Result<(), String> {
    if files.len() > MAX_FILES {
        return Err(format!(
            "Received {} files. The limit is {MAX_FILES}.",
            files.len()
        ));
    }
    let mut paths = HashSet::new();
    let mut total = 0;
    for f in files {
        if f.path.len() > 240 || !f.path.split('/').all(valid_segment) {
            return Err("Use a portable project-relative file path of up to 240 bytes.".into());
        }
        let path = f.path.to_lowercase();
        if !paths.insert(path) {
            return Err("Each file path must be unique.".into());
        }
        let binary;
        let bytes = match f.encoding {
            Encoding::Text => f.content.as_bytes(),
            Encoding::Base64 => {
                binary = STANDARD
                    .decode(&f.content)
                    .map_err(|_| "File content must be standard padded base64.")?;
                &binary
            }
        };
        total += bytes.len();
        if total > MAX_BYTES {
            return Err(format!(
                "Received at least {total} decoded file bytes. The limit is {MAX_BYTES}."
            ));
        }
        let hash = format!("{:x}", Sha256::digest(bytes));
        if f.sha256
            .as_ref()
            .is_some_and(|expected| !expected.eq_ignore_ascii_case(&hash))
        {
            return Err("File SHA-256 mismatch. Resend the original content.".into());
        }
        f.sha256 = Some(hash);
    }
    for path in &paths {
        if path
            .match_indices('/')
            .any(|(i, _)| paths.contains(&path[..i]))
        {
            return Err("A file cannot also be a parent directory.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(path: &str) -> Attachment {
        Attachment {
            path: path.into(),
            encoding: Encoding::Text,
            content: String::new(),
            sha256: None,
        }
    }

    #[test]
    fn validates_portable_paths_and_collisions() {
        for path in [
            "",
            ".",
            "../x",
            "a//b",
            "/x",
            "x:y",
            "dir/.Git/config",
            "CON .bin",
            "aux.txt",
            "COM1",
            "x.",
        ] {
            assert!(validate(&mut [attachment(path)]).is_err(), "{path}");
        }
        assert!(validate(&mut [attachment("A"), attachment("a")]).is_err());
        assert!(validate(&mut [attachment("a/b"), attachment("A")]).is_err());
        assert!(validate(&mut [attachment("src/main.rs")]).is_ok());
    }

    #[test]
    fn accounts_for_decoded_bytes_across_files() {
        let mut files = [attachment("a"), attachment("b")];
        files[0].content = "x".repeat(MAX_BYTES - 1);
        files[1].encoding = Encoding::Base64;
        files[1].content = "AA==".into();
        assert!(validate(&mut files).is_ok());
        files[1].content = "AAA=".into();
        assert!(validate(&mut files).unwrap_err().contains("200001"));
        assert!(validate(&mut vec![attachment("a"); MAX_FILES + 1]).is_err());
    }
}
