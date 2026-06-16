use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, FontFamily, RichText, TextEdit};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const AUTOSAVE_AFTER: Duration = Duration::from_millis(700);

fn main() -> eframe::Result<()> {
    if env::args().any(|arg| arg == "--smoke-test") {
        if let Err(err) = run_smoke_test() {
            eprintln!("smoke fail: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Note")
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Note",
        options,
        Box::new(|cc| {
            install_font_fallbacks(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx, Theme::Black);
            Ok(Box::new(NoteApp::new()))
        }),
    )
}

fn run_smoke_test() -> Result<(), String> {
    let data_path = note_data_path();
    let notes = load_notes(&data_path).map_err(|err| err.to_string())?;
    println!("Note smoke");
    println!("data={}", data_path.display());
    println!("notes={}", notes.len());
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Theme {
    Black,
    White,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Note {
    id: u64,
    title: String,
    content: String,
    created: u64,
    updated: u64,
}

impl Note {
    fn new(title: impl Into<String>) -> Self {
        let now = unix_time();
        Self {
            id: now,
            title: title.into(),
            content: String::new(),
            created: now,
            updated: now,
        }
    }

    fn display_title(&self) -> String {
        let title = self.title.trim();
        if !title.is_empty() {
            return title.to_string();
        }
        self.content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !is_visual_only_markdown_line(line))
            .next()
            .unwrap_or("Untitled")
            .chars()
            .take(48)
            .collect()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct NoteFile {
    version: u32,
    notes: Vec<Note>,
}

struct NoteApp {
    notes: Vec<Note>,
    selected: usize,
    query: String,
    status: String,
    data_path: PathBuf,
    dirty: bool,
    last_edit: Option<Instant>,
    theme: Theme,
    slash: SlashMenu,
    editor_cursor: usize,
    pending_cursor: Option<usize>,
    editing: bool,
}

#[derive(Default)]
struct SlashMenu {
    visible: bool,
    query: String,
    selected: usize,
    start: usize,
    end: usize,
    selection_changed: bool,
}

#[derive(Clone, Copy)]
struct SlashCommand {
    label: &'static str,
    detail: &'static str,
    search: &'static str,
    insert: &'static str,
    cursor: usize,
}

const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        label: "Text",
        detail: "Plain block.",
        search: "plain paragraph p",
        insert: "",
        cursor: 0,
    },
    SlashCommand {
        label: "Heading 1",
        detail: "Big title.",
        search: "h1 header title",
        insert: "# ",
        cursor: 2,
    },
    SlashCommand {
        label: "Heading 2",
        detail: "Section title.",
        search: "h2 header subtitle",
        insert: "## ",
        cursor: 3,
    },
    SlashCommand {
        label: "Heading 3",
        detail: "Small title.",
        search: "h3 header",
        insert: "### ",
        cursor: 4,
    },
    SlashCommand {
        label: "To-do list",
        detail: "Checkbox list.",
        search: "todo task check checkbox",
        insert: "- [ ] ",
        cursor: 6,
    },
    SlashCommand {
        label: "Bulleted list",
        detail: "Dot list.",
        search: "bullet unordered ul list",
        insert: "- ",
        cursor: 2,
    },
    SlashCommand {
        label: "Numbered list",
        detail: "1. 2. 3.",
        search: "number numbered ordered ol list 1.",
        insert: "1. ",
        cursor: 3,
    },
    SlashCommand {
        label: "Quote",
        detail: "Quote block.",
        search: "quote blockquote cite",
        insert: "> ",
        cursor: 2,
    },
    SlashCommand {
        label: "Divider",
        detail: "Line break.",
        search: "divider rule hr line",
        insert: "---\n",
        cursor: 4,
    },
    SlashCommand {
        label: "Code",
        detail: "Code block.",
        search: "code snippet pre",
        insert: "```\n\n```",
        cursor: 4,
    },
    SlashCommand {
        label: "Callout",
        detail: "Note callout.",
        search: "callout note warn info",
        insert: "> Note: ",
        cursor: 8,
    },
    SlashCommand {
        label: "Link",
        detail: "URL.",
        search: "link url href",
        insert: "[text](url)",
        cursor: 1,
    },
    SlashCommand {
        label: "Table",
        detail: "Grid.",
        search: "table grid columns rows",
        insert: "|   |   |\n|---|---|\n|   |   |",
        cursor: 2,
    },
    SlashCommand {
        label: "Bold",
        detail: "Strong text.",
        search: "bold strong b",
        insert: "****",
        cursor: 2,
    },
    SlashCommand {
        label: "Italic",
        detail: "Emphasis.",
        search: "italic emphasis em i",
        insert: "**",
        cursor: 1,
    },
    SlashCommand {
        label: "Strikethrough",
        detail: "Cross out.",
        search: "strike strikethrough delete s",
        insert: "~~~~",
        cursor: 2,
    },
];

impl NoteApp {
    fn new() -> Self {
        let data_path = note_data_path();
        let mut status = "Ready".to_string();
        let mut notes = match load_notes(&data_path) {
            Ok(notes) => notes,
            Err(err) => {
                status = format!("Load fail: {err}");
                Vec::new()
            }
        };
        if notes.is_empty() {
            notes.push(welcome_note());
        }

        Self {
            notes,
            selected: 0,
            query: String::new(),
            status,
            data_path,
            dirty: false,
            last_edit: None,
            theme: Theme::Black,
            slash: SlashMenu::default(),
            editor_cursor: 0,
            pending_cursor: None,
            editing: false,
        }
    }

    fn note_count(&self) -> usize {
        self.notes.len()
    }

    fn mark_dirty(&mut self) {
        if let Some(note) = self.notes.get_mut(self.selected) {
            note.updated = unix_time();
        }
        self.dirty = true;
        self.last_edit = Some(Instant::now());
        self.status = "Unsaved".to_string();
    }

    fn save(&mut self) {
        match save_notes(&self.data_path, &self.notes) {
            Ok(()) => {
                self.dirty = false;
                self.last_edit = None;
                self.status = "Saved".to_string();
            }
            Err(err) => self.status = format!("Save fail: {err}"),
        }
    }

    fn autosave(&mut self) {
        if self.dirty
            && self
                .last_edit
                .is_some_and(|last| last.elapsed() >= AUTOSAVE_AFTER)
        {
            self.save();
        }
    }

    fn new_note(&mut self) {
        self.notes.push(Note::new("Untitled"));
        self.selected = self.notes.len() - 1;
        self.query.clear();
        self.mark_dirty();
        self.status = "New".to_string();
    }

    fn delete_note(&mut self) {
        if self.notes.is_empty() {
            return;
        }
        self.notes.remove(self.selected);
        if self.notes.is_empty() {
            self.notes.push(Note::new("Untitled"));
        }
        self.selected = self.selected.min(self.notes.len().saturating_sub(1));
        self.mark_dirty();
        self.status = "Deleted".to_string();
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_ascii_lowercase();
        self.notes
            .iter()
            .enumerate()
            .filter_map(|(index, note)| {
                if query.is_empty()
                    || note.title.to_ascii_lowercase().contains(&query)
                    || note.content.to_ascii_lowercase().contains(&query)
                {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let save = ctx.input(|input| input.key_pressed(egui::Key::S) && input.modifiers.command);
        let new = ctx.input(|input| input.key_pressed(egui::Key::N) && input.modifiers.command);
        let find = ctx.input(|input| input.key_pressed(egui::Key::F) && input.modifiers.command);
        let clear = ctx.input(|input| input.key_pressed(egui::Key::Escape));

        if save {
            self.save();
        }
        if new {
            self.new_note();
        }
        if find {
            self.status = "Find".to_string();
        }
        if clear {
            self.query.clear();
        }
    }

    fn close_slash(&mut self) {
        self.slash.visible = false;
        self.slash.query.clear();
        self.slash.selected = 0;
        self.slash.start = 0;
        self.slash.end = 0;
        self.slash.selection_changed = false;
    }

    fn slash_move_selection(&mut self, delta: isize) {
        let matches = self.slash_match_indices();
        if matches.is_empty() {
            return;
        }

        let len = matches.len() as isize;
        let new_sel = (self.slash.selected as isize + delta).rem_euclid(len);
        self.slash.selected = new_sel as usize;
        self.slash.selection_changed = true;
    }

    fn slash_match_indices(&self) -> Vec<usize> {
        let query = self.slash.query.to_ascii_lowercase();
        SLASH_COMMANDS
            .iter()
            .enumerate()
            .filter_map(|(index, command)| {
                let label = command.label.to_ascii_lowercase();
                let detail = command.detail.to_ascii_lowercase();
                let search = command.search.to_ascii_lowercase();
                if query.is_empty()
                    || label.contains(&query)
                    || detail.contains(&query)
                    || search.contains(&query)
                {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    fn update_slash_menu(&mut self, content: &str, cursor: usize, focused: bool) {
        if !focused {
            return;
        }

        if let Some((start, query)) = slash_trigger(content, cursor) {
            self.slash.visible = true;
            self.slash.query = query;
            self.slash.start = start;
            self.slash.end = cursor;
            let count = self.slash_match_indices().len();
            self.slash.selected = self.slash.selected.min(count.saturating_sub(1));
        } else {
            self.close_slash();
        }
    }

    fn apply_slash_command(&mut self, command_index: usize) {
        let Some(command) = SLASH_COMMANDS.get(command_index).copied() else {
            return;
        };
        let Some(note) = self.notes.get_mut(self.selected) else {
            return;
        };
        replace_char_range(
            &mut note.content,
            self.slash.start,
            self.slash.end,
            command.insert,
        );
        self.editor_cursor = self.slash.start + command.cursor;
        self.pending_cursor = Some(self.editor_cursor);
        self.close_slash();
        self.mark_dirty();
        self.status = command.label.to_string();
    }

    fn consume_slash_input(&mut self, ui: &mut egui::Ui) {
        if !self.slash.visible {
            return;
        }

        let mut close = false;
        let mut apply = false;
        ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                self.slash_move_selection(-1);
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                self.slash_move_selection(1);
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
            {
                apply = true;
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                close = true;
            }
        });

        if close {
            self.close_slash();
        }
        if apply {
            let matches = self.slash_match_indices();
            if let Some(command_index) = matches.get(self.slash.selected).copied() {
                self.apply_slash_command(command_index);
            }
        }
    }

    fn draw_top(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top").show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("Note").strong());
                ui.separator();
                if ui.button("+").clicked() {
                    self.new_note();
                }
                if ui.button("Save").clicked() {
                    self.save();
                }
                if ui.button("Delete").clicked() {
                    self.delete_note();
                }
                ui.separator();
                if ui
                    .selectable_label(self.theme == Theme::Black, "Black")
                    .clicked()
                {
                    self.theme = Theme::Black;
                    apply_theme(&ctx, self.theme);
                }
                if ui
                    .selectable_label(self.theme == Theme::White, "White")
                    .clicked()
                {
                    self.theme = Theme::White;
                    apply_theme(&ctx, self.theme);
                }
                ui.separator();
                ui.label(self.status_text());
            });
            ui.add_space(4.0);
        });
    }

    fn status_text(&self) -> String {
        if self.status.contains("fail") {
            self.status.clone()
        } else if self.editing {
            "Editing".to_string()
        } else if self.dirty {
            "Unsaved".to_string()
        } else {
            self.status.clone()
        }
    }

    fn draw_list(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("list")
            .resizable(true)
            .default_size(250.0)
            .size_range(180.0..=360.0)
            .show_inside(ui, |ui| {
                ui.heading("Notes");
                ui.label(format!("{} notes", self.note_count()));
                ui.add(
                    TextEdit::singleline(&mut self.query)
                        .hint_text("find")
                        .desired_width(f32::INFINITY),
                );
                ui.separator();

                let indices = self.filtered_indices();
                egui::ScrollArea::vertical()
                    .id_salt("note_list")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        if indices.is_empty() {
                            ui.label("Empty");
                        }
                        for index in indices {
                            let note = &self.notes[index];
                            let title = note.display_title();
                            let selected = index == self.selected;
                            let label = format!("{}  {}", title, note_word_count(note));
                            if ui
                                .selectable_label(selected, truncate(&label, 36))
                                .clicked()
                            {
                                self.selected = index;
                                self.close_slash();
                            }
                        }
                    });
            });
    }

    fn draw_editor(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.notes.is_empty() {
                ui.centered_and_justified(|ui| {
                    if ui.button("New").clicked() {
                        self.new_note();
                    }
                });
                return;
            }

            self.consume_slash_input(ui);
            self.selected = self.selected.min(self.notes.len() - 1);
            let selected = self.selected;
            let mut changed = false;
            let mut body_changed = false;

            let (content, cursor, focused, editing, meta) = {
                let note = &mut self.notes[selected];
                let body_id = ("body", note.id);
                let title = ui.add(
                    TextEdit::singleline(&mut note.title)
                        .id_salt(("title", note.id))
                        .hint_text("title")
                        .desired_width(f32::INFINITY),
                );
                changed |= title.changed();

                ui.add_space(6.0);
                let body = egui::ScrollArea::vertical()
                    .id_salt("editor")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.scope(|ui| {
                            ui.style_mut().visuals.text_cursor.stroke = egui::Stroke::NONE;
                            let mut layouter =
                                |ui: &egui::Ui, string: &dyn egui::TextBuffer, wrap_width: f32| {
                                    markdown_layouter(ui, string.as_str(), wrap_width)
                                };
                            TextEdit::multiline(&mut note.content)
                                .id_salt(body_id)
                                .hint_text("write")
                                .desired_width(f32::INFINITY)
                                .desired_rows(30)
                                .lock_focus(true)
                                .layouter(&mut layouter)
                                .show(ui)
                        })
                        .inner
                    })
                    .inner;

                draw_markdown_overlays(ui, &body, &note.content);

                body_changed |= body.response.changed();
                changed |= body_changed;

                if let Some(cursor) = self.pending_cursor.take() {
                    let cursor = cursor.min(note.content.chars().count());
                    self.editor_cursor = cursor;
                    set_text_cursor(ui.ctx(), body.response.id, cursor);
                    body.response.request_focus();
                } else if let Some(range) = body.cursor_range {
                    self.editor_cursor = range.primary.index;
                }

                if body.response.clicked() {
                    if let Some(pos) = body.response.interact_pointer_pos() {
                        if let Some(cursor) =
                            table_cursor_from_pointer(ui, &body, &note.content, pos)
                        {
                            self.editor_cursor = cursor;
                            set_text_cursor(ui.ctx(), body.response.id, cursor);
                            body.response.request_focus();
                        } else {
                            let text_pos = pos - body.galley_pos;
                            let cursor = body.galley.cursor_from_pos(text_pos);
                            if toggle_checkbox_at(&mut note.content, cursor.index) {
                                body_changed = true;
                                changed = true;
                            }
                        }
                    }
                }

                if body_changed {
                    if let Some(cursor) = apply_smart_newline(&mut note.content, self.editor_cursor)
                    {
                        self.editor_cursor = cursor;
                        set_text_cursor(ui.ctx(), body.response.id, cursor);
                    }
                }

                let meta = format!(
                    "{} words  {} chars",
                    note_word_count(note),
                    note.content.chars().count()
                );

                let body_focused = body.response.has_focus();

                (
                    note.content.clone(),
                    self.editor_cursor,
                    body_focused,
                    title.has_focus() || body_focused,
                    meta,
                )
            };
            self.editing = editing;
            self.update_slash_menu(&content, cursor, focused);

            if changed {
                self.mark_dirty();
            }

            self.draw_slash_menu(ui);
            ui.add_space(18.0);
            ui.label(RichText::new(meta).small().color(Color32::GRAY));
        });
    }

    fn draw_slash_menu(&mut self, ui: &mut egui::Ui) {
        if !self.slash.visible {
            return;
        }

        let matches = self.slash_match_indices();
        if matches.is_empty() {
            return;
        }

        let mut apply = None;

        egui::Area::new(egui::Id::new("slash_menu"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(6.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_min_width(320.0);
                        ui.set_max_height(400.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("BASIC BLOCKS")
                                    .strong()
                                    .color(Color32::GRAY)
                                    .small(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("/{}", self.slash.query))
                                            .italics()
                                            .color(Color32::GRAY),
                                    );
                                },
                            );
                        });
                        ui.add_space(4.0);

                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for (row_index, command_index) in matches.iter().enumerate() {
                                    let command = SLASH_COMMANDS[*command_index];
                                    let selected = row_index == self.slash.selected;
                                    let bg_color = if selected {
                                        ui.visuals().widgets.active.bg_fill
                                    } else {
                                        Color32::TRANSPARENT
                                    };

                                    let response = egui::Frame::NONE
                                        .fill(bg_color)
                                        .corner_radius(4.0)
                                        .inner_margin(egui::Margin::symmetric(8, 6))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.vertical(|ui| {
                                                    ui.label(RichText::new(command.label).strong());
                                                    ui.label(
                                                        RichText::new(command.detail)
                                                            .small()
                                                            .color(Color32::GRAY),
                                                    );
                                                });
                                            });
                                        })
                                        .response;

                                    let response = ui.interact(
                                        response.rect,
                                        ui.id().with(("slash", row_index, command_index)),
                                        egui::Sense::click(),
                                    );
                                    if response.clicked() {
                                        apply = Some(*command_index);
                                    }
                                    if selected && self.slash.selection_changed {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                        self.slash.selection_changed = false;

                        ui.add_space(4.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("↑↓ to navigate, Enter to select, Esc to close")
                                    .small()
                                    .color(Color32::GRAY),
                            );
                        });
                    });
            });

        if let Some(command_index) = apply {
            self.apply_slash_command(command_index);
        }
    }
}

