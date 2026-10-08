//! A picker listing the commits that changed a file, previewing the file's diff in the
//! selected commit. Confirming opens the commit, filtered to the file.

use std::{ops::RangeInclusive, sync::Arc};

use anyhow::Context as _;
use editor::{Editor, EditorSettings, HiddenDiffHunkRenderer, MultiBuffer, PathKey};
use fuzzy::StringMatchCandidate;
use git::repository::{FileLogEntry, RepoPath};
use gpui::{
    AnyElement, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, SharedString, Subscription, Task, WeakEntity, Window,
};
use language::OffsetRangeExt as _;
use language::{Capability, Point};
use picker::{Picker, PickerDelegate, PreviewBackend, PreviewLayout, PreviewUpdate};
use project::{Project, git_store::Repository};
use settings::Settings as _;
use time::{OffsetDateTime, UtcOffset};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use util::ResultExt as _;
use workspace::{ModalView, Workspace};

use crate::commit_view::{
    CommitView, GitBlob, build_buffer, build_buffer_diff, worktree_id_for_repo_path,
};

/// How many commits to list. Older history is reachable through the commit view.
const COMMIT_LIMIT: usize = 2000;

pub(crate) fn register(workspace: &mut Workspace) {
    workspace.register_action(|workspace, _: &git::BrowseFileHistory, window, cx| {
        let Some((repository, repo_path)) = file_history_target(workspace, window, cx) else {
            return;
        };
        FileHistoryPicker::toggle(workspace, repository, repo_path, None, window, cx);
    });
    workspace.register_action(|workspace, _: &git::ShowHistoryForSelection, window, cx| {
        show_history_for_selection(workspace, window, cx).detach_and_log_err(cx);
    });
}

