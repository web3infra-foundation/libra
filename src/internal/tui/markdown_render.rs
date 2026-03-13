//! Lightweight Markdown renderer for the TUI transcript.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme;

#[derive(Clone, Copy)]
struct MarkdownStyles {
    heading: Style,
    heading_marker: Style,
    emphasis: Style,
    strong: Style,
    code_inline: Style,
    code_block: Style,
    link: Style,
    blockquote: Style,
    bullet: Style,
    ordered: Style,
    table_border: Style,
    table_header: Style,
}

impl Default for MarkdownStyles {
    fn default() -> Self {
        Self {
            heading: theme::text::primary().add_modifier(Modifier::BOLD),
            heading_marker: theme::markdown::heading_marker(),
            emphasis: Style::default().add_modifier(Modifier::ITALIC),
            strong: Style::default().add_modifier(Modifier::BOLD),
            code_inline: theme::markdown::code_inline(),
            code_block: theme::markdown::code_block(),
            link: theme::markdown::link(),
            blockquote: theme::markdown::blockquote(),
            bullet: theme::markdown::bullet(),
            ordered: theme::markdown::ordered(),
            table_border: theme::markdown::table_border(),
            table_header: theme::markdown::table_header(),
        }
    }
}

#[derive(Clone)]
struct InlineSpan {
    text: String,
    style: Style,
}

#[derive(Clone)]
enum PrefixKind {
    Plain,
    BlockQuote,
    ListMarker,
}

#[derive(Clone)]
struct PrefixSegment {
    text: String,
    style: Style,
    kind: PrefixKind,
}

#[derive(Clone)]
struct PendingListItem {
    marker: String,
    style: Style,
    indent: usize,
}

#[derive(Clone, Default)]
struct TableCellContent {
    segments: Vec<InlineSpan>,
}

#[derive(Clone)]
struct TableState {
    alignments: Vec<Alignment>,
    header: Vec<TableCellContent>,
    rows: Vec<Vec<TableCellContent>>,
    current_row: Vec<TableCellContent>,
    current_cell: Vec<InlineSpan>,
    in_header: bool,
    in_cell: bool,
}

impl TableState {
    fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            header: Vec::new(),
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: Vec::new(),
            in_header: false,
            in_cell: false,
        }
    }
}

pub fn render_markdown_lines(input: &str, width: u16) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(input, options);
    let mut renderer = Renderer::new(width as usize);
    renderer.render(parser);
    renderer.into_lines()
}

struct Renderer {
    width: usize,
    styles: MarkdownStyles,
    lines: Vec<Line<'static>>,
    current_segments: Vec<InlineSpan>,
    inline_style_stack: Vec<Style>,
    prefix_stack: Vec<PrefixSegment>,
    list_stack: Vec<Option<u64>>,
    pending_list_item: Option<PendingListItem>,
    pending_heading: Option<HeadingLevel>,
    active_link_dest: Option<String>,
    in_code_block: bool,
    needs_block_spacing: bool,
    table_state: Option<TableState>,
}

impl Renderer {
    fn new(width: usize) -> Self {
        Self {
            width: width.max(8),
            styles: MarkdownStyles::default(),
            lines: Vec::new(),
            current_segments: Vec::new(),
            inline_style_stack: Vec::new(),
            prefix_stack: Vec::new(),
            list_stack: Vec::new(),
            pending_list_item: None,
            pending_heading: None,
            active_link_dest: None,
            in_code_block: false,
            needs_block_spacing: false,
            table_state: None,
        }
    }

