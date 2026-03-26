use std::collections::HashSet;

/// Parses WhatsApp-style text formatting and converts it to Pango markup.
/// Only applies formatting when markers are properly paired/closed.
///
/// Supports:
/// - `*bold text*` → **bold text**
/// - `_italic text_` → *italic text*
/// - `~strikethrough text~` → ~~strikethrough~~
/// - `` `monospace text` `` → `monospace`
/// - `` ```code block``` `` → code block
pub fn parse_whatsapp_formatting(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let markers = collect_markers(text);
    let code_pairs = match_code_pairs(&markers);
    let inside_code = build_inside_code_set(&code_pairs);
    let format_pairs = match_formatting_pairs(&markers, &inside_code);

    build_output(text, &code_pairs, &format_pairs)
}

fn collect_markers(text: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    let mut chars = text.chars().enumerate().peekable();

    while let Some((idx, ch)) = chars.next() {
        match ch {
            '`' => collect_backtick_markers(&mut chars, idx, &mut markers),
            '*' => {
                if !is_bullet_point(text, idx, &mut chars) {
                    markers.push(Marker {
                        byte_pos: idx,
                        marker_type: MarkerType::Bold,
                    });
                }
            }
            '_' => markers.push(Marker {
                byte_pos: idx,
                marker_type: MarkerType::Italic,
            }),
            '~' => markers.push(Marker {
                byte_pos: idx,
                marker_type: MarkerType::Strikethrough,
            }),
            _ => {}
        }
    }

    markers
}

fn collect_backtick_markers(
    chars: &mut std::iter::Peekable<std::iter::Enumerate<std::str::Chars<'_>>>,
    idx: usize,
    markers: &mut Vec<Marker>,
) {
    let mut count = 1;
    while chars.peek().map(|(_, c)| c) == Some(&'`') {
        chars.next();
        count += 1;
    }

    if count == 1 {
        markers.push(Marker {
            byte_pos: idx,
            marker_type: MarkerType::InlineCode,
        });
    } else if count >= 3 {
        markers.push(Marker {
            byte_pos: idx,
            marker_type: MarkerType::CodeBlock,
        });
    }
}

fn is_bullet_point(
    text: &str,
    idx: usize,
    chars: &mut std::iter::Peekable<std::iter::Enumerate<std::str::Chars<'_>>>,
) -> bool {
    let is_at_line_start = idx == 0 || text.chars().nth(idx.saturating_sub(1)) == Some('\n');
    let is_followed_by_space = chars.peek().map(|(_, c)| c) == Some(&' ');
    is_at_line_start && is_followed_by_space
}

fn match_code_pairs(markers: &[Marker]) -> Vec<(usize, usize, MarkerType)> {
    let mut pairs = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for (i, current) in markers.iter().enumerate() {
        match current.marker_type {
            MarkerType::InlineCode | MarkerType::CodeBlock => {
                if let Some(&top_idx) = stack.last() {
                    let top = &markers[top_idx];
                    if top.marker_type == current.marker_type && current.byte_pos > top.byte_pos + 1
                    {
                        let open_idx = stack.pop().unwrap();
                        pairs.push((
                            markers[open_idx].byte_pos,
                            current.byte_pos,
                            current.marker_type,
                        ));
                        continue;
                    }
                }
                stack.push(i);
            }
            _ => {}
        }
    }

    pairs
}

fn build_inside_code_set(code_pairs: &[(usize, usize, MarkerType)]) -> HashSet<usize> {
    let mut inside_code = HashSet::new();
    for (open, close, _) in code_pairs {
        for i in *open..=*close {
            inside_code.insert(i);
        }
    }
    inside_code
}

fn match_formatting_pairs(
    markers: &[Marker],
    inside_code: &HashSet<usize>,
) -> Vec<(usize, usize, MarkerType)> {
    let mut pairs = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for (i, current) in markers.iter().enumerate() {
        if matches!(
            current.marker_type,
            MarkerType::InlineCode | MarkerType::CodeBlock
        ) {
            continue;
        }

        if inside_code.contains(&current.byte_pos) {
            continue;
        }

        if let Some(&top_idx) = stack.last() {
            let top = &markers[top_idx];
            if top.marker_type == current.marker_type && current.byte_pos > top.byte_pos + 1 {
                let open_idx = stack.pop().unwrap();
                pairs.push((
                    markers[open_idx].byte_pos,
                    current.byte_pos,
                    current.marker_type,
                ));
                continue;
            }
        }
        stack.push(i);
    }

    pairs
}

