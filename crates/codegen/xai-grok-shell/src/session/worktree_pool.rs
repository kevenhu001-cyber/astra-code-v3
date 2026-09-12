//! Startup cleanup of stale pooled worktrees.
//!
//! A pre-warmed worktree pool (background fill, acquire/claim/release,
//! orphan adoption) once lived here but was never wired into production and
//! has been deleted. This cleanup path remains so `~/.grok/worktree_pool/`
//! directories left behind by dead agent instances are still reclaimed.
//!
//! On startup the pool spawns a background fill task that pre-creates linked
//! worktrees up to `pool_size`. When `acquire()` takes a worktree and the
//! pool drops below capacity, it kicks the fill task to create replacements.
//! `release()` returns used worktrees (cleaned in background) so they can
//! be reused without a full O(N) creation.
//!
//! ## Design
//!
//! - **Bounded (soft)**: The pool targets at most `pool_size` worktrees.
//!   Concurrent operations may briefly exceed the bound by one.
//! - **Background fill**: A single long-lived task creates worktrees via
//!   `WorktreeBuilder` (`GitCheckout` mode). It sleeps when the pool is full
//!   and is woken via `tokio::sync::Notify` when capacity frees up.
//! - **Filesystem-based ready detection**: Both the fill task and `release()`
//!   write a `<pool_id>.ready` sibling marker file next to the worktree
//!   directory. `acquire()` atomically renames `.ready` -> `.claimed` to
//!   prevent races. Markers live outside the worktree to avoid dirtying
//!   `git status`.
//! - **Always linked**: Pool worktrees use the shared object store. New
//!   commits are immediately visible via `git reset --hard`.
//! - **macOS only**: Linux has O(1) BTRFS snapshots; the pool adds value
//!   only on macOS/APFS where worktree creation is O(file_count).
//! - **Multi-instance safe**: Each pool instance gets a unique subdirectory
//!   under `~/.astra/worktree_pool/<instance_id>/` with a `.pid` liveness
//!   file. Startup cleanup only removes directories for dead processes.

use std::path::{Path, PathBuf};

use xai_tty_utils::git_command;

use crate::util::grok_home::grok_home;

const WORKTREE_POOL_LOG: &str = "xai_worktree_pool";

// Types

/// A worktree that has been atomically claimed (`.ready` -> `.claimed`)
/// but not yet synced to the source's current state.
///
/// Returned by [`WorktreePool::try_claim`] for callers that need to
/// claim multiple worktrees up-front (e.g. concurrent fork comparisons)
/// and then sync them concurrently.
pub struct ClaimedWorktree {
    /// Absolute path to the claimed pool worktree.
    pub path: PathBuf,
}

/// Result of acquiring a worktree from the pool.
pub struct AcquiredWorktree {
    /// Absolute path to the worktree (stays in pool directory, synced in place).
    pub path: PathBuf,
    /// Whether this came from the pool (always true when returned; for metrics).
    pub from_pool: bool,
    /// Time to acquire in milliseconds.
    pub acquire_latency_ms: u64,
}

// WorktreePool

/// A bounded pool of linked worktrees for fast concurrent fork setup.
///
/// Pre-creates worktrees in the background up to `pool_size`. If no worktree
/// is ready but creation is in progress, `acquire()` waits for it to finish
/// rather than falling back to on-demand creation. Returned worktrees via
/// `release()` are recycled.
///
/// Each pool instance gets a unique subdirectory under
/// `~/.astra/worktree_pool/<instance_id>/` with a `.pid` file recording
/// the owning process ID.
pub struct WorktreePool {
    /// Unique identifier for this agent instance (UUIDv7).
    instance_id: String,
    /// Cached file count from initial detection.
    pub cached_file_count: usize,
    /// Notify handle to wake the fill task when capacity frees up.
    fill_notify: Arc<Notify>,
    /// Notify handle fired by the fill task (and release) when a worktree
    /// becomes ready. `acquire()` waits on this when the pool has in-progress
    /// creations but no ready worktrees yet.
    ready_notify: Arc<Notify>,
    /// Cancellation token for the fill task.
    cancel: CancellationToken,
    /// Handle to the background fill task (kept alive for the pool's lifetime).
    _fill_handle: tokio::task::JoinHandle<()>,
}

impl WorktreePool {
    /// Create a new pool and start the background fill task.
    ///
    /// The fill task immediately begins pre-creating linked worktrees up to
    /// `pool_size`. Each pool instance gets a unique subdirectory under
    /// `~/.astra/worktree_pool/<instance_id>/`. A `.pid` file is written so
    /// that startup cleanup can determine whether this instance is still alive.
    pub fn new(source_path: PathBuf, config: PoolConfig, cached_file_count: usize) -> Self {
        let instance_id = uuid::Uuid::now_v7().to_string();

        let instance_dir = pool_base_directory().join(&instance_id);
        std::fs::create_dir_all(&instance_dir).ok();
        std::fs::write(instance_dir.join(".pid"), std::process::id().to_string()).ok();

        let fill_notify = Arc::new(Notify::new());
        let ready_notify = Arc::new(Notify::new());
        let cancel = CancellationToken::new();

        let fill_handle = {
            let source = source_path.clone();
            let cfg = config.clone();
            let cancel = cancel.clone();
            let fill_notify = fill_notify.clone();
            let ready_notify = ready_notify.clone();
            let inst_id = instance_id.clone();

            tokio::task::spawn(async move {
                Self::fill_loop(source, cfg, cancel, fill_notify, ready_notify, inst_id).await;
            })
        };

        // Enable git perf features on the source repo. Pool worktrees inherit
        // these via the shared .git/config (linked worktrees share it).
        // Source cache warmup (`warm_source_repo`) is deferred to the fill loop
        // to avoid git lock contention with concurrent `git worktree add`.
        {
            let source = source_path.clone();
            tokio::task::spawn(async move {
                configure_git_perf_features(&source).await;
            });
        }

        tracing::info!(
            source = %source_path.display(),
            pool_size = config.pool_size,
            parallelism = config.parallelism,
            file_count = cached_file_count,
            instance_id = %instance_id,
            "Worktree pool started"
        );
        tracing::info!(
            target: WORKTREE_POOL_LOG,
            source = %source_path.display(),
            pool_size = config.pool_size,
            parallelism = config.parallelism,
            file_count = cached_file_count,
            instance_id = %instance_id,
            instance_dir = %instance_dir.display(),
            "POOL_INIT: worktree pool started"
        );

        Self {
            instance_id,
            cached_file_count,
            fill_notify,
            ready_notify,
            cancel,
            _fill_handle: fill_handle,
        }
    }