/// Opens the picker listing the commits that changed the lines of the active editor's newest
/// selection. `git log -L` addresses lines of the file at `HEAD`, so the selected rows are
/// first mapped through the buffer's uncommitted changes.
fn show_history_for_selection(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Task<anyhow::Result<()>> {
    let Some(editor) = workspace.active_item_as::<Editor>(cx) else {
        return Task::ready(Ok(()));
    };
    let (start, end) = editor.update(cx, |editor, cx| {
        let selection = editor
            .selections
            .newest::<Point>(&editor.display_snapshot(cx));
        let mut end = selection.end;
        // A selection of whole lines ends at the start of the line after them.
        if end.column == 0 && end.row > selection.start.row {
            end.row -= 1;
        }
        (selection.start, end)
    });
    let multibuffer = editor.read(cx).buffer().read(cx);
    let Some((buffer, start)) = multibuffer.point_to_buffer_point(start, cx) else {
        return Task::ready(Ok(()));
    };
    let end = match multibuffer.point_to_buffer_point(end, cx) {
        Some((end_buffer, end)) if end_buffer == buffer => end,
        _ => start,
    };
    let Some(file) = buffer.read(cx).file() else {
        return Task::ready(Ok(()));
    };
    let project_path = project::ProjectPath {
        worktree_id: file.worktree_id(cx),
        path: file.path().clone(),
    };
    let Some((repository, repo_path)) = workspace
        .project()
        .read(cx)
        .git_store()
        .read(cx)
        .repository_and_path_for_project_path(&project_path, cx)
    else {
        return Task::ready(Ok(()));
    };
    let uncommitted_diff = workspace.project().update(cx, |project, cx| {
        project.open_uncommitted_diff(buffer.clone(), cx)
    });

    cx.spawn_in(window, async move |workspace, cx| {
        let uncommitted_diff = uncommitted_diff.await?;
        let line_range = cx.update(|_, cx| {
            let buffer = buffer.read(cx).text_snapshot();
            let diff = uncommitted_diff.read(cx).snapshot(cx);
            let start = diff.buffer_point_to_base_text_point(Point::new(start.row, 0), &buffer);
            let end = diff.buffer_point_to_base_text_point(Point::new(end.row, 0), &buffer);
            start.row + 1..=end.row.max(start.row) + 1
        })?;
        workspace.update_in(cx, |workspace, window, cx| {
            FileHistoryPicker::toggle(
                workspace,
                repository,
                repo_path,
                Some(line_range),
                window,
                cx,
            );
        })
    })
}

/// The file whose history to browse: the file selected in the focused git panel, or else the
/// file of the active editor.
fn file_history_target(
    workspace: &Workspace,
    window: &Window,
    cx: &App,
) -> Option<(Entity<Repository>, RepoPath)> {
    if let Some(panel) = workspace.panel::<crate::git_panel::GitPanel>(cx)
        && panel.read(cx).focus_handle(cx).contains_focused(window, cx)
        && let Some(target) = panel.read(cx).selected_file_history_target()
    {
        return Some(target);
    }

    let editor = workspace.active_item_as::<Editor>(cx)?;
    let file = editor
        .read(cx)
        .file_at(editor.read(cx).selections.newest_anchor().head(), cx)?;
    let project_path = project::ProjectPath {
        worktree_id: file.worktree_id(cx),
        path: file.path().clone(),
    };
    workspace
        .project()
        .read(cx)
        .git_store()
        .read(cx)
        .repository_and_path_for_project_path(&project_path, cx)
}

pub struct FileHistoryPicker {
    picker: Entity<Picker<FileHistoryDelegate>>,
    _subscription: Subscription,
}

impl FileHistoryPicker {
    pub(crate) fn toggle(
        workspace: &mut Workspace,
        repository: Entity<Repository>,
        repo_path: RepoPath,
        line_range: Option<RangeInclusive<u32>>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let project = workspace.project().clone();
        let weak_workspace = workspace.weak_handle();
        workspace.toggle_modal(window, cx, |window, cx| {
            Self::new(
                project,
                weak_workspace,
                repository,
                repo_path,
                line_range,
                window,
                cx,
            )
        });
    }

    fn new(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        repository: Entity<Repository>,
        repo_path: RepoPath,
        line_range: Option<RangeInclusive<u32>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let preview = cx.new(|cx| CommitFilePreview::new(project, repository.clone(), window, cx));
        let delegate = FileHistoryDelegate {
            workspace,
            repository: repository.clone(),
            repo_path: repo_path.clone(),
            line_range: line_range.clone(),
            entries: None,
            matches: Vec::new(),
            selected_index: 0,
            timezone: UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC),
            preview: preview.clone(),
        };
        let picker = cx.new(|cx| {
            Picker::uniform_list_with_preview(
                delegate,
                Arc::new(CommitFilePreviewHandle(preview)),
                window,
                cx,
            )
        });

        let log = repository.update(cx, |repository, _| {
            repository.file_log(repo_path, line_range, COMMIT_LIMIT)
        });
        cx.spawn_in(window, {
            let picker = picker.downgrade();
            async move |_, cx| {
                let entries = log.await?;
                picker.update_in(cx, |picker, window, cx| {
                    match entries {
                        Ok(entries) => picker.delegate.entries = Some(entries),
                        Err(error) => {
                            picker.delegate.entries = Some(Vec::new());
                            picker.delegate.preview.update(cx, |preview, cx| {
                                preview.show_message(error.to_string().into(), cx)
                            });
                        }
                    }
                    picker.refresh(window, cx);
                })
            }
        })
        .detach_and_log_err(cx);

        let subscription = cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| {
            cx.emit(DismissEvent);
        });
        Self {
            picker,
            _subscription: subscription,
        }
    }
}

impl ModalView for FileHistoryPicker {}

impl EventEmitter<DismissEvent> for FileHistoryPicker {}

impl Focusable for FileHistoryPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for FileHistoryPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("FileHistoryPicker")
            .child(self.picker.clone())
    }
}

struct HistoryMatch {
    entry_index: usize,
    positions: Vec<usize>,
}

pub struct FileHistoryDelegate {
    workspace: WeakEntity<Workspace>,
    repository: Entity<Repository>,
    repo_path: RepoPath,
    /// The 1-based lines of the file at `HEAD` whose history is listed, or the whole file.
    line_range: Option<RangeInclusive<u32>>,
    /// `None` until the history has loaded.
    entries: Option<Vec<FileLogEntry>>,
    matches: Vec<HistoryMatch>,
    selected_index: usize,
    timezone: UtcOffset,
    preview: Entity<CommitFilePreview>,
}

