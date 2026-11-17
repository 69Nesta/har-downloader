use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    DefaultTerminal, Frame,
};

use crate::filter::{valid_status_pattern, Filters};
use crate::har::{format_size, parse_size, Entry};
use crate::save::{self, OutLayout, Report};

#[derive(Clone, Copy, PartialEq)]
enum InputKind {
    Search,
    Status,
    Size,
    Output,
}

#[derive(Clone, Copy, PartialEq)]
enum PickKind {
    Type,
    Domain,
}

struct PickItem {
    value: String,
    count: usize,
    checked: bool,
}

enum Mode {
    Normal,
    Input { kind: InputKind, buf: String, prev: String },
    Pick { kind: PickKind, items: Vec<PickItem>, state: ListState },
    Confirm,
    Help,
}

struct App {
    entries: Vec<Entry>,
    filters: Filters,
    /// Indices into `entries` that pass the filters.
    visible: Vec<usize>,
    selected: HashSet<usize>,
    table: TableState,
    mode: Mode,
    out_dir: String,
    layout: OutLayout,
    message: Option<(String, bool)>,
    har_name: String,
    quit: bool,
}

pub fn run(entries: Vec<Entry>, filters: Filters, out: PathBuf, layout: OutLayout, har_name: String) -> Result<()> {
    let mut app = App {
        entries,
        filters,
        visible: vec![],
        selected: HashSet::new(),
        table: TableState::default(),
        mode: Mode::Normal,
        out_dir: out.to_string_lossy().into_owned(),
        layout,
        message: None,
        har_name,
        quit: false,
    };
    app.refilter();

    let mut terminal = ratatui::init();
    let res = app.run(&mut terminal);
    ratatui::restore();
    res
}

pub fn report_text(r: &Report, out: &Path) -> String {
    let dir = std::path::absolute(out).unwrap_or_else(|_| out.to_path_buf());
    let mut s = format!("Saved {} file(s) to {}", r.saved, dir.display());
    if r.duplicates > 0 {
        s += &format!(" · {} duplicate URL(s) skipped", r.duplicates);
    }
    if r.skipped > 0 {
        s += &format!(" · {} without body skipped", r.skipped);
    }
    if !r.errors.is_empty() {
        s += &format!(" · {} error(s): {}", r.errors.len(), r.errors[0]);
    }
    s
}