    /// The instance-scoped pool directory for this pool.
    fn instance_dir(&self) -> PathBuf {
        pool_base_directory().join(&self.instance_id)
    }

    /// Adopt orphaned worktrees from dead pool instances into this instance.
    ///
    /// Called at the start of the fill loop (before creating new worktrees).
    /// For each candidate (up to `hard_cap`):
    /// 1. Validate source-repo match (`.git` link points to our repo)
    /// 2. Atomic adoption: rename → fix backlink → fix .git link (sync, no yield)
    /// 3. Reset + clean via `spawn_blocking`
    /// 4. Warm git caches via `spawn_blocking`
    /// 5. Write `.ready` marker
    ///
    /// Excess/invalid candidates are destroyed via `spawn_blocking`.
    /// Returns the number of successfully adopted worktrees.
    async fn adopt_orphan_worktrees(
        instance_dir: &Path,
        source_path: &Path,
        hard_cap: usize,
        ready_notify: &Notify,
    ) -> usize {
        let candidates = take_adoptable_worktrees();
        Self::adopt_orphan_worktrees_impl(
            instance_dir,
            source_path,
            hard_cap,
            ready_notify,
            candidates,
        )
        .await
    }

    /// Core implementation that takes candidates as a parameter.
    /// This allows tests to inject candidates directly without using global state.
    async fn adopt_orphan_worktrees_impl(
        instance_dir: &Path,
        source_path: &Path,
        hard_cap: usize,
        ready_notify: &Notify,
        candidates: Vec<AdoptableWorktree>,
    ) -> usize {
        if candidates.is_empty() {
            return 0;
        }

        tracing::info!(
            target: WORKTREE_POOL_LOG,
            candidate_count = candidates.len(),
            hard_cap,
            "POOL_ADOPT_START: attempting to adopt orphan worktrees"
        );

        // Derive the expected .git/worktrees/ prefix for source-repo validation.
        let git_worktrees_dir = source_path.join(".git/worktrees");

        let mut adopted = 0usize;
        let mut old_instance_dirs: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();

        for candidate in candidates {
            if adopted >= hard_cap {
                // Already have enough — destroy the rest in background.
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %candidate.old_path.display(),
                    pool_id = %candidate.pool_id,
                    "POOL_ADOPT_EXCESS: hard cap reached, destroying excess orphan"
                );
                let old = candidate.old_path.clone();
                tokio::task::spawn_blocking(move || destroy_worktree_sync(&old));
                old_instance_dirs.insert(
                    candidate
                        .old_path
                        .parent()
                        .unwrap_or(&candidate.old_path)
                        .to_path_buf(),
                );
                continue;
            }

            // Source-repo validation: check that the .git link points to our repo.
            let git_file = candidate.old_path.join(".git");
            let repo_matches = std::fs::read_to_string(&git_file)
                .ok()
                .and_then(|c| c.trim().strip_prefix("gitdir: ").map(PathBuf::from))
                .is_some_and(|gitdir| gitdir.starts_with(&git_worktrees_dir));

            if !repo_matches {
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %candidate.old_path.display(),
                    pool_id = %candidate.pool_id,
                    "POOL_ADOPT_REPO_MISMATCH: orphan belongs to different repo, destroying"
                );
                let old = candidate.old_path.clone();
                tokio::task::spawn_blocking(move || destroy_worktree_sync(&old));
                old_instance_dirs.insert(
                    candidate
                        .old_path
                        .parent()
                        .unwrap_or(&candidate.old_path)
                        .to_path_buf(),
                );
                continue;
            }

            let new_path = instance_dir.join(&candidate.pool_id);
            let adopt_start = std::time::Instant::now();

            // ── Atomic adoption block (sync, no yield between these three) ──
            // Prevents `git worktree prune` from seeing a stale backlink.

            // a. Move worktree dir to new instance (atomic on same FS)
            if let Err(e) = std::fs::rename(&candidate.old_path, &new_path) {
                tracing::warn!(
                    target: WORKTREE_POOL_LOG,
                    old_path = %candidate.old_path.display(),
                    new_path = %new_path.display(),
                    error = %e,
                    "POOL_ADOPT_RENAME_FAILED: rename failed (concurrent claim?), skipping"
                );
                continue;
            }

            // b. Fix backlink: .git/worktrees/<pool_id>/gitdir → new path
            //    Save old content first so we can restore on failure (write()
            //    may truncate before failing, leaving a corrupted backlink).
            let gitdir_backlink = git_worktrees_dir.join(&candidate.pool_id).join("gitdir");
            let old_backlink = std::fs::read_to_string(&gitdir_backlink).ok();
            let new_git_path = format!("{}\n", new_path.join(".git").display());
            if let Err(e) = std::fs::write(&gitdir_backlink, &new_git_path) {
                tracing::warn!(
                    target: WORKTREE_POOL_LOG,
                    path = %gitdir_backlink.display(),
                    error = %e,
                    "POOL_ADOPT_BACKLINK_FAILED: failed to fix backlink, reverting"
                );
                let _ = std::fs::rename(&new_path, &candidate.old_path);
                if let Some(old) = old_backlink {
                    let _ = std::fs::write(&gitdir_backlink, old);
                }
                continue;
            }

            // c. Fix .git link inside worktree (technically a no-op for same
            //    source repo, but written unconditionally for robustness)
            let gitdir_link = format!(
                "gitdir: {}\n",
                git_worktrees_dir.join(&candidate.pool_id).display()
            );
            let _ = std::fs::write(new_path.join(".git"), &gitdir_link);

            // ── End atomic block — backlink is now valid ──

            // d. Reset to current HEAD + clean dirty files (on blocking pool)
            let reset_path = new_path.clone();
            let reset_clean_ok = tokio::task::spawn_blocking(move || {
                let reset_ok = git_command()
                    .args(["reset", "--hard", "HEAD"])
                    .current_dir(&reset_path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());

                let clean_ok = git_command()
                    .args(["clean", "-fdx"])
                    .current_dir(&reset_path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());

                (reset_ok, clean_ok)
            })
            .await;