impl Drop for NoteApp {
    fn drop(&mut self) {
        let _ = save_notes(&self.data_path, &self.notes);
    }
}

impl eframe::App for NoteApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.shortcuts(ctx);
        self.autosave();
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        apply_theme(ui.ctx(), self.theme);
        self.draw_top(ui);
        self.draw_list(ui);
        self.draw_editor(ui);
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let mut visuals = match theme {
        Theme::Black => egui::Visuals::dark(),
        Theme::White => egui::Visuals::light(),
    };

    match theme {
        Theme::Black => {
            visuals.window_fill = Color32::BLACK;
            visuals.panel_fill = Color32::BLACK;
            visuals.extreme_bg_color = Color32::BLACK;
            visuals.faint_bg_color = Color32::from_gray(18);
            visuals.text_cursor.stroke = egui::Stroke::new(2.0, Color32::WHITE);
            visuals.override_text_color = Some(Color32::WHITE);
            visuals.widgets.noninteractive.bg_fill = Color32::BLACK;
            visuals.widgets.inactive.bg_fill = Color32::from_gray(24);
            visuals.widgets.hovered.bg_fill = Color32::from_gray(42);
            visuals.widgets.active.bg_fill = Color32::from_gray(64);
        }
        Theme::White => {
            visuals.window_fill = Color32::WHITE;
            visuals.panel_fill = Color32::WHITE;
            visuals.extreme_bg_color = Color32::WHITE;
            visuals.faint_bg_color = Color32::from_gray(236);
            visuals.text_cursor.stroke = egui::Stroke::new(2.0, Color32::BLACK);
            visuals.override_text_color = Some(Color32::BLACK);
            visuals.widgets.noninteractive.bg_fill = Color32::WHITE;
            visuals.widgets.inactive.bg_fill = Color32::from_gray(232);
            visuals.widgets.hovered.bg_fill = Color32::from_gray(214);
            visuals.widgets.active.bg_fill = Color32::from_gray(190);
        }
    }

    ctx.set_visuals(visuals);
}

