//! How much of the machine an operation may use.
//!
//! Reading and writing both spread work over threads and both hold buffers
//! whose size follows from that, so the knobs are shared rather than defined
//! twice with different names.

use std::num::{NonZeroU64, NonZeroUsize};

/// Thread configuration for parallel operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Threads {
    /// Automatically determine thread count.
    #[default]
    Auto,
    /// Use a specific number of threads.
    ///
    /// The count must be non-zero. Use [`NonZeroUsize`] to ensure
    /// this at compile time. If you have a value that might be zero, use
    /// [`Threads::count_or_single`] instead.
    Count(NonZeroUsize),
    /// Single-threaded operation.
    Single,
}

impl Threads {
    /// Creates a `Threads::Count` variant from a `usize`.
    ///
    /// Returns `Threads::Single` if the count is zero, otherwise returns
    /// `Threads::Count` with the specified thread count.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::Threads;
    ///
    /// // Zero becomes Single
    /// assert_eq!(Threads::count_or_single(0), Threads::Single);
    ///
    /// // Non-zero becomes Count
    /// assert_eq!(Threads::count_or_single(4).count(), 4);
    /// ```
    pub fn count_or_single(n: usize) -> Self {
        match NonZeroUsize::new(n) {
            Some(count) => Self::Count(count),
            None => Self::Single,
        }
    }

    /// Returns the actual thread count.
    ///
    /// # Thread Count Resolution
    ///
    /// - `Threads::Auto`: Returns the number of available CPUs, minimum 1
    /// - `Threads::Count(n)`: Returns `n.get()` (always >= 1 since NonZeroUsize)
    /// - `Threads::Single`: Returns 1
    pub fn count(&self) -> usize {
        match self {
            Self::Auto => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            Self::Count(n) => n.get(),
            Self::Single => 1,
        }
    }

    /// Returns whether only one thread may be used.
    ///
    /// Resolved rather than matched on: [`Self::Single`],
    /// [`Self::count_or_single(1)`](Self::count_or_single) and [`Self::Auto`]
    /// on a single-core machine all mean one thread, and all behave the same
    /// way for it.
    ///
    /// A writer takes this as an instruction about the output as well as the
    /// speed: one thread writes one unbroken LZMA2 stream, which is the
    /// smallest a level can produce. Any count above one writes the same bytes
    /// as any other count above one, on any machine.
    pub fn is_single(&self) -> bool {
        self.count() == 1
    }
}

/// How much memory concurrent work may reserve.
///
/// A compressor's match finder runs to roughly twelve times its dictionary, so
/// a writer told to use every core of a large machine can reserve gigabytes
/// unless something says otherwise. This is what bounds that: it decides how
/// many encoders run at once, and how much data waits between them. Lowering it
/// costs speed rather than correctness, and never changes the bytes written.
///
/// It is not a ceiling on the process. Work that must be held whole costs what
/// it costs, and one such item larger than the limit exceeds it. That is most
/// work: only a large entry going straight into the sink is compressed as it is
/// read, and a solid block, a filter, encryption of the data, or the async
/// writer each rule that out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryLimit {
    /// Pick a limit from what the machine has.
    ///
    /// With the `sysinfo` feature this is what the cores can put to use out of
    /// the memory actually available; without it, a fixed default that suits
    /// an ordinary desktop. A cgroup cap is honoured either way, since it is
    /// read from files rather than asked of the machine, and it is the figure
    /// a process is killed for exceeding.
    #[default]
    Auto,
    /// Allow concurrent work to reserve about this many bytes.
    ///
    /// Values below 16 MiB are raised to it: one encoder has to run whatever it
    /// costs, so a smaller figure can only mean "as little as possible" - and a
    /// single encoder at a high level exceeds even that.
    Bytes(NonZeroU64),
}

/// The limit `MemoryLimit::Auto` falls back to when the machine cannot be asked.
///
/// Enough for several concurrent encoders at the default level, which is what
/// it takes to keep a multi-core machine busy, and small enough not to be a
/// surprise on a modest one.
const DEFAULT_MEMORY_LIMIT: u64 = 512 * 1024 * 1024;

/// The smallest limit worth honouring.
///
/// One encoder has to run whatever it costs - refusing to compress would be
/// worse than exceeding a budget - so a limit below this only means "as little
/// as possible".
const MINIMUM_MEMORY_LIMIT: u64 = 16 * 1024 * 1024;

/// What one core needs before it stops waiting for memory.
///
/// A core busy on the default level holds a match finder of about 96 MiB, the
/// block it is compressing, and the output it is producing - and it needs a
/// second block already in hand, or it idles from the moment it finishes until
/// the writer collects the block in front of it. That comes to roughly this
/// much per core at the largest block a stream reaches.
///
/// Measured against that arithmetic rather than taken from it. On twenty-four
/// cores over twenty-four 50 MB entries, which is the shape that depends on
/// this figure, 85 MiB per core took 72.9 s, 170 took 59.0, and 256 took 38.5;
/// 341 and 512 took 39.3 and 40.3, so the curve is flat from a little under
/// this figure onwards and there is nothing to buy above it. A single large
/// entry and a corpus of mixed sizes are flat across the whole range, being
/// bounded by the window ceiling and by batch sizes rather than by memory.
///
/// It is a figure for the default level rather than the level in use, because
/// the budget is decided once for the process and a level is chosen per write.
/// A higher level costs more per core and simply runs fewer of them, which is
/// the right answer for a setting that would otherwise reserve thirty
/// gigabytes because a machine has thirty cores.
const BUDGET_PER_CORE: u64 = 340 * 1024 * 1024;

impl MemoryLimit {
    /// Creates a limit of `bytes`, or [`MemoryLimit::Auto`] if that is zero.
    pub fn bytes_or_auto(bytes: u64) -> Self {
        match NonZeroU64::new(bytes) {
            Some(bytes) => Self::Bytes(bytes),
            None => Self::Auto,
        }
    }

    /// Returns the limit in bytes.
    pub fn bytes(&self) -> u64 {
        match self {
            Self::Auto => Self::detected(),
            Self::Bytes(bytes) => bytes.get().max(MINIMUM_MEMORY_LIMIT),
        }
    }

    /// Returns a limit derived from the machine, asked once per process.
    ///
    /// Asking costs tens of microseconds - a `System`, a read of
    /// /proc/meminfo, and the cgroup files - and the writer asks several times
    /// for every entry it accepts, which on an archive of many small files is
    /// most of the time it spends. Once is also the more useful answer: a
    /// budget that drifted with whatever else the machine was doing would make
    /// two runs of the same command behave differently for no reason the
    /// caller could see.
    fn detected() -> u64 {
        static DETECTED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        *DETECTED.get_or_init(Self::detect_once)
    }

    /// Works out the budget from the machine. Called once, by [`Self::detected`].
    fn detect_once() -> u64 {
        // How many cores the budget has to feed. `Threads::Auto` is what a
        // caller who has not said otherwise gets, so it is what the budget is
        // sized for; a caller who asks for fewer threads simply leaves some of
        // it unused, which is a ceiling doing its job rather than an error.
        let cores = Threads::Auto.count() as u64;

        #[cfg(feature = "sysinfo")]
        {
            use sysinfo::System;

            let mut system = System::new();
            system.refresh_memory();

            // Nothing else may be asked until this is known to be answerable:
            // `cgroup_limits` asserts that the total is non-zero, so on a
            // machine whose /proc/meminfo cannot be read it panics rather than
            // returning None. Zero here means the question failed, which is
            // what the fixed default is for.
            if system.total_memory() == 0 {
                // `cgroup_limits` asserts on a zero total rather than
                // returning None, so nothing below may be asked.
                return Self::budget_for(cgroup_headroom(), cores);
            }

            // Inside a container the host's figures describe a machine this
            // process does not have: the cgroup is what it may actually use,
            // and exceeding that is killed rather than merely slow.
            //
            // Read here rather than taken from `sysinfo`, which looks only at
            // the root of the hierarchy - the container's own group when the
            // process has a cgroup namespace, and something with no limit on
            // it at all under a systemd unit with `MemoryMax`. Its figure is
            // also the cap less `memory.current`, which counts cached files,
            // so where both apply it is the smaller and would always win.
            //
            // It remains the fallback for a hierarchy this cannot make sense
            // of, since an understated budget costs speed while a missing cap
            // costs the process.
            let capped = cgroup_headroom()
                .or_else(|| System::cgroup_limits(&system).map(|limits| limits.free_memory));

            let available = match capped {
                Some(capped) => capped.min(system.available_memory()),
                None => system.available_memory(),
            };

            Self::budget_for(Some(available), cores)
        }

        // Without the feature the machine cannot be asked how much it has -
        // but a cgroup cap is read from files rather than asked, and it is the
        // figure that gets a process killed for exceeding it, so it still
        // applies. A cgroup with nothing left answers zero, which is an
        // answer: it earns the floor, not the fixed default.
        #[cfg(not(feature = "sysinfo"))]
        Self::budget_for(cgroup_headroom(), cores)
    }