            match reset_clean_ok {
                Ok((true, true)) => {}
                Ok((reset_ok, clean_ok)) => {
                    tracing::warn!(
                        target: WORKTREE_POOL_LOG,
                        path = %new_path.display(),
                        reset_ok,
                        clean_ok,
                        "POOL_ADOPT_RESET_FAILED: git reset/clean failed, destroying"
                    );
                    let p = new_path.clone();
                    tokio::task::spawn_blocking(move || destroy_worktree_sync(&p));
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        target: WORKTREE_POOL_LOG,
                        path = %new_path.display(),
                        error = %e,
                        "POOL_ADOPT_RESET_FAILED: spawn_blocking panicked, destroying"
                    );
                    let p = new_path.clone();
                    tokio::task::spawn_blocking(move || destroy_worktree_sync(&p));
                    continue;
                }
            }

            // e. Warm git caches (on blocking pool)
            warm_git_caches(&new_path).await;

            // f. Write .ready marker
            let claiming = marker_path(&new_path, CLAIMING_SUFFIX);
            let ready = marker_path(&new_path, READY_SUFFIX);
            if tokio::fs::write(&claiming, "").await.is_ok()
                && tokio::fs::rename(&claiming, &ready).await.is_ok()
            {
                let adopt_ms = adopt_start.elapsed().as_millis() as u64;
                adopted += 1;
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %new_path.display(),
                    pool_id = %candidate.pool_id,
                    old_instance = %candidate.old_instance_id,
                    adopt_ms,
                    adopted,
                    "POOL_ADOPT_OK: orphan worktree adopted successfully"
                );
                // Wake acquire() — an adopted worktree is now ready.
                ready_notify.notify_waiters();
            } else {
                tracing::warn!(
                    target: WORKTREE_POOL_LOG,
                    path = %new_path.display(),
                    "POOL_ADOPT_MARKER_FAILED: failed to write .ready marker"
                );
                let p = new_path;
                tokio::task::spawn_blocking(move || destroy_worktree_sync(&p));
            }

            old_instance_dirs.insert(
                candidate
                    .old_path
                    .parent()
                    .unwrap_or(&candidate.old_path)
                    .to_path_buf(),
            );
        }

        // Clean up old instance directories (now empty or have only marker files).
        for old_dir in old_instance_dirs {
            tokio::task::spawn_blocking(move || {
                let _ = std::fs::remove_dir_all(&old_dir);
            });
        }

        tracing::info!(
            target: WORKTREE_POOL_LOG,
            adopted,
            hard_cap,
            "POOL_ADOPT_DONE: adoption complete"
        );

        adopted
    }

    // Background fill loop

    /// Background task that creates linked worktrees until the pool is full.
    /// Sleeps when full, woken by `fill_notify` when `acquire()` frees
    /// capacity. Fires `ready_notify` after each worktree becomes ready so
    /// that `acquire()` can stop waiting.
    async fn fill_loop(
        source: PathBuf,
        config: PoolConfig,
        cancel: CancellationToken,
        fill_notify: Arc<Notify>,
        ready_notify: Arc<Notify>,
        instance_id: String,
    ) {
        // Hard cap: never create more than pool_size * 2 worktrees over the
        // entire lifetime of this pool instance. This is a defence-in-depth
        // against runaway creation if `count_instance_worktrees` returns a
        // stale/wrong value (e.g. a concurrent cleanup process deleting
        // directories from our instance dir while we're still alive)
        let hard_cap = config.pool_size.saturating_mul(2).max(6);

        // Adopt orphan worktrees from dead instances before creating new ones.
        // Adopted worktrees get .ready markers and are immediately claimable,
        // so the capacity check below sees them and only creates the deficit.
        let instance_dir = pool_base_directory().join(&instance_id);
        Self::adopt_orphan_worktrees(&instance_dir, &source, hard_cap, &ready_notify).await;

        // Warm the source repo's git caches BEFORE creating worktrees.
        // This runs a full `git status` (without --no-optional-locks) so that
        // fsmonitor, untracked-cache, and split-index data are written to the
        // index. Placed here (inside the fill loop, after adoption, before
        // creation) to avoid git lock contention with `git worktree add`.
        // By the time `configure_git_perf_features` finishes (spawned in
        // `new()`), the perf config is in place for this status to populate.
        warm_source_repo(&source).await;

        let mut created_total: usize = 0;

        loop {
            if created_total >= hard_cap {
                tracing::warn!(
                    target: WORKTREE_POOL_LOG,
                    created_total,
                    hard_cap,
                    pool_size = config.pool_size,
                    "FILL_HARD_CAP: reached lifetime creation limit, fill loop stopping"
                );
                // Don't exit — just sleep forever waiting for cancellation.
                // The pool can still function with whatever worktrees exist.
                cancel.cancelled().await;
                return;
            }

            let current = count_instance_worktrees(&instance_id);
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                current_count = current,
                pool_size = config.pool_size,
                created_total,
                hard_cap,
                "FILL_CHECK: checking pool capacity"
            );
            if current >= config.pool_size {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = fill_notify.notified() => continue,
                }
            }

            if cancel.is_cancelled() {
                return;
            }

            let remaining = config.pool_size.saturating_sub(current);
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                current_count = current,
                pool_size = config.pool_size,
                remaining,
                created_total,
                "FILL_START: creating pooled worktree"
            );
            let fill_start = std::time::Instant::now();
            match create_pooled_worktree(&source, &instance_id, config.parallelism).await {
                Ok(pool_path) => {
                    created_total += 1;
                    let create_ms = fill_start.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: WORKTREE_POOL_LOG,
                        path = %pool_path.display(),
                        create_ms,
                        created_total,
                        "FILL_CREATED: worktree created, warming caches"
                    );

                    // Warm caches before marking ready so the worktree is hot
                    // when acquire() claims it. This is safe because the
                    // worktree has no `.ready` marker yet, so acquire() can't
                    // claim it while warmup is running.
                    let warm_start = std::time::Instant::now();
                    warm_git_caches(&pool_path).await;
                    let warm_ms = warm_start.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: WORKTREE_POOL_LOG,
                        path = %pool_path.display(),
                        warm_ms,
                        "FILL_WARMED: git caches warmed"
                    );

                    // Write .ready marker atomically (same pattern as release).
                    // Markers are sibling files next to the worktree dir so
                    // they don't dirty the git working tree.
                    let claiming = marker_path(&pool_path, CLAIMING_SUFFIX);
                    let ready = marker_path(&pool_path, READY_SUFFIX);
                    if let Err(e) = tokio::fs::write(&claiming, "").await {
                        tracing::warn!(
                            path = %pool_path.display(),
                            "Fill: failed to write .claiming marker: {e}"
                        );
                        tracing::warn!(
                            target: WORKTREE_POOL_LOG,
                            path = %pool_path.display(),
                            marker = %claiming.display(),
                            error = %e,
                            "FILL_ERROR: failed to write .claiming marker"
                        );
                        remove_worktree_registration(&pool_path).await;
                        continue;
                    }
                    if let Err(e) = tokio::fs::rename(&claiming, &ready).await {
                        tracing::warn!(
                            path = %pool_path.display(),
                            "Fill: failed to rename .claiming -> .ready: {e}"
                        );
                        tracing::warn!(
                            target: WORKTREE_POOL_LOG,
                            path = %pool_path.display(),
                            error = %e,
                            "FILL_ERROR: failed to rename .claiming -> .ready"
                        );
                        remove_worktree_registration(&pool_path).await;
                        continue;
                    }
                    let total_ms = fill_start.elapsed().as_millis() as u64;
                    tracing::debug!(
                        path = %pool_path.display(),
                        remaining = remaining.saturating_sub(1),
                        "Fill: created pooled worktree"
                    );
                    tracing::info!(
                        target: WORKTREE_POOL_LOG,
                        path = %pool_path.display(),
                        remaining = remaining.saturating_sub(1),
                        total_ms,
                        create_ms,
                        warm_ms,
                        created_total,
                        "FILL_READY: pooled worktree marked ready"
                    );
                    // Wake any acquire() waiting for a ready worktree
                    ready_notify.notify_waiters();
                }
                Err(e) => {
                    tracing::warn!("Fill: worktree creation failed: {e}");
                    tracing::warn!(
                        target: WORKTREE_POOL_LOG,
                        error = %e,
                        elapsed_ms = fill_start.elapsed().as_millis() as u64,
                        created_total,
                        "FILL_ERROR: worktree creation failed"
                    );
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }
    }

    /// Try to atomically claim a ready worktree from the instance directory.
    ///
    /// Scans for `<pool_id>.ready` sibling marker files, then attempts to
    /// claim one by atomically renaming `.ready` -> `.claimed`. The rename
    /// is atomic on both APFS and ext4, so concurrent `acquire()` calls
    /// cannot claim the same worktree.
    ///
    /// Returns `Some(path)` on success, `None` if no ready worktree could
    /// be claimed.
    // PERF: Uses sync `std::fs` (read_dir, metadata, rename) on an async
    // path. At pool_size 2-3 this scans a handful of entries and completes
    // in microseconds. Would need `spawn_blocking` if pool sizes grow.
    fn try_claim_ready_worktree(&self) -> Option<PathBuf> {
        let instance_dir = self.instance_dir();
        let entries = std::fs::read_dir(&instance_dir).ok()?;

        for entry in entries.flatten() {
            let entry_path = entry.path();

            // Look for <pool_id>.ready marker files (not directories).
            let name = entry_path.file_name()?.to_string_lossy().to_string();
            if !name.ends_with(READY_SUFFIX) {
                continue;
            }

            // Derive the worktree directory path by stripping the suffix.
            let base = &name[..name.len() - READY_SUFFIX.len()];
            let worktree_dir = instance_dir.join(base);

            let ready_marker = entry_path;
            let claimed_marker = marker_path(&worktree_dir, CLAIMED_SUFFIX);

            // Atomic claim: rename .ready -> .claimed. If another caller
            // already renamed it, this returns Err(NotFound) and we skip.
            if std::fs::rename(&ready_marker, &claimed_marker).is_ok() {
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %worktree_dir.display(),
                    "CLAIM: atomically claimed worktree (.ready -> .claimed)"
                );
                return Some(worktree_dir);
            }
        }

        tracing::debug!(
            target: WORKTREE_POOL_LOG,
            "CLAIM: no ready worktree found in pool"
        );
        None
    }

    /// Count the number of ready worktrees in this pool instance.
    ///
    /// Scans the instance directory for `.ready` marker files. This is a
    /// fast O(pool_size) operation since pool sizes are small (typically 2-3).
    ///
    /// Used as a readiness gate by callers that need to know whether the
    /// pool has enough pre-created worktrees to satisfy a concurrent claim
    /// without falling back to slow on-demand creation.
    pub fn count_ready_worktrees(&self) -> usize {
        let instance_dir = self.instance_dir();
        let entries = match std::fs::read_dir(&instance_dir) {
            Ok(entries) => entries,
            Err(_) => return 0,
        };

        entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.ends_with(READY_SUFFIX)
            })
            .count()
    }

    // Claim + Sync (two-phase acquire for concurrent use)

    /// Atomically claim a ready worktree without syncing it.
    ///
    /// Performs the instant `.ready` → `.claimed` rename.
    /// Returns `Some(ClaimedWorktree)` if a worktree was claimed, `None`
    /// if the pool is empty.
    ///
    /// Use this when you need to claim multiple worktrees up-front (e.g. for
    /// concurrent fork comparisons) and then sync them concurrently via
    /// [`sync_claimed`](Self::sync_claimed). Wakes the fill task to create
    /// replacements for each claimed worktree.
    pub fn try_claim(&self) -> Option<ClaimedWorktree> {
        let pool_path = self.try_claim_ready_worktree()?;

        // Wake fill task to create a replacement
        self.fill_notify.notify_one();

        tracing::info!(
            target: WORKTREE_POOL_LOG,
            path = %pool_path.display(),
            "TRY_CLAIM_OK: claimed worktree"
        );
        Some(ClaimedWorktree { path: pool_path })
    }

    // Acquire

    /// Acquire a worktree from the pool, syncing it to the source's current state.
    ///
    /// Atomically claims a ready worktree via `.ready` -> `.claimed` rename.
    /// The worktree stays in the pool directory (no `git worktree move`) and
    /// is synced in place. Returns the pool path directly.
    ///
    /// If no worktree is ready but the fill task is creating one, waits up
    /// to 60s. Returns `None` only when the pool is truly empty with no
    /// pending creations, or the wait times out.
    #[tracing::instrument(skip(self), fields(pool_hit, acquire_ms))]
    pub async fn acquire(
        &self,
        session_id: &str,
        source_cwd: &Path,
        copy_dirty: bool,
    ) -> Option<AcquiredWorktree> {
        tracing::info!(
            target: WORKTREE_POOL_LOG,
            session_id = %session_id,
            source = %source_cwd.display(),
            copy_dirty,
            "ACQUIRE_START: attempting to acquire worktree from pool"
        );
        let acquire_start = std::time::Instant::now();

        let pool_path = match self.try_claim_ready_worktree() {
            Some(claimed) => claimed,
            None => {
                let in_progress = count_instance_worktrees(&self.instance_id);
                if in_progress == 0 {
                    tracing::info!(
                        target: WORKTREE_POOL_LOG,
                        session_id = %session_id,
                        "ACQUIRE_MISS: pool empty, no in-progress worktrees"
                    );
                    return None;
                }

                tracing::debug!(
                    in_progress,
                    "Pool has in-progress worktrees, waiting for ready"
                );
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    session_id = %session_id,
                    in_progress,
                    "ACQUIRE_WAIT: no ready worktree, waiting for fill task"
                );
                let wait_deadline = tokio::time::Instant::now() + Duration::from_secs(60);
                loop {
                    tokio::select! {
                        _ = self.ready_notify.notified() => {},
                        _ = self.cancel.cancelled() => {
                            tracing::debug!("Pool shutting down, aborting acquire wait");
                            tracing::info!(
                                target: WORKTREE_POOL_LOG,
                                session_id = %session_id,
                                elapsed_ms = acquire_start.elapsed().as_millis() as u64,
                                "ACQUIRE_CANCELLED: pool shutting down"
                            );
                            return None;
                        }
                        _ = tokio::time::sleep_until(wait_deadline) => {
                            tracing::debug!("Timed out waiting for pool worktree");
                            tracing::warn!(
                                target: WORKTREE_POOL_LOG,
                                session_id = %session_id,
                                elapsed_ms = acquire_start.elapsed().as_millis() as u64,
                                "ACQUIRE_TIMEOUT: timed out waiting 60s for pool worktree"
                            );
                            return None;
                        }
                    }
                    if let Some(claimed) = self.try_claim_ready_worktree() {
                        tracing::info!(
                            target: WORKTREE_POOL_LOG,
                            session_id = %session_id,
                            path = %claimed.display(),
                            wait_ms = acquire_start.elapsed().as_millis() as u64,
                            "ACQUIRE_WAIT_OK: claimed worktree after waiting"
                        );
                        break claimed;
                    }
                    if tokio::time::Instant::now() >= wait_deadline {
                        return None;
                    }
                }
            }
        };

        // Wake fill task to create a replacement
        self.fill_notify.notify_one();

        // Sync state in place (no git worktree move needed)
        tracing::info!(
            target: WORKTREE_POOL_LOG,
            session_id = %session_id,
            path = %pool_path.display(),
            "ACQUIRE_SYNC: syncing worktree to source state"
        );
        let source = source_cwd.to_path_buf();
        let dest = pool_path.clone();
        // Pool worktrees are known-clean: they were either freshly created
        // (GitCheckout) or just released (reset+clean). Skip the expensive
        // `git clean` walk (~800ms on 106K-file repos).
        let sync_result = tokio::task::spawn_blocking(move || {
            let sync = WorktreeSync::new(&source, &dest);
            sync.sync_worktree_opts(copy_dirty, /* skip_clean */ true)
        })
        .await;

        match sync_result {
            Ok(Ok(report)) => {
                let acquire_ms = acquire_start.elapsed().as_millis() as u64;
                tracing::Span::current().record("pool_hit", true);
                tracing::Span::current().record("acquire_ms", acquire_ms);
                tracing::info!(
                    session_id = %session_id,
                    path = %pool_path.display(),
                    acquire_ms = acquire_ms,
                    head_moved = report.head_moved,
                    dirty_files_copied = report.dirty_files_copied,
                    files_deleted = report.files_deleted,
                    staged_entries = report.staged_entries,
                    clean_skipped = report.clean_skipped,
                    pool_hit = true,
                    "A/B worktree acquired from pool"
                );
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    session_id = %session_id,
                    path = %pool_path.display(),
                    acquire_ms,
                    head_moved = report.head_moved,
                    dirty_files_copied = report.dirty_files_copied,
                    files_deleted = report.files_deleted,
                    staged_entries = report.staged_entries,
                    clean_skipped = report.clean_skipped,
                    head_resolve_ms = report.head_resolve_ms,
                    reset_hard_ms = report.reset_hard_ms,
                    clean_ms = report.clean_ms,
                    dirty_sync_ms = report.dirty_sync_ms,
                    git_status_ms = report.git_status_ms,
                    file_ops_ms = report.file_ops_ms,
                    staged_replay_ms = report.staged_replay_ms,
                    "ACQUIRE_OK: worktree acquired and synced from pool"
                );

                Some(AcquiredWorktree {
                    path: pool_path,
                    from_pool: true,
                    acquire_latency_ms: acquire_ms,
                })
            }
            Ok(Err(e)) => {
                tracing::Span::current().record("pool_hit", false);
                tracing::warn!("Pool worktree sync failed: {e}, falling back to on-demand");
                tracing::warn!(
                    target: WORKTREE_POOL_LOG,
                    session_id = %session_id,
                    path = %pool_path.display(),
                    elapsed_ms = acquire_start.elapsed().as_millis() as u64,
                    error = %e,
                    "ACQUIRE_SYNC_ERROR: sync failed, falling back to on-demand"
                );
                None
            }
            Err(e) => {
                tracing::warn!("Pool worktree sync task panicked: {e}");
                tracing::warn!(
                    target: WORKTREE_POOL_LOG,
                    session_id = %session_id,
                    path = %pool_path.display(),
                    elapsed_ms = acquire_start.elapsed().as_millis() as u64,
                    error = %e,
                    "ACQUIRE_SYNC_PANIC: sync task panicked"
                );
                None
            }
        }
    }

    // Release

    /// Release a worktree after a concurrent fork comparison completes.
    ///
    /// If the path is inside this pool's instance directory, the worktree is
    /// cleaned in place (`git reset --hard` + `git clean -fdx`) and re-marked
    /// `.ready`. No `git worktree move` is needed since `acquire()` hands out
    /// pool paths directly.
    ///
    /// If the path is outside the pool directory (on-demand fallback worktree),
    /// it is destroyed via `git worktree remove --force`.
    #[tracing::instrument(skip(self))]
    pub fn release(&self, worktree_path: PathBuf) {
        let instance_dir = self.instance_dir();

        // If the path isn't inside our pool dir, it's an on-demand worktree.
        if !worktree_path.starts_with(&instance_dir) {
            tracing::debug!("Non-pool worktree, destroying");
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                path = %worktree_path.display(),
                "RELEASE_NON_POOL: path outside pool dir, scheduling cleanup"
            );
            schedule_cleanup(worktree_path);
            return;
        }

        tracing::info!(
            target: WORKTREE_POOL_LOG,
            path = %worktree_path.display(),
            "RELEASE_START: cleaning worktree for reuse"
        );

        let ready_notify = self.ready_notify.clone();

        tokio::task::spawn(async move {
            let release_start = std::time::Instant::now();
            // Remove the .claimed sibling marker (acquire left it)
            let _ = tokio::fs::remove_file(marker_path(&worktree_path, CLAIMED_SUFFIX)).await;

            // Clean the worktree in place
            let pool_path = worktree_path.clone();
            let clean_start = std::time::Instant::now();
            let clean_result = tokio::task::spawn_blocking(move || {
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %pool_path.display(),
                    "RELEASE_GIT_RESET: running git reset --hard HEAD"
                );
                let r1 = git_command()
                    .args(["reset", "--hard", "HEAD"])
                    .current_dir(&pool_path)
                    .output();

                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %pool_path.display(),
                    success = r1.as_ref().map(|o| o.status.success()).unwrap_or(false),
                    "RELEASE_GIT_CLEAN: running git clean -fdx"
                );
                let r2 = git_command()
                    .args(["clean", "-fdx"])
                    .current_dir(&pool_path)
                    .output();

                let ok = matches!(
                    (&r1, &r2),
                    (Ok(o1), Ok(o2)) if o1.status.success() && o2.status.success()
                );
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %pool_path.display(),
                    reset_ok = r1.as_ref().map(|o| o.status.success()).unwrap_or(false),
                    clean_ok = r2.as_ref().map(|o| o.status.success()).unwrap_or(false),
                    overall_ok = ok,
                    "RELEASE_GIT_DONE: git reset+clean completed"
                );
                ok
            })
            .await;
            let clean_ms = clean_start.elapsed().as_millis() as u64;

            match clean_result {
                Ok(true) => {
                    tracing::info!(
                        target: WORKTREE_POOL_LOG,
                        path = %worktree_path.display(),
                        clean_ms,
                        "RELEASE_CLEANED: git reset/clean succeeded, warming caches"
                    );
                    // Warm caches before marking ready
                    warm_git_caches(&worktree_path).await;

                    // Re-mark as ready (sibling marker file)
                    let claiming = marker_path(&worktree_path, CLAIMING_SUFFIX);
                    let ready = marker_path(&worktree_path, READY_SUFFIX);
                    if let Err(e) = tokio::fs::write(&claiming, "").await {
                        tracing::warn!("Failed to write .claiming marker: {e}");
                        tracing::warn!(
                            target: WORKTREE_POOL_LOG,
                            path = %worktree_path.display(),
                            error = %e,
                            "RELEASE_ERROR: failed to write .claiming marker"
                        );
                        remove_worktree_registration(&worktree_path).await;
                        return;
                    }
                    if let Err(e) = tokio::fs::rename(&claiming, &ready).await {
                        tracing::warn!("Failed to rename .claiming -> .ready: {e}");
                        tracing::warn!(
                            target: WORKTREE_POOL_LOG,
                            path = %worktree_path.display(),
                            error = %e,
                            "RELEASE_ERROR: failed to rename .claiming -> .ready"
                        );
                        remove_worktree_registration(&worktree_path).await;
                    } else {
                        let total_ms = release_start.elapsed().as_millis() as u64;
                        tracing::debug!(
                            path = %worktree_path.display(),
                            "Release: worktree cleaned and marked ready"
                        );
                        tracing::info!(
                            target: WORKTREE_POOL_LOG,
                            path = %worktree_path.display(),
                            total_ms,
                            clean_ms,
                            "RELEASE_READY: worktree cleaned and marked ready for reuse"
                        );
                        ready_notify.notify_waiters();
                    }
                }
                _ => {
                    tracing::debug!("Release: git reset/clean failed, destroying worktree");
                    tracing::warn!(
                        target: WORKTREE_POOL_LOG,
                        path = %worktree_path.display(),
                        clean_ms,
                        "RELEASE_FAILED: git reset/clean failed, destroying worktree"
                    );
                    remove_worktree_registration(&worktree_path).await;
                }
            }
        });
    }

    // Shutdown

    /// Shut down the pool: cancel the fill task and remove this instance's
    /// pool directory.
    pub fn shutdown(&self) {
        tracing::info!(
            target: WORKTREE_POOL_LOG,
            instance_id = %self.instance_id,
            instance_dir = %self.instance_dir().display(),
            "SHUTDOWN: shutting down worktree pool"
        );
        self.cancel.cancel();
        let _ = std::fs::remove_dir_all(self.instance_dir());
    }
}