    fn render<'a>(&mut self, parser: Parser<'a>) {
        for event in parser {
            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) => self.push_text(text.as_ref()),
                Event::Code(code) => self.push_inline(code.as_ref(), self.styles.code_inline),
                Event::SoftBreak => {
                    if self.in_code_block {
                        self.flush_line(false);
                    } else {
                        self.push_text(" ");
                    }
                }
                Event::HardBreak => self.flush_line(false),
                Event::Rule => {
                    self.ensure_block_spacing();
                    self.lines.push(Line::from(vec![Span::styled(
                        "─".repeat(self.width.min(24)),
                        theme::text::subtle(),
                    )]));
                    self.needs_block_spacing = true;
                }
                Event::Html(html) | Event::InlineHtml(html) => self.push_text(html.as_ref()),
                Event::InlineMath(math) | Event::DisplayMath(math) => {
                    self.push_inline(math.as_ref(), self.styles.code_inline)
                }
                Event::FootnoteReference(_) | Event::TaskListMarker(_) => {}
            }
        }
        self.flush_line(false);
    }

    fn into_lines(mut self) -> Vec<Line<'static>> {
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }

    fn start_tag<'a>(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.ensure_block_spacing(),
            Tag::Heading { level, .. } => {
                self.ensure_block_spacing();
                self.pending_heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.ensure_block_spacing();
                self.prefix_stack.push(PrefixSegment {
                    text: "> ".to_string(),
                    style: self.styles.blockquote,
                    kind: PrefixKind::BlockQuote,
                });
            }
            Tag::List(start) => {
                self.ensure_block_spacing();
                self.list_stack.push(start);
            }
            Tag::Item => self.start_list_item(),
            Tag::CodeBlock(kind) => {
                self.ensure_block_spacing();
                self.in_code_block = true;
                if let CodeBlockKind::Fenced(lang) = kind
                    && !lang.is_empty()
                {
                    self.lines.push(Line::from(vec![Span::styled(
                        format!("  [{lang}]"),
                        self.styles.code_block.add_modifier(Modifier::BOLD),
                    )]));
                }
                self.prefix_stack.push(PrefixSegment {
                    text: "  ".to_string(),
                    style: self.styles.code_block,
                    kind: PrefixKind::Plain,
                });
            }
            Tag::Table(alignments) => self.start_table(alignments),
            Tag::TableHead => self.start_table_head(),
            Tag::TableRow => self.start_table_row(),
            Tag::TableCell => self.start_table_cell(),
            Tag::Emphasis => self.push_style(self.styles.emphasis),
            Tag::Strong => self.push_style(self.styles.strong),
            Tag::Strikethrough => {
                self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT))
            }
            Tag::Link { dest_url, .. } => {
                self.active_link_dest = Some(dest_url.to_string());
                self.push_style(self.styles.link);
            }
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Image { .. }
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line(false);
                self.needs_block_spacing = true;
            }
            TagEnd::Heading(_) => {
                self.flush_line(false);
                self.pending_heading = None;
                self.needs_block_spacing = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line(false);
                self.pop_prefix(PrefixKind::BlockQuote);
                self.needs_block_spacing = true;
            }
            TagEnd::List(_) => {
                self.flush_line(false);
                self.list_stack.pop();
                self.needs_block_spacing = true;
            }
            TagEnd::Item => {
                self.flush_line(false);
                self.needs_block_spacing = false;
            }
            TagEnd::CodeBlock => {
                self.flush_line(false);
                self.in_code_block = false;
                self.pop_prefix(PrefixKind::Plain);
                self.needs_block_spacing = true;
            }
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => self.end_table_head(),
            TagEnd::TableRow => self.end_table_row(),
            TagEnd::TableCell => self.end_table_cell(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_style_stack.pop();
            }
            TagEnd::Link => {
                self.inline_style_stack.pop();
                if let Some(dest) = self.active_link_dest.take() {
                    self.push_inline(" (", Style::default());
                    self.push_inline(&dest, self.styles.link);
                    self.push_inline(")", Style::default());
                }
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Image
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn push_style(&mut self, style: Style) {
        let current = self.inline_style_stack.last().copied().unwrap_or_default();
        self.inline_style_stack.push(current.patch(style));
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.table_state.as_ref().is_some_and(|table| table.in_cell) {
            let mut style = self.inline_style_stack.last().copied().unwrap_or_default();
            if self.pending_heading.is_some() {
                style = self.styles.heading.patch(style);
            }
            self.push_inline(text, style);
            return;
        }
        let mut style = self.inline_style_stack.last().copied().unwrap_or_default();
        if self.pending_heading.is_some() {
            style = self.styles.heading.patch(style);
        }
        if self.in_code_block {
            self.push_code_block_text(text, self.styles.code_block.patch(style));
            return;
        }
        self.push_inline(text, style);
    }

    fn push_inline(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if let Some(table) = self.table_state.as_mut()
            && table.in_cell
        {
            table.current_cell.push(InlineSpan {
                text: text.to_string(),
                style,
            });
            return;
        }
        self.current_segments.push(InlineSpan {
            text: text.to_string(),
            style,
        });
    }

    fn start_list_item(&mut self) {
        self.flush_line(false);
        let depth = self.list_stack.len().max(1);
        let marker = if let Some(last) = self.list_stack.last_mut() {
            let marker = if let Some(index) = last {
                let marker = format!("{}. ", *index);
                *index += 1;
                PendingListItem {
                    indent: marker.width(),
                    marker,
                    style: self.styles.ordered,
                }
            } else {
                PendingListItem {
                    indent: 2,
                    marker: "• ".to_string(),
                    style: self.styles.bullet,
                }
            };
            marker
        } else {
            PendingListItem {
                indent: 2,
                marker: "• ".to_string(),
                style: self.styles.bullet,
            }
        };
        let nesting_prefix = "  ".repeat(depth.saturating_sub(1));
        self.pending_list_item = Some(PendingListItem {
            indent: marker.indent + nesting_prefix.width(),
            marker: format!("{nesting_prefix}{}", marker.marker),
            style: marker.style,
        });
    }

    fn ensure_block_spacing(&mut self) {
        self.flush_line(false);
        if self.needs_block_spacing && !self.lines.is_empty() && !self.last_line_is_blank() {
            self.lines.push(Line::default());
        }
        self.needs_block_spacing = false;
    }

    fn last_line_is_blank(&self) -> bool {
        self.lines
            .last()
            .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
    }

    fn flush_line(&mut self, force_blank: bool) {
        if self.table_state.as_ref().is_some_and(|table| table.in_cell) {
            self.push_inline(" ", Style::default());
            return;
        }
        let prefixes = self.build_prefixes();
        if self.current_segments.is_empty() {
            if force_blank || prefixes.iter().any(|prefix| !prefix.text.is_empty()) {
                self.lines.push(prefixes_to_line(&prefixes));
            }
            self.pending_list_item = None;
            return;
        }

        if self.in_code_block {
            let mut spans = prefix_spans(&prefixes, false);
            spans.extend(
                self.current_segments
                    .drain(..)
                    .map(|segment| Span::styled(segment.text, segment.style)),
            );
            self.lines.push(Line::from(spans));
            self.pending_list_item = None;
            return;
        }

        let wrapped = wrap_segments(
            self.current_segments.drain(..).collect(),
            self.width,
            &prefixes,
        );
        self.lines.extend(wrapped);
        self.pending_list_item = None;
    }

    fn build_prefixes(&self) -> Vec<PrefixSegment> {
        let mut prefixes = self.prefix_stack.clone();
        if let Some(level) = self.pending_heading {
            prefixes.push(PrefixSegment {
                text: format!("{} ", "#".repeat(level as usize)),
                style: self.styles.heading_marker,
                kind: PrefixKind::Plain,
            });
        }
        if let Some(item) = &self.pending_list_item {
            prefixes.push(PrefixSegment {
                text: item.marker.clone(),
                style: item.style,
                kind: PrefixKind::ListMarker,
            });
        }
        prefixes
    }

    fn pop_prefix(&mut self, kind: PrefixKind) {
        if let Some(index) = self.prefix_stack.iter().rposition(|segment| {
            std::mem::discriminant(&segment.kind) == std::mem::discriminant(&kind)
        }) {
            self.prefix_stack.remove(index);
        }
    }

    fn start_table(&mut self, alignments: Vec<Alignment>) {
        self.ensure_block_spacing();
        self.table_state = Some(TableState::new(alignments));
    }

    fn start_table_head(&mut self) {
        if let Some(table) = self.table_state.as_mut() {
            table.in_header = true;
        }
    }

    fn end_table_head(&mut self) {
        if let Some(table) = self.table_state.as_mut() {
            if table.header.is_empty() && !table.current_row.is_empty() {
                table.header = std::mem::take(&mut table.current_row);
            }
            table.in_header = false;
        }
    }

    fn start_table_row(&mut self) {
        if let Some(table) = self.table_state.as_mut() {
            table.current_row.clear();
        }
    }

    fn end_table_row(&mut self) {
        if let Some(table) = self.table_state.as_mut() {
            let row = std::mem::take(&mut table.current_row);
            if table.header.is_empty() && table.in_header {
                table.header = row;
            } else if !row.is_empty() {
                table.rows.push(row);
            }
        }
    }

    fn start_table_cell(&mut self) {
        if let Some(table) = self.table_state.as_mut() {
            table.current_cell.clear();
            table.in_cell = true;
        }
    }

    fn end_table_cell(&mut self) {
        if let Some(table) = self.table_state.as_mut() {
            let cell = TableCellContent {
                segments: std::mem::take(&mut table.current_cell),
            };
            table.current_row.push(cell);
            table.in_cell = false;
        }
    }

    fn end_table(&mut self) {
        let Some(table) = self.table_state.take() else {
            return;
        };
        let lines = render_table(table, self.width, &self.styles);
        self.lines.extend(lines);
        self.needs_block_spacing = true;
    }

    fn push_code_block_text(&mut self, text: &str, style: Style) {
        for (idx, line) in text.split('\n').enumerate() {
            if idx > 0 {
                self.flush_line(false);
            }
            if !line.is_empty() {
                self.push_inline(line, style);
            }
        }
    }
}