fn install_font_fallbacks(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let candidates = [
        ("menlo", "/System/Library/Fonts/Menlo.ttc", true),
        (
            "apple_sd_gothic",
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
            false,
        ),
        (
            "apple_gothic",
            "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
            false,
        ),
        (
            "arial_unicode",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            false,
        ),
    ];

    for (name, path, monospace_first) in candidates {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        fonts
            .font_data
            .insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));

        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.push(name.to_string());
        }
        if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
            if monospace_first {
                family.insert(0, name.to_string());
            } else {
                family.push(name.to_string());
            }
        }
    }

    ctx.set_fonts(fonts);
}

fn load_notes(path: &Path) -> io::Result<Vec<Note>> {
    if path.exists() {
        let data = fs::read_to_string(path)?;
        let file = serde_json::from_str::<NoteFile>(&data)
            .or_else(|_| {
                serde_json::from_str::<Vec<Note>>(&data).map(|notes| NoteFile { version: 1, notes })
            })
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        return Ok(file.notes);
    }

    let legacy = legacy_data_path();
    if legacy.exists() {
        let data = fs::read_to_string(legacy)?;
        return Ok(parse_legacy_notes(&data));
    }

    Ok(Vec::new())
}

fn save_notes(path: &Path, notes: &[Note]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = NoteFile {
        version: 1,
        notes: notes.to_vec(),
    };
    let data = serde_json::to_string_pretty(&file)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(path, data)
}