    /// The budget for a machine with this much memory free, out of this much,
    /// with this many cores to feed.
    ///
    /// What the cores can actually use, and never more than half of what is
    /// free. Both halves matter. A fraction of free memory alone takes no
    /// account of the machine it is running on: a quarter of what was free
    /// bounded a twenty-four core writer to sixteen busy cores on one corpus
    /// and to three on a smaller machine, because the window a budget buys
    /// narrows as the blocks in a stream grow. Cores alone would be worse in
    /// the other direction, reserving eight gigabytes on a machine with two to
    /// spare.
    ///
    /// Half rather than all of what is free: the figure is what the writer may
    /// reserve, not what the process occupies, and leaving nothing for the page
    /// cache would cost more in reading than the parallelism gains.
    ///
    /// The fixed default is for a machine that could not be asked at all, and
    /// only that - `None` here, rather than an answer that came out small.
    /// Reaching for it whenever the figure was low is what made the setting
    /// perverse: a machine with 63 MiB free was handed 512 MiB, and one with
    /// 64 MiB free, 16 MiB. A cgroup sitting exactly on its cap answers zero,
    /// which is an answer, and the floor is what that deserves.
    fn budget_for(available: Option<u64>, cores: u64) -> u64 {
        let Some(available) = available else {
            return DEFAULT_MEMORY_LIMIT;
        };

        let wanted = cores.max(1).saturating_mul(BUDGET_PER_CORE);
        wanted.min(available / 2).max(MINIMUM_MEMORY_LIMIT)
    }
}

/// Returns how much the cgroup this process is in has left, if it is capped.
///
/// The figure that gets a process killed for exceeding it. A container runtime
/// puts the process in a namespace where its own cgroup is the root, so the
/// limit is one file away; a systemd unit with `MemoryMax` does not, and the
/// limit then sits several levels above the leaf while the root says "max".
/// Walking up from the leaf covers both, and answers with the tightest cap on
/// the way, since a parent's limit binds its children.
///
/// `None` when nothing along the way is capped, which is the ordinary case on
/// a desktop, or when the files cannot be read at all.
#[cfg(target_os = "linux")]
fn cgroup_headroom() -> Option<u64> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    headroom_from(&own, &mounts)
}

/// The headroom described by a /proc/self/cgroup and a /proc/self/mountinfo.
///
/// Split out from the files they come from so that a test can lay out a
/// hierarchy of its own: what this has to get right is which directories are
/// consulted and which of their numbers wins, and neither is observable from
/// the machine the test runs on.
#[cfg(target_os = "linux")]
fn headroom_from(own: &str, mounts: &str) -> Option<u64> {
    let mut headroom = None;

    for dir in cgroup_dirs(own, mounts) {
        // v2 keeps both a hard cap and a throttling threshold, and a process
        // is killed for the first while merely slowed by the second; v1 names
        // the hard cap differently. Whichever of them exists here, the
        // tightest one is what this level allows.
        let Some(limit) = ["memory.max", "memory.high", "memory.limit_in_bytes"]
            .iter()
            .filter_map(|file| read_cgroup_value(&dir, file))
            .min()
        else {
            continue;
        };

        // A cap that is readable while its usage is not says nothing about
        // what is left, and the answer is then none of it rather than all of
        // it: the two figures come from the same kernel, so one without the
        // other means the reading is wrong rather than the cgroup empty.
        // Dropping the level instead would discard the cap altogether and
        // leave the writer sizing itself for the whole machine.
        let Some(used) = cgroup_usage(&dir) else {
            headroom = Some(0);
            continue;
        };

        // Cached pages this cap cannot reclaim, because something under it was
        // promised them. Where that cannot be established, nothing under this
        // cap is assumed reclaimable at all.
        //
        // Added to the breakdown rather than compared with it: a floor in one
        // branch and anonymous memory in another are different pages, and
        // taking the larger of the two reads the smaller as free. Bounded by
        // what the group actually holds, since the two do overlap wherever the
        // protected branch is holding anonymous memory of its own, and the sum
        // would then be more than there is.
        let used = match protected_below(&dir) {
            Some(protected) => {
                let bound = used.saturating_add(protected);
                whole_usage(&dir).map_or(bound, |whole| bound.min(whole))
            }
            None => whole_usage(&dir).unwrap_or(limit),
        };

        let level = limit.saturating_sub(used);
        headroom = Some(headroom.map_or(level, |seen: u64| seen.min(level)));
    }

    headroom
}

/// How many groups a walk of one subtree may look at.
///
/// A cap usually sits over a handful of groups - a container, a service, a
/// user session - and the walk is over in microseconds. It is a machine with
/// thousands of them under one cap that this bounds, where the walk would cost
/// more than the answer is worth; the caller falls back to counting everything
/// as occupied rather than to guessing.
///
/// Groups rather than directory entries. A cgroup directory holds dozens of
/// controller files beside its children, so counting entries spent this on
/// about a hundred groups: the desktop this was written on has 136 of them and
/// 7538 entries, and the fallback would have triggered on the machine the
/// figure was meant to leave alone.
#[cfg(target_os = "linux")]
const SUBTREE_BUDGET: u32 = 4096;

/// How much memory beneath `dir` the kernel has promised not to reclaim.
///
/// Reclaim protection is relative to what the reclaim is for. A group's own
/// `memory.min` does not protect it from pressure created by its own cap: the
/// kernel is reclaiming to keep that group inside its limit, and the floor
/// says nothing about that. What the floor does bind is reclaim driven from
/// above - so the floors that matter to a cap are the ones underneath it.
///
/// They are rarely on the path this process sits on. A service alongside it in
/// the same slice, with a floor of its own, holds cached pages that the slice's
/// cap cannot take back, and nothing in this process's own directories says so.
/// So the subtree is walked rather than the path.
///
/// Floors on different branches add up, since each protects its own pages. A
/// floor inside another floor does not: a child's effective floor is bounded by
/// its parent's, so the larger of the two is what that branch holds.
///
/// `None` when the subtree cannot be walked or is too large to, which is the
/// caller's signal to stop treating any of the cache as reclaimable.
#[cfg(target_os = "linux")]
fn protected_below(dir: &str) -> Option<u64> {
    let mut budget = SUBTREE_BUDGET;
    protected_within(dir, &mut budget)
}

#[cfg(target_os = "linux")]
fn protected_within(dir: &str, budget: &mut u32) -> Option<u64> {
    let mut total = 0u64;

    // Every failure below answers `None` rather than skipping what it could
    // not read. A walk that quietly leaves out a branch reports less protected
    // memory than there is, which is the direction that gets a process killed;
    // the caller's fallback is what an incomplete answer deserves.
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }

        // Counted here, where the entry is known to be a group. A cgroup
        // directory holds dozens of files beside its children.
        if *budget == 0 {
            return None;
        }
        *budget -= 1;

        let child = entry.path();
        let child = child.to_str()?;

        // A promise only binds the memory a group is actually holding: the
        // kernel protects up to the floor, not the floor regardless. On this
        // machine a service promised 64 MiB is holding 1.7, and a session
        // slice promised 250 MiB is holding 21, so charging the writer the
        // configured figure gives away parallelism for memory nobody has.
        let held = whole_usage(child)?;
        let floor = read_floor(child)?.min(held);

        total = total.saturating_add(floor.max(protected_within(child, budget)?));
    }

    Some(total)
}

