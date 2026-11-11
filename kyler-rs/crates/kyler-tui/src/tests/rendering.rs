use super::{ready, render};
use crate::agent::AgentHandle;
use crate::{
    app,
    components::{status, transcript},
    effects::Effects,
    layout::View,
    state::{Entry, Phase},
};

use std::{collections::HashSet, time::Instant};

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use kyler_protocol::activity::ActivityKind;

#[test]
fn composer_is_pinned_for_short_and_long_conversations_without_sidebar() {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut view = View::default();
    let mut state = ready();
    state.entries.push(Entry::Message {
        role: "user".into(),
        content: "\u{4f60}\u{597d}".into(),
    });
    let short = render(&mut terminal, &mut view, &mut state);
    assert!(short.contains("kyler"));
    assert!(short.contains("╭┄┄"));
    assert!(short.contains("┆"));
    assert!(short.contains("\u{4f60}\u{597d}"));
    assert!(short.contains("Read-only · Workspace access · Full access"));
    assert!(!short.contains("Total usage"));
    assert!(!short.contains("Enter Send"));
    assert!(!short.contains("This turn"));
    assert!(!short.contains("Session list"));
    let early = view.input.y;
    assert_eq!(view.input.bottom(), 28);
    state.entries.push(Entry::Message {
        role: "assistant".into(),
        content: (0..80)
            .map(|n| format!("\u{7b2c} {n} \u{884c}\n"))
            .collect(),
    });
    state.transcript_version += 1;
    let long = render(&mut terminal, &mut view, &mut state);
    assert!(
        !long.contains("kyler"),
        "Logo must scroll away with history"
    );
    let bottom = view.input.y;
    assert_eq!(bottom, early);
    assert_eq!(view.input.bottom(), 28);
    assert!(state.max_scroll > 0);
    state.entries.push(Entry::Message {
        role: "assistant".into(),
        content: "\u{8ffd}\u{52a0}\u{5185}\u{5bb9}".repeat(100),
    });
    state.transcript_version += 1;
    render(&mut terminal, &mut view, &mut state);
    assert_eq!(view.input.y, bottom);
    state.scroll_by(-isize::MAX);
    let top = render(&mut terminal, &mut view, &mut state);
    assert!(top.contains("kyler") && top.contains("╭┄┄"));
    assert_eq!(view.input.y, bottom);
}

#[test]
fn welcome_logo_is_in_top_border_and_card_is_capped() {
    let mut state = ready();
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    for (width, height) in [(100, 32), (160, 40), (45, 24), (120, 32)] {
        terminal.backend_mut().resize(width, height);
        terminal.autoresize().unwrap();
        state.follow = false;
        state.scroll = 0;
        let screen = render(&mut terminal, &mut view, &mut state);
        let rows: Vec<_> = screen.lines().collect();
        let logo = rows.iter().position(|r| r.contains("kyler")).unwrap();
        let border = rows.iter().position(|r| r.contains("╭┄")).unwrap();
        assert_eq!(logo, border);
        assert!(rows[border].starts_with("  ╭"));
        use unicode_width::UnicodeWidthStr;

        assert_eq!(
            rows[border].trim().width(),
            usize::from((width - 4).min(72))
        );
        assert_eq!(view.input.width, width - 4);
        assert_eq!(view.transcript.width, width - 4);
        assert!(view.input.bottom() < height);
    }
}

#[test]
fn unicode_multiline_input_scrolls_with_visible_cursor_and_preserves_paste() {
    let client = AgentHandle::new(Default::default(), None);
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(128);
    let mut state = ready();
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    let source = "\u{4e2d}\u{6587}🙂 e\u{301}\n".repeat(20);
    assert!(!app::handle_input(
        Input::Paste(source.clone()),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view
    ));
    assert_eq!(state.composer.lines().join("\n"), source);
    assert_eq!(state.phase, Phase::Ready);
    render(&mut terminal, &mut view, &mut state);
    assert!(state.composer.scroll_offset().0 > 0);
    assert!(view.input.height <= 8);
    let cursor = terminal.get_cursor_position().unwrap();
    assert!(view.input.contains(cursor));
    let before = state.composer.lines().len();
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert_eq!(state.composer.lines().len(), before + 1);
    assert_eq!(state.phase, Phase::Ready);
}

