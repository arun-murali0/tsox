// Names one file of a project. It exists so that identity derived from a source
// position (TypeParameterId in types.rs) stays unambiguous once several files share
// semantic state: byte offset 40 in `a.ts` and byte offset 40 in `b.ts` are different
// declarations. It is a plain index, not a path, so type identity never depends on a
// filesystem or an Oxc allocation; ProjectFiles owns the path-to-id mapping. Ordered so
// that project-level results (module graph layers, check reports) can be sorted into a
// stable order that does not depend on which thread finished first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileId(u32);

impl FileId {
    /// The id every single-file check uses.
    pub const ROOT: Self = Self(0);

    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u32 {
        self.0
    }
}