impl Drop for WorktreePool {
    fn drop(&mut self) {
        tracing::info!(
            target: WORKTREE_POOL_LOG,
            instance_id = %self.instance_id,
            instance_dir = %self.instance_dir().display(),
            "DROP: worktree pool being dropped"
        );
        self.cancel.cancel();
        let _ = std::fs::remove_dir_all(self.instance_dir());
    }
}

// Pool activation logic

/// Determine if the worktree pool should be enabled for the given repo.
///
/// Gates on:
/// 1. `config.enabled` (explicit disable)
/// 2. Platform: macOS only (Linux has O(1) BTRFS snapshots)
/// 3. Pool-eligible mode: Tui, Stdio, or Headless (modes that may need
///    concurrent worktrees)
/// 4. Repo size: file count >= threshold
pub fn should_enable_pool(
    file_count: usize,
    config: &PoolConfig,
    is_ab_capable_mode: bool,
) -> bool {
    if !config.enabled {
        tracing::debug!("Worktree pool: disabled by config");
        tracing::info!(target: WORKTREE_POOL_LOG, "POOL_GATE: disabled by config");
        return false;
    }

    if !cfg!(target_os = "macos") {
        tracing::debug!("Worktree pool: not macOS, skipping");
        tracing::info!(target: WORKTREE_POOL_LOG, "POOL_GATE: not macOS, skipping");
        return false;
    }

    if !is_ab_capable_mode {
        tracing::debug!("Worktree pool: not in A/B-capable mode, skipping");
        tracing::info!(target: WORKTREE_POOL_LOG, "POOL_GATE: not in A/B-capable mode, skipping");
        return false;
    }

    if file_count < config.file_count_threshold {
        tracing::debug!(
            file_count = file_count,
            threshold = config.file_count_threshold,
            "Worktree pool: repo below threshold"
        );
        tracing::info!(
            target: WORKTREE_POOL_LOG,
            file_count,
            threshold = config.file_count_threshold,
            "POOL_GATE: repo below file count threshold"
        );
        return false;
    }

    tracing::info!(
        target: WORKTREE_POOL_LOG,
        file_count,
        threshold = config.file_count_threshold,
        pool_size = config.pool_size,
        "POOL_GATE: all gates passed, pool will be enabled"
    );
    true
}