fn prefixes_to_line(prefixes: &[PrefixSegment]) -> Line<'static> {
    Line::from(prefix_spans(prefixes, false))
}

fn prefix_spans(prefixes: &[PrefixSegment], subsequent: bool) -> Vec<Span<'static>> {
    prefixes
        .iter()
        .map(|segment| {
            let text = if subsequent && matches!(segment.kind, PrefixKind::ListMarker) {
                " ".repeat(segment.text.width())
            } else {
                segment.text.clone()
            };
            Span::styled(text, segment.style)
        })
        .collect()
}

fn wrap_segments(
    segments: Vec<InlineSpan>,
    total_width: usize,
    prefixes: &[PrefixSegment],
) -> Vec<Line<'static>> {
    let first_prefix = prefix_spans(prefixes, false);
    let rest_prefix = prefix_spans(prefixes, true);
    let first_width = total_width
        .saturating_sub(spans_width(&first_prefix))
        .max(1);
    let rest_width = total_width.saturating_sub(spans_width(&rest_prefix)).max(1);

    let tokens = tokenize_segments(&segments);
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0usize;
    let mut line_prefix = first_prefix.clone();
    let mut available = first_width;

    for token in tokens {
        let token_width = token.text.width();
        if token.is_whitespace {
            if current.is_empty() {
                continue;
            }
            if current_width + token_width > available {
                lines.push(assemble_line(&line_prefix, std::mem::take(&mut current)));
                line_prefix = rest_prefix.clone();
                available = rest_width;
                current_width = 0;
                continue;
            }
            current_width += token_width;
            current.push(Span::styled(token.text, token.style));
            continue;
        }

        if current_width + token_width <= available {
            current_width += token_width;
            current.push(Span::styled(token.text, token.style));
            continue;
        }

        if !current.is_empty() {
            lines.push(assemble_line(&line_prefix, std::mem::take(&mut current)));
            line_prefix = rest_prefix.clone();
            available = rest_width;
            current_width = 0;
        }

        if token_width <= available {
            current_width = token_width;
            current.push(Span::styled(token.text, token.style));
            continue;
        }

        for chunk in split_text_to_width(&token.text, available) {
            let chunk_width = chunk.width();
            if current_width > 0 && current_width + chunk_width > available {
                lines.push(assemble_line(&line_prefix, std::mem::take(&mut current)));
                line_prefix = rest_prefix.clone();
                available = rest_width;
                current_width = 0;
            }
            current_width += chunk_width;
            current.push(Span::styled(chunk, token.style));
            if current_width >= available {
                lines.push(assemble_line(&line_prefix, std::mem::take(&mut current)));
                line_prefix = rest_prefix.clone();
                available = rest_width;
                current_width = 0;
            }
        }
    }

    if !current.is_empty() {
        lines.push(assemble_line(&line_prefix, current));
    } else if lines.is_empty() {
        lines.push(Line::from(first_prefix));
    }

    lines
}