impl FileHistoryDelegate {
    fn selected_entry(&self) -> Option<&FileLogEntry> {
        let history_match = self.matches.get(self.selected_index)?;
        self.entries.as_ref()?.get(history_match.entry_index)
    }

    fn update_preview(&self, window: &mut Window, cx: &mut App) {
        let entry = self.selected_entry().cloned();
        self.preview.update(cx, |preview, cx| match entry {
            Some(entry) => preview.show_commit(entry, window, cx),
            None => preview.show_message("No commits to preview".into(), cx),
        });
    }

    fn format_timestamp(&self, timestamp: i64) -> String {
        let timestamp =
            OffsetDateTime::from_unix_timestamp(timestamp).unwrap_or(OffsetDateTime::now_utc());
        time_format::format_localized_timestamp(
            timestamp,
            OffsetDateTime::now_utc(),
            self.timezone,
            time_format::TimestampFormat::Relative,
        )
    }

    /// The text matched by the query: the commit's short SHA, subject, and author.
    fn match_text(entry: &FileLogEntry) -> String {
        format!(
            "{} {} {}",
            short_sha(&entry.sha),
            entry.subject,
            entry.author_name
        )
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..git::SHORT_SHA_LENGTH).unwrap_or(sha)
}

impl PickerDelegate for FileHistoryDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "file history"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        let path = self.repo_path.as_unix_str();
        match &self.line_range {
            Some(lines) if lines.start() == lines.end() => {
                format!("Search the history of {path}:{}…", lines.start()).into()
            }
            Some(lines) => format!(
                "Search the history of {path}:{}-{}…",
                lines.start(),
                lines.end()
            )
            .into(),
            None => format!("Search the history of {path}…").into(),
        }
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some(match &self.entries {
            None => "Loading history…".into(),
            Some(entries) if entries.is_empty() => {
                if self.line_range.is_some() {
                    "No commits changed these lines".into()
                } else {
                    "No commits changed this file".into()
                }
            }
            Some(_) => "No matching commits".into(),
        })
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
        self.update_preview(window, cx);
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let Some(entries) = self.entries.as_ref() else {
            return Task::ready(());
        };
        if query.is_empty() {
            self.matches = (0..entries.len())
                .map(|entry_index| HistoryMatch {
                    entry_index,
                    positions: Vec::new(),
                })
                .collect();
            self.selected_index = 0;
            self.update_preview(window, cx);
            return Task::ready(());
        }

        let candidates: Vec<StringMatchCandidate> = entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| StringMatchCandidate::new(ix, &Self::match_text(entry)))
            .collect();
        cx.spawn_in(window, async move |picker, cx| {
            let string_matches = fuzzy::match_strings(
                &candidates,
                &query,
                false,
                true,
                COMMIT_LIMIT,
                &Default::default(),
                cx.background_executor().clone(),
            )
            .await;
            picker
                .update_in(cx, |picker, window, cx| {
                    let delegate = &mut picker.delegate;
                    // Keep the newest-first order of the history rather than sorting by score.
                    let mut matches: Vec<HistoryMatch> = string_matches
                        .into_iter()
                        .map(|string_match| HistoryMatch {
                            entry_index: string_match.candidate_id,
                            positions: string_match.positions,
                        })
                        .collect();
                    matches.sort_by_key(|history_match| history_match.entry_index);
                    delegate.matches = matches;
                    delegate.selected_index = 0;
                    delegate.update_preview(window, cx);
                })
                .log_err();
        })
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        CommitView::open(
            entry.sha.to_string(),
            self.repository.downgrade(),
            self.workspace.clone(),
            None,
            Some(entry.path.clone()),
            window,
            cx,
        );
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn try_get_preview_data_for_match(&self, _cx: &App) -> Option<PreviewUpdate> {
        // The preview is driven directly, see `update_preview`.
        None
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let history_match = self.matches.get(ix)?;
        let entry = self.entries.as_ref()?.get(history_match.entry_index)?;
        let match_text = Self::match_text(entry);
        let sha_len = short_sha(&entry.sha).len();
        let subject_range = sha_len + 1..sha_len + 1 + entry.subject.len();
        let positions_in = |range: std::ops::Range<usize>| -> Vec<usize> {
            history_match
                .positions
                .iter()
                .filter(|position| range.contains(*position))
                .map(|position| position - range.start)
                .collect()
        };
        let author_start = subject_range.end + 1;
        let renamed_from = (entry.path != self.repo_path).then(|| entry.path.as_unix_str());

        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .child(
                                    HighlightedLabel::new(
                                        match_text[..sha_len].to_string(),
                                        positions_in(0..sha_len),
                                    )
                                    .size(LabelSize::Small)
                                    .color(Color::Accent)
                                    .buffer_font(cx),
                                )
                                .child(
                                    div().flex_1().min_w_0().child(
                                        HighlightedLabel::new(
                                            entry.subject.clone(),
                                            positions_in(subject_range),
                                        )
                                        .truncate(),
                                    ),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    HighlightedLabel::new(
                                        entry.author_name.clone(),
                                        positions_in(author_start..match_text.len()),
                                    )
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                                )
                                .child(
                                    Label::new(format!(
                                        "· {}",
                                        self.format_timestamp(entry.commit_timestamp)
                                    ))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                                )
                                .when_some(renamed_from, |this, path| {
                                    this.child(
                                        Label::new(format!("· {path}"))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .truncate(),
                                    )
                                }),
                        ),
                ),
        )
    }
}