#[test]
fn narrow_layouts_resize_without_losing_draft_and_wrap_graphemes() {
    let mut state = ready();
    state
        .composer
        .insert_str("\u{4e2d}\u{6587}🙂e\u{301}\u{4e0d}\u{4f1a}\u{4e22}\u{5931}");
    let draft = state.composer.lines().to_vec();
    let mut view = View::default();
    for (w, h) in [(100, 30), (35, 14), (20, 12), (10, 5), (80, 24)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let _ = render(&mut terminal, &mut view, &mut state);
        assert_eq!(state.composer.lines(), draft.as_slice());
    }
    let lines = transcript::wrap(vec![ratatui::text::Line::from("\u{4e2d}🙂e\u{301}")], 2);
    let texts: Vec<String> = lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(texts, vec!["\u{4e2d}", "🙂", "e\u{301}"]);
}

#[test]
fn older_server_without_window_metadata_does_not_infer_a_ratio() {
    let mut state = ready();
    state.info=Some(serde_json::from_value(serde_json::json!({"model":"legacy-model","context_window_tokens":64000,"workspace_root":null,"working_directory":null})).unwrap());
    assert_eq!(
        status::text(&state),
        "Read-only · Workspace access · Full access"
    );
}

#[test]
fn footer_omits_context_window_budgets() {
    let mut state = ready();
    let info = state.info.as_mut().unwrap();
    info.context_window_tokens = 64000;
    info.sliding_window_tokens = Some(32000);
    assert_eq!(
        status::text(&state),
        "Read-only · Workspace access · Full access"
    );
}

