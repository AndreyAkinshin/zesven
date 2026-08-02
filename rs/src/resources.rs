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
    /// With the `sysinfo` feature this is a fraction of the memory actually
    /// available; without it, a fixed default that suits an ordinary desktop.
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
            let total = system.total_memory();
            if total == 0 {
                return Self::budget_for(0, 0, cores);
            }

            // Inside a container the host's figures describe a machine this
            // process does not have: the cgroup is what it may actually use,
            // and exceeding that is killed rather than merely slow.
            let available = match System::cgroup_limits(&system) {
                Some(limits) => limits.free_memory.min(system.available_memory()),
                None => system.available_memory(),
            };

            Self::budget_for(available, total, cores)
        }

        // Without the feature there is no way to ask at all, which is the
        // case the fixed default exists for.
        #[cfg(not(feature = "sysinfo"))]
        Self::budget_for(0, 0, cores)
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
    /// The fixed default is only for a machine that cannot be asked at all -
    /// neither figure known. Reaching for it whenever the answer came out small
    /// is what made the setting perverse: a machine with 63 MiB free was handed
    /// 512 MiB, and one with 64 MiB free, 16 MiB.
    fn budget_for(available: u64, total: u64, cores: u64) -> u64 {
        if available > 0 {
            let wanted = cores.max(1).saturating_mul(BUDGET_PER_CORE);
            return wanted.min(available / 2).max(MINIMUM_MEMORY_LIMIT);
        }
        if total > 0 {
            // The question was answered: this machine really has nothing left.
            return MINIMUM_MEMORY_LIMIT;
        }
        DEFAULT_MEMORY_LIMIT
    }
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
            let budget = MemoryLimit::budget_for(available, TOTAL, 8);
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
    #[test]
    fn test_the_default_is_only_for_an_unanswerable_machine() {
        assert_eq!(MemoryLimit::budget_for(0, 0, 8), DEFAULT_MEMORY_LIMIT);
        // Answered, and the answer is that there is nothing to spare.
        assert_eq!(
            MemoryLimit::budget_for(0, 8 * 1024 * 1024 * 1024, 8),
            MINIMUM_MEMORY_LIMIT
        );
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
            let budget = MemoryLimit::budget_for(PLENTY, PLENTY, cores);
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

        let budget = MemoryLimit::budget_for(FREE, 64 * 1024 * 1024 * 1024, 128);
        assert_eq!(budget, FREE / 2);
    }

    /// A machine with a core and nothing else still gets enough to run one.
    #[test]
    fn test_a_small_machine_stays_above_the_floor() {
        let budget = MemoryLimit::budget_for(8 * 1024 * 1024, 512 * 1024 * 1024, 1);
        assert_eq!(budget, MINIMUM_MEMORY_LIMIT);
    }

    #[test]
    fn test_memory_limit_honours_an_explicit_value() {
        let limit = MemoryLimit::bytes_or_auto(256 * 1024 * 1024);
        assert_eq!(limit.bytes(), 256 * 1024 * 1024);
    }
}