fn parse_legacy_notes(data: &str) -> Vec<Note> {
    data.split('\x1E')
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }
            let mut lines = block.splitn(2, '\n');
            let mut note = Note::new(lines.next().unwrap_or("Untitled"));
            note.content = lines.next().unwrap_or("").to_string();
            Some(note)
        })
        .collect()
}

fn note_data_path() -> PathBuf {
    if let Ok(path) = env::var("NOTE_PATH") {
        return PathBuf::from(path);
    }
    home_dir().join(".note_data.json")
}

fn legacy_data_path() -> PathBuf {
    home_dir().join(".note_data.txt")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn welcome_note() -> Note {
    let mut note = Note::new("Grass");
    note.content = ",,,,,,,,,,,,,,,,,,,,,,,,,,,,,,,,,, <- touch".to_string();
    note
}

fn note_word_count(note: &Note) -> usize {
    note.content.split_whitespace().count()
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('~');
    out
}

fn slash_trigger(text: &str, cursor: usize) -> Option<(usize, String)> {
    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let mut index = cursor;

    while index > 0 {
        let ch = chars[index - 1];
        if ch == '/' {
            let query = chars[index..cursor].iter().collect::<String>();
            return Some((index - 1, query));
        }
        if ch == '\n' || ch.is_whitespace() {
            break;
        }
        index -= 1;
    }

    None
}

fn toggle_checkbox_at(text: &mut String, cursor: usize) -> bool {
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("- [ ] ") {
            Some("- [x] ")
        } else if trimmed.starts_with("- [x] ") {
            Some("- [ ] ")
        } else {
            None
        };

        if let Some(replacement) = marker {
            let offset = line.find("- [").unwrap_or(0);
            let start = line_start + line[..offset].chars().count();
            let end = start + 6;
            if cursor >= start && cursor <= end {
                replace_char_range(text, start, end, replacement);
                return true;
            }
        }
        line_start += line.chars().count();
    }
    false
}

fn apply_smart_newline(text: &mut String, cursor: usize) -> Option<usize> {
    if cursor == 0 || text.chars().nth(cursor - 1) != Some('\n') {
        return None;
    }

    let before = text.chars().take(cursor - 1).collect::<String>();
    let prev_line = before.lines().last().unwrap_or("");
    let indent = prev_line
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    let trimmed = prev_line.trim_start();

    if let Some((number, has_content)) = numbered_marker(trimmed) {
        if !has_content {
            let remove_start = cursor - 1 - prev_line.chars().count();
            replace_char_range(text, remove_start, cursor, "");
            return Some(remove_start);
        }

        let insert = format!("{indent}{}. ", number.saturating_add(1));
        replace_char_range(text, cursor, cursor, &insert);
        return Some(cursor + insert.chars().count());
    }

    let prefix = if trimmed.starts_with("- [ ] ") || trimmed.starts_with("- [x] ") {
        "- [ ] "
    } else if trimmed.starts_with("- ") {
        "- "
    } else if trimmed.starts_with("> ") {
        "> "
    } else {
        return None;
    };

    if trimmed.trim_end() == prefix.trim_end() {
        let remove_start = cursor - 1 - prev_line.chars().count();
        replace_char_range(text, remove_start, cursor, "");
        Some(remove_start)
    } else {
        let insert = format!("{indent}{prefix}");
        replace_char_range(text, cursor, cursor, &insert);
        Some(cursor + insert.chars().count())
    }
}

fn numbered_marker(trimmed: &str) -> Option<(usize, bool)> {
    let digit_count = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }

    let mut chars = trimmed.chars().skip(digit_count);
    if chars.next() != Some('.') {
        return None;
    }
    let rest = &trimmed[digit_count + 1..];
    if rest.chars().next().is_some_and(|ch| !ch.is_whitespace()) {
        return None;
    }
    let has_content = !rest.trim().is_empty();

    let number = trimmed
        .chars()
        .take(digit_count)
        .collect::<String>()
        .parse::<usize>()
        .ok()?;
    Some((number, has_content))
}

