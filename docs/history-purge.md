# Purging files from git history

> **Executed 2026-08-16.** The full history was rewritten with git
> filter-branch (index-filter) to remove `watch-history.json` (a ~4.5 MB
> Google Takeout export, originally committed in `25359a4`) and the
> personal server-admin scripts `scripts/upgrade-server.sh`,
> `scripts/relink-oidc-user.sh`, and `scripts/fix-group-ownership.sh`, and
> to scrub a tunnel-provider domain mention from the historical
> `SECURITY.md` blob.
> The rewritten branches (`v0.4.0`) and tags (`v0.2.0`, `v0.3.0`) were
> force-pushed; every commit hash after the earliest touched commit
> changed. Existing clones and forks retain the old history — the purge
> only cleans the canonical remote. The playbook below is kept for any
> future purge.

Removing a file in a new commit does **not** remove it from git history —
anyone with the repository can still retrieve it from any commit that
contained it.

## Warning before you run anything below

- **These commands rewrite git history.** Every commit after `25359a4` gets a
  new hash. This is not the same as a normal commit/revert.
- **A force-push is required** to publish the rewritten history
  (`git push --force-with-lease`), which will conflict with anyone who has
  the current history checked out or branched from it.
- **Existing forks, clones, and local checkouts keep their own copy** of the
  file and of the unrewritten history regardless of what you do to the
  canonical repository. This purge only removes the file going forward from
  the primary remote; it cannot reach into copies other people already have.
  If the data is sensitive, treat it as already exposed and rotate/revoke
  anything derived from it rather than relying on the history purge alone.
- Anyone with open pull requests or local branches based on the old history
  will need to rebase or re-clone after the force-push.
- Take a full backup/mirror clone before running any of this, in case
  something goes wrong.

Do not run these unless you have coordinated the force-push with anyone else
who has access to the repository.

## Option A: `git filter-repo` (recommended)

```sh
# 1. Install git-filter-repo if not already available
#    (pip install git-filter-repo, or via your package manager)

# 2. Make a fresh mirror clone to operate on (safer than rewriting your working copy directly)
git clone --mirror <repo-url> nightlio-purge.git
cd nightlio-purge.git

# 3. Strip the file from every commit in history
git filter-repo --path watch-history.json --invert-paths

# 4. Push the rewritten history back to the origin remote
git push --force-with-lease origin --all
git push --force-with-lease origin --tags
```

## Option B: BFG Repo-Cleaner (alternative)

```sh
# 1. Download BFG (https://rtyley.github.io/bfg-repo-cleaner/)

# 2. Make a fresh mirror clone to operate on
git clone --mirror <repo-url> nightlio-purge.git

# 3. Strip the file from every commit in history
java -jar bfg.jar --delete-files watch-history.json nightlio-purge.git

# 4. Clean up refs and repack
cd nightlio-purge.git
git reflog expire --expire=now --all
git gc --prune=now --aggressive

# 5. Push the rewritten history back to the origin remote
git push --force-with-lease origin --all
git push --force-with-lease origin --tags
```

## After the push

1. Notify anyone with a fork or local clone that history was rewritten; they
   should re-clone or hard-reset their branches onto the new history rather
   than merging/pulling.
2. Confirm the file is gone from history on the canonical remote:
   `git log --all --oneline -- watch-history.json` should return nothing.
3. If the export contained credentials or tokens (Google Takeout data
   generally does not, but verify), rotate those separately — a history
   purge does not undo prior exposure.
