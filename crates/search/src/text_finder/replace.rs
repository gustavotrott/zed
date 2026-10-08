use std::ops::Range;

use collections::{HashMap, HashSet};
use gpui::{AnyElement, Entity, EntityId, Focusable as _, KeyContext, TaskExt as _, prelude::*};
use language::{Buffer, BufferSnapshot};
use picker::Picker;
use project::search::SearchQuery;
use text::ToOffset as _;
use ui::{IconButton, IconButtonShape, Tooltip, prelude::*};

use super::delegate::{Delegate, Entry};
use crate::{ReplaceAll, ReplaceNext, ToggleReplace};

impl Delegate {
    pub(crate) fn toggle_replace(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.replace_enabled = !self.replace_enabled;
        let editor_to_focus = if self.replace_enabled {
            self.replacement_editor.as_ref()
        } else {
            self.query_editor.as_ref()
        };
        if let Some(editor) = editor_to_focus {
            window.focus(&editor.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// Replaces the selected match and moves on to the next one.
    pub(crate) fn replace_next(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(Entry::Match(match_index)) = self.entries.get(self.selected_index) else {
            return;
        };
        let match_index = *match_index;
        self.replace_matches(&[match_index], window, cx);
    }

    /// Replaces the multi-selected matches, or every match when there's no multi-selection.
    pub(crate) fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let match_indices: Vec<usize> = if self.selected_matches.is_empty() {
            (0..self.matches.len()).collect()
        } else {
            self.matches
                .iter()
                .enumerate()
                .filter(|(_, search_match)| {
                    self.selected_matches
                        .iter()
                        .any(|selected| selected == *search_match)
                })
                .map(|(index, _)| index)
                .collect()
        };
        self.selected_matches.clear();
        self.replace_matches(&match_indices, window, cx);
    }

    fn query_with_replacement(&self, cx: &App) -> Option<SearchQuery> {
        let replacement = self.replacement_editor.as_ref()?.read(cx).text(cx);
        Some(self.active_query.clone()?.with_replacement(replacement))
    }

    fn replace_matches(
        &mut self,
        match_indices: &[usize],
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        if !self.replace_enabled || match_indices.is_empty() {
            return;
        }
        let Some(query) = self.query_with_replacement(cx) else {
            return;
        };

        let mut buffers: HashMap<EntityId, (Entity<Buffer>, Vec<usize>)> = HashMap::default();
        for &match_index in match_indices {
            let Some(search_match) = self.matches.get(match_index) else {
                continue;
            };
            buffers
                .entry(search_match.buffer.entity_id())
                .or_insert_with(|| (search_match.buffer.clone(), Vec::new()))
                .1
                .push(match_index);
        }

        let mut buffers_to_save = Vec::new();
        for (buffer, buffer_match_indices) in buffers.into_values() {
            let was_dirty = buffer.read(cx).is_dirty();
            let snapshot = buffer.read(cx).snapshot();
            let edits: Vec<(Range<usize>, String)> = buffer_match_indices
                .iter()
                .filter_map(|&match_index| {
                    let search_match = self.matches.get(match_index)?;
                    let range = search_match.anchor_range.start.to_offset(&snapshot)
                        ..search_match.anchor_range.end.to_offset(&snapshot);
                    let replacement = replacement_for_range(&query, &snapshot, range.clone())?;
                    Some((range, replacement))
                })
                .collect();
            if edits.is_empty() {
                continue;
            }
            buffer.update(cx, |buffer, cx| {
                buffer.start_transaction();
                buffer.edit(edits, None, cx);
                buffer.end_transaction(cx);
            });
            // Buffers the finder touched may not be open anywhere, so write the replacements to
            // disk. A buffer that already had unsaved edits is left for the user to save, so
            // their own changes aren't saved behind their back.
            if !was_dirty {
                buffers_to_save.push(buffer);
            }
        }

        let project = self.project(cx).clone();
        for buffer in buffers_to_save {
            project
                .update(cx, |project, cx| project.save_buffer(buffer, cx))
                .detach_and_log_err(cx);
        }

        let replaced: HashSet<usize> = match_indices.iter().copied().collect();
        let mut index = 0;
        self.matches.retain(|_| {
            let keep = !replaced.contains(&index);
            index += 1;
            keep
        });
        self.refresh_match_positions(cx);
        self.unique_files = self
            .matches
            .iter()
            .map(|search_match| search_match.path.clone())
            .collect();

        // Keep the selection on the row the replaced match occupied, which now holds the next match.
        let selected_index = self.selected_index;
        self.rebuild_entries();
        self.selected_index = (selected_index..self.entries.len())
            .chain((0..selected_index.min(self.entries.len())).rev())
            .find(|&index| matches!(self.entries.get(index), Some(Entry::Match(_))))
            .unwrap_or(0);
        cx.defer_in(window, |picker, window, cx| {
            picker.matches_changed(window, cx);
        });
        cx.notify();
    }

    /// Recomputes the offsets and line numbers of the matches from their anchors, which stay
    /// valid across the edits made by replacing.
    fn refresh_match_positions(&mut self, cx: &App) {
        let mut snapshots: HashMap<EntityId, BufferSnapshot> = HashMap::default();
        for search_match in &mut self.matches {
            let snapshot = snapshots
                .entry(search_match.buffer.entity_id())
                .or_insert_with(|| search_match.buffer.read(cx).snapshot());
            let range = search_match.anchor_range.start.to_offset(snapshot)
                ..search_match.anchor_range.end.to_offset(snapshot);
            let point = snapshot.offset_to_point(range.start);
            search_match.range = range;
            search_match.match_start_byte_column = point.column;
            search_match.line_number = point.row + 1;
        }
    }

    pub(crate) fn render_replace_row(&self, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        if !self.replace_enabled {
            return None;
        }
        let replacement_editor = self.replacement_editor.clone()?;
        let focus_handle = self.focus_handle.clone();
        let has_matches = !self.matches.is_empty();
        let replace_all_label = if self.selected_matches.is_empty() {
            "Replace All Matches"
        } else {
            "Replace Selected Matches"
        };
        let mut key_context = KeyContext::default();
        key_context.add("TextFinderReplace");

        Some(
            h_flex()
                .key_context(key_context)
                .h_9()
                .px_2p5()
                .gap_1()
                .flex_none()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    Icon::new(IconName::Replace)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(div().flex_1().child(replacement_editor))
                .child(
                    IconButton::new("text-finder-replace-next", IconName::ReplaceNext)
                        .shape(IconButtonShape::Square)
                        .icon_size(IconSize::Small)
                        .disabled(!has_matches)
                        .tooltip({
                            let focus_handle = focus_handle.clone();
                            move |_window, cx| {
                                Tooltip::for_action_in(
                                    "Replace Selected Match",
                                    &ReplaceNext,
                                    &focus_handle,
                                    cx,
                                )
                            }
                        })
                        .on_click(cx.listener(|picker, _, window, cx| {
                            picker.delegate.replace_next(window, cx);
                        })),
                )
                .child(
                    IconButton::new("text-finder-replace-all", IconName::ReplaceAll)
                        .shape(IconButtonShape::Square)
                        .icon_size(IconSize::Small)
                        .disabled(!has_matches)
                        .tooltip(move |_window, cx| {
                            Tooltip::for_action_in(
                                replace_all_label,
                                &ReplaceAll,
                                &focus_handle,
                                cx,
                            )
                        })
                        .on_click(cx.listener(|picker, _, window, cx| {
                            picker.delegate.replace_all(window, cx);
                        })),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn render_replace_toggle(&self, cx: &mut Context<Picker<Self>>) -> AnyElement {
        let focus_handle = self.focus_handle.clone();
        IconButton::new("text-finder-toggle-replace", IconName::Replace)
            .icon_size(IconSize::Small)
            .toggle_state(self.replace_enabled)
            .tooltip(move |_window, cx| {
                Tooltip::for_action_in("Toggle Replace", &ToggleReplace, &focus_handle, cx)
            })
            .on_click(cx.listener(|picker, _, window, cx| {
                picker.delegate.toggle_replace(window, cx);
            }))
            .into_any_element()
    }
}

/// The text replacing `range`, expanding regex capture groups against the matched line the same
/// way the buffer and project searches do.
fn replacement_for_range(
    query: &SearchQuery,
    snapshot: &BufferSnapshot,
    range: Range<usize>,
) -> Option<String> {
    if !query.replacement_requires_context() {
        return query.replacement().map(str::to_string);
    }
    let start = snapshot.offset_to_point(range.start);
    let end = snapshot.offset_to_point(range.end);
    if start.row == end.row {
        let line_start = snapshot.point_to_offset(text::Point::new(start.row, 0));
        let line_end =
            snapshot.point_to_offset(text::Point::new(start.row, snapshot.line_len(start.row)));
        let line: String = snapshot.text_for_range(line_start..line_end).collect();
        query
            .replacement_for(&line, range.start - line_start..range.end - line_start)
            .map(|replacement| replacement.into_owned())
    } else {
        let text: String = snapshot.text_for_range(range).collect();
        let hit = 0..text.len();
        query
            .replacement_for(&text, hit)
            .map(|replacement| replacement.into_owned())
    }
}