// Startup cleanup (multi-instance safe, runs at most once per process)

/// Guard ensuring the expensive directory walk + `git worktree remove`
/// loop runs at most once per process.
static CLEANUP_ONCE: std::sync::Once = std::sync::Once::new();

static REGISTRATION_CLEANUP_ONCE: std::sync::Once = std::sync::Once::new();

// Orphan adoption

/// A worktree from a dead pool instance that passed structural validation
/// and may be adoptable by a new pool instance.
///
/// Collected during startup cleanup, consumed by `WorktreePool::new()`.
pub struct AdoptableWorktree {
    /// Current path: `~/.astra/worktree_pool/<old_instance>/<pool_id>/`
    pub old_path: PathBuf,
    /// The pool_id (directory name, also the key in `.git/worktrees/`)
    pub pool_id: String,
    /// The old instance_id (for logging and old-dir cleanup)
    pub old_instance_id: String,
}

/// Module-scoped cache for adoptable worktrees found during cleanup.
/// Written once by `cleanup_stale_pool_worktrees_inner()`, consumed
/// once by `take_adoptable_worktrees()` in `WorktreePool::new()`.
static ADOPTABLE_CACHE: std::sync::Mutex<Option<Vec<AdoptableWorktree>>> =
    std::sync::Mutex::new(None);