struct CommitFilePreviewHandle(Entity<CommitFilePreview>);

impl PreviewBackend for CommitFilePreviewHandle {
    fn update(&self, _update: PreviewUpdate, _window: &mut Window, _cx: &mut App) {}

    fn render(&self, layout: PreviewLayout, cx: &mut App) -> AnyElement {
        if layout == PreviewLayout::Hidden {
            return gpui::Empty.into_any_element();
        }
        self.0.read(cx).render(cx).into_any_element()
    }

    fn adjust_to_new_size(&self, _window: &mut Window, _cx: &mut App) {}

    fn clear(&self, cx: &mut App) {
        self.0.update(cx, |preview, cx| {
            preview.show_message("No commits to preview".into(), cx)
        });
    }
}

/// Shows the diff a commit made to one file, with every hunk expanded.
struct CommitFilePreview {
    project: Entity<Project>,
    repository: Entity<Repository>,
    editor: Entity<Editor>,
    multibuffer: Entity<MultiBuffer>,
    message: Option<SharedString>,
    shown_sha: Option<SharedString>,
    load_task: Task<()>,
}

impl CommitFilePreview {
    fn new(
        project: Entity<Project>,
        repository: Entity<Repository>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let multibuffer = cx.new(|_| MultiBuffer::new(Capability::ReadOnly));
        let editor = cx.new(|cx| {
            let mut editor =
                Editor::for_multibuffer(multibuffer.clone(), Some(project.clone()), window, cx);
            let show_line_numbers = EditorSettings::get_global(cx).gutter.line_numbers;
            editor.set_read_only(true);
            editor.set_expand_all_diff_hunks(cx);
            editor.set_diff_hunk_renderer(Some(Arc::new(HiddenDiffHunkRenderer)), cx);
            editor.disable_scrollbars_and_minimap(window, cx);
            editor.disable_inline_diagnostics();
            editor.disable_diagnostics(cx);
            editor.set_show_line_numbers(show_line_numbers, cx);
            editor.set_show_breakpoints(false, cx);
            editor.set_show_bookmarks(false, cx);
            editor.set_show_code_actions(false, cx);
            editor.set_show_runnables(false, cx);
            editor.set_show_wrap_guides(false, cx);
            editor.set_show_indent_guides(false, cx);
            editor
        });
        Self {
            project,
            repository,
            editor,
            multibuffer,
            message: Some("Loading history…".into()),
            shown_sha: None,
            load_task: Task::ready(()),
        }
    }

    fn show_message(&mut self, message: SharedString, cx: &mut Context<Self>) {
        self.shown_sha = None;
        self.load_task = Task::ready(());
        self.message = Some(message);
        cx.notify();
    }