/// The floor a group has been promised, if it can be established.
///
/// Absent means nothing is promised, which is the ordinary case and the
/// kernel's own default. Present and unreadable, or present and not a number,
/// means the promise cannot be established - and a promise that cannot be read
/// is not the same as no promise at all.
#[cfg(target_os = "linux")]
fn read_floor(dir: &str) -> Option<u64> {
    match std::fs::read_to_string(format!("{dir}/memory.min")) {
        Ok(contents) => contents.trim().parse().ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(0),
        Err(_) => None,
    }
}

/// Everything a cgroup holds, cache included.
///
/// The figure to fall back on when what is reclaimable cannot be told from
/// what is not: it is never an understatement, which is the direction that
/// costs a process rather than a little speed.
#[cfg(target_os = "linux")]
fn whole_usage(dir: &str) -> Option<u64> {
    ["memory.current", "memory.usage_in_bytes"]
        .iter()
        .find_map(|file| read_cgroup_value(dir, file))
}

#[cfg(not(target_os = "linux"))]
fn cgroup_headroom() -> Option<u64> {
    None
}

/// Where a cgroup hierarchy is mounted, and which part of it is visible.
///
/// `root` is the subtree the mount exposes: usually the whole hierarchy, but
/// a bind mount of a branch shows that branch as if it were the top, and then
/// the paths in /proc/self/cgroup are not relative to the mount point until
/// that prefix is taken off them.
#[cfg(target_os = "linux")]
struct CgroupMount {
    root: String,
    point: String,
    v2: bool,
}

/// Yields every directory to look in for a cap, leaf first.
///
/// One line of /proc/self/cgroup is `hierarchy:controllers:path`. Version 2
/// writes a single line with an empty controller field and keeps everything
/// under one mount; version 1 writes a line per controller, mounted per
/// controller, and only the memory one has anything to say here.
///
/// Every ancestor of the process's own directory is yielded too, up to the
/// mount point, because a cap on a parent binds everything beneath it and is
/// not repeated in the children. Nothing above the mount point is yielded:
/// what is not mounted cannot be read, whatever it may say.
#[cfg(target_os = "linux")]
fn cgroup_dirs(own: &str, mounts: &str) -> impl Iterator<Item = String> {
    let mounts = cgroup_mounts(mounts);
    let mut dirs = Vec::new();

    for line in own.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(_hierarchy), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        let v2 = controllers.is_empty();
        if !v2 && !controllers.split(',').any(|name| name == "memory") {
            continue;
        }

        for mount in mounts.iter().filter(|mount| mount.v2 == v2) {
            // A mount showing only a branch of the hierarchy says nothing
            // about a process outside that branch.
            let Some(relative) = strip_cgroup_root(path, &mount.root) else {
                continue;
            };

            let leaf = format!("{}{relative}", mount.point);
            let mut current = leaf.as_str();
            dirs.push(current.to_string());
            while current.len() > mount.point.len() {
                match current.rfind('/') {
                    Some(cut) if cut >= mount.point.len() => current = &current[..cut],
                    _ => break,
                }
                dirs.push(current.to_string());
            }
        }
    }

    dirs.into_iter()
}

/// Returns `path` relative to the subtree `root`, if it is inside it.
#[cfg(target_os = "linux")]
fn strip_cgroup_root<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    if root == "/" {
        return Some(path);
    }
    match path.strip_prefix(root) {
        // The mount is of this exact branch, which is then its own top.
        Some("") => Some(""),
        // Only a prefix that ends at a separator is a parent directory:
        // "/user.slice" is not the parent of "/user.slices".
        Some(rest) if rest.starts_with('/') => Some(rest),
        _ => None,
    }
}

/// Reads the cgroup mounts out of a /proc/self/mountinfo.
///
/// A line is `id parent major:minor root point options... - type source opts`,
/// and the fields before the separator are fixed while those after are not,
/// so the two halves are taken apart separately.
///
/// Not assumed to be /sys/fs/cgroup: the kernel does not require it, and a
/// wrong guess here is a cap that goes unnoticed, which is the failure this
/// whole function exists to prevent.
#[cfg(target_os = "linux")]
fn cgroup_mounts(mounts: &str) -> Vec<CgroupMount> {
    mounts
        .lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;

            let mut head = before.split_whitespace().skip(3);
            let root = head.next()?;
            let point = head.next()?;

            let mut tail = after.split_whitespace();
            let fstype = tail.next()?;
            let _source = tail.next()?;
            let options = tail.next().unwrap_or("");

            // Both are paths, and the kernel escapes the characters that
            // would otherwise break the field layout.
            let root = unescape_mount_path(root);
            // A trailing separator would double up when a path is appended,
            // and "/" is the one mount point that has one.
            let point = unescape_mount_path(point.trim_end_matches('/'));

            match fstype {
                "cgroup2" => Some(CgroupMount {
                    root,
                    point,
                    v2: true,
                }),
                // Version 1 mounts one controller per directory, and the
                // memory one is the only tree with a cap in it.
                "cgroup" if options.split(',').any(|name| name == "memory") => Some(CgroupMount {
                    root,
                    point,
                    v2: false,
                }),
                _ => None,
            }
        })
        .collect()
}

/// How much of a cgroup's allowance is spoken for and cannot be given back.
///
/// Deliberately not `memory.current`, which counts the page cache: on an
/// ordinary desktop session most of that figure is cached files, which the
/// kernel drops the moment anything needs the memory. Subtracting it said a
/// session with 3.5 GB of anonymous memory and 18.6 GB of cache had a
/// gigabyte to spare, and a writer sized for a gigabyte took 73.6 s over a
/// corpus it does in 38.8 s.
///
/// What is counted is what a reclaim cannot free: anonymous pages, kernel
/// slab, socket buffers, and shared memory. The last of those is counted
/// deliberately even though the kernel files it under the page cache, because
/// tmpfs and shm segments can only go to swap, and a cgroup with swap turned
/// off - which is how a container is usually run - cannot shed a byte of
/// them. Reading 384 MiB of tmpfs under a 512 MiB cap as free space is a
/// kill rather than a slowdown.
#[cfg(target_os = "linux")]
fn cgroup_usage(dir: &str) -> Option<u64> {
    let whole = whole_usage(dir);

    let broken_down = std::fs::read_to_string(format!("{dir}/memory.stat"))
        .ok()
        .and_then(|stat| parse_cgroup_usage(&stat));

    // Nothing to break the figure down with, so the whole of it stands. An
    // overstated usage costs speed; an understated one costs the process -
    // which is also why an unreadable usage is not read as an empty cgroup.
    let (Some(counted), Some(whole)) = (broken_down, whole) else {
        return whole;
    };

    // The breakdown is only trusted as far as it adds up. Every line this
    // knows about is either counted as occupied or named as reclaimable, so
    // between them they should account for what the group holds; whatever is
    // left over is a line that did not exist when this was written, and the
    // kernel does add them. Charging the remainder to the writer is what keeps
    // a future field from quietly becoming free memory.
    let explained = counted.occupied.saturating_add(counted.reclaimable);
    let unexplained = whole.saturating_sub(explained);

    Some(counted.occupied.saturating_add(unexplained).min(whole))
}

/// What one `memory.stat` says about a cgroup, in two figures.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct CgroupBreakdown {
    /// Memory a reclaim cannot hand back.
    occupied: u64,
    /// Memory it can, and which is therefore the writer's to use.
    reclaimable: u64,
}