#[test]
fn user_background_fills_wrapped_rows_and_assistant_has_no_name_heading() {
    use ratatui::style::Color;

    let entries = vec![
        Entry::Message {
            role: "user".into(),
            content: "\u{4e2d}\u{6587}🙂 **literal**\nnext".into(),
        },
        Entry::Message {
            role: "assistant".into(),
            content: "\u{4f60}\u{597d}，**\u{6b22}\u{8fce}**。\n```rust\nlet x = 1;\n```".into(),
        },
    ];
    let rows = transcript::wrap(transcript::lines_with_width(&entries, 14), 14);
    let user_rows: Vec<_> = rows
        .iter()
        .filter(|line| line.style.bg == Some(Color::Rgb(55, 60, 70)))
        .collect();
    assert!(user_rows.len() >= 3);
    for row in user_rows {
        assert_eq!(row.style.bg, Some(Color::Rgb(55, 60, 70)));
        assert_eq!(row.width(), 14);
    }
    let text = rows
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("● \u{4f60}\u{597d}"));
    assert!(!text.contains("kyler"));
    assert!(!text.contains("› \u{4f60}"));
    let literal = rows
        .iter()
        .filter(|line| line.style.bg == Some(Color::Rgb(55, 60, 70)))
        .map(|line| {
            let text = line.to_string();
            text.chars()
                .skip(2)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<String>();
    assert!(literal.contains("**literal**"));
}

#[test]
fn borderless_composer_matches_message_width_background_and_spacing() {
    use crate::components::composer::MESSAGE_BACKGROUND;

    let mut state = ready();
    state.entries = vec![
        Entry::Message {
            role: "user".into(),
            content: "hello".into(),
        },
        Entry::Message {
            role: "assistant".into(),
            content: "world".into(),
        },
    ];
    let rows = transcript::lines(&state.entries);
    assert_eq!(rows.len(), 6);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    render(&mut terminal, &mut view, &mut state);
    let buffer = terminal.backend().buffer();
    assert_eq!(view.input.width, view.transcript.width);
    assert_eq!(buffer[(view.input.x, view.input.y + 1)].symbol(), ">");
    assert_eq!(buffer[(view.input.x, view.input.y)].symbol(), " ");
    for y in view.input.y..view.input.bottom() {
        let mut x = view.input.x;
        while x < view.input.right() {
            assert_eq!(buffer[(x, y)].bg, MESSAGE_BACKGROUND);
            use unicode_width::UnicodeWidthStr;

            x += buffer[(x, y)].symbol().width().max(1) as u16;
        }
    }
}

fn tool_entry(name: &str, status: &str) -> Entry {
    Entry::Tool {
        id: name.into(),
        name: name.into(),
        status: status.into(),
        started: Instant::now(),
        seconds: Some(0.1),
        activity: Some(ActivityKind::Explore),
        source: Default::default(),
        output_bytes: Some(16),
        summary: None,
        error: None,
    }
}

#[test]
fn consecutive_tools_share_category_tree_and_cached_updates_keep_every_call() {
    let mut state = ready();
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(160, 32)).unwrap();
    state.entries.extend([
        tool_entry("read_file", "running"),
        tool_entry("list_dir", "success"),
    ]);
    let initial = render(&mut terminal, &mut view, &mut state);
    assert!(initial.contains("Explore · 2 calls"));
    assert!(initial.contains("├─ ◌ read_file"));
    assert!(initial.contains("└─ ✓ list_dir"));
    if let Entry::Tool { status, .. } = &mut state.entries[0] {
        *status = "error".into();
    }
    state.transcript_version += 1;
    let updated = render(&mut terminal, &mut view, &mut state);
    assert!(updated.contains("├─ ! read_file"));
    assert!(updated.contains("Failed"));
    assert!(updated.contains("└─ ✓ list_dir"));
    assert_eq!(updated.matches("list_dir").count(), 1);
    state.entries.push(tool_entry("grep_files", "success"));
    state.transcript_version += 1;
    let appended = render(&mut terminal, &mut view, &mut state);
    assert!(appended.contains("Explore · 3 calls"));
    assert!(appended.contains("├─ ✓ list_dir"));
    assert!(appended.contains("└─ ✓ grep_files"));
    state.entries.push(Entry::Message {
        role: "assistant".into(),
        content: "Reading completed.".into(),
    });
    state.entries.push(tool_entry("skills_resource", "success"));
    state.transcript_version += 1;
    let separated = render(&mut terminal, &mut view, &mut state);
    assert!(separated.find("grep_files").unwrap() < separated.find("Reading completed.").unwrap());
    assert!(
        separated.find("Reading completed.").unwrap() < separated.find("skills_resource").unwrap()
    );
    assert_eq!(separated.matches("list_dir").count(), 1);
    terminal.backend_mut().resize(60, 32);
    terminal.autoresize().unwrap();
    let narrow = render(&mut terminal, &mut view, &mut state);
    for name in ["read_file", "list_dir", "grep_files", "skills_resource"] {
        assert_eq!(narrow.matches(name).count(), 1);
    }
    assert!(
        !narrow
            .lines()
            .any(|row| row.contains("read_file") && row.contains("list_dir"))
    );
}

#[test]
fn tool_rows_wrap_long_unicode_labels_without_clipping_or_losing_calls() {
    let entries = [
        tool_entry("\u{6587}\u{4ef6}🙂e\u{301}_reader", "running"),
        tool_entry("list_dir", "success"),
    ];
    let rows = transcript::tool_rows(&entries, 12);
    assert!(rows.iter().all(|row| row.width() <= 12));
    let text = rows
        .iter()
        .filter_map(|row| {
            let text = row.to_string();
            ["  ├─ ", "  └─ ", "  │  ", "     "]
                .iter()
                .find_map(|prefix| text.strip_prefix(prefix).map(str::to_owned))
        })
        .collect::<String>();
    assert!(text.contains("\u{6587}\u{4ef6}🙂e\u{301}_reader"));
    assert!(text.contains("list_dir"));
    assert!(!text.contains('�'));
}