fn build_output(
    text: &str,
    code_pairs: &[(usize, usize, MarkerType)],
    format_pairs: &[(usize, usize, MarkerType)],
) -> String {
    let mut matched_pairs: Vec<(usize, usize, MarkerType)> = Vec::new();
    matched_pairs.extend(code_pairs);
    matched_pairs.extend(format_pairs);

    let mut result = String::with_capacity(text.len() * 2);
    let mut chars = text.chars().enumerate().peekable();
    let mut active_formats: Vec<MarkerType> = Vec::new();

    while let Some((idx, ch)) = chars.next() {
        if ch == '`' {
            let start_idx = idx;
            let mut count = 1;
            while chars.peek().map(|(_, c)| c) == Some(&'`') {
                chars.next();
                count += 1;
            }

            let marker_type = if count == 1 {
                Some(MarkerType::InlineCode)
            } else if count >= 3 {
                Some(MarkerType::CodeBlock)
            } else {
                None
            };

            if let Some(mtype) = marker_type
                && output_matched_marker(
                    &mut result,
                    start_idx,
                    mtype,
                    &matched_pairs,
                    &mut active_formats,
                )
            {
                continue;
            }
            for _ in 0..count {
                result.push('`');
            }
            continue;
        }

        let marker_type = match ch {
            '*' => Some(MarkerType::Bold),
            '_' => Some(MarkerType::Italic),
            '~' => Some(MarkerType::Strikethrough),
            _ => None,
        };

        if let Some(mtype) = marker_type
            && output_matched_marker(&mut result, idx, mtype, &matched_pairs, &mut active_formats)
        {
            continue;
        }

        match ch {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            _ => result.push(ch),
        }
    }

    result
}