    fn show_commit(&mut self, entry: FileLogEntry, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_sha.as_ref() == Some(&entry.sha) {
            return;
        }
        self.shown_sha = Some(entry.sha.clone());

        let commit_diff = self.repository.update(cx, |repository, cx| {
            repository.load_commit_diff(entry.sha.to_string(), false, cx)
        });
        let worktree_id = worktree_id_for_repo_path(
            self.repository.read(cx),
            self.project.read(cx),
            &entry.path,
            cx,
        );
        let language_registry = self.project.read(cx).languages().clone();

        self.load_task = cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let worktree_id = worktree_id.context("project has no worktrees")?;
                let commit_diff = commit_diff.await?;
                let Some(file) = commit_diff
                    .files
                    .into_iter()
                    .find(|file| file.path == entry.path)
                else {
                    return Ok(None);
                };
                if file.is_binary {
                    return Ok(None);
                }
                let is_deleted = file.new_text.is_none();
                let blob = Arc::new(GitBlob {
                    path: file.path.clone(),
                    worktree_id,
                    is_deleted,
                    is_binary: false,
                    display_name: format!(
                        "{} - {}",
                        short_sha(&entry.sha),
                        file.path.as_unix_str()
                    ),
                }) as Arc<dyn language::File>;
                let buffer = build_buffer(
                    file.new_text.unwrap_or_default(),
                    blob,
                    &language_registry,
                    cx,
                )
                .await?;
                let diff =
                    build_buffer_diff(file.old_text, &buffer, &language_registry, cx).await?;
                anyhow::Ok(Some((buffer, diff)))
            }
            .await;