#[test]
fn tool_category_trees_preserve_execution_order_across_category_changes() {
    let read = tool_entry("read_file", "success");
    let mut command = tool_entry("run_command", "running");
    if let Entry::Tool { activity, .. } = &mut command {
        *activity = Some(ActivityKind::Execute);
    }
    let entries = [read, command, tool_entry("list_dir", "success")];
    let rows = transcript::tool_rows(&entries, 80);
    let text = rows
        .iter()
        .map(|row| row.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text.matches("Explore · 1 call").count(), 2);
    assert_eq!(text.matches("Execute · 1 call").count(), 1);
    assert!(text.find("read_file").unwrap() < text.find("run_command").unwrap());
    assert!(text.find("run_command").unwrap() < text.find("list_dir").unwrap());
    assert_eq!(text.matches("└─").count(), 3);
}

#[test]
fn assistant_tool_spacing_trims_boundary_newlines_but_preserves_paragraphs() {
    let mut state = ready();
    state.entries.push(Entry::Message {
        role: "assistant".into(),
        content: "\n\nChecking files.\n\n   \n".into(),
    });
    state.entries.push(tool_entry("read_file", "success"));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut view = View::default();
    let screen = render(&mut terminal, &mut view, &mut state);
    let rows = screen.lines().collect::<Vec<_>>();
    let message = rows
        .iter()
        .position(|row| row.contains("Checking files."))
        .unwrap();
    let tools = rows
        .iter()
        .position(|row| row.contains("Explore · 1 call"))
        .unwrap();
    assert_eq!(
        tools,
        message + 2,
        "Only one separator row should precede a tool group"
    );
    let paragraphs = transcript::lines(&[Entry::Message {
        role: "assistant".into(),
        content: "\nFirst paragraph.\n\nSecond paragraph.\n\n".into(),
    }]);
    assert_eq!(paragraphs.len(), 4);
    assert!(paragraphs[0].to_string().contains("First paragraph."));
    assert!(paragraphs[1].to_string().trim().is_empty());
    assert!(paragraphs[2].to_string().contains("Second paragraph."));
}

#[test]
fn footer_shows_three_fixed_presets_and_highlights_only_the_current_preset() {
    let mut state = ready();
    state.permissions = Some(serde_json::from_value(serde_json::json!({
        "sandbox":"workspace-write", "approval":{"policy":"on-request","reviewer":"user"},
        "selected":{"mode":"workspace_access","revision":"one","approval":{"policy":"on-request","reviewer":"user"}},
        "active":null,"options":[],"workspace":"/project","ceiling":"Workspace write"
    })).unwrap());
    let text = status::text(&state);
    assert_eq!(text, "Read-only · Workspace access · Full access");
    for hidden in [
        "workspace-write",
        "on-request",
        "auto_review",
        "reviewer",
        "▼",
        "▾",
    ] {
        assert!(!text.contains(hidden));
    }
    use ratatui::style::{Color, Modifier};

    use kyler_protocol::api::permissions::PermissionMode;

    for mode in PermissionMode::ALL {
        state.permissions.as_mut().unwrap().selected.mode = mode;
        let footer = status::footer(&state);
        assert_eq!(
            footer.to_string(),
            "Read-only · Workspace access · Full access"
        );
        let highlighted = footer
            .spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::BOLD))
            .collect::<Vec<_>>();
        assert_eq!(highlighted.len(), 1);
        assert_eq!(highlighted[0].content, mode.label());
        assert_eq!(highlighted[0].style.fg, Some(Color::Cyan));
    }
    state.permissions.as_mut().unwrap().selected.mode = PermissionMode::AutoReview;
    assert!(
        status::footer(&state)
            .spans
            .iter()
            .any(|span| span.content == "Workspace access"
                && span.style.add_modifier.contains(Modifier::BOLD))
    );
}

