use egui::{
    native_text_input::{self as bridge, Edit},
    text::{CCursor, CCursorRange},
    *,
};

struct Editor {
    ctx: Context,
    text: String,
    id: Id,
    time: f64,
}
impl Editor {
    fn new(text: &str) -> Self {
        Self {
            ctx: Context::default(),
            text: text.into(),
            id: Id::new("native-editor"),
            time: 0.0,
        }
    }
    fn frame(
        &mut self,
        edit: Option<Edit>,
        events: Vec<Event>,
        password: bool,
        enabled: bool,
        limit: usize,
    ) -> Option<bridge::Snapshot> {
        bridge::begin_frame(&self.ctx, true, Some(self.id), edit);
        self.ctx.memory_mut(|m| m.request_focus(self.id));
        self.time += 1.0 / 60.0;
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                ui.add_enabled(
                    enabled,
                    TextEdit::multiline(&mut self.text)
                        .id(self.id)
                        .password(password)
                        .char_limit(limit),
                );
            },
        );
        output.textures_delta.clear();
        bridge::snapshot(&self.ctx)
    }
    fn edit(&self, text: &str) -> Edit {
        Edit {
            id: self.id,
            before: self.text.clone().into(),
            text: text.into(),
            selection: CCursorRange::one(CCursor::new(text.chars().count())),
            composition: None,
        }
    }
}

#[test]
fn native_text_input_publishes_the_full_focused_text_and_reverse_selection() {
    let text = "中文🙂 alpha\n".repeat(1000);
    let mut editor = Editor::new(&text);
    editor.frame(None, vec![], false, true, usize::MAX);
    let mut state = TextEdit::load_state(&editor.ctx, editor.id).unwrap();
    let selected = CCursorRange {
        primary: CCursor::new(1),
        secondary: CCursor::new(8),
        h_pos: None,
    };
    state.cursor.set_char_range(Some(selected));
    state.store(&editor.ctx, editor.id);
    let snapshot = editor.frame(None, vec![], false, true, usize::MAX).unwrap();
    assert_eq!(snapshot.text.as_ref(), text);
    assert_eq!(snapshot.selection, selected);
    assert!(snapshot.text.len() > 4000);
}

#[test]
fn native_text_input_replacement_is_immediately_undoable_and_redoable() {
    let original = "Original 🙂\n第二行";
    let translated = "译文🙂\nSecond line";
    let mut editor = Editor::new(original);
    editor.frame(None, vec![], false, true, usize::MAX);
    let edit = editor.edit(translated);
    editor.frame(Some(edit), vec![], false, true, usize::MAX);
    assert_eq!(editor.text, translated);
    let key = |key, shift| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            ctrl: true,
            command: true,
            shift,
            ..Modifiers::NONE
        },
    };
    editor.frame(None, vec![key(Key::Z, false)], false, true, usize::MAX);
    assert_eq!(editor.text, original);
    editor.frame(None, vec![key(Key::Y, false)], false, true, usize::MAX);
    assert_eq!(editor.text, translated);
}

#[test]
fn native_text_input_rejects_stale_wrong_editor_and_over_limit_replacements() {
    let mut editor = Editor::new("original");
    editor.frame(None, vec![], false, true, usize::MAX);
    let mut stale = editor.edit("translation");
    stale.before = "old text".into();
    editor.frame(Some(stale), vec![], false, true, usize::MAX);
    assert_eq!(editor.text, "original");
    let mut wrong = editor.edit("translation");
    wrong.id = Id::new("another-editor");
    editor.frame(Some(wrong), vec![], false, true, usize::MAX);
    assert_eq!(editor.text, "original");
    let edit = editor.edit("too many characters");
    editor.frame(Some(edit), vec![], false, true, 8);
    assert_eq!(editor.text, "original");
}

#[test]
fn native_text_input_never_exposes_or_changes_password_and_disabled_fields() {
    for (password, enabled) in [(true, true), (false, false)] {
        let mut editor = Editor::new("secret");
        let edit = editor.edit("replaced");
        assert!(
            editor
                .frame(Some(edit), vec![], password, enabled, usize::MAX)
                .is_none()
        );
        assert_eq!(editor.text, "secret");
    }
}

#[test]
fn native_text_input_composition_metadata_does_not_insert_a_second_copy() {
    let mut editor = Editor::new("before");
    editor.frame(None, vec![], false, true, usize::MAX);
    let mut edit = editor.edit("before中文");
    edit.composition = Some(6..8);
    editor.frame(
        Some(edit),
        vec![Event::Ime(ImeEvent::Preedit {
            text: "中文".into(),
            active_range_chars: None,
        })],
        false,
        true,
        usize::MAX,
    );
    assert_eq!(editor.text, "before中文");
    assert_eq!(bridge::composition(&editor.ctx, editor.id), Some(6..8));
    let edit = editor.edit("before中文");
    editor.frame(
        Some(edit),
        vec![Event::Ime(ImeEvent::Commit("中文".into()))],
        false,
        true,
        usize::MAX,
    );
    assert_eq!(editor.text, "before中文");
    assert!(bridge::composition(&editor.ctx, editor.id).is_none());
    editor.frame(
        None,
        vec![Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                ctrl: true,
                command: true,
                ..Modifiers::NONE
            },
        }],
        false,
        true,
        usize::MAX,
    );
    assert_eq!(editor.text, "before");
}
