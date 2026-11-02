use bumpalo::Bump;

// Scratch memory for one worker thread: data that is built, used and thrown away inside
// a single job. Rule 1 of LLD 2.1 is that nothing in a bump needs Drop, because Bump
// never runs destructors and a value that owns heap memory would leak it on every
// reset. alloc therefore takes only Copy values, which cannot have a destructor, and
// text goes in through alloc_str. Anything that has to outlive the job is copied out
// into an owned value before reset.
//
// This is separate from oxc_allocator::Allocator on purpose. Oxc parses into its own
// allocator and an Oxc AST cannot live in another one, so the two are not
// interchangeable and this type does not try to hold AST nodes.
//
// allocated_bytes is public because dhat only sees allocations that go through the
// global allocator, not a bump's chunks (LLD 2.3), so a memory report has to add the
// bump's bytes itself.
pub struct WorkerScratch {
    bump: Bump,
}

impl WorkerScratch {
    pub fn new() -> Self {
        Self { bump: Bump::new() }
    }

    /// A scratchpad with bytes of room already reserved, so a job of known size never
    /// has to grow it.
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            bump: Bump::with_capacity(bytes),
        }
    }

    /// Frees everything allocated so far at once and keeps the largest chunk for the
    /// next job. Taking &mut self means no allocation can still be borrowed.
    pub fn reset(&mut self) {
        self.bump.reset();
    }

    // Each allocation comes from fresh memory, so two results of alloc never alias even
    // though they are handed out through a shared reference. Bumpalo's own alloc has the
    // same allow.
    #[allow(clippy::mut_from_ref)]
    pub fn alloc<T: Copy>(&self, value: T) -> &mut T {
        self.bump.alloc(value)
    }

    pub fn alloc_str(&self, text: &str) -> &str {
        self.bump.alloc_str(text)
    }

    /// The memory the scratchpad holds, in use or not. Kept across reset.
    pub fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes()
    }
}

impl Default for WorkerScratch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerScratch;

    #[test]
    fn values_and_text_are_readable_until_the_reset() {
        let scratch = WorkerScratch::new();
        let number = scratch.alloc(41u32);
        *number += 1;
        assert_eq!(*number, 42);
        assert_eq!(scratch.alloc_str("temporary"), "temporary");
    }

    #[test]
    fn a_reset_keeps_the_memory_for_the_next_job() {
        let mut scratch = WorkerScratch::with_capacity(4096);
        let reserved = scratch.allocated_bytes();

        for value in 0..64u32 {
            scratch.alloc(value);
        }
        assert_eq!(
            scratch.allocated_bytes(),
            reserved,
            "the job fit the reservation"
        );

        scratch.reset();
        assert_eq!(
            scratch.allocated_bytes(),
            reserved,
            "reset does not give memory back"
        );

        for value in 0..64u32 {
            scratch.alloc(value);
        }
        assert_eq!(
            scratch.allocated_bytes(),
            reserved,
            "the second job reused it"
        );
    }
}
