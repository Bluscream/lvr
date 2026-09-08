//! Steam KeyValues / VDF text parsing and in-place editing utilities.

use std::fs;
use std::path::Path;
use anyhow::{Context, Result, anyhow};

/// Replace a value in a Steam VDF file located at `key_path`.
/// Creates an atomic temporary file before renaming to ensure data integrity.
pub fn edit_vdf(path: &Path, key_path: &[&str], value: &str) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let updated = replace_value(&text, key_path, value)
        .ok_or_else(|| anyhow!("{} has no {} entry", path.display(), key_path.join(" / ")))?;
    if updated == text {
        return Ok(());
    }
    let backup = path.with_extension("vdf.lvr.bak");
    let _ = fs::write(&backup, &text);
    let tmp = path.with_extension("vdf.lvr.tmp");
    fs::write(&tmp, &updated).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Find a value at `key_path` in a Steam VDF text string.
pub fn find_value(text: &str, key_path: &[&str]) -> Option<String> {
    let (start, end) = value_span(text, key_path)?;
    Some(unescape(&text[start..end]))
}

/// Replace a value at `key_path` in a Steam VDF text string.
pub fn replace_value(text: &str, key_path: &[&str], value: &str) -> Option<String> {
    if let Some((start, end)) = value_span(text, key_path) {
        let mut out = String::with_capacity(text.len() + value.len());
        out.push_str(&text[..start]);
        out.push_str(&escape(value));
        out.push_str(&text[end..]);
        Some(out)
    } else {
        None
    }
}

/// Walk a Steam text VDF and return the byte range of the value belonging to
/// `key_path`. Matching is case-insensitive (Steam is inconsistent) and the
/// path may skip intermediate levels, so `["apps", "438100", "LaunchOptions"]`
/// finds the key wherever the `apps` block happens to sit.
pub fn value_span(text: &str, key_path: &[&str]) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    let mut matched = 0usize;
    let mut pending_key: Option<String> = None;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let (token, start, end) = read_string(text, index)?;
                index = end;
                match pending_key.take() {
                    Some(key) => {
                        if matched == key_path.len() - 1
                            && key.eq_ignore_ascii_case(key_path[matched])
                        {
                            return Some((start + 1, end - 1));
                        }
                    }
                    None => pending_key = Some(token),
                }
            }
            b'{' => {
                let key = pending_key.take().unwrap_or_default();
                stack.push(matched);
                if matched < key_path.len() - 1 && key.eq_ignore_ascii_case(key_path[matched]) {
                    matched += 1;
                }
                index += 1;
            }
            b'}' => {
                matched = stack.pop().unwrap_or(0);
                pending_key = None;
                index += 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += text[index..].find('\n').map(|n| n + 1).unwrap_or(bytes.len() - index);
            }
            _ => index += 1,
        }
    }
    None
}

/// Read the quoted string starting at `index`; returns (unescaped contents,
/// opening-quote offset, offset just past the closing quote).
pub fn read_string(text: &str, index: usize) -> Option<(String, usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => {
                cursor += 1;
                if cursor >= bytes.len() {
                    return None;
                }
                match bytes[cursor] {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'\\' => out.push('\\'),
                    b'"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other as char);
                    }
                }
                cursor += 1;
            }
            b'"' => return Some((out, index, cursor + 1)),
            byte => {
                out.push(byte as char);
                cursor += 1;
            }
        }
    }
    None
}

pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
"InstallConfigStore"
{
    "Software"
    {
        "Valve"
        {
            "Steam"
            {
                "CompatToolMapping"
                {
                    "0"
                    {
                        "name"      "proton_experimental"
                        "priority"  "75"
                    }
                    "438100"
                    {
                        "name"      "GE-Proton9-25"
                        "config"    ""
                        "priority"  "250"
                    }
                }
            }
        }
    }
}
"#;

    const LOCAL: &str = r#"
"UserLocalConfigStore"
{
    "Software"
    {
        "Valve"
        {
            "Steam"
            {
                "apps"
                {
                    "620980"
                    {
                        "LaunchOptions"     "%command% --other"
                    }
                    "438100"
                    {
                        "LaunchOptions"     "WINEDLLOVERRIDES=\"iyuv_32=\" %command% --enable-hw-video-decoding"
                        "Playtime2wks"      "8239"
                    }
                }
            }
        }
    }
    "system"
    {
        "LaunchOptions"     "not this one"
    }
}
"#;

    #[test]
    fn reads_the_compat_tool_and_launch_options_of_one_app() {
        assert_eq!(
            find_value(CONFIG, &["CompatToolMapping", "438100", "name"]).as_deref(),
            Some("GE-Proton9-25")
        );
        assert_eq!(
            find_value(LOCAL, &["apps", "438100", "LaunchOptions"]).as_deref(),
            Some(r#"WINEDLLOVERRIDES="iyuv_32=" %command% --enable-hw-video-decoding"#)
        );
        assert_eq!(
            find_value(LOCAL, &["apps", "620980", "LaunchOptions"]).as_deref(),
            Some("%command% --other")
        );
        assert_eq!(find_value(CONFIG, &["CompatToolMapping", "999", "name"]), None);
    }

    #[test]
    fn replacing_a_value_touches_nothing_else() {
        let updated =
            replace_value(CONFIG, &["CompatToolMapping", "438100", "name"], "Proton-GE RTSP Latest")
                .expect("entry exists");
        assert!(updated.contains(r#""name"      "Proton-GE RTSP Latest""#));
        assert!(updated.contains(r#""name"      "proton_experimental""#));
        assert_eq!(updated.len(), CONFIG.len() + "Proton-GE RTSP Latest".len() - "GE-Proton9-25".len());
    }

    #[test]
    fn replacing_launch_options_escapes_quotes() {
        let wanted = r#"WINEDLLOVERRIDES="iyuv_32=" %command% --enable-avpro-in-proton"#;
        let updated = replace_value(LOCAL, &["apps", "438100", "LaunchOptions"], wanted)
            .expect("entry exists");
        assert!(updated.contains(r#"WINEDLLOVERRIDES=\"iyuv_32=\" %command% --enable-avpro-in-proton"#));
        assert_eq!(
            find_value(&updated, &["apps", "438100", "LaunchOptions"]).as_deref(),
            Some(wanted)
        );
        assert!(updated.contains(r#""LaunchOptions"     "not this one""#));
    }
}