/// Consume the adoptable worktree candidates collected during cleanup.
///
/// Returns the full list on the first call, empty `Vec` on subsequent calls
/// (the `Option` is `.take()`n). Safe to call from any thread.
pub fn take_adoptable_worktrees() -> Vec<AdoptableWorktree> {
    ADOPTABLE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_default()
}

/// Remove pooled worktrees belonging to dead agent instances.
///
/// **The expensive part (directory walk + `git worktree remove`) runs at
/// most once per process.** Multiple call sites (`initialize`,
/// `new_session`, experiment fetch) may race to invoke this; only the
/// first caller does the real work, the rest return instantly.
///
/// Stale registration removal is gated separately so the first caller
/// that provides a `source_git_root` triggers it, even if the directory
/// cleanup already ran from an earlier call with `None`.
///
/// Multi-instance safe: iterates instance subdirectories under
/// `~/.astra/worktree_pool/`, reads each `.pid` file, and checks
/// whether the PID is still alive. Only cleans directories where the
/// owning process is dead.
///
/// Three-step cleanup per dead instance:
/// 1. `git worktree remove --force` each worktree in the dead instance dir.
/// 2. Delete the instance directory from disk.
/// 3. Remove any remaining stale *pool-owned* registrations from the source
///    repo's `.git/worktrees/` as a safety net.
///
/// This is a **synchronous** function intended to be called via
/// `tokio::task::spawn_blocking` so it runs on the thread pool and
/// never competes with the agent's single-threaded `LocalSet`.
pub fn cleanup_stale_pool_worktrees(source_git_root: Option<&Path>) {
    CLEANUP_ONCE.call_once(|| {
        cleanup_stale_pool_worktrees_inner();
    });

    if let Some(git_root) = source_git_root {
        let root = git_root.to_path_buf();
        REGISTRATION_CLEANUP_ONCE.call_once(move || {
            let removed = xai_fast_worktree::remove_stale_worktree_registrations_under(
                &root,
                &pool_base_directory(),
            );
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                git_root = %root.display(),
                removed,
                "CLEANUP_PRUNE_DONE: stale pool registration removal completed"
            );
        });
    }
}