            this.update_in(cx, |this, window, cx| {
                if this.shown_sha.as_ref() != Some(&entry.sha) {
                    return;
                }
                match result {
                    Ok(Some((buffer, diff))) => {
                        let hunk_ranges: Vec<_> = {
                            let snapshot = buffer.read(cx).snapshot();
                            let diff_snapshot = diff.read(cx).snapshot(cx);
                            let ranges: Vec<_> = diff_snapshot
                                .hunks(&snapshot)
                                .map(|hunk| hunk.buffer_range.to_point(&snapshot))
                                .collect();
                            if ranges.is_empty() {
                                vec![Point::zero()..snapshot.max_point()]
                            } else {
                                ranges
                            }
                        };
                        this.multibuffer.update(cx, |multibuffer, cx| {
                            multibuffer.clear(cx);
                            multibuffer.set_excerpts_for_path(
                                PathKey::for_buffer(&buffer, cx),
                                buffer,
                                hunk_ranges,
                                editor::multibuffer_context_lines(cx),
                                cx,
                            );
                            multibuffer.add_diff(diff, cx);
                        });
                        this.editor.update(cx, |editor, cx| {
                            editor.set_scroll_position(gpui::Point::default(), window, cx);
                        });
                        this.message = None;
                    }
                    Ok(None) => {
                        this.message = Some("No textual changes to this file to preview".into());
                    }
                    Err(error) => {
                        this.message = Some(format!("Failed to load the commit: {error}").into());
                    }
                }
                cx.notify();
            })
            .log_err();
        });
    }

    fn render(&self, cx: &App) -> impl IntoElement {
        match &self.message {
            Some(message) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Label::new(message.clone()).color(Color::Muted))
                .into_any_element(),
            None => div()
                .size_full()
                .overflow_hidden()
                .bg(cx.theme().colors().editor_background)
                .child(self.editor.clone())
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{TestAppContext, VisualTestContext};
    use project::{FakeFs, Project};
    use serde_json::json;
    use settings::SettingsStore;

    use super::*;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            language_model::init(cx);
            crate::init(cx);
        });
    }

    fn log_entry(sha: char, subject: &str, author_name: &str, path: &str) -> FileLogEntry {
        FileLogEntry {
            sha: sha.to_string().repeat(40).into(),
            author_name: author_name.to_string().into(),
            commit_timestamp: 1_700_000_000,
            subject: subject.to_string().into(),
            path: RepoPath::new(path).unwrap(),
        }
    }

    #[gpui::test]
    async fn test_browse_file_history(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            Path::new(util::path!("/project")),
            json!({
                ".git": {},
                "file.txt": "contents",
            }),
        )
        .await;
        fs.set_file_log(
            Path::new(util::path!("/project/.git")),
            RepoPath::new("file.txt").unwrap(),
            vec![
                log_entry('a', "Fix the parser", "Alice", "file.txt"),
                log_entry('b', "Rename the file", "Bob", "file.txt"),
                log_entry('c', "Add the parser", "Alice", "old.txt"),
            ],
        );
        let project = Project::test(fs.clone(), [Path::new(util::path!("/project"))], cx).await;
        cx.run_until_parked();

        let window = cx.add_window(|window, cx| {
            workspace::MultiWorkspace::test_new(project.clone(), window, cx)
        });
        let workspace = window
            .read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone())
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);

        let repository = project.read_with(cx, |project, cx| {
            project
                .active_repository(cx)
                .expect("should have a repository")
        });
        workspace.update_in(cx, |workspace, window, cx| {
            FileHistoryPicker::toggle(
                workspace,
                repository,
                RepoPath::new("file.txt").unwrap(),
                None,
                window,
                cx,
            );
        });
        cx.run_until_parked();

        let picker = workspace.update(cx, |workspace, cx| {
            workspace
                .active_modal::<FileHistoryPicker>(cx)
                .expect("the picker should be open")
                .read(cx)
                .picker
                .clone()
        });
        let subjects = |picker: &Picker<FileHistoryDelegate>| -> Vec<String> {
            let delegate = &picker.delegate;
            delegate
                .matches
                .iter()
                .map(|history_match| {
                    delegate.entries.as_ref().unwrap()[history_match.entry_index]
                        .subject
                        .to_string()
                })
                .collect()
        };
        picker.read_with(cx, |picker, _| {
            assert_eq!(
                subjects(picker),
                vec!["Fix the parser", "Rename the file", "Add the parser"]
            );
        });

        picker.update_in(cx, |picker, window, cx| {
            picker.set_query("parser", window, cx);
        });
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert_eq!(subjects(picker), vec!["Fix the parser", "Add the parser"]);
        });

        picker.update_in(cx, |picker, window, cx| {
            picker.set_query("bob", window, cx);
        });
        cx.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(subjects(picker), vec!["Rename the file"]);
            assert_eq!(
                picker.delegate.preview.read(cx).shown_sha.as_deref(),
                Some("b".repeat(40).as_str())
            );
        });
    }

    #[gpui::test]
    async fn test_show_history_for_selection_maps_lines_to_head(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            Path::new(util::path!("/project")),
            json!({
                ".git": {},
                "file.txt": "new 0\nnew 1\nzero\none\ntwo\n",
            }),
        )
        .await;
        fs.set_head_for_repo(
            Path::new(util::path!("/project/.git")),
            &[("file.txt", "zero\none\ntwo\n".to_string())],
            "deadbeef",
        );
        let project = Project::test(fs.clone(), [Path::new(util::path!("/project"))], cx).await;
        cx.run_until_parked();

        let window = cx.add_window(|window, cx| {
            workspace::MultiWorkspace::test_new(project.clone(), window, cx)
        });
        let workspace = window
            .read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone())
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);

        let editor = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    util::path!("/project/file.txt").into(),
                    workspace::OpenOptions::default(),
                    window,
                    cx,
                )
            })
            .await
            .unwrap()
            .downcast::<Editor>()
            .unwrap();
        cx.run_until_parked();

        // Select the lines "one" and "two", whole lines, ending at the start of the next line.
        editor.update_in(cx, |editor, window, cx| {
            editor.change_selections(Default::default(), window, cx, |selections| {
                selections.select_ranges([Point::new(3, 0)..Point::new(5, 0)])
            });
        });
        cx.dispatch_action(git::ShowHistoryForSelection);
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            let picker = workspace
                .active_modal::<FileHistoryPicker>(cx)
                .expect("the picker should be open");
            let delegate = &picker.read(cx).picker.read(cx).delegate;
            assert_eq!(delegate.repo_path.as_unix_str(), "file.txt");
            assert_eq!(delegate.line_range, Some(2..=3));
        });
    }
}