/// Sums the unreclaimable lines of a version 2 `memory.stat`.
///
/// The file is `name value` per line. What is counted is everything a reclaim
/// cannot hand back:
///
/// - `anon`, the anonymous pages;
/// - `shmem`, which the kernel files under the page cache but which can only
///   leave for swap, and a cgroup with swap off cannot shed at all;
/// - `unevictable`, which is pinned by definition;
/// - `kernel`, the aggregate that covers slab, kernel stacks, page tables,
///   percpu and vmalloc together, less `slab_reclaimable`. Counting slab by
///   hand missed the rest of the aggregate, and counting the aggregate whole
///   overstated it by the dentry and inode caches the kernel gives back on
///   demand: on this machine that is 430 MB of the 600 MB `kernel` reports,
///   and charging it to the writer is a budget cut for memory that is there.
/// - `sock`, which the kernel accounts separately from `kernel` - they are
///   distinct counters, and socket buffers are not inside the aggregate - so
///   it is added whether or not the aggregate is there. On a process holding
///   large network buffers, folding it into the fallback lost it entirely.
/// - `hugetlb`, where the kernel is charging huge pages to the cgroup, which
///   nothing reclaims either.
///
/// Older kernels have no `kernel` line, so its parts are added up instead;
/// where both exist only the aggregate is used, or they would count twice.
///
/// `None` for a version 1 file, which has no `anon` line - and no way to tell
/// tmpfs from ordinary cache either, since it files both under `cache` and
/// reports `mapped_file` across both. There is nothing to subtract safely
/// there, so the caller falls back to the whole of `usage_in_bytes`: an
/// overstated usage costs speed, an understated one costs the process.
#[cfg(target_os = "linux")]
fn parse_cgroup_usage(stat: &str) -> Option<CgroupBreakdown> {
    let mut anon = None;
    // Counted whatever else the file holds, since none of these is inside the
    // kernel aggregate.
    let mut apart = 0u64;
    let mut kernel = None;
    let mut kernel_parts = 0u64;
    let mut reclaimable_slab = 0u64;
    let mut file = 0u64;
    let mut shmem = 0u64;
    let mut unevictable = 0u64;

    for line in stat.lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(value)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };

        match name {
            "anon" => anon = Some(value),
            "file" => file = value,
            "shmem" | "unevictable" | "sock" | "hugetlb" => {
                // Kept as well as counted: both of these live inside `file`,
                // which is named reclaimable below, and the two halves have to
                // stay disjoint or what they fail to account for is
                // understated.
                match name {
                    "shmem" => shmem = value,
                    "unevictable" => unevictable = value,
                    _ => {}
                }
                apart = apart.saturating_add(value);
            }
            "kernel" => kernel = Some(value),
            "slab_reclaimable" => reclaimable_slab = value,
            // The fallback for a kernel too old to write the aggregate. It
            // names the unreclaimable half of the slab directly, so nothing
            // has to be taken back off it.
            "slab_unreclaimable" | "kernel_stack" | "pagetables" | "percpu" | "vmalloc" => {
                kernel_parts = kernel_parts.saturating_add(value);
            }
            _ => {}
        }
    }

    let anon = anon?;
    let kernel = match kernel {
        // The aggregate counts the whole slab, and half of it is cache the
        // kernel hands back when asked.
        Some(kernel) => kernel.saturating_sub(reclaimable_slab),
        None => kernel_parts,
    };

    Some(CgroupBreakdown {
        occupied: anon.saturating_add(apart).saturating_add(kernel),
        // The cache, less what is already counted as occupied, plus the half
        // of the slab the kernel gives back on demand.
        //
        // Shared memory and pinned pages both live inside `file` and are both
        // counted above, so leaving them here would make the two halves
        // overlap - and an overlap hides exactly as much of an unrecognised
        // counter as it double-counts. A cap holding 300 MiB of pinned cache
        // and 600 MiB of something this does not know about read as 600 MiB
        // occupied rather than 900.
        //
        // `unevictable` is an LRU figure and some of it may be anonymous
        // rather than cached, in which case this takes off more than it
        // should. That is the safe direction: it understates what can be
        // reclaimed rather than what is held.
        reclaimable: file
            .saturating_sub(shmem)
            .saturating_sub(unevictable)
            .saturating_add(reclaimable_slab),
    })
}

/// Undoes the escaping the kernel applies to a path in mountinfo.
///
/// The fields are separated by spaces, so a path containing one would make the
/// line ambiguous; the kernel writes that space, and a tab, a newline or a
/// backslash, as a backslash and three octal digits. Left as written, the path
/// is opened literally - a directory named with "\040" in it, which is not
/// there - and the cap goes unnoticed.
#[cfg(target_os = "linux")]
fn unescape_mount_path(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut rest = field;

    while let Some(cut) = rest.find('\\') {
        out.push_str(&rest[..cut]);
        let digits = rest.get(cut + 1..cut + 4).unwrap_or("");
        match u8::from_str_radix(digits, 8) {
            Ok(byte) if digits.len() == 3 => {
                out.push(byte as char);
                rest = &rest[cut + 4..];
            }
            // Not an escape the kernel wrote, so it is a backslash meaning
            // itself, and the path is whatever it says.
            _ => {
                out.push('\\');
                rest = &rest[cut + 1..];
            }
        }
    }

    out.push_str(rest);
    out
}

/// Reads one cgroup file as a byte count.
///
/// `None` for a file that is absent, unreadable, or says "max", which is how
/// both versions spell "no limit here".
#[cfg(target_os = "linux")]
fn read_cgroup_value(dir: &str, file: &str) -> Option<u64> {
    let contents = std::fs::read_to_string(format!("{dir}/{file}")).ok()?;
    parse_cgroup_value(&contents)
}