fn assemble_line(prefix: &[Span<'static>], mut content: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = prefix.to_vec();
    spans.append(&mut content);
    Line::from(spans)
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

#[derive(Clone)]
struct Token {
    text: String,
    style: Style,
    is_whitespace: bool,
}

fn tokenize_segments(segments: &[InlineSpan]) -> Vec<Token> {
    let mut tokens = Vec::new();
    for segment in segments {
        let mut buffer = String::new();
        let mut last_whitespace = None;
        for ch in segment.text.chars() {
            let is_whitespace = ch.is_whitespace();
            if last_whitespace.is_some_and(|last| last != is_whitespace) && !buffer.is_empty() {
                tokens.push(Token {
                    text: std::mem::take(&mut buffer),
                    style: segment.style,
                    is_whitespace: last_whitespace.unwrap_or(false),
                });
            }
            buffer.push(ch);
            last_whitespace = Some(is_whitespace);
        }
        if !buffer.is_empty() {
            tokens.push(Token {
                text: buffer,
                style: segment.style,
                is_whitespace: last_whitespace.unwrap_or(false),
            });
        }
    }
    tokens
}

fn split_text_to_width(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut width = 0usize;

    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if width + ch_width > max_width && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
            width = 0;
        }
        current.push(ch);
        width += ch_width;
    }

    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

fn render_table(
    table: TableState,
    total_width: usize,
    styles: &MarkdownStyles,
) -> Vec<Line<'static>> {
    let col_count = table
        .alignments
        .len()
        .max(table.header.len())
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
    if col_count == 0 {
        return Vec::new();
    }

    let header = normalize_row(table.header, col_count);
    let rows = table
        .rows
        .into_iter()
        .map(|row| normalize_row(row, col_count))
        .collect::<Vec<_>>();
    let alignments = normalize_alignments(table.alignments, col_count);
    let widths = compute_table_widths(&header, &rows, total_width, col_count);

    let mut out = Vec::new();
    out.push(table_border_line(
        '┌',
        '┬',
        '┐',
        &widths,
        styles.table_border,
    ));
    if !header.is_empty() {
        out.extend(render_table_row(
            &header,
            &widths,
            &alignments,
            true,
            styles,
        ));
        out.push(table_border_line(
            '├',
            '┼',
            '┤',
            &widths,
            styles.table_border,
        ));
    }
    for row in &rows {
        out.extend(render_table_row(row, &widths, &alignments, false, styles));
    }
    out.push(table_border_line(
        '└',
        '┴',
        '┘',
        &widths,
        styles.table_border,
    ));
    out
}