#[test]
fn markdown_blocks_keep_gutters_styles_and_unicode_at_narrow_widths() {
    use crate::components::markdown;

    use ratatui::style::{Color, Modifier};

    let text = "# Findings\n\n- A long list item with Unicode \u{4e2d}\u{6587} and **bold** text.\n> A quotation that wraps onto several rows.\n\n```rust\n    let value = 42;\n\n    value\n```\nAfter the code.";
    let rows = markdown::render(text, 24);
    assert!(rows.iter().all(|row| row.width() <= 24));
    assert!(rows[0].to_string().starts_with("● Findings"));
    assert!(rows[0].style.add_modifier.contains(Modifier::BOLD));
    let list = rows
        .iter()
        .position(|row| row.to_string().starts_with("  - "))
        .unwrap();
    assert!(rows[list + 1].to_string().starts_with("    "));
    let quote = rows
        .iter()
        .position(|row| row.to_string().starts_with("  │ "))
        .unwrap();
    assert!(rows[quote + 1].to_string().starts_with("  │ "));
    let code = rows
        .iter()
        .filter(|row| row.style.bg == Some(Color::Rgb(35, 39, 47)))
        .collect::<Vec<_>>();
    assert_eq!(code.len(), 4);
    assert!(code.iter().all(|row| row.width() == 24));
    assert!(code[1].to_string().starts_with("      let"));
    assert!(code[2].to_string().trim().is_empty());
    assert!(rows.last().unwrap().to_string().contains("After the code."));
    assert_eq!(rows.last().unwrap().style.bg, None);
}

#[test]
fn markdown_tables_adapt_to_width_without_losing_cells_or_links() {
    use crate::components::markdown;

    let text = "| Name | Result |\n| :--- | ---: |\n| Parser | **Passed** |\n| Renderer | Ready |\n\n[Documentation](https://example.com/docs)";
    let wide = markdown::render(text, 80);
    assert!(
        wide.iter()
            .any(|row| row.to_string().contains("Name") && row.to_string().contains("│"))
    );
    assert!(!wide.iter().any(|row| row.to_string().contains(":---")));
    let narrow = markdown::render(text, 16);
    assert!(narrow.iter().all(|row| row.width() <= 16));
    let text = narrow
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Name: Parser"));
    assert!(text.contains("Result: Passed"));
    assert!(text.contains("Name: Renderer"));
    let flattened = narrow
        .iter()
        .flat_map(|row| row.spans.iter())
        .map(|span| span.content.trim())
        .collect::<String>();
    assert!(flattened.contains("https://example.com/docs"));
}

#[test]
fn incomplete_streaming_fences_and_inline_markup_keep_visible_content() {
    use crate::components::markdown;

    let open = markdown::render("```rust\n    let value = 42;", 40);
    let closed = markdown::render("```rust\n    let value = 42;\n```", 40);
    assert_eq!(
        open, closed,
        "Closing a fence must not shift existing code rows"
    );
    let rows = markdown::render("Reading **unfinished\n\nNext paragraph.\n\n", 40);
    assert!(rows[0].to_string().contains("**unfinished"));
    assert!(rows[1].to_string().trim().is_empty());
    assert_eq!(rows.len(), 3);
    let sanitized = markdown::render("hello\u{1b}[31m\u{7}world", 40);
    assert!(
        sanitized
            .iter()
            .all(|row| !row.to_string().contains('\u{1b}') && !row.to_string().contains('\u{7}'))
    );
}

