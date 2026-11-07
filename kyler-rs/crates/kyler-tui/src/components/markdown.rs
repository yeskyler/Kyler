//! Parse CommonMark directly into terminal styles. HTML is always literal text.
use super::{composer::clean, syntax, transcript::wrap};

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug)]
struct Node {
    tag: Option<Tag<'static>>,
    event: Option<Event<'static>>,
    range: Range<usize>,
    children: Vec<Node>,
}
fn parse(source: &str) -> Vec<Node> {
    let mut stack = vec![Node {
        tag: None,
        event: None,
        range: 0..source.len(),
        children: Vec::new(),
    }];
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(tag) => stack.push(Node {
                tag: Some(tag.into_static()),
                event: None,
                range,
                children: Vec::new(),
            }),
            Event::End(_) => {
                if stack.len() > 1 {
                    let mut node = stack.pop().unwrap();
                    node.range.end = range.end;
                    stack.last_mut().unwrap().children.push(node);
                }
            }
            event => stack.last_mut().unwrap().children.push(Node {
                tag: None,
                event: Some(event.into_static()),
                range,
                children: Vec::new(),
            }),
        }
    }
    stack.pop().unwrap().children
}

#[derive(Clone, Default)]
struct Context {
    prefix: String,
    style: Style,
}
struct Row {
    spans: Vec<Span<'static>>,
    first: String,
    continuation: String,
    style: Style,
    code: bool,
}
impl Row {
    fn new(spans: Vec<Span<'static>>, context: &Context) -> Self {
        Self {
            spans,
            first: context.prefix.clone(),
            continuation: context.prefix.clone(),
            style: context.style,
            code: false,
        }
    }
}

pub fn render(source: &str, width: u16) -> Vec<Line<'static>> {
    let source = clean(source);
    let context = Context {
        prefix: "  ".into(),
        ..Default::default()
    };
    let mut rows = blocks(&parse(&source), &source, &context, width);
    if let Some(first) = rows.first_mut() {
        if first.first.starts_with("  ") {
            first.first.replace_range(..2, "● ");
        }
    }
    rows.into_iter()
        .flat_map(|row| terminal_row(row, width))
        .collect()
}

fn blocks(nodes: &[Node], source: &str, context: &Context, width: u16) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut previous_end = None;
    let mut index = 0;
    while index < nodes.len() {
        let node = &nodes[index];
        if !is_block(node) {
            let end = nodes[index..]
                .iter()
                .position(is_block)
                .map_or(nodes.len(), |offset| index + offset);
            rows.extend(
                inline_lines(&nodes[index..end], Style::default())
                    .into_iter()
                    .map(|spans| Row::new(spans, context)),
            );
            previous_end = Some(nodes[end - 1].range.end);
            index = end;
            continue;
        }
        if let Some(end) = previous_end {
            if node.range.start >= end {
                // Source gaps preserve deliberate paragraph spacing without padding each event.
                let gap = &source[end..node.range.start];
                for _ in 0..gap.bytes().filter(|byte| *byte == b'\n').count() {
                    rows.push(Row::new(Vec::new(), context));
                }
            }
        }
        match &node.tag {
            Some(Tag::Paragraph) | Some(Tag::Heading { .. }) => {
                let mut context = context.clone();
                if let Some(Tag::Heading { level, .. }) = node.tag {
                    context.style = context.style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
                    context.style = match level {
                        HeadingLevel::H1 => context.style.add_modifier(Modifier::UNDERLINED),
                        HeadingLevel::H3 => context.style.add_modifier(Modifier::ITALIC),
                        _ => context.style,
                    };
                }
                rows.extend(
                    inline_lines(&node.children, Style::default())
                        .into_iter()
                        .map(|spans| Row::new(spans, &context)),
                );
            }
            Some(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(language) => {
                        language.split_whitespace().next().unwrap_or("")
                    }
                    CodeBlockKind::Indented => "",
                };
                let context = Context {
                    prefix: context.prefix.clone(),
                    style: context.style.fg(Color::Gray).bg(syntax::CODE_BACKGROUND),
                };
                let mut label = Row::new(
                    vec![Span::styled(
                        if language.is_empty() {
                            "code"
                        } else {
                            language
                        }
                        .to_owned(),
                        Style::default().fg(Color::DarkGray),
                    )],
                    &context,
                );
                label.code = true;
                rows.push(label);
                let text = plain_text(&node.children);
                if !text.is_empty() {
                    for spans in syntax::highlight(&text, language) {
                        let mut row = Row::new(spans, &context);
                        row.code = true;
                        rows.push(row);
                    }
                }
            }
            Some(Tag::BlockQuote(_)) => {
                let context = Context {
                    prefix: format!("{}│ ", context.prefix),
                    style: context.style.fg(Color::Green),
                };
                rows.extend(blocks(&node.children, source, &context, width));
            }
            Some(Tag::List(start)) => {
                for (index, item) in node.children.iter().enumerate() {
                    let marker = start
                        .map(|start| format!("{}. ", start.saturating_add(index as u64)))
                        .unwrap_or_else(|| "- ".into());
                    let item_context = Context {
                        prefix: format!("{}{}", context.prefix, " ".repeat(marker.width())),
                        style: context.style,
                    };
                    let mut item_rows = blocks(&item.children, source, &item_context, width);
                    if item_rows.is_empty() {
                        item_rows.push(Row::new(Vec::new(), &item_context));
                    }
                    item_rows[0].first.replace_range(
                        ..item_context.prefix.len(),
                        &format!("{}{marker}", context.prefix),
                    );
                    rows.extend(item_rows);
                }
            }
            Some(Tag::Table(alignment)) => rows.extend(table(node, alignment, context, width)),
            Some(Tag::HtmlBlock) => {
                rows.extend(
                    plain_text(&node.children)
                        .trim_end_matches('\n')
                        .split('\n')
                        .map(|text| Row::new(vec![Span::raw(text.to_owned())], context)),
                );
            }
            None if matches!(node.event, Some(Event::Rule)) => rows.push(Row::new(
                vec![Span::styled(
                    "─".repeat(
                        usize::from(width)
                            .saturating_sub(context.prefix.width())
                            .min(36),
                    ),
                    Style::default().fg(Color::DarkGray),
                )],
                context,
            )),
            _ => {}
        }
        previous_end = Some(node.range.end);
        index += 1;
    }
    rows
}

