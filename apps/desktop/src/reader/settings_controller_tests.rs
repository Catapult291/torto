use super::*;
use std::sync::Arc;

#[test]
fn unchanged_settings_keep_prepared_pages_focus_units_and_restore_target() {
    let (mut reader, _, range) = crate::reader::semantic_layout::tests::fixture();
    let style = reader.reader.style();
    let mut settings = AppliedSettings {
        spread: style.spread,
        reading_mode: reader.reading_mode,
        focus_style: reader.focus_style,
        hide_cursor_in_focus_mode: reader.hide_cursor_in_focus_mode,
        interface_typography: Default::default(),
        typography: style.typography,
        typesetting: style.typesetting,
        plugin_settings: reader.plugin_settings.clone(),
        language: reader.language,
        theme: Default::default(),
        selection_granularity: reader.selection_granularity,
        shortcuts: reader.shortcuts.clone(),
        sync_settings: reader.sync_settings.clone(),
        sync_password: reader.sync_password.clone(),
    };
    reader.apply_global_settings(&settings);
    let layout = reader.current_scroll_layout().unwrap();
    reader.rebuild_focus_units(&layout);
    let units = reader.focus_units.len();
    reader.scroll_target_source = Some(range.clone());
    reader.apply_global_settings(&settings);
    assert!(Arc::ptr_eq(
        &layout,
        reader.scroll_section.as_ref().unwrap()
    ));
    assert_eq!(reader.focus_units.len(), units);
    assert!(reader.focus_units_ready);
    assert_eq!(reader.scroll_target_source, Some(range));

    reader.scroll_viewport = Some(crate::reader::ScrollViewportState {
        size: egui::vec2(800.0, 600.0),
        offset_y: 0.0,
    });
    reader.page_scene();
    let cached = reader.page_scenes.clone();
    assert!(!cached.is_empty());
    let revision = reader.scene_revision;
    settings.focus_style = crate::preferences::FocusStyle::SurroundingDim;
    reader.apply_global_settings(&settings);
    assert_eq!(reader.focus_style, settings.focus_style);
    assert_ne!(reader.scene_revision, revision);
    assert!(Arc::ptr_eq(
        &layout,
        reader.scroll_section.as_ref().unwrap()
    ));
    reader.page_scene();
    assert!(
        cached
            .iter()
            .all(|(key, value)| Arc::ptr_eq(value, &reader.page_scenes[key]))
    );
    assert_eq!(reader.focus_units.len(), units);
    let revision = reader.scene_revision;
    reader.apply_global_settings(&settings);
    assert_eq!(
        reader.scene_revision, revision,
        "unchanged focus style is quiet"
    );

    settings.typography.font_size += 1.0;
    reader.apply_global_settings(&settings);
    assert!(reader.scroll_section.is_none());
    assert!(reader.focus_units.is_empty());
}