fn normalize_row(mut row: Vec<TableCellContent>, col_count: usize) -> Vec<TableCellContent> {
    while row.len() < col_count {
        row.push(TableCellContent::default());
    }
    row.truncate(col_count);
    row
}

fn normalize_alignments(mut alignments: Vec<Alignment>, col_count: usize) -> Vec<Alignment> {
    while alignments.len() < col_count {
        alignments.push(Alignment::None);
    }
    alignments.truncate(col_count);
    alignments
}

fn compute_table_widths(
    header: &[TableCellContent],
    rows: &[Vec<TableCellContent>],
    total_width: usize,
    col_count: usize,
) -> Vec<usize> {
    let frame_width = 3 * col_count + 1;
    let budget = total_width.saturating_sub(frame_width).max(col_count);
    let min_col_width = if budget >= col_count * 3 { 3 } else { 1 };
    let mut widths = vec![min_col_width; col_count];

    for (idx, cell) in header.iter().enumerate() {
        widths[idx] = widths[idx].max(cell_plain_width(cell));
    }
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell_plain_width(cell));
        }
    }

    let mut sum: usize = widths.iter().sum();
    while sum > budget {
        let Some((idx, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, width)| **width > min_col_width)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[idx] -= 1;
        sum -= 1;
    }

    widths
}

fn cell_plain_width(cell: &TableCellContent) -> usize {
    let joined = cell
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    joined
        .lines()
        .map(str::trim)
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

fn render_table_row(
    row: &[TableCellContent],
    widths: &[usize],
    alignments: &[Alignment],
    is_header: bool,
    styles: &MarkdownStyles,
) -> Vec<Line<'static>> {
    let rendered_cells = row
        .iter()
        .zip(widths.iter())
        .map(|(cell, width)| render_table_cell(cell, *width, is_header, styles))
        .collect::<Vec<_>>();
    let max_lines = rendered_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut out = Vec::with_capacity(max_lines);

    for line_idx in 0..max_lines {
        let mut spans = vec![Span::styled("│", styles.table_border)];
        for ((cell_lines, width), alignment) in rendered_cells
            .iter()
            .zip(widths.iter())
            .zip(alignments.iter())
        {
            spans.push(Span::raw(" "));
            let cell_line = cell_lines
                .get(line_idx)
                .cloned()
                .unwrap_or_else(Line::default);
            let aligned = align_table_cell_line(cell_line, *width, *alignment);
            spans.extend(aligned.spans);
            spans.push(Span::raw(" "));
            spans.push(Span::styled("│", styles.table_border));
        }
        out.push(Line::from(spans));
    }

    out
}