#[test]
fn user_wrapping_retains_gutter_and_tool_states_are_distinct() {
    let rows = transcript::lines_with_width(
        &[Entry::Message {
            role: "user".into(),
            content: "a long message that wraps across rows".into(),
        }],
        16,
    );
    assert!(rows[1].to_string().starts_with("> "));
    assert!(rows[2].to_string().starts_with("  "));
    for (status, label) in [
        ("cancelled", "Cancelled"),
        ("denied", "Permission denied"),
        ("pending", "Awaiting approval"),
        ("error", "Failed"),
    ] {
        let text = transcript::tool_rows(&[tool_entry("read_file", status)], 80)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(label));
        if status != "error" {
            assert!(!text.contains("Failed"));
        }
    }
}

#[test]
fn scrollback_stays_on_same_message_during_output_and_resize() {
    let mut state = ready();
    state.entries = (0..20)
        .map(|index| Entry::Message {
            role: "assistant".into(),
            content: format!("Message {index:02}: {}", "readable content ".repeat(8)),
        })
        .collect();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut view = View::default();
    render(&mut terminal, &mut view, &mut state);
    state.scroll_by(-30);
    let before = render(&mut terminal, &mut view, &mut state);
    let scroll = state.scroll;
    state.entries.push(Entry::Message {
        role: "assistant".into(),
        content: "New response".into(),
    });
    state.transcript_version += 1;
    let updated = render(&mut terminal, &mut view, &mut state);
    assert_eq!(state.scroll, scroll);
    assert!(updated.contains("New activity"));
    let marker = before
        .lines()
        .find_map(|line| {
            let start = line.find("Message ")?;
            Some(line[start..start + 10].to_owned())
        })
        .unwrap();
    terminal.backend_mut().resize(60, 30);
    terminal.autoresize().unwrap();
    let resized = render(&mut terminal, &mut view, &mut state);
    assert!(
        resized.contains(&marker),
        "Resizing should preserve the message being read"
    );
    state.scroll_by(isize::MAX);
    let latest = render(&mut terminal, &mut view, &mut state);
    assert!(state.follow);
    assert!(latest.contains("New response"));
    assert!(!latest.contains("New activity"));
}

#[test]
fn commonmark_styles_nested_markup_escapes_lists_and_literal_user_input() {
    use crate::components::markdown;

    use ratatui::style::{Color, Modifier};

    let rows = markdown::render(
        "# One\n## Two\n### Three\n\n**bold _and italic_** with ~~removed~~ and `code` and \\*literal\\*.\n\n- parent\n  - child\n- [x] done\n\n> quote",
        60,
    );
    assert!(rows[0].style.add_modifier.contains(Modifier::UNDERLINED));
    assert!(rows[1].style.add_modifier.contains(Modifier::BOLD));
    assert!(rows[2].style.add_modifier.contains(Modifier::ITALIC));
    assert!(rows.iter().flat_map(|row| &row.spans).any(|span| {
        span.content.contains("italic")
            && span
                .style
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)
    }));
    assert!(
        rows.iter()
            .flat_map(|row| &row.spans)
            .any(|span| span.content.contains("removed")
                && span.style.add_modifier.contains(Modifier::CROSSED_OUT))
    );
    assert!(
        rows.iter()
            .flat_map(|row| &row.spans)
            .any(|span| span.content.contains("code") && span.style.fg == Some(Color::Cyan))
    );
    let text = rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("*literal*"));
    assert!(text.contains("    - child"));
    assert!(text.contains("☑ done"));
    assert!(
        rows.iter()
            .any(|row| row.to_string().contains("│ quote") && row.style.fg == Some(Color::Green))
    );
    let user = crate::components::messages::user::render("**hello** _world_", 60);
    assert!(
        user.iter()
            .any(|row| row.to_string().contains("**hello** _world_"))
    );
}

