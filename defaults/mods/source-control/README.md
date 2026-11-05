# Source Control

A native GPUI panel for Git changes, commit history, and repository operations.
Open **Source Control** from the tab bar. It follows the focused terminal's
folder by default; the repository menu can select another local folder,
worktree, or connected machine.

## Changes and commits

- **Changes**, **Staged Changes**, and **Merge Changes** show working tree,
  index, and conflicted files separately. Use `+` to stage and `−` to unstage.
- Click a file to open its diff. Select changed lines or hunk headers, then
  choose **Stage Selected** or **Unstage Selected**. **Discard Selected** asks
  before removing selected working tree changes.
- **Commit** commits the index. If nothing is staged, it asks whether to stage
  all changes first. The adjacent menu offers Commit All, Amend, Sign Off,
  Signed, Empty, Commit & Push, Commit & Sync, and Undo Last Commit. Normal
  commits preserve the user's Git signing configuration.
- When there are no file changes, the primary action becomes **Publish
  Branch**, **Push**, or **Sync Changes**, according to the current branch.
- Stage All and Unstage All leave unresolved conflicts alone. Resolve and
  explicitly stage each conflicted file before committing.

## History and repository tools

The graph shows up to 200 commits initially; **Load more** extends it. Click a
commit to inspect its metadata and changed files. Its context menu offers
branch and tag creation, detached checkout, cherry-pick, revert, and reset.

The branch picker and **More Actions** menu provide fetch, pull, push, sync,
branch management, merge, rebase, stash, tags, worktrees, repository selection,
and clone. Destructive actions have confirmation prompts. Force Push is a
separate action and uses a lease for the exact remote commit last observed.

The panel refreshes every 10 seconds. Changes made in the diff and merge views
also trigger a refresh. Actions check that the selected repository, branch,
and operation still match before changing Git state.

## Merge conflicts

Click a file in **Merge Changes** to compare Current and Incoming, optionally
show Base, and edit Result. The sides follow Git's `ours` and `theirs` index
stages. **Accept Current** and **Accept Incoming** update the result editor;
**Save Result** writes the file, and **Save & Stage** marks it resolved.
Saving rejects stale file or conflict snapshots. Save & Stage also checks for
remaining conflict markers.

## Limits

- This mod does not run the VS Code extension host or third-party SCM
  extensions.
- Line operations support ordinary text modifications. New files, deletions,
  renames, mode changes, binary files, and combined diffs use file operations.
- Remote machines support Git operations on whole files. Partial patch
  transfer and saving merge results on remote machines are not supported.
- The merge editor supports ordinary text files with both sides present.
  Resolve deleted-file, binary, symlink, submodule, and large-file conflicts
  at the file level or in an external editor, then stage them.
- This panel uses the default 30-second `exec` timeout and the 8 MiB output limit.
  Use a terminal for longer clones, hooks, or other commands. Authentication
  and signing use the existing Git environment; this panel does not provide
  an interactive credential or signing prompt.

## References

The implementation uses Herdr's mod APIs and the Git CLI. The following
official VS Code sources informed the interaction model and Git behavior,
at revision `e934a7a384ac95319b46ceb4d9fa78b191625d9d`:

- [Source Control action button](https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/actionButton.ts)
- [Git commands](https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/git.ts)
- [Repository operations](https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/repository.ts)
- [Line staging behavior](https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/staging.ts)