struct TableOverlayRow {
    left: f32,
    top: f32,
    bottom: f32,
    start_char: usize,
    end_char: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TableCell {
    text: String,
    raw_start: usize,
    raw_end: usize,
    clean_start: usize,
    clean_end: usize,
}

fn draw_markdown_overlays(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    text: &str,
) {
    let painter = ui.painter();
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let table_rows = table_overlay_rows(output, &lines);
    let mut current_char_idx = 0;

    for line in &lines {
        let trimmed = line.trim_start();
        let clean = trimmed.trim_end();
        let char_count = line.chars().count();

        if trimmed.starts_with("> ") {
            let offset = line.find('>').unwrap_or(0);
            let char_offset = line[..offset].chars().count();
            let start = current_char_idx + char_offset;
            let rect = output
                .galley
                .pos_from_cursor(egui::text::CCursor::new(start))
                .translate(output.galley_pos.to_vec2());

            painter.line_segment(
                [
                    rect.left_top() + egui::vec2(-2.0, 0.0),
                    rect.left_bottom() + egui::vec2(-2.0, 0.0),
                ],
                egui::Stroke::new(3.0, Color32::GRAY),
            );
        }

        if trimmed.starts_with("- [ ] ") || trimmed.starts_with("- [x] ") {
            let is_checked = trimmed.starts_with("- [x] ");
            let offset = line.find("- [").unwrap_or(0);
            let char_offset = line[..offset].chars().count();
            let start = current_char_idx + char_offset;
            let rect = output
                .galley
                .pos_from_cursor(egui::text::CCursor::new(start + 2))
                .translate(output.galley_pos.to_vec2());

            let box_size = 14.0;
            let box_rect = egui::Rect::from_min_size(
                rect.min + egui::vec2(2.0, (rect.height() - box_size) / 2.0),
                egui::vec2(box_size, box_size),
            );
            let fill = if is_checked {
                Color32::from_rgb(100, 200, 100)
            } else {
                Color32::TRANSPARENT
            };
            let stroke = if is_checked {
                egui::Stroke::NONE
            } else {
                egui::Stroke::new(1.5, Color32::GRAY)
            };

            painter.rect(box_rect, 3.0, fill, stroke, egui::StrokeKind::Middle);

            if is_checked {
                let p1 = box_rect.min + egui::vec2(3.0, 7.0);
                let p2 = box_rect.min + egui::vec2(6.0, 10.0);
                let p3 = box_rect.min + egui::vec2(11.0, 4.0);
                painter.line_segment([p1, p2], egui::Stroke::new(2.0, ui.visuals().window_fill));
                painter.line_segment([p2, p3], egui::Stroke::new(2.0, ui.visuals().window_fill));
            }
        }

        if is_divider_line(clean) {
            let offset = line.find(clean).unwrap_or(0);
            let char_offset = line[..offset].chars().count();
            let start = current_char_idx + char_offset;
            let end = start + clean.chars().count();
            let left = output
                .galley
                .pos_from_cursor(egui::text::CCursor::new(start))
                .translate(output.galley_pos.to_vec2());
            let right = output
                .galley
                .pos_from_cursor(egui::text::CCursor::new(end))
                .translate(output.galley_pos.to_vec2());
            let y = left.center().y;
            painter.line_segment(
                [
                    egui::pos2(left.left(), y),
                    egui::pos2(right.right().max(left.left() + 160.0), y),
                ],
                egui::Stroke::new(1.0, Color32::DARK_GRAY),
            );
        }

        current_char_idx += char_count;
    }

    let cursor = if output.response.has_focus() && cursor_should_show(ui) {
        output.cursor_range.map(|range| range.primary.index)
    } else {
        None
    };
    let table_cursor_drawn = draw_table_overlays(ui, &lines, &table_rows, cursor);
    if !table_cursor_drawn {
        draw_text_cursor_overlay(ui, output);
    }
}

fn table_overlay_rows(
    output: &egui::widgets::text_edit::TextEditOutput,
    lines: &[&str],
) -> Vec<Option<TableOverlayRow>> {
    let mut rows = Vec::with_capacity(lines.len());
    let mut current_char_idx = 0;

    for line in lines {
        let mut row = None;

        if line.trim_start().starts_with('|') {
            let mut left = None;
            let mut top = None;
            let mut bottom = None;

            for (i, ch) in line.chars().enumerate() {
                if ch == '|' {
                    let rect = output
                        .galley
                        .pos_from_cursor(egui::text::CCursor::new(current_char_idx + i))
                        .translate(output.galley_pos.to_vec2());

                    left.get_or_insert(rect.left());
                    top.get_or_insert(rect.top());
                    bottom = Some(rect.bottom());
                }
            }

            if let (Some(left), Some(top), Some(bottom)) = (left, top, bottom) {
                row = Some(TableOverlayRow {
                    left,
                    top,
                    bottom,
                    start_char: current_char_idx,
                    end_char: current_char_idx + line.chars().count(),
                });
            }
        }

        rows.push(row);
        current_char_idx += line.chars().count();
    }

    rows
}

fn draw_table_overlays(
    ui: &egui::Ui,
    lines: &[&str],
    rows: &[Option<TableOverlayRow>],
    cursor: Option<usize>,
) -> bool {
    let painter = ui.painter();
    let font_id = egui::FontId::monospace(14.0);
    let text_color = ui.visuals().text_color();
    let stroke = egui::Stroke::new(1.0, Color32::DARK_GRAY);
    let fill = ui.visuals().faint_bg_color;
    let cell_pad = 10.0_f32;
    let min_cell_width = 56.0_f32;
    let mut index = 0;
    let mut cursor_drawn = false;

    while index < rows.len() {
        if rows[index].is_none() {
            index += 1;
            continue;
        }

        let start = index;
        while index < rows.len() && rows[index].is_some() {
            index += 1;
        }
        let end = index;

        let parsed_rows = lines[start..end]
            .iter()
            .map(|line| parse_table_cells(line))
            .collect::<Vec<_>>();
        let Some(col_count) = parsed_rows
            .iter()
            .map(Vec::len)
            .filter(|count| *count > 0)
            .min()
        else {
            continue;
        };

        let block_left = rows[start..end]
            .iter()
            .flatten()
            .map(|row| row.left)
            .fold(f32::INFINITY, f32::min);
        let block_top = rows[start].as_ref().map_or(0.0, |row| row.top);
        let block_bottom = rows[end - 1].as_ref().map_or(block_top, |row| row.bottom);

        let mut col_widths = vec![min_cell_width; col_count];
        for cells in &parsed_rows {
            for (cell_index, cell) in cells.iter().take(col_count).enumerate() {
                let galley = painter.layout_no_wrap(cell.text.clone(), font_id.clone(), text_color);
                col_widths[cell_index] =
                    col_widths[cell_index].max(galley.size().x + cell_pad * 2.0);
            }
        }

        let block_width = col_widths.iter().sum::<f32>();
        let block_right = block_left + block_width;

        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(block_left, block_top),
                egui::pos2(block_right, block_bottom),
            ),
            0.0,
            fill,
        );

        let mut x = block_left;
        painter.line_segment(
            [egui::pos2(x, block_top), egui::pos2(x, block_bottom)],
            stroke,
        );
        for width in &col_widths {
            x += *width;
            painter.line_segment(
                [egui::pos2(x, block_top), egui::pos2(x, block_bottom)],
                stroke,
            );
        }

        painter.line_segment(
            [
                egui::pos2(block_left, block_top),
                egui::pos2(block_right, block_top),
            ],
            stroke,
        );

        for (row_offset, row) in rows[start..end].iter().flatten().enumerate() {
            painter.line_segment(
                [
                    egui::pos2(block_left, row.bottom),
                    egui::pos2(block_right, row.bottom),
                ],
                stroke,
            );

            let Some(cells) = parsed_rows.get(row_offset) else {
                continue;
            };
            let mut cell_left = block_left;
            for (cell_index, cell) in cells.iter().take(col_count).enumerate() {
                if !cell.text.is_empty() {
                    painter.text(
                        egui::pos2(cell_left + cell_pad, (row.top + row.bottom) / 2.0),
                        egui::Align2::LEFT_CENTER,
                        &cell.text,
                        font_id.clone(),
                        text_color,
                    );
                }
                cell_left += col_widths[cell_index];
            }
        }

        if let Some(cursor) = cursor {
            cursor_drawn |= draw_table_cursor(
                ui,
                rows,
                &parsed_rows,
                &col_widths,
                start,
                end,
                block_left,
                cell_pad,
                cursor,
                &font_id,
                text_color,
            );
        }
    }

    cursor_drawn
}