fn cleanup_stale_pool_worktrees_inner() {
    let pool_dir = pool_base_directory();
    tracing::info!(
        target: WORKTREE_POOL_LOG,
        pool_dir = %pool_dir.display(),
        "CLEANUP_START: scanning for dead instance pool directories"
    );
    let Ok(instances) = std::fs::read_dir(&pool_dir) else {
        tracing::info!(
            target: WORKTREE_POOL_LOG,
            pool_dir = %pool_dir.display(),
            "CLEANUP_SKIP: pool directory does not exist or unreadable"
        );
        return;
    };

    let mut cleaned_count = 0u32;
    let mut dead_instance_count = 0u32;

    for instance_entry in instances.flatten() {
        let instance_path = instance_entry.path();
        if !instance_path.is_dir() {
            continue;
        }

        let pid_alive = match std::fs::read_to_string(instance_path.join(".pid")) {
            Ok(contents) => match contents.trim().parse::<u32>() {
                Ok(pid) => {
                    let alive = crate::util::is_process_alive(pid);
                    tracing::info!(
                        target: WORKTREE_POOL_LOG,
                        instance_dir = %instance_path.display(),
                        pid,
                        alive,
                        "CLEANUP_CHECK_PID: checked instance PID"
                    );
                    alive
                }
                Err(_) => false,
            },
            Err(_) => false,
        };

        if pid_alive {
            tracing::debug!(
                instance_dir = %instance_path.display(),
                "Skipping live instance pool directory"
            );
            continue;
        }

        dead_instance_count += 1;

        tracing::info!(
            target: WORKTREE_POOL_LOG,
            instance_dir = %instance_path.display(),
            "CLEANUP_DEAD: found dead instance pool directory"
        );

        // Deregister and delete every worktree subdirectory
        // (The old pool adopted structurally valid worktrees here; with the pool gone they are reclaimed like any other stale directory.)
        if let Ok(entries) = std::fs::read_dir(&instance_path) {
            for wt_entry in entries.flatten() {
                let wt_path = wt_entry.path();
                if !wt_path.is_dir() {
                    continue; // skip .pid, marker files
                }

                let p = wt_path.to_string_lossy().to_string();
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %wt_path.display(),
                    "CLEANUP_GIT_REMOVE: running git worktree remove --force"
                );
                let result = git_command()
                    .args(["worktree", "remove", "--force", &p])
                    .output();
                tracing::info!(
                    target: WORKTREE_POOL_LOG,
                    path = %wt_path.display(),
                    success = result.as_ref().map(|o| o.status.success()).unwrap_or(false),
                    "CLEANUP_GIT_REMOVE_DONE: git worktree remove completed"
                );
                let _ = std::fs::remove_dir_all(&wt_path);
            }
        }

        let _ = std::fs::remove_dir_all(&instance_path);
        cleaned_count += 1;
    }

    tracing::info!(
        target: WORKTREE_POOL_LOG,
        dead_instance_count,
        cleaned_count,
        "CLEANUP_DONE: finished scanning for dead instances"
    );
}

/// The base pool directory under `~/.astra/`.
/// Instance-scoped directories live under this: `worktree_pool/<instance_id>/`.
fn pool_base_directory() -> PathBuf {
    grok_home().join("worktree_pool")
}

/// Count worktree subdirectories in the instance dir that are NOT claimed.
///
/// Excludes directories with a sibling `<name>.claimed` marker — those are
/// being used by `acquire()` and should not count against pool capacity.
/// This prevents the fill task from thinking the pool is full when entries
/// are mid-acquisition.
// PERF: Uses sync `std::fs::read_dir` on an async path. Acceptable at
// current pool sizes (2-3 entries) but would need `spawn_blocking` if the
// pool grows significantly.
fn count_instance_worktrees(instance_id: &str) -> usize {
    let instance_dir = pool_base_directory().join(instance_id);
    let Ok(entries) = std::fs::read_dir(&instance_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            let p = e.path();
            p.is_dir() && !marker_path(&p, CLAIMED_SUFFIX).exists()
        })
        .count()
}