#[test]
fn syntax_highlighting_preserves_code_and_unknown_language_is_readable() {
    use crate::components::{markdown, syntax};

    let source = "fn main() {\n    let name = \"Kyler\";\n}\n";
    let rows = syntax::highlight(source, "rust");
    let text = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text, source.trim_end_matches('\n'));
    let colors = rows
        .iter()
        .flatten()
        .filter_map(|span| span.style.fg)
        .collect::<HashSet<_>>();
    assert!(colors.len() > 1);
    let rows = markdown::render("```unknown-language\n    keep **literal**\n```", 40);
    assert!(
        rows.iter()
            .any(|row| row.to_string().contains("    keep **literal**"))
    );
    assert!(rows.iter().all(|row| row.width() <= 40));
}

#[test]
fn commonmark_table_uses_rendered_width_and_preserves_escaped_pipes() {
    let rows = crate::components::markdown::render(
        "| Name | Value |\n| --- | --- |\n| A\\|B | **Ready** |",
        30,
    );
    let text = rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("A|B"));
    assert!(text.contains("Ready"));
    assert!(!text.contains("**"));
    assert!(rows.iter().all(|row| row.width() <= 30));
}

#[test]
fn composer_keeps_a_clean_right_edge_without_a_second_block_cursor() {
    use crate::components::composer::MESSAGE_BACKGROUND;
    use ratatui::style::Modifier;

    let mut state = ready();
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(42, 20)).unwrap();
    for text in ["", "short", &"long text ".repeat(60)] {
        state.composer = crate::components::composer::new();
        state.composer.insert_str(text);
        render(&mut terminal, &mut view, &mut state);
        let buffer = terminal.backend().buffer();
        for y in view.input.y..view.input.bottom() {
            let edge = &buffer[(view.input.right() - 1, y)];
            assert_eq!(edge.symbol(), " ");
            assert_eq!(edge.bg, MESSAGE_BACKGROUND);
            for x in view.input.x..view.input.right() {
                assert!(!buffer[(x, y)].modifier.contains(Modifier::REVERSED));
            }
        }
    }
}

#[test]
fn structured_history_restores_tool_tree_and_keeps_all_assistant_text() {
    use kyler_protocol::api::history::{HistoryEvent, HistoryExecution};
    let events: Vec<HistoryEvent> = serde_json::from_value(serde_json::json!([
        {"id":"u","execution_id":"e","timestamp_ms":1000,"content":[{"type":"message","role":"user","text":"Inspect"},{"type":"attachment","attachment":{"id":"file","name":"notes.txt","kind":"text","bytes":12,"sha256":"hash","created_at":1000}}]},
        {"id":"a","execution_id":"e","timestamp_ms":1100,"content":[{"type":"message","role":"assistant","text":"Reading the project."}]},
        {"id":"t","execution_id":"e","timestamp_ms":1200,"content":[{"type":"tool_call","call_id":"call","name":"read_file","arguments":{},"summary":"main.rs","presentation":{"step":1,"activity":"explore","source":{"kind":"builtin"},"started_at":1200,"finished_at":1800}}]},
        {"id":"r","execution_id":"e","timestamp_ms":1800,"content":[{"type":"tool_result","call_id":"call","name":"read_file","status":"success","output":"contents","output_bytes":8,"truncated":false}]},
        {"id":"f","execution_id":"e","timestamp_ms":2000,"content":[{"type":"message","role":"assistant","text":"Verified the result."}]}
    ])).unwrap();
    let mut state = ready();
    state.history(
        events,
        vec![HistoryExecution {
            execution_id: "e".into(),
            status: "completed".into(),
            started_at: 1000,
            finished_at: Some(2000),
        }],
    );
    let lines = transcript::lines(&state.entries)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Reading the project.",
        "Verified the result.",
        "Explore",
        "read_file",
        "notes.txt",
        "Completed",
    ] {
        assert!(lines.contains(expected), "missing {expected}");
    }
    assert!(state.entries.iter().any(|entry| matches!(entry, Entry::Tool { status, output_bytes: Some(8), seconds: Some(seconds), .. } if status == "success" && *seconds == 0.6)));
}