fn draw_table_cursor(
    ui: &egui::Ui,
    rows: &[Option<TableOverlayRow>],
    parsed_rows: &[Vec<TableCell>],
    col_widths: &[f32],
    start: usize,
    end: usize,
    block_left: f32,
    cell_pad: f32,
    cursor: usize,
    font_id: &egui::FontId,
    text_color: Color32,
) -> bool {
    for row_index in start..end {
        let Some(row) = &rows[row_index] else {
            continue;
        };
        if cursor < row.start_char || cursor > row.end_char {
            continue;
        }

        let Some(cells) = parsed_rows.get(row_index - start) else {
            return false;
        };
        let local_cursor = cursor.saturating_sub(row.start_char);
        let Some((cell_index, cell)) = table_cell_at_cursor(cells, local_cursor) else {
            return false;
        };

        let cell_left = block_left + col_widths.iter().take(cell_index).sum::<f32>();
        let display_offset = table_cell_display_offset(cell, local_cursor);
        let prefix = cell.text.chars().take(display_offset).collect::<String>();
        let prefix_width = ui
            .painter()
            .layout_no_wrap(prefix, font_id.clone(), text_color)
            .size()
            .x;
        let x = cell_left + cell_pad + prefix_width;
        ui.painter().line_segment(
            [egui::pos2(x, row.top), egui::pos2(x, row.bottom)],
            ui.visuals().text_cursor.stroke,
        );
        return true;
    }

    false
}

fn table_cursor_from_pointer(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    text: &str,
    pos: egui::Pos2,
) -> Option<usize> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let rows = table_overlay_rows(output, &lines);
    let font_id = egui::FontId::monospace(14.0);
    let text_color = ui.visuals().text_color();
    let cell_pad = 10.0_f32;
    let min_cell_width = 56.0_f32;
    let mut index = 0;

    while index < rows.len() {
        if rows[index].is_none() {
            index += 1;
            continue;
        }

        let start = index;
        while index < rows.len() && rows[index].is_some() {
            index += 1;
        }
        let end = index;

        let parsed_rows = lines[start..end]
            .iter()
            .map(|line| parse_table_cells(line))
            .collect::<Vec<_>>();
        let col_count = parsed_rows
            .iter()
            .map(Vec::len)
            .filter(|count| *count > 0)
            .min()?;

        let block_left = rows[start..end]
            .iter()
            .flatten()
            .map(|row| row.left)
            .fold(f32::INFINITY, f32::min);
        let block_top = rows[start].as_ref().map_or(0.0, |row| row.top);
        let block_bottom = rows[end - 1].as_ref().map_or(block_top, |row| row.bottom);

        let mut col_widths = vec![min_cell_width; col_count];
        for cells in &parsed_rows {
            for (cell_index, cell) in cells.iter().take(col_count).enumerate() {
                let galley =
                    ui.painter()
                        .layout_no_wrap(cell.text.clone(), font_id.clone(), text_color);
                col_widths[cell_index] =
                    col_widths[cell_index].max(galley.size().x + cell_pad * 2.0);
            }
        }

        let block_width = col_widths.iter().sum::<f32>();
        let block_right = block_left + block_width;
        if !egui::Rect::from_min_max(
            egui::pos2(block_left, block_top),
            egui::pos2(block_right, block_bottom),
        )
        .contains(pos)
        {
            continue;
        }

        let row_index = rows[start..end]
            .iter()
            .enumerate()
            .find_map(|(offset, row)| {
                let row = row.as_ref()?;
                (pos.y >= row.top && pos.y <= row.bottom).then_some(start + offset)
            })?;
        let row = rows[row_index].as_ref()?;
        let cells = parsed_rows.get(row_index - start)?;
        let mut cell_left = block_left;

        for (cell_index, width) in col_widths.iter().take(col_count).enumerate() {
            let cell_right = cell_left + *width;
            if pos.x <= cell_right || cell_index == col_count - 1 {
                let cell = cells.get(cell_index).or_else(|| cells.last())?;
                let text_x = pos.x - cell_left - cell_pad;
                return Some(
                    row.start_char
                        + table_cell_cursor_from_x(ui, cell, text_x, &font_id, text_color),
                );
            }
            cell_left = cell_right;
        }
    }

    None
}

fn table_cell_at_cursor(cells: &[TableCell], local_cursor: usize) -> Option<(usize, &TableCell)> {
    for (index, cell) in cells.iter().enumerate() {
        if local_cursor <= cell.raw_end {
            return Some((index, cell));
        }
    }
    cells.last().map(|cell| (cells.len() - 1, cell))
}

fn table_cell_display_offset(cell: &TableCell, local_cursor: usize) -> usize {
    if cell.text.is_empty() || local_cursor <= cell.clean_start {
        return 0;
    }
    if local_cursor >= cell.clean_end {
        return cell.text.chars().count();
    }
    local_cursor - cell.clean_start
}

fn table_cell_cursor_from_x(
    ui: &egui::Ui,
    cell: &TableCell,
    text_x: f32,
    font_id: &egui::FontId,
    text_color: Color32,
) -> usize {
    let char_count = cell.text.chars().count();
    if char_count == 0 || text_x <= 0.0 {
        return cell.clean_start;
    }

    let mut previous_width = 0.0;
    for offset in 1..=char_count {
        let prefix = cell.text.chars().take(offset).collect::<String>();
        let width = ui
            .painter()
            .layout_no_wrap(prefix, font_id.clone(), text_color)
            .size()
            .x;
        let middle = (previous_width + width) / 2.0;
        if text_x < middle {
            return cell.clean_start + offset - 1;
        }
        previous_width = width;
    }

    cell.clean_end
}

fn draw_text_cursor_overlay(ui: &egui::Ui, output: &egui::widgets::text_edit::TextEditOutput) {
    if !output.response.has_focus() {
        return;
    }
    let Some(cursor_range) = output.cursor_range else {
        return;
    };
    if !cursor_should_show(ui) {
        return;
    }

    let rect = output
        .galley
        .pos_from_cursor(cursor_range.primary)
        .translate(output.galley_pos.to_vec2());
    let stroke = ui.visuals().text_cursor.stroke;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), rect.top()),
            egui::pos2(rect.left(), rect.bottom()),
        ],
        stroke,
    );
}

fn cursor_should_show(ui: &egui::Ui) -> bool {
    let cursor = &ui.visuals().text_cursor;
    if !cursor.blink {
        return true;
    }

    let cycle = cursor.on_duration + cursor.off_duration;
    if cycle <= 0.0 {
        return true;
    }
    let time = ui.input(|input| input.time as f32);
    time % cycle < cursor.on_duration
}