fn is_block(node: &Node) -> bool {
    matches!(
        node.tag,
        Some(
            Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::BlockQuote(_)
                | Tag::List(_)
                | Tag::Table(_)
                | Tag::HtmlBlock
        )
    ) || matches!(node.event, Some(Event::Rule))
}

fn inline_lines(nodes: &[Node], style: Style) -> Vec<Vec<Span<'static>>> {
    let mut lines = vec![Vec::new()];
    append_inline(nodes, style, &mut lines);
    lines
}
fn append_inline(nodes: &[Node], style: Style, lines: &mut Vec<Vec<Span<'static>>>) {
    for node in nodes {
        let mut span =
            |text: String, style| lines.last_mut().unwrap().push(Span::styled(text, style));
        match &node.tag {
            Some(Tag::Emphasis) => {
                append_inline(&node.children, style.add_modifier(Modifier::ITALIC), lines)
            }
            Some(Tag::Strong) => {
                append_inline(&node.children, style.add_modifier(Modifier::BOLD), lines)
            }
            Some(Tag::Strikethrough) => append_inline(
                &node.children,
                style.add_modifier(Modifier::CROSSED_OUT),
                lines,
            ),
            Some(Tag::Link { dest_url, .. }) => {
                let safe = dest_url.starts_with("https://")
                    || dest_url.starts_with("http://")
                    || !dest_url.contains(':');
                let link_style = if safe {
                    style.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED)
                } else {
                    style
                };
                append_inline(&node.children, link_style, lines);
                if safe && plain_text(&node.children) != dest_url.as_ref() {
                    lines.last_mut().unwrap().push(Span::styled(
                        format!(" ({dest_url})"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
            Some(Tag::Image { .. }) => span(
                format!("[Image: {}]", plain_text(&node.children)),
                style.fg(Color::DarkGray),
            ),
            Some(_) => append_inline(&node.children, style, lines),
            None => match &node.event {
                Some(Event::Text(text) | Event::Html(text) | Event::InlineHtml(text)) => {
                    span(text.to_string(), style)
                }
                Some(Event::Code(text)) => span(text.to_string(), style.fg(Color::Cyan)),
                Some(Event::SoftBreak | Event::HardBreak) => lines.push(Vec::new()),
                Some(Event::TaskListMarker(checked)) => span(
                    if *checked { "☑ " } else { "☐ " }.into(),
                    style.fg(if *checked {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Some(Event::FootnoteReference(label)) => span(format!("[{label}]"), style),
                _ => {}
            },
        }
    }
}
fn plain_text(nodes: &[Node]) -> String {
    let mut text = String::new();
    for node in nodes {
        match &node.event {
            Some(
                Event::Text(value)
                | Event::Code(value)
                | Event::Html(value)
                | Event::InlineHtml(value),
            ) => text.push_str(value),
            Some(Event::SoftBreak | Event::HardBreak) => text.push('\n'),
            _ => text.push_str(&plain_text(&node.children)),
        }
    }
    text
}

fn table(node: &Node, alignment: &[Alignment], context: &Context, width: u16) -> Vec<Row> {
    let cells = node
        .children
        .iter()
        .map(|row| {
            row.children
                .iter()
                .map(|cell| {
                    inline_lines(&cell.children, Style::default())
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let widths = (0..columns)
        .map(|column| {
            cells
                .iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.iter().map(Span::width).sum::<usize>())
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let required =
        context.prefix.width() + widths.iter().sum::<usize>() + columns.saturating_sub(1) * 3;
    let mut rows = Vec::new();
    if required <= usize::from(width) {
        for (index, cells) in cells.iter().enumerate() {
            let mut spans = Vec::new();
            for column in 0..columns {
                if column > 0 {
                    spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                }
                let cell = cells.get(column).cloned().unwrap_or_default();
                let spare =
                    widths[column].saturating_sub(cell.iter().map(Span::width).sum::<usize>());
                let left = match alignment.get(column) {
                    Some(Alignment::Right) => spare,
                    Some(Alignment::Center) => spare / 2,
                    _ => 0,
                };
                spans.push(Span::raw(" ".repeat(left)));
                spans.extend(cell);
                spans.push(Span::raw(" ".repeat(spare - left)));
            }
            let mut row = Row::new(spans, context);
            if index == 0 {
                row.style = row
                    .style
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            rows.push(row);
        }
    } else if let Some(headers) = cells.first() {
        for (index, cells) in cells.iter().skip(1).enumerate() {
            if index > 0 {
                rows.push(Row::new(Vec::new(), context));
            }
            for column in 0..columns {
                let header = headers
                    .get(column)
                    .map(|cell| {
                        cell.iter()
                            .map(|span| span.content.as_ref())
                            .collect::<String>()
                    })
                    .unwrap_or_else(|| "Value".into());
                let mut spans = vec![Span::styled(
                    format!("{header}: "),
                    Style::default().add_modifier(Modifier::BOLD),
                )];
                spans.extend(
                    cells
                        .get(column)
                        .cloned()
                        .unwrap_or_else(|| vec![Span::raw("—")]),
                );
                rows.push(Row::new(spans, context));
            }
        }
        if cells.len() == 1 {
            rows.push(Row::new(
                headers.iter().flatten().cloned().collect(),
                context,
            ));
        }
    }
    rows
}

fn terminal_row(row: Row, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1);
    let gutter = row.first.width().min(usize::from(width.saturating_sub(1)));
    let clip = |text: &str| {
        let mut used = 0;
        text.graphemes(true)
            .take_while(|glyph| {
                used += glyph.width();
                used <= gutter
            })
            .collect::<String>()
    };
    let body_width = width.saturating_sub(gutter as u16).max(1);
    let body = if row.code {
        wrap(vec![Line::from(row.spans).style(row.style)], body_width)
    } else {
        wrap_prose(row.spans, body_width)
    };
    body.into_iter()
        .enumerate()
        .map(|(index, body)| {
            let prefix = clip(if index == 0 {
                &row.first
            } else {
                &row.continuation
            });
            let mut spans = vec![Span::raw(prefix)];
            spans.extend(body.spans);
            Line::from(spans).style(row.style)
        })
        .collect()
}

/// Prefer word boundaries for prose; code uses exact grapheme wrapping instead.
fn wrap_prose(spans: Vec<Span<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut rows = Vec::new();
    let mut current = Vec::new();
    let mut used = 0;
    for span in spans {
        for token in span.content.split_word_bounds() {
            let cells = token.width();
            if cells + used > width && used > 0 {
                rows.push(Line::from(std::mem::take(&mut current)));
                used = 0;
                if token.trim().is_empty() {
                    continue;
                }
            }
            if cells > width {
                let mut wrapped = wrap(
                    vec![Line::from(Span::styled(token.to_owned(), span.style))],
                    width as u16,
                );
                let tail = wrapped.pop().unwrap();
                rows.extend(wrapped);
                used = tail.width();
                current = tail.spans;
            } else {
                current.push(Span::styled(token.to_owned(), span.style));
                used += cells;
            }
        }
    }
    rows.push(Line::from(current));
    rows
}