/// Create one linked worktree for the pool using `WorktreeBuilder`.
///
/// Always creates **linked** worktrees (shared object store) with
/// `CleanAll` mode. Placed under `~/.astra/worktree_pool/<instance_id>/<uuid>/`.
async fn create_pooled_worktree(
    source: &Path,
    instance_id: &str,
    parallelism: usize,
) -> anyhow::Result<PathBuf> {
    let pool_id = uuid::Uuid::now_v7().to_string();
    let dest = pool_base_directory().join(instance_id).join(&pool_id);

    tracing::info!(
        target: WORKTREE_POOL_LOG,
        source = %source.display(),
        dest = %dest.display(),
        instance_id,
        pool_id,
        parallelism,
        "CREATE_POOLED_START: creating worktree via git checkout (checkout.workers={parallelism})"
    );

    let create_start = std::time::Instant::now();
    let source = source.to_path_buf();
    let dest_clone = dest.clone();

    let result = tokio::task::spawn_blocking(move || {
        WorktreeBuilder::new(&source, &dest_clone)
            .creation_mode(xai_fast_worktree::CreationMode::GitCheckout)
            .parallelism(parallelism)
            .worktree_kind(xai_fast_worktree::WorktreeKind::Pool)
            .create()
    })
    .await?;

    let create_ms = create_start.elapsed().as_millis() as u64;
    match &result {
        Ok(_) => {
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                dest = %dest.display(),
                create_ms,
                "CREATE_POOLED_OK: worktree created successfully"
            );
        }
        Err(e) => {
            tracing::warn!(
                target: WORKTREE_POOL_LOG,
                dest = %dest.display(),
                create_ms,
                "CREATE_POOLED_ERROR: WorktreeBuilder::create failed: {e:?}"
            );
        }
    }
    result?;

    Ok(dest)
}

/// Schedule async cleanup of a discarded pooled worktree.
///
/// Used on error paths (stale eviction, failed move, etc.) to deregister
/// the linked worktree from `.git/worktrees/` and remove the directory.
/// Also removes any sibling marker files (`.ready`, `.claimed`, `.claiming`).
///
/// Uses `rm -rf` + deregister instead of `git worktree remove --force`,
/// which is ~10x faster on large repos (the kernel handles bulk deletion
/// more efficiently than git's per-file walk).
fn schedule_cleanup(path: PathBuf) {
    tracing::info!(
        target: WORKTREE_POOL_LOG,
        path = %path.display(),
        "SCHEDULE_CLEANUP: scheduling async cleanup of discarded worktree"
    );
    tokio::task::spawn(async move {
        // Remove sibling marker files regardless of whether the dir exists.
        for suffix in [READY_SUFFIX, CLAIMED_SUFFIX, CLAIMING_SUFFIX] {
            let _ = tokio::fs::remove_file(marker_path(&path, suffix)).await;
        }

        if path.exists() {
            let cleanup_start = std::time::Instant::now();
            let path_clone = path.clone();
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                path = %path.display(),
                "SCHEDULE_CLEANUP_FAST: rm -rf + deregister"
            );
            let result = tokio::task::spawn_blocking(move || {
                xai_fast_worktree::remove_worktree(&path_clone)
            })
            .await;
            let cleanup_ms = cleanup_start.elapsed().as_millis() as u64;
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                path = %path.display(),
                success = result.as_ref().ok().map(|r| r.is_ok()).unwrap_or(false),
                cleanup_ms,
                "SCHEDULE_CLEANUP_DONE: worktree cleanup completed"
            );
        } else {
            tracing::info!(
                target: WORKTREE_POOL_LOG,
                path = %path.display(),
                "SCHEDULE_CLEANUP_SKIP: path does not exist"
            );
        }
    });
}

/// Deregister a linked worktree and remove its directory.
///
/// Used on error paths where creation succeeded but a subsequent step
/// (e.g. sync or marker write) failed, leaving a registered worktree
/// at the given path. Also cleans up any sibling marker files.
///
/// Uses `rm -rf` + deregister instead of `git worktree remove --force`.
async fn remove_worktree_registration(path: &Path) {
    tracing::info!(
        target: WORKTREE_POOL_LOG,
        path = %path.display(),
        "DEREGISTER_START: removing worktree registration"
    );
    let deregister_start = std::time::Instant::now();

    // Remove sibling marker files.
    for suffix in [READY_SUFFIX, CLAIMED_SUFFIX, CLAIMING_SUFFIX] {
        let _ = tokio::fs::remove_file(marker_path(path, suffix)).await;
    }

    let path_owned = path.to_path_buf();
    let result =
        tokio::task::spawn_blocking(move || xai_fast_worktree::remove_worktree(&path_owned)).await;
    let deregister_ms = deregister_start.elapsed().as_millis() as u64;
    tracing::info!(
        target: WORKTREE_POOL_LOG,
        path = %path.display(),
        success = result.as_ref().ok().map(|r| r.is_ok()).unwrap_or(false),
        deregister_ms,
        "DEREGISTER_DONE: worktree deregistered and directory removed"
    );
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_base_directory() {
        let dir = pool_base_directory();
        assert!(dir.to_string_lossy().contains("worktree_pool"));
    }

    #[test]
    fn test_cleanup_stale_only_removes_dead_instances() {
        let live_dir =
            pool_base_directory().join(format!("live-instance-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&live_dir).unwrap();
        std::fs::write(live_dir.join(".pid"), std::process::id().to_string()).unwrap();

        let dead_dir =
            pool_base_directory().join(format!("dead-instance-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dead_dir).unwrap();
        std::fs::write(dead_dir.join(".pid"), "4000000000").unwrap();
        let fake_wt_a = dead_dir.join("fake-wt-aaa");
        let fake_wt_b = dead_dir.join("fake-wt-bbb");
        std::fs::create_dir_all(&fake_wt_a).unwrap();
        std::fs::create_dir_all(&fake_wt_b).unwrap();
        std::fs::write(fake_wt_a.join("file.txt"), "leftover").unwrap();
        std::fs::write(fake_wt_b.join("file.txt"), "leftover").unwrap();

        // Call the inner function directly to bypass the process-global `Once` guard (other tests may have already triggered it)
        cleanup_stale_pool_worktrees_inner();

        assert!(
            live_dir.exists(),
            "Live pool's instance dir should NOT be cleaned"
        );
        assert!(!dead_dir.exists(), "Dead instance dir should be cleaned up");

        let _ = std::fs::remove_dir_all(&live_dir);
    }
}
