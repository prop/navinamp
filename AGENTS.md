# Agent instructions

## Workflow

- **Open a pull request for every new feature or fix.** Work on a topic branch
  and merge through a PR (`gh pr create --base master`) only after CI passes.
  Never push feature or fix commits directly to `master`.
- **Do not merge a pull request without the user's explicit approval.** Open
  the PR, wait for CI, and report back; merging requires the user's go-ahead.
- **Delete the branch after merging its PR.** Merge with
  `gh pr merge <n> --merge --delete-branch` (or delete the remote branch
  afterwards); never leave a merged topic branch behind.