/// Parses what one of those files holds.
///
/// `None` for "max", which is how both versions spell "no limit here", and for
/// anything that is not a plain number.
#[cfg(target_os = "linux")]
fn parse_cgroup_value(contents: &str) -> Option<u64> {
    let value: u64 = contents.trim().parse().ok()?;

    // v1 spells "no limit" as a number near u64::MAX rather than as a word,
    // page-aligned rather than exact, so it cannot be compared for equality -
    // and it is only four kilobytes under half of u64, which is why a bound
    // there let it through. Nothing this side of an exabyte is a real cap.
    const NOT_A_CAP: u64 = 1 << 60;
    (value < NOT_A_CAP).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threads_count_never_zero() {
        assert!(Threads::Auto.count() >= 1);
        assert_eq!(Threads::Single.count(), 1);
        assert_eq!(Threads::count_or_single(0), Threads::Single);
        assert_eq!(Threads::count_or_single(7).count(), 7);
    }

    #[test]
    fn test_threads_is_single() {
        assert!(Threads::Single.is_single());
        assert!(Threads::count_or_single(1).is_single());
        assert!(!Threads::count_or_single(4).is_single());
    }

    #[test]
    fn test_memory_limit_has_a_floor() {
        // A limit smaller than one encoder is rounded up rather than obeyed,
        // because a writer that refuses to run is worse than one that uses a
        // little more than it was told.
        let tiny = MemoryLimit::bytes_or_auto(1);
        assert_eq!(tiny.bytes(), MINIMUM_MEMORY_LIMIT);
    }

    #[test]
    fn test_memory_limit_zero_means_auto() {
        assert_eq!(MemoryLimit::bytes_or_auto(0), MemoryLimit::Auto);
        assert!(MemoryLimit::Auto.bytes() >= MINIMUM_MEMORY_LIMIT);
    }

    /// A machine with less memory free must not get a larger budget.
    ///
    /// Down to and including nothing left: the fixed default used to be reached
    /// from below, so the least memory produced the largest budget.
    #[test]
    fn test_detected_limit_never_grows_as_memory_shrinks() {
        const TOTAL: u64 = 8 * 1024 * 1024 * 1024;

        let mut previous = u64::MAX;
        for available in [
            TOTAL,
            1024 * 1024 * 1024,
            256 * 1024 * 1024,
            64 * 1024 * 1024,
            63 * 1024 * 1024,
            16 * 1024 * 1024,
            1,
            0,
        ] {
            let budget = MemoryLimit::budget_for(Some(available), 8);
            assert!(
                budget <= previous,
                "{available} bytes free yielded {budget}, more than the \
                 {previous} allowed with more memory",
            );
            assert!(budget >= MINIMUM_MEMORY_LIMIT, "below the floor");
            previous = budget;
        }
    }

    /// The fixed default is for a machine that cannot be asked, and only that.
    ///
    /// A cgroup sitting exactly on its cap answers zero, and zero is an
    /// answer. Treating it as "could not ask" handed such a process the fixed
    /// 512 MiB - the largest budget in the function - at the moment it had
    /// nothing at all, which is the OOM this reads cgroups to avoid.
    #[test]
    fn test_the_default_is_only_for_an_unanswerable_machine() {
        assert_eq!(MemoryLimit::budget_for(None, 8), DEFAULT_MEMORY_LIMIT);
        assert_eq!(MemoryLimit::budget_for(Some(0), 8), MINIMUM_MEMORY_LIMIT);
        assert!(MemoryLimit::budget_for(Some(0), 8) < DEFAULT_MEMORY_LIMIT);
    }

    /// More cores to feed means a larger budget, on the same machine.
    ///
    /// This is what a fraction of free memory could not express: the budget
    /// buys a window of blocks, and a window that fills eight cores leaves
    /// three quarters of a thirty-two core machine waiting.
    #[test]
    fn test_a_larger_machine_gets_a_larger_budget() {
        const PLENTY: u64 = 256 * 1024 * 1024 * 1024;

        let mut previous = 0;
        for cores in [1, 2, 8, 32, 128] {
            let budget = MemoryLimit::budget_for(Some(PLENTY), cores);
            assert!(
                budget > previous,
                "{cores} cores yielded {budget}, no more than the {previous} \
                 that fewer cores were given",
            );
            previous = budget;
        }
    }

    /// What is free bounds the answer, however many cores ask for more.
    ///
    /// Half of it, so that the page cache is not evicted by the writer that
    /// depends on it: reading the next entry off a cold cache costs more than
    /// the extra parallelism returns.
    #[test]
    fn test_free_memory_bounds_the_budget() {
        const FREE: u64 = 2 * 1024 * 1024 * 1024;

        let budget = MemoryLimit::budget_for(Some(FREE), 128);
        assert_eq!(budget, FREE / 2);
    }

    /// A machine with a core and nothing else still gets enough to run one.
    #[test]
    fn test_a_small_machine_stays_above_the_floor() {
        let budget = MemoryLimit::budget_for(Some(8 * 1024 * 1024), 1);
        assert_eq!(budget, MINIMUM_MEMORY_LIMIT);
    }

    /// Records what a group is holding, which every group in a real hierarchy
    /// does and which the reading now insists on: a floor binds only the
    /// memory actually there, and a breakdown is trusted only as far as it
    /// accounts for it.
    #[cfg(target_os = "linux")]
    fn holds(dir: &std::path::Path, bytes: u64) {
        std::fs::write(dir.join("memory.current"), format!("{bytes}\n")).expect("current");
    }

    /// What a `memory.stat` says is occupied, which is what most of these
    /// tests are about; the reclaimable half is checked where it matters.
    #[cfg(target_os = "linux")]
    fn occupied_of(stat: &str) -> Option<u64> {
        super::parse_cgroup_usage(stat).map(|breakdown| breakdown.occupied)
    }

    /// A mountinfo naming `mount` as the cgroup2 mount of the whole tree.
    #[cfg(target_os = "linux")]
    fn mountinfo(mount: &str) -> String {
        format!("42 40 0:29 / {mount} rw,nosuid - cgroup2 cgroup2 rw,nsdelegate\n")
    }

    /// A cap several levels above the process still binds it.
    ///
    /// This is the case a container runtime does not produce and a systemd
    /// unit with `MemoryMax` does: the process sits in a leaf, the cap is on
    /// an ancestor, and the root of the hierarchy says "max". Reading only the
    /// root - which is what asking the usual way does - answers "no limit" and
    /// the writer sizes itself for the whole machine, which under a cap of two
    /// gigabytes is a kill rather than a slowdown.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_cap_on_an_ancestor_is_found() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("user.slice/app.slice/run-1.scope");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(mount.path().join("memory.max"), "max\n").expect("root");
        std::fs::write(
            mount.path().join("user.slice/memory.max"),
            format!("{}\n", 2u64 << 30),
        )
        .expect("cap");
        // v2 keeps a usage breakdown at every level, and the one that matters
        // is the one beside the cap. Most of it here is page cache, which a
        // reclaim frees and which must not be counted against the writer.
        std::fs::write(
            mount.path().join("user.slice/memory.stat"),
            format!(
                "anon {}\nfile {}\nslab 0\nsock 0\nshmem 0\n",
                512u64 << 20,
                8u64 << 30
            ),
        )
        .expect("usage");
        holds(&mount.path().join("user.slice"), (8u64 << 30) + (512 << 20));
        holds(&mount.path().join("user.slice/app.slice"), 0);
        holds(&leaf, 0);

        let own = "0::/user.slice/app.slice/run-1.scope\n";
        assert_eq!(
            super::headroom_from(own, &mountinfo(root)),
            Some((2u64 << 30) - (512 << 20)),
            "the cap on the ancestor was not found, or its usage was miscounted",
        );
    }

    /// A hierarchy with nothing capped anywhere has nothing to say.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_an_uncapped_hierarchy_reports_nothing() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        std::fs::create_dir_all(mount.path().join("user.slice")).expect("dirs");
        std::fs::write(mount.path().join("memory.max"), "max\n").expect("root");

        assert_eq!(
            super::headroom_from("0::/user.slice\n", &mountinfo(root)),
            None
        );
    }

    /// The tightest cap on the way up is the one that binds.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_the_tightest_cap_wins() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("outer/inner");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(
            mount.path().join("outer/memory.max"),
            format!("{}\n", 8u64 << 30),
        )
        .expect("outer");
        std::fs::write(mount.path().join("outer/memory.current"), "0\n").expect("outer usage");
        std::fs::write(leaf.join("memory.max"), format!("{}\n", 1u64 << 30)).expect("inner");
        std::fs::write(leaf.join("memory.current"), "0\n").expect("inner usage");

        assert_eq!(
            super::headroom_from("0::/outer/inner\n", &mountinfo(root)),
            Some(1 << 30)
        );
    }

    /// Version 1 writes a line per controller, and only one of them counts.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_the_first_version_is_read_from_its_own_tree() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("memory/limited");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(
            leaf.join("memory.limit_in_bytes"),
            format!("{}\n", 3u64 << 30),
        )
        .expect("cap");
        std::fs::write(
            leaf.join("memory.usage_in_bytes"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("usage");

        let mounts = format!(
            "31 25 0:26 / {root}/memory rw - cgroup cgroup rw,memory\n             32 25 0:27 / {root}/cpu rw - cgroup cgroup rw,cpu\n"
        );
        let own = "4:cpu,cpuacct:/limited\n3:memory:/limited\n";
        assert_eq!(super::headroom_from(own, &mounts), Some(2 << 30));
    }

    /// The hierarchy is found where it is mounted, not where it usually is.
    ///
    /// The kernel does not require /sys/fs/cgroup, and a cap that is looked
    /// for in the wrong place is a cap that goes unnoticed - which is the
    /// failure this reads cgroups to prevent, arrived at from the other side.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_the_mount_point_is_taken_from_mountinfo() {
        let mount = tempfile::tempdir().expect("tempdir");
        let elsewhere = mount.path().join("run/cgroups");
        std::fs::create_dir_all(elsewhere.join("capped")).expect("dirs");
        std::fs::write(
            elsewhere.join("capped/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(elsewhere.join("capped/memory.current"), "0\n").expect("usage");

        let mounts = mountinfo(elsewhere.to_str().expect("utf-8"));
        assert_eq!(super::headroom_from("0::/capped\n", &mounts), Some(1 << 30));

        // And nothing is found when the mount is not there at all.
        assert_eq!(super::headroom_from("0::/capped\n", ""), None);
    }

    /// A mount of one branch shows that branch as the top of the tree.
    ///
    /// The paths in /proc/self/cgroup stay absolute in the hierarchy, so they
    /// are not paths under such a mount until the branch is taken off them.
    /// Appending them whole reads a directory that is not there, and the cap
    /// goes unnoticed.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_mount_of_one_branch_is_understood() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        std::fs::create_dir_all(mount.path().join("run-1.scope")).expect("dirs");
        std::fs::write(
            mount.path().join("run-1.scope/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(mount.path().join("run-1.scope/memory.current"), "0\n").expect("usage");

        // The mount exposes /user.slice, so the process's own
        // /user.slice/run-1.scope is run-1.scope under the mount point.
        let mounts = format!("42 40 0:29 /user.slice {root} rw - cgroup2 cgroup2 rw,nsdelegate\n");
        assert_eq!(
            super::headroom_from("0::/user.slice/run-1.scope\n", &mounts),
            Some(1 << 30),
        );

        // A process outside the branch the mount exposes is not described by
        // it, and a prefix that does not end at a separator is not a parent.
        assert_eq!(
            super::headroom_from("0::/user.slices/other\n", &mounts),
            None
        );
        assert_eq!(super::headroom_from("0::/system.slice/x\n", &mounts), None);
    }

    /// Cached files inside a cgroup are not memory the writer cannot have.
    ///
    /// A desktop session holds gigabytes of cached files, and counting them as
    /// spoken for is how a machine with twenty gigabytes free was read as
    /// having one, and ran a quarter of the encoders it could have.
    ///
    /// The figures are this machine's, so the relationship between them is a
    /// real one: `kernel` covers `slab` rather than sitting beside it.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_cached_files_do_not_count_against_the_budget() {
        let stat = "anon 1338310656\nfile 21077241856\nkernel 730398720\n\
                    kernel_stack 12566528\npagetables 0\npercpu 2053168\nsock 8192\n\
                    vmalloc 229376\nshmem 72044544\nunevictable 0\nslab 685656672\n\
                    slab_reclaimable 430216992\n";
        assert_eq!(
            occupied_of(stat),
            Some(1338310656 + 72044544 + (730398720 - 430216992) + 8192),
            "the file cache was counted, or something unreclaimable was not",
        );

        // Twenty gigabytes of cache, and it is the anonymous pages that decide.
        assert!(occupied_of(stat).expect("parsed") < 3 << 30);

        // Not a breakdown this can use.
        assert_eq!(occupied_of("something else\n"), None);
    }

    /// Version 1 cannot be broken down, so the whole usage stands.
    ///
    /// Its stat file has no `anon` line, and no way to tell tmpfs from
    /// ordinary cache: both land in `cache`, and `mapped_file` spans the two.
    /// Counting `total_rss` alone read 384 MiB of tmpfs under a 512 MiB cap as
    /// free space and handed out a budget against memory that was not there.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_the_first_version_is_counted_whole() {
        let stat = "cache 402653184\nrss 134217728\nmapped_file 402653184\ntotal_rss 134217728\n";
        assert_eq!(
            occupied_of(stat),
            None,
            "a v1 breakdown was trusted, and it cannot account for tmpfs",
        );

        // And the level then reports no headroom at all, rather than the
        // 384 MiB that counting only the resident set would have left.
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");
        let leaf = mount.path().join("memory/capped");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(
            leaf.join("memory.limit_in_bytes"),
            format!("{}\n", 512u64 << 20),
        )
        .expect("cap");
        std::fs::write(leaf.join("memory.stat"), stat).expect("stat");
        std::fs::write(
            leaf.join("memory.usage_in_bytes"),
            format!("{}\n", 512u64 << 20),
        )
        .expect("usage");

        let mounts = format!("31 25 0:26 / {root}/memory rw - cgroup cgroup rw,memory\n");
        assert_eq!(
            super::headroom_from("3:memory:/capped\n", &mounts),
            Some(0),
            "a full v1 cgroup was read as having room",
        );
    }

    /// Kernel memory is counted by its aggregate, not by the parts named here.
    ///
    /// `kernel` covers slab, kernel stacks, page tables, percpu and vmalloc
    /// together, and adding up only the ones this code happens to know about
    /// leaves the rest as headroom that does not exist. On this machine the
    /// aggregate is 730 MB against 686 MB of slab.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_kernel_memory_is_counted_in_full() {
        let with_aggregate =
            "anon 1000\nfile 5000\nkernel 800\nslab 700\nslab_reclaimable 500\nsock 10\nshmem 0\n";
        assert_eq!(
            occupied_of(with_aggregate),
            Some(1000 + (800 - 500) + 10),
            "the aggregate was ignored, counted twice with its parts, kept the \
             reclaimable slab, or took the socket buffers down with it",
        );

        // An older kernel writes the parts and no aggregate.
        let parts_only = "anon 1000\nfile 5000\nslab_unreclaimable 700\nslab_reclaimable 900\n\
                          sock 10\nkernel_stack 40\npagetables 30\npercpu 20\nvmalloc 10\n";
        assert_eq!(
            occupied_of(parts_only),
            Some(1000 + 700 + 10 + 40 + 30 + 20 + 10),
            "the older layout counted the reclaimable half of the slab",
        );
    }

    /// A cap whose usage cannot be read leaves no room, not all of it.
    ///
    /// The two files are written by the same kernel, so one without the other
    /// means something is wrong with the reading rather than that the cgroup is
    /// empty. Answering "all of it" there sized the writer for a cap it had no
    /// measurement of, and the answer is cached for the life of the process.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_cap_without_a_usage_is_not_free_space() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("capped");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(leaf.join("memory.max"), format!("{}\n", 2u64 << 30)).expect("cap");
        // No memory.current and no memory.stat beside it.

        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some(0),
            "an unreadable usage was read as an empty cgroup",
        );

        // And `None` would not have been safe either: it means "no cap here"
        // to everything above, which sizes the writer for the whole machine.
        // What the budget does with an answer of zero is take the floor.
        assert_eq!(MemoryLimit::budget_for(Some(0), 24), MINIMUM_MEMORY_LIMIT);
    }

    /// Memory a cgroup is guaranteed to keep is not headroom for anyone.
    ///
    /// `memory.min` is a floor the kernel will not reclaim below, even under
    /// pressure from a cap further up: it kills instead. The dangerous shape
    /// is the one below - the cap on an ancestor, the floor on a descendant -
    /// because the ancestor's own files say nothing about it, and its cached
    /// pages then look like room the writer can have. This is not
    /// hypothetical: systemd sets a floor on the user slice of this machine.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_floor_below_the_cap_is_not_headroom() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("outer/inner");
        std::fs::create_dir_all(&leaf).expect("leaf");

        // The cap is here, and by its own breakdown it is nearly empty:
        // almost everything it holds is cache.
        std::fs::write(
            mount.path().join("outer/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(
            mount.path().join("outer/memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 8u64 << 20, 600u64 << 20),
        )
        .expect("outer stat");

        // The floor is one level down, where nothing above it is looking.
        std::fs::write(leaf.join("memory.min"), format!("{}\n", 256u64 << 20)).expect("floor");
        std::fs::write(
            leaf.join("memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 4u64 << 20, 300u64 << 20),
        )
        .expect("inner stat");
        holds(&mount.path().join("outer"), 608u64 << 20);
        holds(&leaf, 304u64 << 20);

        assert_eq!(
            super::headroom_from("0::/outer/inner\n", &mountinfo(root)),
            // The anonymous pages the cap holds, plus the floor promised
            // beneath it: different pages, so they add.
            Some((1u64 << 30) - (8 << 20) - (256 << 20)),
            "a floor held below the cap was offered up as free memory",
        );
    }

    /// A group's own floor does not protect it from its own cap.
    ///
    /// Reclaim protection is relative to what the reclaim is for: when the
    /// kernel is reclaiming to keep a group inside its own limit, that group's
    /// `memory.min` has nothing to say. Applying it anyway was safe against an
    /// OOM and expensive in parallelism, since it wrote off memory the cap can
    /// in fact have.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_groups_own_floor_does_not_bind_its_own_cap() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("outer/inner");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(
            mount.path().join("outer/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(
            mount.path().join("outer/memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 50u64 << 20, 300u64 << 20),
        )
        .expect("outer stat");
        std::fs::write(
            mount.path().join("outer/memory.current"),
            format!("{}\n", 350u64 << 20),
        )
        .expect("outer usage");
        // Ignored: it binds pressure from above, not this group's own cap.
        std::fs::write(
            mount.path().join("outer/memory.min"),
            format!("{}\n", 200u64 << 20),
        )
        .expect("outer floor");
        // Honoured: the cap cannot reclaim what was promised beneath it.
        std::fs::write(leaf.join("memory.min"), format!("{}\n", 150u64 << 20))
            .expect("inner floor");
        holds(&leaf, 200u64 << 20);

        assert_eq!(
            super::headroom_from("0::/outer/inner\n", &mountinfo(root)),
            Some((1u64 << 30) - (50 << 20) - (150 << 20)),
            "the cap was bound by its own floor, or not by the one below it",
        );
    }

    /// A floor in a branch this process is not in still binds the cap above it.
    ///
    /// This is the shape that no amount of looking at the process's own
    /// directories can find: a service alongside it in the same slice, with a
    /// promise of its own, holding cached pages the slice's cap cannot take
    /// back. Reading them as reclaimable is how a writer sizes itself for
    /// memory the kernel will not hand over.
    ///
    /// Floors in different branches add up, because each protects its own
    /// pages; a floor inside another floor does not, since a child's effective
    /// floor is bounded by its parent's.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_floor_in_another_branch_is_not_headroom() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let ours = mount.path().join("slice/ours");
        let theirs = mount.path().join("slice/theirs");
        let nested = theirs.join("deeper");
        std::fs::create_dir_all(&ours).expect("ours");
        std::fs::create_dir_all(&nested).expect("theirs");

        std::fs::write(
            mount.path().join("slice/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        // By its own breakdown the slice is nearly empty: it is almost all
        // cache, and the cache is spoken for.
        std::fs::write(
            mount.path().join("slice/memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 8u64 << 20, 900u64 << 20),
        )
        .expect("stat");

        std::fs::write(theirs.join("memory.min"), format!("{}\n", 500u64 << 20))
            .expect("their floor");
        // Inside their floor, so it adds nothing.
        std::fs::write(nested.join("memory.min"), format!("{}\n", 400u64 << 20))
            .expect("nested floor");
        std::fs::write(ours.join("memory.min"), format!("{}\n", 100u64 << 20)).expect("our floor");
        holds(&mount.path().join("slice"), 908u64 << 20);
        holds(&ours, 100u64 << 20);
        holds(&theirs, 500u64 << 20);
        holds(&nested, 400u64 << 20);

        assert_eq!(
            super::headroom_from("0::/slice/ours\n", &mountinfo(root)),
            Some((1u64 << 30) - (8 << 20) - (600 << 20)),
            "a promise made to another branch was counted as free memory",
        );
    }

    /// Protected cache and unrelated anonymous memory are different pages.
    ///
    /// Taking the larger of the two reads the smaller as free. Under a 1 GiB
    /// cap holding 400 MiB of anonymous memory outside the protected branch
    /// and 500 MiB of promised cache inside it, that reported 524 MiB of room
    /// where there are 124, and half of the difference is enough to be killed
    /// for. They are added instead, bounded by what the group actually holds,
    /// since the two do overlap wherever a protected branch has anonymous
    /// pages of its own.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_protected_cache_and_anonymous_memory_do_not_overlap() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let ours = mount.path().join("slice/ours");
        let theirs = mount.path().join("slice/theirs");
        std::fs::create_dir_all(&ours).expect("ours");
        std::fs::create_dir_all(&theirs).expect("theirs");

        std::fs::write(
            mount.path().join("slice/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(
            mount.path().join("slice/memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 400u64 << 20, 500u64 << 20),
        )
        .expect("stat");
        std::fs::write(
            mount.path().join("slice/memory.current"),
            format!("{}\n", 900u64 << 20),
        )
        .expect("current");
        std::fs::write(theirs.join("memory.min"), format!("{}\n", 500u64 << 20))
            .expect("their floor");
        holds(&ours, 0);
        holds(&theirs, 500u64 << 20);

        assert_eq!(
            super::headroom_from("0::/slice/ours\n", &mountinfo(root)),
            Some((1u64 << 30) - (900 << 20)),
            "the anonymous memory and the promised cache were treated as the \
             same pages",
        );
    }

    /// A floor that cannot be read is not the same as no floor.
    ///
    /// Skipping what the walk could not make sense of reports less protected
    /// memory than there is, which is the direction that gets a process
    /// killed. Anything unreadable or malformed ends the walk, and the cap
    /// then counts everything it holds.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_an_unreadable_floor_ends_the_walk() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let ours = mount.path().join("slice/ours");
        let theirs = mount.path().join("slice/theirs");
        std::fs::create_dir_all(&ours).expect("ours");
        std::fs::create_dir_all(&theirs).expect("theirs");

        std::fs::write(
            mount.path().join("slice/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(
            mount.path().join("slice/memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 8u64 << 20, 700u64 << 20),
        )
        .expect("stat");
        std::fs::write(
            mount.path().join("slice/memory.current"),
            format!("{}\n", 708u64 << 20),
        )
        .expect("current");

        // A floor of "max" is a promise this cannot put a number on, and so is
        // anything else that is not a plain count.
        std::fs::write(theirs.join("memory.min"), "max\n").expect("their floor");
        holds(&ours, 0);
        holds(&theirs, 8u64 << 20);

        assert_eq!(
            super::headroom_from("0::/slice/ours\n", &mountinfo(root)),
            Some((1u64 << 30) - (708 << 20)),
            "a floor that could not be read was taken as no floor at all",
        );

        // A floor that is simply absent is the kernel's own default, and means
        // what it says: nothing is promised here.
        std::fs::remove_file(theirs.join("memory.min")).expect("remove");
        assert_eq!(
            super::headroom_from("0::/slice/ours\n", &mountinfo(root)),
            Some((1u64 << 30) - (8 << 20)),
        );
    }

    /// The bound counts groups, not the files a group is made of.
    ///
    /// A cgroup directory holds dozens of controller files beside its
    /// children, so a budget spent per directory entry runs out at around a
    /// hundred groups: the desktop this was written on has 136 groups and 7538
    /// entries under the hierarchy. Counting entries meant the fallback fired
    /// on exactly the ordinary machine the walk exists to serve, and every
    /// cached page on it was written off as occupied.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_the_bound_counts_groups_rather_than_files() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("capped");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(leaf.join("memory.max"), format!("{}\n", 1u64 << 30)).expect("cap");
        std::fs::write(
            leaf.join("memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 8u64 << 20, 700u64 << 20),
        )
        .expect("stat");
        std::fs::write(leaf.join("memory.current"), format!("{}\n", 708u64 << 20))
            .expect("current");

        // Few groups, each with as many files as a real one carries.
        let groups = 8;
        let files_each = super::SUBTREE_BUDGET / groups + 1;
        for group in 0..groups {
            let child = leaf.join(format!("group-{group}"));
            std::fs::create_dir(&child).expect("group");
            holds(&child, 0);
            for file in 0..files_each {
                std::fs::write(child.join(format!("controller.{file}")), "0\n").expect("file");
            }
        }

        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some((1u64 << 30) - (8 << 20)),
            "the walk gave up on a handful of groups because of the files \
             inside them",
        );
    }

    /// A promise binds the memory a group holds, not the figure it was given.
    ///
    /// The kernel protects up to `memory.min`, not `memory.min` regardless of
    /// what is there. Charging the configured number gives away parallelism
    /// for memory nobody has, and the gap is real on an ordinary machine: this
    /// one runs a service promised 64 MiB while holding 1.7, and a session
    /// slice promised 250 MiB while holding 21.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_floor_binds_only_what_a_group_holds() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let ours = mount.path().join("slice/ours");
        let idle = mount.path().join("slice/idle");
        std::fs::create_dir_all(&ours).expect("ours");
        std::fs::create_dir_all(&idle).expect("idle");

        std::fs::write(
            mount.path().join("slice/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(
            mount.path().join("slice/memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 8u64 << 20, 300u64 << 20),
        )
        .expect("stat");
        holds(&mount.path().join("slice"), 308u64 << 20);
        holds(&ours, 0);

        // Promised 250 MiB, holding two.
        std::fs::write(idle.join("memory.min"), format!("{}\n", 250u64 << 20)).expect("floor");
        holds(&idle, 2u64 << 20);

        assert_eq!(
            super::headroom_from("0::/slice/ours\n", &mountinfo(root)),
            Some((1u64 << 30) - (8 << 20) - (2 << 20)),
            "a floor was charged in full against a group that is nearly empty",
        );
    }

    /// A breakdown is trusted only as far as it accounts for the group.
    ///
    /// The kernel adds lines to `memory.stat` as it gains things to report,
    /// and a line this does not know about would otherwise be neither counted
    /// nor named reclaimable - which is to say, silently free. Whatever the
    /// breakdown cannot explain is charged to the writer instead, so a future
    /// field is a little less parallelism rather than an OOM.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_what_the_breakdown_cannot_explain_is_occupied() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("capped");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(leaf.join("memory.max"), format!("{}\n", 1u64 << 30)).expect("cap");

        // 100 MiB of anonymous memory, 200 MiB of cache, and 300 MiB the
        // breakdown does not account for at all.
        std::fs::write(
            leaf.join("memory.stat"),
            format!(
                "anon {}\nfile {}\nkernel 0\nsomething_new {}\n",
                100u64 << 20,
                200u64 << 20,
                300u64 << 20
            ),
        )
        .expect("stat");
        holds(&leaf, 600u64 << 20);

        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some((1u64 << 30) - (400 << 20)),
            "a line the breakdown did not recognise was treated as free memory",
        );

        // What it does explain is not charged twice: the same group without
        // the unknown line leaves the cache to the writer.
        std::fs::write(
            leaf.join("memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 100u64 << 20, 200u64 << 20),
        )
        .expect("stat");
        holds(&leaf, 300u64 << 20);

        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some((1u64 << 30) - (100 << 20)),
        );
    }

    /// The two halves of the breakdown must not describe the same pages.
    ///
    /// Reconciliation only finds what a breakdown missed if occupied and
    /// reclaimable are disjoint. Pinned pages are inside `file` and counted as
    /// occupied, so leaving them in the reclaimable half hid an unrecognised
    /// counter byte for byte: a 1 GiB cap holding 300 MiB of pinned cache and
    /// 600 MiB of something new read as 600 MiB occupied rather than 900, and
    /// offered 424 MiB of room where there are 124.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_an_overlap_cannot_hide_an_unrecognised_counter() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("capped");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(leaf.join("memory.max"), format!("{}\n", 1u64 << 30)).expect("cap");
        std::fs::write(
            leaf.join("memory.stat"),
            format!(
                "anon 0\nfile {}\nunevictable {}\nkernel 0\nsomething_new {}\n",
                300u64 << 20,
                300u64 << 20,
                600u64 << 20
            ),
        )
        .expect("stat");
        holds(&leaf, 900u64 << 20);

        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some((1u64 << 30) - (900 << 20)),
            "the pinned cache was counted on both sides, and hid the counter \
             this does not recognise",
        );

        // The same group without the unknown counter: the pinned pages are
        // still occupied, and nothing else is invented.
        std::fs::write(
            leaf.join("memory.stat"),
            format!(
                "anon 0\nfile {}\nunevictable {}\nkernel 0\n",
                300u64 << 20,
                300u64 << 20
            ),
        )
        .expect("stat");
        holds(&leaf, 300u64 << 20);

        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some((1u64 << 30) - (300 << 20)),
        );
    }

    /// A subtree too large to walk is not assumed to be reclaimable.
    ///
    /// Without the floors underneath it a cap cannot tell which of its cached
    /// pages it may actually take back, so it counts everything it holds, which
    /// is what `memory.current` reports. The walk is bounded because a machine
    /// running thousands of groups under one cap would otherwise pay for the
    /// walk on every process start.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_subtree_too_large_to_walk_counts_everything() {
        let mount = tempfile::tempdir().expect("tempdir");
        let root = mount.path().to_str().expect("utf-8");

        let leaf = mount.path().join("capped");
        std::fs::create_dir_all(&leaf).expect("leaf");
        std::fs::write(leaf.join("memory.max"), format!("{}\n", 1u64 << 30)).expect("cap");
        std::fs::write(
            leaf.join("memory.stat"),
            format!("anon {}\nfile {}\nkernel 0\n", 8u64 << 20, 700u64 << 20),
        )
        .expect("stat");
        std::fs::write(leaf.join("memory.current"), format!("{}\n", 708u64 << 20))
            .expect("current");

        // Small enough to walk: the cache is nobody's, so it is the writer's.
        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some((1u64 << 30) - (8 << 20)),
        );

        for i in 0..=super::SUBTREE_BUDGET {
            let child = leaf.join(format!("group-{i}"));
            std::fs::create_dir(&child).expect("child");
            holds(&child, 0);
        }

        assert_eq!(
            super::headroom_from("0::/capped\n", &mountinfo(root)),
            Some((1u64 << 30) - (708 << 20)),
            "a subtree that could not be accounted for had its cache \
             written off as free",
        );
    }

    /// Socket buffers are accounted apart from the kernel aggregate.
    ///
    /// The kernel keeps them in a counter of their own rather than inside
    /// `kernel`, so a process holding large network buffers loses all of them
    /// from the reckoning if `sock` is only summed as a stand-in for a missing
    /// aggregate.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_socket_buffers_survive_the_aggregate() {
        let stat =
            "anon 1000\nfile 9000\nkernel 2000\nslab 1900\nslab_reclaimable 0\nsock 500000\n";
        assert_eq!(
            occupied_of(stat),
            Some(1000 + 2000 + 500000),
            "half a megabyte of socket buffers went missing",
        );
    }

    /// Huge pages charged to the cgroup are not reclaimable either.
    ///
    /// The line only exists where the kernel is charging them, and where it
    /// does they are part of `memory.current` and count against the cap like
    /// anything else.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_huge_pages_count_against_the_cap() {
        let stat = "anon 1000\nfile 9000\nkernel 0\nhugetlb 2097152\n";
        assert_eq!(occupied_of(stat), Some(1000 + 2097152));

        // Absent on a kernel that does not charge them, and absence is zero
        // rather than a reason to give up on the breakdown.
        assert_eq!(occupied_of("anon 1000\nfile 9000\nkernel 0\n"), Some(1000));
    }

    /// Pinned pages cannot be reclaimed, whatever they are pinned by.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_unevictable_memory_is_not_free_space() {
        let stat = "anon 100\nfile 9000\nunevictable 5000\nkernel 0\nshmem 0\n";
        assert_eq!(occupied_of(stat), Some(5100));
    }

    /// A mount point with a space in it is written escaped, and read escaped.
    ///
    /// The kernel writes a space as `\040` so the fields stay separable. Taken
    /// literally it is a directory that does not exist, and the cap goes
    /// unnoticed - which is the failure this reads cgroups to prevent.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_an_escaped_mount_point_is_decoded() {
        let mount = tempfile::tempdir().expect("tempdir");
        let awkward = mount.path().join("cgroup mount");
        std::fs::create_dir_all(awkward.join("capped")).expect("dirs");
        std::fs::write(
            awkward.join("capped/memory.max"),
            format!("{}\n", 1u64 << 30),
        )
        .expect("cap");
        std::fs::write(awkward.join("capped/memory.current"), "0\n").expect("usage");

        let written = awkward.to_str().expect("utf-8").replace(' ', "\\040");
        let mounts = format!("42 40 0:29 / {written} rw - cgroup2 cgroup2 rw,nsdelegate\n");

        assert_eq!(
            super::headroom_from("0::/capped\n", &mounts),
            Some(1 << 30),
            "the escaped mount point was opened literally",
        );

        assert_eq!(super::unescape_mount_path("a\\040b\\011c"), "a b\tc");
        // A backslash that is not an escape means itself.
        assert_eq!(super::unescape_mount_path("a\\bc"), "a\\bc");
    }

    /// "No limit" is spelled as a word in v2 and as a huge number in v1.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_no_limit_is_recognised_however_it_is_written() {
        assert_eq!(super::parse_cgroup_value("max\n"), None);
        assert_eq!(super::parse_cgroup_value("9223372036854771712\n"), None);
        assert_eq!(super::parse_cgroup_value(""), None);
        assert_eq!(super::parse_cgroup_value("2147483648\n"), Some(2 << 30));
    }

    #[test]
    fn test_memory_limit_honours_an_explicit_value() {
        let limit = MemoryLimit::bytes_or_auto(256 * 1024 * 1024);
        assert_eq!(limit.bytes(), 256 * 1024 * 1024);
    }
}
