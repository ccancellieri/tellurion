pub(super) fn append_top_level_sequence(
    source: &str,
    section: &str,
    item: &str,
) -> anyhow::Result<String> {
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let key = format!("{section}:");
    let mut offset = 0usize;
    let mut section_line = None;
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if body.starts_with(&key) {
            if section_line.is_some() {
                anyhow::bail!("config contains more than one top-level '{section}' key");
            }
            section_line = Some((offset, offset + body.len(), offset + line.len(), body));
        }
        offset += line.len();
    }
    let (line_start, body_end, line_end, body) = section_line
        .ok_or_else(|| anyhow::anyhow!("config has no top-level '{section}' sequence"))?;
    let trimmed = body[key.len()..].trim_start();

    if let Some(after_empty) = trimmed.strip_prefix("[]") {
        if !after_empty.trim_start().is_empty() && !after_empty.trim_start().starts_with('#') {
            unsupported(section)?;
        }
        let empty_start = body_end - trimmed.len();
        let empty_end = empty_start + 2;
        let mut updated = String::with_capacity(source.len() + item.len() + 8);
        updated.push_str(&source[..line_start + key.len()]);
        updated.push_str(&source[empty_end..body_end]);
        updated.push_str(newline);
        updated.push_str(&indent_item(item, "  ", newline));
        updated.push_str(newline);
        updated.push_str(&source[line_end..]);
        return Ok(updated);
    }

    if !trimmed.is_empty() && !trimmed.starts_with('#') {
        unsupported(section)?;
    }

    let mut insert_at = source.len();
    let mut item_indent = None;
    let mut scan_offset = line_end;
    for line in source[line_end..].split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let trimmed_line = body.trim_start();
        let indent_len = body.len() - trimmed_line.len();
        if !trimmed_line.is_empty() && !trimmed_line.starts_with('#') {
            if indent_len == 0 {
                insert_at = scan_offset;
                break;
            }
            if item_indent.is_none() {
                if !trimmed_line.starts_with('-') {
                    unsupported(section)?;
                }
                item_indent = Some(&body[..indent_len]);
            }
        } else if indent_len == 0 && !body.is_empty() {
            insert_at = scan_offset;
            break;
        }
        scan_offset += line.len();
    }
    let item_indent = item_indent.ok_or_else(|| unsupported_error(section))?;
    let mut updated = String::with_capacity(source.len() + item.len() + item_indent.len() + 2);
    updated.push_str(&source[..insert_at]);
    if !updated.ends_with('\n') {
        updated.push_str(newline);
    }
    updated.push_str(&indent_item(item, item_indent, newline));
    updated.push_str(newline);
    updated.push_str(&source[insert_at..]);
    Ok(updated)
}

fn indent_item(item: &str, indent: &str, newline: &str) -> String {
    item.split('\n')
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join(newline)
}

fn unsupported(section: &str) -> anyhow::Result<()> {
    Err(unsupported_error(section))
}

fn unsupported_error(section: &str) -> anyhow::Error {
    anyhow::anyhow!("top-level '{section}' supports only a block sequence or an inline empty list")
}