fn render_table_cell(
    cell: &TableCellContent,
    width: usize,
    is_header: bool,
    styles: &MarkdownStyles,
) -> Vec<Line<'static>> {
    let mut lines = wrap_segments(cell.segments.clone(), width, &[]);
    if lines.is_empty() {
        lines.push(Line::default());
    }
    if is_header {
        lines = lines
            .into_iter()
            .map(|line| patch_line_style(line, styles.table_header))
            .collect();
    }
    lines
}

fn align_table_cell_line(
    mut line: Line<'static>,
    width: usize,
    alignment: Alignment,
) -> Line<'static> {
    let content_width = line_width(&line);
    let padding = width.saturating_sub(content_width);
    let (left, right) = match alignment {
        Alignment::Right => (padding, 0),
        Alignment::Center => (padding / 2, padding - (padding / 2)),
        Alignment::None | Alignment::Left => (0, padding),
    };

    let mut spans = Vec::new();
    if left > 0 {
        spans.push(Span::raw(" ".repeat(left)));
    }
    spans.append(&mut line.spans);
    if right > 0 {
        spans.push(Span::raw(" ".repeat(right)));
    }
    Line::from(spans)
}

fn patch_line_style(line: Line<'static>, style: Style) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|span| Span::styled(span.content, style.patch(span.style)))
        .collect::<Vec<_>>();
    Line::from(spans).style(style)
}

fn line_width(line: &Line<'static>) -> usize {
    line.spans.iter().map(|span| span.content.width()).sum()
}

fn table_border_line(
    left: char,
    mid: char,
    right: char,
    widths: &[usize],
    style: Style,
) -> Line<'static> {
    let mut text = String::new();
    text.push(left);
    for (idx, width) in widths.iter().enumerate() {
        text.push_str(&"─".repeat(width + 2));
        text.push(if idx + 1 == widths.len() { right } else { mid });
    }
    Line::from(vec![Span::styled(text, style)])
}

#[cfg(test)]
mod tests {
    use super::render_markdown_lines;

    fn lines_to_strings(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn renders_basic_markdown_blocks() {
        let lines = render_markdown_lines(
            "# Title\n\n- first item\n- second item\n\n```rust\nfn main() {}\n```",
            60,
        );
        let rendered = lines_to_strings(&lines);
        assert!(rendered.iter().any(|line| line.contains("# Title")));
        assert!(rendered.iter().any(|line| line.contains("• first item")));
        assert!(rendered.iter().any(|line| line.contains("fn main() {}")));
    }

    #[test]
    fn preserves_code_block_line_breaks() {
        let lines =
            render_markdown_lines("```rust\nfn main() {\n    println!(\"hi\");\n}\n```", 60);
        let rendered = lines_to_strings(&lines);
        assert!(rendered.iter().any(|line| line.contains("[rust]")));
        assert!(rendered.iter().any(|line| line.contains("fn main() {")));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("println!(\"hi\");"))
        );
        assert!(rendered.iter().any(|line| line.trim_end().ends_with('}')));
    }

    #[test]
    fn wraps_list_continuations_with_indent() {
        let lines = render_markdown_lines(
            "- this is a long list item that should wrap onto another line",
            24,
        );
        let rendered = lines_to_strings(&lines);
        assert!(rendered.first().is_some_and(|line| line.starts_with("• ")));
        assert!(rendered.get(1).is_some_and(|line| line.starts_with("  ")));
    }

    #[test]
    fn renders_table_with_borders() {
        let lines = render_markdown_lines(
            "| Name | Value |\n| ---- | ----: |\n| alpha | 12 |\n| beta | 345 |",
            60,
        );
        let rendered = lines_to_strings(&lines);
        assert!(rendered.iter().any(|line| line.contains("┌")));
        assert!(rendered.iter().any(|line| line.contains("│ Name")));
        assert!(rendered.iter().any(|line| line.contains("│ alpha")));
        assert!(rendered.iter().any(|line| line.contains("└")));
    }

    #[test]
    fn wraps_table_cells_when_narrow() {
        let lines = render_markdown_lines(
            "| Column | Details |\n| --- | --- |\n| alpha | this cell should wrap in a narrow terminal |",
            28,
        );
        let rendered = lines_to_strings(&lines);
        assert!(rendered.iter().any(|line| line.contains("this cell")));
        assert!(rendered.iter().any(|line| line.contains("should")));
    }
}