fn output_matched_marker(
    result: &mut String,
    idx: usize,
    mtype: MarkerType,
    matched_pairs: &[(usize, usize, MarkerType)],
    active_formats: &mut Vec<MarkerType>,
) -> bool {
    let is_matched = matched_pairs
        .iter()
        .any(|(open, close, t)| (*open == idx || *close == idx) && *t == mtype);

    if is_matched {
        let is_opening = matched_pairs
            .iter()
            .any(|(open, _, t)| *open == idx && *t == mtype);

        if is_opening {
            result.push_str(mtype.open_tag());
            active_formats.push(mtype);
        } else {
            result.push_str(mtype.close_tag());
            if let Some(pos) = active_formats.iter().rposition(|&t| t == mtype) {
                active_formats.remove(pos);
            }
        }
        true
    } else {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Marker {
    byte_pos: usize,
    marker_type: MarkerType,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MarkerType {
    Bold,
    Italic,
    Strikethrough,
    InlineCode,
    CodeBlock,
}

impl MarkerType {
    fn open_tag(self) -> &'static str {
        match self {
            Self::Bold => "<b>",
            Self::Italic => "<i>",
            Self::Strikethrough => "<s>",
            Self::InlineCode | Self::CodeBlock => "<tt>",
        }
    }

    fn close_tag(self) -> &'static str {
        match self {
            Self::Bold => "</b>",
            Self::Italic => "</i>",
            Self::Strikethrough => "</s>",
            Self::InlineCode | Self::CodeBlock => "</tt>",
        }
    }
}

/// Returns true if the text contains any `WhatsApp` formatting markers.
pub fn has_formatting(text: &str) -> bool {
    text.contains('*') || text.contains('_') || text.contains('~') || text.contains('`')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold() {
        assert_eq!(parse_whatsapp_formatting("*bold text*"), "<b>bold text</b>");
    }

    #[test]
    fn test_italic() {
        assert_eq!(
            parse_whatsapp_formatting("_italic text_"),
            "<i>italic text</i>"
        );
    }

    #[test]
    fn test_strikethrough() {
        assert_eq!(
            parse_whatsapp_formatting("~strikethrough text~"),
            "<s>strikethrough text</s>"
        );
    }

    #[test]
    fn test_inline_code() {
        assert_eq!(parse_whatsapp_formatting("`code`"), "<tt>code</tt>");
    }

    #[test]
    fn test_code_block() {
        assert_eq!(
            parse_whatsapp_formatting("```code block```"),
            "<tt>code block</tt>"
        );
    }

    #[test]
    fn test_combined() {
        assert_eq!(
            parse_whatsapp_formatting("*bold* and _italic_"),
            "<b>bold</b> and <i>italic</i>"
        );
    }

    #[test]
    fn test_nested() {
        assert_eq!(
            parse_whatsapp_formatting("*_bold italic_*"),
            "<b><i>bold italic</i></b>"
        );
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(
            parse_whatsapp_formatting("*test <>&\"*"),
            "<b>test &lt;&gt;&amp;&quot;</b>"
        );
    }

    #[test]
    fn test_plain_text() {
        assert_eq!(parse_whatsapp_formatting("hello world"), "hello world");
    }

    #[test]
    fn test_empty() {
        assert_eq!(parse_whatsapp_formatting(""), "");
    }

    #[test]
    fn test_unclosed_tags() {
        assert_eq!(
            parse_whatsapp_formatting("*unclosed bold"),
            "*unclosed bold"
        );
        assert_eq!(
            parse_whatsapp_formatting("_unclosed italic"),
            "_unclosed italic"
        );
        assert_eq!(
            parse_whatsapp_formatting("~unclosed strikethrough"),
            "~unclosed strikethrough"
        );
        assert_eq!(
            parse_whatsapp_formatting("`unclosed code"),
            "`unclosed code"
        );
    }

    #[test]
    fn test_unmatched_closing_only() {
        assert_eq!(parse_whatsapp_formatting("bold*"), "bold*");
        assert_eq!(parse_whatsapp_formatting("italic_"), "italic_");
    }

    #[test]
    fn test_code_no_formatting() {
        assert_eq!(
            parse_whatsapp_formatting("`*bold in code*`"),
            "<tt>*bold in code*</tt>"
        );
    }

    #[test]
    fn test_code_block_no_formatting() {
        assert_eq!(
            parse_whatsapp_formatting("```*bold* _italic_```"),
            "<tt>*bold* _italic_</tt>"
        );
    }

    #[test]
    fn test_code_block_with_multiple_lines() {
        assert_eq!(
            parse_whatsapp_formatting("```line1\nline2```"),
            "<tt>line1\nline2</tt>"
        );
    }

    #[test]
    fn test_two_backticks_literal() {
        assert_eq!(parse_whatsapp_formatting("``"), "``");
    }

    #[test]
    fn test_has_formatting() {
        assert!(has_formatting("*bold*"));
        assert!(has_formatting("_italic_"));
        assert!(has_formatting("~strike~"));
        assert!(has_formatting("`code`"));
        assert!(!has_formatting("plain text"));
    }

    #[test]
    fn test_mixed_unclosed_and_closed() {
        assert_eq!(
            parse_whatsapp_formatting("*closed* and *unclosed"),
            "<b>closed</b> and *unclosed"
        );
        assert_eq!(
            parse_whatsapp_formatting("*unclosed and _closed_"),
            "*unclosed and <i>closed</i>"
        );
    }

    #[test]
    fn test_overlapping_invalid() {
        assert_eq!(
            parse_whatsapp_formatting("*bold _italic* bold_"),
            "*bold _italic* bold_"
        );
    }

    #[test]
    fn test_bullet_points() {
        assert_eq!(
            parse_whatsapp_formatting("* bullet point 1\n* bullet point 2"),
            "* bullet point 1\n* bullet point 2"
        );
        assert_eq!(
            parse_whatsapp_formatting("* bullet with *bold* text"),
            "* bullet with <b>bold</b> text"
        );
    }

    #[test]
    fn test_edge_cases_from_user() {
        assert_eq!(
            parse_whatsapp_formatting("*_bold and italic*_"),
            "*_bold and italic*_"
        );
        assert_eq!(
            parse_whatsapp_formatting("*bold and italic_"),
            "*bold and italic_"
        );
        assert_eq!(
            parse_whatsapp_formatting("*This bold never ends"),
            "*This bold never ends"
        );
        assert_eq!(
            parse_whatsapp_formatting("```monospace without closing"),
            "```monospace without closing"
        );
        assert_eq!(parse_whatsapp_formatting("**"), "**");
        assert_eq!(parse_whatsapp_formatting("__"), "__");
        assert_eq!(
            parse_whatsapp_formatting("*This is bold\non multiple lines*"),
            "<b>This is bold\non multiple lines</b>"
        );
    }
}
