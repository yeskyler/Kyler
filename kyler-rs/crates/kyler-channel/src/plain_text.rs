//! Render Markdown for messaging transports that only accept plain text.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub(crate) fn render(markdown: &str) -> String {
    let mut output = String::new();
    let mut links = Vec::new();
    let mut list_numbers = Vec::new();
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        match event {
            Event::Text(text)
            | Event::Code(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => output.push_str(&text),
            Event::SoftBreak | Event::HardBreak => output.push('\n'),
            Event::Start(Tag::Link { dest_url, .. }) => links.push(dest_url.into_string()),
            Event::End(TagEnd::Link) => {
                if let Some(url) = links.pop() {
                    // Local deliverables are sent separately; never leak server paths.
                    if url.starts_with("https://")
                        || url.starts_with("http://")
                        || url.starts_with("mailto:")
                    {
                        if !output.ends_with(&url) {
                            output.push_str(&format!(" ({url})"));
                        }
                    }
                }
            }
            Event::Start(Tag::List(start)) => list_numbers.push(start),
            Event::End(TagEnd::List(_)) => {
                list_numbers.pop();
                newline(&mut output);
            }
            Event::Start(Tag::Item) => {
                newline(&mut output);
                match list_numbers.last_mut() {
                    Some(Some(number)) => {
                        output.push_str(&format!("{number}. "));
                        *number += 1;
                    }
                    _ => output.push_str("• "),
                }
            }
            Event::Start(Tag::TableCell) => {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push_str(" · ");
                }
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::TableRow
                | TagEnd::TableHead
                | TagEnd::BlockQuote(_),
            ) => newline(&mut output),
            Event::TaskListMarker(done) => output.push_str(if done { "[x] " } else { "[ ] " }),
            Event::Rule => newline(&mut output),
            // Raw HTML is not a supported QQ text format.
            _ => {}
        }
    }
    output.trim().to_owned()
}
fn newline(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_code_links_and_table_values_without_markdown_wrappers() {
        let text = render(
            "# 标题\n\n**你好** [官网](https://example.com)\n\n```sh\necho '**keep**'\n```\n\n| 名称 | 值 |\n|---|---|\n| 项目 | 42 |\n\n[报告](/home/private/report.pdf)",
        );
        assert!(text.contains("标题\n你好 官网 (https://example.com)"));
        assert!(text.contains("echo '**keep**'"));
        assert!(text.contains("项目 · 42"));
        assert!(!text.contains("```"));
        assert!(!text.contains("/home/private"));
    }
}