impl App {
    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    self.on_key(k);
                }
            }
        }
        Ok(())
    }

    fn refilter(&mut self) {
        self.visible = (0..self.entries.len())
            .filter(|&i| self.filters.matches(&self.entries[i]))
            .collect();
        let sel = self.table.selected().unwrap_or(0);
        self.table.select(if self.visible.is_empty() {
            None
        } else {
            Some(sel.min(self.visible.len() - 1))
        });
    }

    fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let cur = self.table.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, self.visible.len() as isize - 1);
        self.table.select(Some(next as usize));
    }

    /// Selected files if any, otherwise everything currently visible.
    fn targets(&self) -> Vec<usize> {
        if self.selected.is_empty() {
            self.visible.clone()
        } else {
            let mut v: Vec<usize> = self.selected.iter().copied().collect();
            v.sort_unstable();
            v
        }
    }

    fn pick_items(&self, kind: PickKind) -> Vec<PickItem> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for e in self.entries.iter().filter(|e| self.filters.include_empty || e.has_body()) {
            let key = match kind {
                PickKind::Type => e.category.to_string(),
                PickKind::Domain => e.host.clone(),
            };
            *counts.entry(key).or_default() += 1;
        }
        let active = match kind {
            PickKind::Type => &self.filters.types,
            PickKind::Domain => &self.filters.domains,
        };
        let mut items: Vec<PickItem> = counts
            .into_iter()
            .map(|(value, count)| PickItem { checked: active.contains(&value), value, count })
            .collect();
        items.sort_by(|a, b| b.count.cmp(&a.count).then(a.value.cmp(&b.value)));
        items
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), true));
    }

    // ---------- input ----------

    fn on_key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        self.mode = match mode {
            Mode::Normal => self.key_normal(k),
            Mode::Input { kind, buf, prev } => self.key_input(k, kind, buf, prev),
            Mode::Pick { kind, items, state } => self.key_pick(k, kind, items, state),
            Mode::Confirm => self.key_confirm(k),
            Mode::Help => Mode::Normal,
        };
    }

    fn key_normal(&mut self, k: KeyEvent) -> Mode {
        self.message = None;
        let input = |kind, buf: String| Mode::Input { kind, prev: buf.clone(), buf };
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(20),
            KeyCode::PageUp => self.move_by(-20),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX / 2),
            KeyCode::Char(' ') => {
                if let Some(&i) = self.table.selected().and_then(|s| self.visible.get(s)) {
                    if !self.selected.remove(&i) {
                        self.selected.insert(i);
                    }
                    self.move_by(1);
                }
            }
            KeyCode::Char('a') => {
                if self.visible.iter().all(|i| self.selected.contains(i)) {
                    for i in &self.visible {
                        self.selected.remove(i);
                    }
                } else {
                    self.selected.extend(self.visible.iter().copied());
                }
            }
            KeyCode::Char('c') => self.selected.clear(),
            KeyCode::Char('t') => {
                return Mode::Pick {
                    kind: PickKind::Type,
                    items: self.pick_items(PickKind::Type),
                    state: ListState::default().with_selected(Some(0)),
                }
            }
            KeyCode::Char('d') => {
                return Mode::Pick {
                    kind: PickKind::Domain,
                    items: self.pick_items(PickKind::Domain),
                    state: ListState::default().with_selected(Some(0)),
                }
            }
            KeyCode::Char('/') => return input(InputKind::Search, self.filters.query().to_string()),
            KeyCode::Char('s') => return input(InputKind::Status, self.filters.status.clone()),
            KeyCode::Char('z') => {
                let cur = if self.filters.min_size > 0 {
                    format!("{}k", self.filters.min_size as f64 / 1024.0)
                } else {
                    String::new()
                };
                return input(InputKind::Size, cur);
            }
            KeyCode::Char('o') => return input(InputKind::Output, self.out_dir.clone()),
            KeyCode::Char('b') => {
                self.filters.include_empty = !self.filters.include_empty;
                self.refilter();
            }
            KeyCode::Char('r') => {
                self.filters.reset();
                self.refilter();
                self.info("Filters reset");
            }
            KeyCode::Char('l') => {
                self.layout = self.layout.next();
                self.info(format!("Layout: {}", self.layout.label()));
            }
            KeyCode::Enter | KeyCode::Char('w') => {
                if self.targets().is_empty() {
                    self.error("Nothing to save");
                } else {
                    return Mode::Confirm;
                }
            }
            KeyCode::Char('?') => return Mode::Help,
            _ => {}
        }
        Mode::Normal
    }

    fn key_input(&mut self, k: KeyEvent, kind: InputKind, mut buf: String, prev: String) -> Mode {
        match k.code {
            KeyCode::Esc => {
                if kind == InputKind::Search {
                    self.filters.set_query(&prev);
                    self.refilter();
                }
                return Mode::Normal;
            }
            KeyCode::Enter => {
                let ok = match kind {
                    InputKind::Search => true,
                    InputKind::Status if valid_status_pattern(&buf) => {
                        self.filters.status = buf.trim().to_string();
                        true
                    }
                    InputKind::Status => {
                        self.error("Status: digits, x and commas only (e.g. 200 or 2xx,304)");
                        false
                    }
                    InputKind::Size => match parse_size(&buf) {
                        Some(n) => {
                            self.filters.min_size = n;
                            true
                        }
                        None => {
                            self.error("Size examples: 500, 10k, 1.5m");
                            false
                        }
                    },
                    InputKind::Output if !buf.trim().is_empty() => {
                        self.out_dir = buf.trim().to_string();
                        true
                    }
                    InputKind::Output => {
                        self.error("Output folder cannot be empty");
                        false
                    }
                };
                if ok {
                    self.message = None;
                    self.refilter();
                    return Mode::Normal;
                }
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(c) => buf.push(c),
            _ => {}
        }
        if kind == InputKind::Search {
            self.filters.set_query(&buf);
            self.refilter();
        }
        Mode::Input { kind, buf, prev }
    }

    fn key_pick(&mut self, k: KeyEvent, kind: PickKind, mut items: Vec<PickItem>, mut state: ListState) -> Mode {
        let cur = state.selected().unwrap_or(0);
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => state.select(Some((cur + 1).min(items.len().saturating_sub(1)))),
            KeyCode::Up | KeyCode::Char('k') => state.select(Some(cur.saturating_sub(1))),
            KeyCode::Char(' ') => {
                if let Some(it) = items.get_mut(cur) {
                    it.checked = !it.checked;
                }
            }
            KeyCode::Char('a') => {
                let all = items.iter().all(|i| i.checked);
                items.iter_mut().for_each(|i| i.checked = !all);
            }
            KeyCode::Enter => {
                let values: Vec<String> = items.into_iter().filter(|i| i.checked).map(|i| i.value).collect();
                match kind {
                    PickKind::Type => self.filters.types = values,
                    PickKind::Domain => self.filters.domains = values,
                }
                self.refilter();
                return Mode::Normal;
            }
            _ => {}
        }
        Mode::Pick { kind, items, state }
    }

    fn key_confirm(&mut self, k: KeyEvent) -> Mode {
        if matches!(k.code, KeyCode::Char('y') | KeyCode::Enter) {
            let out = PathBuf::from(&self.out_dir);
            let report = {
                let targets: Vec<&Entry> = self.targets().into_iter().map(|i| &self.entries[i]).collect();
                save::save(&targets, &out, self.layout)
            };
            let text = report_text(&report, &out);
            if report.errors.is_empty() {
                self.info(text);
                self.selected.clear();
            } else {
                self.error(text);
            }
        }
        Mode::Normal
    }

    // ---------- drawing ----------

    fn draw(&mut self, f: &mut Frame) {
        let [header, body, footer] =
            Layout::vertical([Constraint::Length(4), Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
        self.draw_header(f, header);
        self.draw_table(f, body);
        self.draw_footer(f, footer);

        let area = f.area();
        match &mut self.mode {
            Mode::Pick { kind, items, state } => draw_pick(f, area, *kind, items, state),
            Mode::Confirm => {
                let n = self.targets().len();
                let what = if self.selected.is_empty() { "matching" } else { "selected" };
                let dir = std::path::absolute(&self.out_dir).unwrap_or_else(|_| PathBuf::from(&self.out_dir));
                let lines = vec![
                    Line::from(format!("Save {n} {what} file(s)")).bold(),
                    Line::from(format!("to     {}", dir.display())),
                    Line::from(format!("layout {}", self.layout.label())),
                    Line::from(""),
                    Line::from(vec![key("y"), Span::raw(" save   "), key("n"), Span::raw(" cancel")]),
                ];
                popup(f, area, " Save ", lines, 60);
            }
            Mode::Help => draw_help(f, area),
            _ => {}
        }
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let fl = &self.filters;
        let empty = self.entries.iter().filter(|e| !e.has_body()).count();
        let val = |active: bool, s: String| if active { Span::from(s).yellow().bold() } else { Span::from(s).dark_gray() };
        let list = |v: &Vec<String>| if v.is_empty() { "all".to_string() } else { v.join(",") };

        let line1 = Line::from(vec![
            Span::raw(format!("{} / {} files", self.visible.len(), self.entries.len())),
            Span::raw("   selected: "),
            val(!self.selected.is_empty(), self.selected.len().to_string()),
            Span::raw("   out: "),
            Span::from(self.out_dir.clone()).cyan(),
            Span::raw(format!(" [{}]", self.layout.label())),
            Span::raw(if empty > 0 {
                format!("   {empty} without body ({})", if fl.include_empty { "shown" } else { "hidden" })
            } else {
                String::new()
            })
            .dark_gray(),
        ]);
        let mut url = if fl.query().is_empty() { "—".to_string() } else { fl.query().to_string() };
        if fl.regex_error {
            url += " (text)";
        }
        let line2 = Line::from(vec![
            Span::raw("type: "),
            val(!fl.types.is_empty(), list(&fl.types)),
            Span::raw("  domain: "),
            val(!fl.domains.is_empty(), list(&fl.domains)),
            Span::raw("  url: "),
            val(!fl.query().is_empty(), url),
            Span::raw("  status: "),
            val(!fl.status.is_empty(), if fl.status.is_empty() { "any".into() } else { fl.status.clone() }),
            Span::raw("  size ≥ "),
            val(fl.min_size > 0, format_size(fl.min_size)),
        ]);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(Line::from(format!(" har-downloader · {} ", self.har_name)).bold());
        f.render_widget(Paragraph::new(vec![line1, line2]).block(block), area);
    }

    fn draw_table(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.visible.iter().map(|&i| {
            let e = &self.entries[i];
            let sel = self.selected.contains(&i);
            let status_style = match e.status {
                200..=299 => Style::new().green(),
                300..=399 => Style::new().cyan(),
                400.. => Style::new().red(),
                _ => Style::new().dark_gray(),
            };
            let row = Row::new(vec![
                Cell::from(if sel { "[x]" } else { "[ ]" }),
                Cell::from(e.status.to_string()).style(status_style),
                Cell::from(e.category),
                Cell::from(format_size(e.size)),
                Cell::from(e.display_url()),
            ]);
            if !e.has_body() {
                row.dark_gray()
            } else if sel {
                row.yellow()
            } else {
                row
            }
        });
        let pos = self.table.selected().map(|s| s + 1).unwrap_or(0);
        let table = Table::new(
            rows,
            [
                Constraint::Length(3),
                Constraint::Length(4),
                Constraint::Length(7),
                Constraint::Length(9),
                Constraint::Fill(1),
            ],
        )
        .header(Row::new(["", "Code", "Type", "Size", "URL"]).bold().underlined())
        .row_highlight_style(Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(format!(" Files {pos}/{} ", self.visible.len())),
        );
        f.render_stateful_widget(table, area, &mut self.table);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let line = if let Mode::Input { kind, buf, .. } = &self.mode {
            let label = match kind {
                InputKind::Search => "URL regex/text",
                InputKind::Status => "Status (200, 2xx, 2xx,304)",
                InputKind::Size => "Min size (500, 10k, 1.5m)",
                InputKind::Output => "Output folder",
            };
            let mut spans = vec![Span::from(format!(" {label}: ")).cyan().bold(), Span::raw(buf.clone()), Span::raw("█")];
            if *kind == InputKind::Search && self.filters.regex_error {
                spans.push(Span::raw("  invalid regex, matching as plain text").red());
            }
            spans.push(Span::raw("   enter ok · esc cancel").dark_gray());
            Line::from(spans)
        } else if let Some((msg, err)) = &self.message {
            let s = Span::raw(format!(" {msg}"));
            Line::from(if *err { s.red().bold() } else { s.green() })
        } else {
            let mut spans = vec![];
            for (k, label) in [
                ("space", "select"),
                ("a", "all"),
                ("t", "type"),
                ("d", "domain"),
                ("/", "url"),
                ("s", "status"),
                ("z", "size"),
                ("l", "layout"),
                ("o", "out"),
                ("enter", "save"),
                ("?", "help"),
                ("q", "quit"),
            ] {
                spans.push(Span::raw(" "));
                spans.push(key(k));
                spans.push(Span::raw(format!(" {label} ")).dark_gray());
            }
            Line::from(spans)
        };
        f.render_widget(Paragraph::new(line), area);
    }
}

fn key(k: &str) -> Span<'static> {
    Span::from(k.to_string()).bold().cyan()
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [a] = Layout::horizontal([Constraint::Length(w.min(area.width))]).flex(Flex::Center).areas(area);
    let [a] = Layout::vertical([Constraint::Length(h.min(area.height))]).flex(Flex::Center).areas(a);
    a
}

