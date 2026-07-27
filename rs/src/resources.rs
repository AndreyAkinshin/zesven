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
    /// A single thread is not merely slower: it is also the only setting under
    /// which a writer produces the same bytes on every machine, since nothing
    /// then depends on how many cores are present.
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
/// It is not a ceiling on the process. Work that must be held whole - an entry
/// small enough to be compressed in memory, or anything the async writer does,
/// since it buffers every entry - costs what it costs, and one such item larger
/// than the limit exceeds it.
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

    /// Returns a limit derived from the machine, where that can be asked.
    fn detected() -> u64 {
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
                return Self::budget_for(0, 0);
            }

            // Inside a container the host's figures describe a machine this
            // process does not have: the cgroup is what it may actually use,
            // and exceeding that is killed rather than merely slow.
            let available = match System::cgroup_limits(&system) {
                Some(limits) => limits.free_memory.min(system.available_memory()),
                None => system.available_memory(),
            };

            Self::budget_for(available, total)
        }

        // Without the feature there is no way to ask at all, which is the
        // case the fixed default exists for.
        #[cfg(not(feature = "sysinfo"))]
        Self::budget_for(0, 0)
    }

    /// The budget for a machine with this much memory free, out of this much.
    ///
    /// A quarter of what is free: enough to matter on a large machine, and not
    /// so much that a writer crowds out everything else. Never below the floor,
    /// since one encoder has to run whatever it costs.
    ///
    /// The fixed default is only for a machine that cannot be asked at all -
    /// neither figure known. Reaching for it whenever the answer came out small
    /// is what made the setting perverse: a machine with 63 MiB free was handed
    /// 512 MiB, and one with 64 MiB free, 16 MiB.
    fn budget_for(available: u64, total: u64) -> u64 {
        if available > 0 {
            return (available / 4).max(MINIMUM_MEMORY_LIMIT);
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
            let budget = MemoryLimit::budget_for(available, TOTAL);
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
        assert_eq!(MemoryLimit::budget_for(0, 0), DEFAULT_MEMORY_LIMIT);
        // Answered, and the answer is that there is nothing to spare.
        assert_eq!(
            MemoryLimit::budget_for(0, 8 * 1024 * 1024 * 1024),
            MINIMUM_MEMORY_LIMIT
        );
    }

    #[test]
    fn test_memory_limit_honours_an_explicit_value() {
        let limit = MemoryLimit::bytes_or_auto(256 * 1024 * 1024);
        assert_eq!(limit.bytes(), 256 * 1024 * 1024);
    }
}