fn markdown_layouter(ui: &egui::Ui, text: &str, wrap_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    let color = ui.visuals().text_color();

    let mut in_code_block = false;

    for line in text.split_inclusive('\n') {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
        }

        let mut font_id = egui::FontId::proportional(16.0);
        let mut line_color = color;
        let mut italics = false;
        let mut background = egui::Color32::TRANSPARENT;

        let trimmed = line.trim_start();

        let mut header_level = 0;
        let mut is_quote = false;

        if in_code_block || line.starts_with("```") {
            font_id = egui::FontId::monospace(14.0);
            background = ui.visuals().extreme_bg_color;
        } else if trimmed.starts_with("# ") {
            header_level = 1;
            font_id = egui::FontId::proportional(32.0);
        } else if trimmed.starts_with("## ") {
            header_level = 2;
            font_id = egui::FontId::proportional(24.0);
        } else if trimmed.starts_with("### ") {
            header_level = 3;
            font_id = egui::FontId::proportional(20.0);
        } else if trimmed.starts_with("> ") {
            is_quote = true;
            line_color = egui::Color32::GRAY;
            italics = true;
            background = ui.visuals().faint_bg_color;
        }

        if header_level > 0 {
            let offset = line.find("#").unwrap();
            let marker_len = header_level + 1; // "# " is 2, "## " is 3
            let format = egui::TextFormat {
                font_id: font_id.clone(),
                color: line_color,
                background,
                italics,
                ..Default::default()
            };
            if offset > 0 {
                job.append(&line[..offset], 0.0, format.clone());
            }
            let mut marker_format = format.clone();
            marker_format.color = egui::Color32::TRANSPARENT;
            marker_format.font_id = egui::FontId::proportional(1.0);
            job.append(&line[offset..offset + marker_len], 0.0, marker_format);
            job.append(&line[offset + marker_len..], 0.0, format);
            continue;
        }

        if is_quote {
            let offset = line.find(">").unwrap();
            let format = egui::TextFormat {
                font_id: font_id.clone(),
                color: line_color,
                background,
                italics,
                ..Default::default()
            };
            if offset > 0 {
                job.append(&line[..offset], 0.0, format.clone());
            }
            let mut marker_format = format.clone();
            marker_format.color = egui::Color32::TRANSPARENT;
            marker_format.font_id = egui::FontId::proportional(1.0);
            job.append(&line[offset..offset + 2], 0.0, marker_format);
            job.append(&line[offset + 2..], 0.0, format);
            continue;
        }

        if trimmed.starts_with("- [ ] ") || trimmed.starts_with("- [x] ") {
            let offset = line.find("- [").unwrap();
            let format = egui::TextFormat {
                font_id: font_id.clone(),
                color: line_color,
                background,
                italics,
                ..Default::default()
            };
            if offset > 0 {
                job.append(&line[..offset], 0.0, format.clone());
            }
            let checkbox = &line[offset..offset + 6];
            let checked = checkbox == "- [x] ";
            let mut cb_format = format.clone();
            cb_format.color = egui::Color32::TRANSPARENT;
            cb_format.font_id = egui::FontId::monospace(16.0);
            job.append(checkbox, 0.0, cb_format);
            let rest = &line[offset + 6..];
            let mut rest_format = format.clone();
            if checked {
                rest_format.color = egui::Color32::GRAY;
                rest_format.strikethrough = egui::Stroke::new(1.0, egui::Color32::GRAY);
            }
            job.append(rest, 0.0, rest_format);
            continue;
        }

        if is_divider_line(trimmed.trim_end()) {
            let mut format = egui::TextFormat {
                font_id: font_id.clone(),
                color: Color32::TRANSPARENT,
                background,
                italics,
                ..Default::default()
            };
            format.background = Color32::TRANSPARENT;
            job.append(line, 0.0, format);
            continue;
        }

        if is_table_divider_line(trimmed.trim_end()) {
            let format = egui::TextFormat {
                font_id: egui::FontId::monospace(14.0),
                color: Color32::TRANSPARENT,
                background: Color32::TRANSPARENT,
                italics,
                ..Default::default()
            };
            job.append(line, 0.0, format);
            continue;
        }

        if trimmed.starts_with("|") {
            let format = egui::TextFormat {
                font_id: egui::FontId::monospace(14.0),
                color: Color32::TRANSPARENT,
                background: Color32::TRANSPARENT,
                italics,
                ..Default::default()
            };
            job.append(line, 0.0, format);
            continue;
        }

        let format = egui::TextFormat {
            font_id,
            color: line_color,
            background,
            italics,
            ..Default::default()
        };
        job.append(line, 0.0, format);
    }

    if text.is_empty() {
        job.append(
            "",
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(16.0),
                color,
                ..Default::default()
            },
        );
    }

    job.wrap.max_width = wrap_width;
    ui.painter().layout_job(job)
}

fn is_divider_line(line: &str) -> bool {
    matches!(line.trim(), "---" | "***" | "___")
}

fn is_table_divider_line(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return false;
    }

    let mut cells = 0;
    for cell in trimmed.trim_matches('|').split('|') {
        if !is_table_separator_cell(cell) {
            return false;
        }
        cells += 1;
    }

    cells > 0
}

fn is_table_separator_cell(cell: &str) -> bool {
    let cell = cell.trim();
    if cell.is_empty() {
        return false;
    }
    let dashes = cell.trim_matches(':');
    dashes.len() >= 3 && dashes.chars().all(|ch| ch == '-')
}

#[cfg(test)]
fn table_cells(line: &str) -> Vec<String> {
    parse_table_cells(line)
        .into_iter()
        .map(|cell| cell.text)
        .collect()
}

fn parse_table_cells(line: &str) -> Vec<TableCell> {
    if !line.trim_start().starts_with('|') {
        return Vec::new();
    }

    let chars = line.chars().collect::<Vec<_>>();
    let line_end = chars
        .iter()
        .rposition(|ch| *ch != '\n' && *ch != '\r')
        .map_or(0, |index| index + 1);
    let pipes = chars
        .iter()
        .take(line_end)
        .enumerate()
        .filter_map(|(index, ch)| (*ch == '|').then_some(index))
        .collect::<Vec<_>>();
    if pipes.is_empty() {
        return Vec::new();
    }

    let mut cells = pipes
        .windows(2)
        .map(|pair| build_table_cell(&chars, pair[0] + 1, pair[1]))
        .collect::<Vec<_>>();

    if let Some(last_pipe) = pipes.last().copied() {
        if last_pipe + 1 < line_end {
            cells.push(build_table_cell(&chars, last_pipe + 1, line_end));
        }
    }

    cells
}

fn build_table_cell(chars: &[char], raw_start: usize, raw_end: usize) -> TableCell {
    let mut clean_start = raw_start;
    let mut clean_end = raw_end;

    while clean_start < clean_end && chars[clean_start].is_whitespace() {
        clean_start += 1;
    }
    while clean_end > clean_start && chars[clean_end - 1].is_whitespace() {
        clean_end -= 1;
    }

    let trimmed = chars[clean_start..clean_end].iter().collect::<String>();
    if is_table_separator_cell(&trimmed) {
        return TableCell {
            text: String::new(),
            raw_start,
            raw_end,
            clean_start,
            clean_end: clean_start,
        };
    }

    while clean_start < clean_end && matches!(chars[clean_start], '-' | ':') {
        clean_start += 1;
    }
    while clean_end > clean_start && matches!(chars[clean_end - 1], '-' | ':') {
        clean_end -= 1;
    }
    while clean_start < clean_end && chars[clean_start].is_whitespace() {
        clean_start += 1;
    }
    while clean_end > clean_start && chars[clean_end - 1].is_whitespace() {
        clean_end -= 1;
    }

    TableCell {
        text: chars[clean_start..clean_end].iter().collect(),
        raw_start,
        raw_end,
        clean_start,
        clean_end,
    }
}