fn popup(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line>, width: u16) {
    let rect = centered(area, width, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    let block = Block::bordered().border_type(BorderType::Rounded).title(title.to_string()).cyan();
    f.render_widget(Paragraph::new(lines).block(block).white(), rect);
}

fn draw_pick(f: &mut Frame, area: Rect, kind: PickKind, items: &[PickItem], state: &mut ListState) {
    let title = match kind {
        PickKind::Type => " Filter by type ",
        PickKind::Domain => " Filter by domain ",
    };
    let width = items.iter().map(|i| i.value.len() + 16).max().unwrap_or(30).clamp(44, 80) as u16;
    let rect = centered(area, width, items.len() as u16 + 3);
    f.render_widget(Clear, rect);
    let list = List::new(items.iter().map(|i| {
        let item = ListItem::new(format!("{} {}  ({})", if i.checked { "[x]" } else { "[ ]" }, i.value, i.count));
        if i.checked { item.yellow() } else { item }
    }))
    .block(
        Block::bordered()
            .border_type(BorderType::Rounded)
            .title(title)
            .title_bottom(Line::from(" space toggle · a all · enter apply · esc cancel ").dark_gray())
            .cyan(),
    )
    .highlight_style(Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
    .white();
    f.render_stateful_widget(list, rect, state);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let rows = [
        ("↑ ↓ / j k", "move   (PgUp/PgDn, g/G for top/bottom)"),
        ("space", "select / unselect file"),
        ("a", "select all visible (again = unselect)"),
        ("c", "clear selection"),
        ("t", "filter by type"),
        ("d", "filter by domain"),
        ("/", "filter by URL (regex or text, live)"),
        ("s", "filter by status (200, 2xx, 2xx,304)"),
        ("z", "filter by minimum size (10k, 1m)"),
        ("b", "show / hide entries without body"),
        ("r", "reset filters"),
        ("l", "cycle layout: tree / flat / by type"),
        ("o", "change output folder"),
        ("enter / w", "save selected (or all visible if none)"),
        ("q / esc", "quit"),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| Line::from(vec![Span::from(format!(" {k:<11}")).bold().cyan(), Span::raw(*d)]))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(" press any key to close").dark_gray());
    popup(f, area, " Keys ", lines, 62);
}
