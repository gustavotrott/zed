> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.
# Zed (fork by gustavotrott)

This is a fork of [Zed](https://github.com/zed-industries/zed) that adds a few features I miss, mostly coming from JetBrains IDEs. I'll keep maintaining it, merging the latest upstream Zed every week, until these features are implemented in the official project.

Every feature lives in its own branch, based on upstream `main`, so it can be reviewed, tried, or copied into the official project on its own. The `main` branch of this fork is upstream `main` with all the feature branches merged.

## Download

Builds are published on the [Releases](https://github.com/gustavotrott/zed/releases) page. They are built from this fork's `main` using the `dev` release channel, so they install as **Zed Dev** next to an official Zed install and don't auto-update to the official release.

The macOS builds are not notarized. After copying `Zed Dev.app` to `/Applications`, allow it to open with:

```sh
xattr -dr com.apple.quarantine "/Applications/Zed Dev.app"
```

## Features not in the official Zed

| Feature | Branch | Commits | Upstream discussion |
| --- | --- | --- | --- |
| **Editable picker preview**: with `"editable_picker_preview": true`, the preview of the File Finder and Text Finder can be edited in place. `picker::ToggleFocusPreview` (cmd-alt-o / ctrl-alt-o) moves focus between the query and the preview; edits are saved when moving to another result or closing the picker. | [`feature/editable-picker-preview`](https://github.com/gustavotrott/zed/tree/feature/editable-picker-preview) | [`0e21a96`](https://github.com/gustavotrott/zed/commit/0e21a96d24) | [#61867](https://github.com/zed-industries/zed/discussions/61867) |
| **Text Finder instead of Project Search**: with `"search": { "use_text_finder_for_project_search": true }`, "Find in Project" (`pane::DeploySearch`) and the project panel's "Find in Folder…" open the Text Finder modal instead of a Project Search tab. The folder is shown as a removable filter in the modal. | [`feature/text-finder-as-project-search`](https://github.com/gustavotrott/zed/tree/feature/text-finder-as-project-search) | [`df6ce67`](https://github.com/gustavotrott/zed/commit/df6ce67883) | |
| **Replace in the Text Finder**: `search::ToggleReplace` (cmd-shift-h / ctrl-shift-h) shows a replacement input in the Text Finder, like JetBrains' "Replace in Files". Enter replaces the selected match, `search::ReplaceAll` (cmd-enter / ctrl-alt-enter) replaces every match, or only the multi-selected ones. Built on top of the previous feature. | [`feature/text-finder-replace`](https://github.com/gustavotrott/zed/tree/feature/text-finder-replace) | [`70d2723`](https://github.com/gustavotrott/zed/commit/70d2723eba) | |
| **Browse file history in a picker**: `git::BrowseFileHistory` ("Browse File History" in the editor and git panel context menus) lists the commits that changed the file, with an instant side-by-side (or unified, per `diff_view_style`) preview of the diff each commit made to that file. Click selects a commit, double-click or Enter opens the file, cmd-enter / ctrl-enter opens the commit. | [`feature/git-file-history-picker`](https://github.com/gustavotrott/zed/tree/feature/git-file-history-picker) | [`ab7f0d2`](https://github.com/gustavotrott/zed/commit/ab7f0d2a55) | [#61356](https://github.com/zed-industries/zed/discussions/61356) |
| **Show History for Selection**: `git::ShowHistoryForSelection` ("Show History for Selection" in the editor context menu) lists only the commits that changed the selected lines (`git log -L`), in the same picker. Built on top of the previous feature. | [`feature/git-selection-history`](https://github.com/gustavotrott/zed/tree/feature/git-selection-history) | [`6c41f3e`](https://github.com/gustavotrott/zed/commit/6c41f3e6a9) | [#54672](https://github.com/zed-industries/zed/discussions/54672) |

Branches that build on another feature contain that feature's commit too; to see only one feature's changes, look at its commit, or compare the branch with the one it builds on (for example [`feature/text-finder-as-project-search...feature/text-finder-replace`](https://github.com/gustavotrott/zed/compare/feature/text-finder-as-project-search...feature/text-finder-replace)). Commit hashes change when the branches are rebased onto a newer upstream, so the branches are the reference.

## Keeping the fork up to date

Each week the feature branches are rebased onto the latest upstream `main`, and this fork's `main` is rebuilt from upstream `main` plus all the feature branches:

```sh
git fetch zed main
# --update-refs also moves the branches a stacked feature builds on.
git rebase zed/main feature/editable-picker-preview
git rebase --update-refs zed/main feature/text-finder-replace
git rebase --update-refs zed/main feature/git-selection-history

git switch main
git reset --hard zed/main
for branch in feature/editable-picker-preview feature/text-finder-replace feature/git-selection-history; do
  git merge --no-ff --no-edit "$branch"
done
git cherry-pick <the commit updating this README>
git push --force-with-lease fork main feature/editable-picker-preview \
  feature/text-finder-as-project-search feature/text-finder-replace \
  feature/git-file-history-picker feature/git-selection-history
```

(`zed` is the `https://github.com/zed-industries/zed.git` remote and `fork` is this repository.)

---

The rest of this file is the original Zed README.

# Zed

[![Zed](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/zed-industries/zed/main/assets/badge/v0.json)](https://zed.dev)
[![CI](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml/badge.svg)](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml)

Welcome to Zed, a high-performance, multiplayer code editor from the creators of [Atom](https://github.com/atom/atom) and [Tree-sitter](https://github.com/tree-sitter/tree-sitter).

---

### Installation

On macOS, Linux, and Windows you can [download Zed directly](https://zed.dev/download) or install Zed via your local package manager ([macOS](https://zed.dev/docs/installation#macos)/[Linux](https://zed.dev/docs/linux#installing-via-a-package-manager)/[Windows](https://zed.dev/docs/windows#package-managers)).

Other platforms are not yet available:

- Web ([tracking discussion](https://github.com/zed-industries/zed/discussions/26195))

### Developing Zed

- [Building Zed for macOS](./docs/src/development/macos.md)
- [Building Zed for Linux](./docs/src/development/linux.md)
- [Building Zed for Windows](./docs/src/development/windows.md)

### Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for ways you can contribute to Zed.

Also... we're hiring! Check out our [jobs](https://zed.dev/jobs) page for open roles.

### Licensing

Zed source code is licensed primarily under GPL-3.0-or-later, with Apache-2.0 components where marked.

License information for third party dependencies must be correctly provided for CI to pass.

We use [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) to automatically comply with open source licenses. If CI is failing, check the following:

- Is it showing a `no license specified` error for a crate you've created? If so, add `publish = false` under `[package]` in your crate's Cargo.toml.
- Is the error `failed to satisfy license requirements` for a dependency? If so, first determine what license the project has and whether this system is sufficient to comply with this license's requirements. If you're unsure, ask a lawyer. Once you've verified that this system is acceptable add the license's SPDX identifier to the `accepted` array in `script/licenses/zed-licenses.toml`.
- Is `cargo-about` unable to find the license for a dependency? If so, add a clarification field at the end of `script/licenses/zed-licenses.toml`, as specified in the [cargo-about book](https://embarkstudios.github.io/cargo-about/cli/generate/config.html#crate-configuration).

## Sponsorship

Zed is developed by **Zed Industries, Inc.**, a for-profit company.

If you’d like to financially support the project, you can do so via GitHub Sponsors.
Sponsorships go directly to Zed Industries and are used as general company revenue.
There are no perks or entitlements associated with sponsorship.