fn is_visual_only_markdown_line(line: &str) -> bool {
    is_divider_line(line) || is_table_divider_line(line)
}

fn set_text_cursor(ctx: &egui::Context, id: egui::Id, cursor: usize) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        if let Some(mut ccursor) = state.cursor.char_range() {
            ccursor.primary.index = cursor;
            ccursor.secondary.index = cursor;
            state.cursor.set_char_range(Some(ccursor));
            egui::TextEdit::store_state(ctx, id, state);
        }
    }
}

fn replace_char_range(text: &mut String, start: usize, end: usize, replacement: &str) {
    let start_byte = char_to_byte(text, start);
    let end_byte = char_to_byte(text, end);
    if start_byte <= end_byte && end_byte <= text.len() {
        text.replace_range(start_byte..end_byte, replacement);
    }
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_notes_keep_content() {
        let notes = parse_legacy_notes("A\none\ntwo\x1EB\nthree");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].title, "A");
        assert_eq!(notes[0].content, "one\ntwo");
        assert_eq!(notes[1].title, "B");
    }

    #[test]
    fn empty_title_uses_content() {
        let note = Note {
            id: 1,
            title: String::new(),
            content: "\nhello world".to_string(),
            created: 1,
            updated: 1,
        };
        assert_eq!(note.display_title(), "hello world");
    }

    #[test]
    fn default_note_is_grass() {
        let note = welcome_note();
        assert_eq!(note.title, "Grass");
        assert_eq!(note.content, ",,,,,,,,,,,,,,,,,,,,,,,,,,,,,,,,,, <- touch");
    }

    #[test]
    fn display_title_skips_visual_markdown_lines() {
        let note = Note {
            id: 1,
            title: String::new(),
            content: "---\n|---|---|\nreal title".to_string(),
            created: 1,
            updated: 1,
        };
        assert_eq!(note.display_title(), "real title");
    }

    #[test]
    fn table_divider_lines_match_spacing_and_alignment() {
        assert!(is_table_divider_line("|---|---|"));
        assert!(is_table_divider_line("| --- | --- |"));
        assert!(is_table_divider_line("|:---|---:|:---:|"));
        assert!(!is_table_divider_line("| one | two |"));
        assert!(!is_table_divider_line("---"));
    }

    #[test]
    fn table_cells_clean_visual_markers() {
        assert_eq!(
            table_cells("|-가능하다--|---|"),
            vec!["가능하다".to_string(), String::new()]
        );
        assert_eq!(
            table_cells("|이런   | 리스트도  |"),
            vec!["이런".to_string(), "리스트도".to_string()]
        );
    }

    #[test]
    fn table_cursor_offset_uses_rendered_cell_text() {
        let cells = parse_table_cells("|까 아닐까|되는거라고 해야할까|");
        let (_, cell) = table_cell_at_cursor(&cells, 2).unwrap();
        assert_eq!(cell.text, "까 아닐까");
        assert_eq!(table_cell_display_offset(cell, 2), 1);

        let stripped_cells = parse_table_cells("|-가능하다--|---|");
        let (_, stripped) = table_cell_at_cursor(&stripped_cells, 4).unwrap();
        assert_eq!(stripped.text, "가능하다");
        assert_eq!(table_cell_display_offset(stripped, 4), 2);
    }

    #[test]
    fn truncate_marks_long_text() {
        assert_eq!(truncate("abcdef", 4), "abc~");
    }

    #[test]
    fn save_load_round_trips() {
        let path = env::temp_dir().join(format!("note-test-{}.json", unix_time()));
        let notes = vec![Note::new("A")];
        save_notes(&path, &notes).unwrap();
        let loaded = load_notes(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(loaded[0].title, "A");
    }

    #[test]
    fn slash_trigger_reads_query() {
        assert_eq!(slash_trigger("hello /to", 9), Some((6, "to".to_string())));
        assert_eq!(slash_trigger("hello /to", 5), None);
    }

    #[test]
    fn replacing_char_range_handles_unicode() {
        let mut text = "한글 /ti".to_string();
        replace_char_range(&mut text, 3, 6, "# ");
        assert_eq!(text, "한글 # ");
    }

    #[test]
    fn checkbox_toggles_by_cursor() {
        let mut text = "- [ ] done".to_string();
        assert!(toggle_checkbox_at(&mut text, 2));
        assert_eq!(text, "- [x] done");
        assert!(toggle_checkbox_at(&mut text, 2));
        assert_eq!(text, "- [ ] done");
    }

    #[test]
    fn smart_newline_continues_todo() {
        let mut text = "- [ ] one\n".to_string();
        let cursor = text.chars().count();
        let next = apply_smart_newline(&mut text, cursor);
        assert_eq!(next, Some("- [ ] one\n- [ ] ".chars().count()));
        assert_eq!(text, "- [ ] one\n- [ ] ");
    }

    #[test]
    fn smart_newline_continues_numbered() {
        let mut text = "1. one\n".to_string();
        let cursor = text.chars().count();
        let next = apply_smart_newline(&mut text, cursor);
        assert_eq!(next, Some("1. one\n2. ".chars().count()));
        assert_eq!(text, "1. one\n2. ");
    }

    #[test]
    fn smart_newline_stops_empty_numbered() {
        let mut text = "1.\n".to_string();
        let cursor = text.chars().count();
        let next = apply_smart_newline(&mut text, cursor);
        assert_eq!(next, Some(0));
        assert_eq!(text, "");
    }

    #[test]
    fn smart_newline_stops_empty_numbered_after_item() {
        let mut text = "1. one\n2. \n".to_string();
        let cursor = text.chars().count();
        let next = apply_smart_newline(&mut text, cursor);
        assert_eq!(next, Some("1. one\n".chars().count()));
        assert_eq!(text, "1. one\n");
    }

    #[test]
    fn smart_newline_continues_double_digit_number() {
        let mut text = "9. nine\n".to_string();
        let cursor = text.chars().count();
        let next = apply_smart_newline(&mut text, cursor);
        assert_eq!(next, Some("9. nine\n10. ".chars().count()));
        assert_eq!(text, "9. nine\n10. ");
    }

    #[test]
    fn smart_newline_ignores_deleted_number_marker() {
        let mut text = ". item\n".to_string();
        let cursor = text.chars().count();
        let next = apply_smart_newline(&mut text, cursor);
        assert_eq!(next, None);
        assert_eq!(text, ". item\n");
    }

    #[test]
    fn slash_matches_aliases() {
        let mut app = NoteApp::new();
        app.slash.query = "ol".to_string();
        let labels = app
            .slash_match_indices()
            .into_iter()
            .map(|index| SLASH_COMMANDS[index].label)
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Numbered list"));
    }

    #[test]
    fn smart_newline_removes_empty_marker() {
        let mut text = "- [ ] \n".to_string();
        let cursor = text.chars().count();
        let next = apply_smart_newline(&mut text, cursor);
        assert_eq!(next, Some(0));
        assert_eq!(text, "");
    }
}
