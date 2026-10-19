// On-disk projects for the multi-file benchmarks and the heap profile. Shared by
// benches/module_graph_benchmark.rs, benches/module_graph_iai.rs and
// examples/dhat_heap.rs, each of which includes this file with #[path]. Every target
// uses a different subset, so unused items are expected here.
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

// A project written under the temp directory and, unless made persistent, removed when
// it is dropped. Files are written before measuring starts, so only the work on them is
// timed or counted.
pub struct Project {
    pub root: PathBuf,
    pub entries: Vec<PathBuf>,
    keep: bool,
}

impl Project {
    // Removed on drop. The process id keeps two benchmark processes apart.
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("tsox-bench-{name}-{}", std::process::id()));
        Self::at(root, false)
    }

    // A fixed path that is left on disk. iai-callgrind passes its arguments by value into
    // the measured function, so a project that deleted itself on drop would count the
    // deletion; the argument is just the entry paths and the directory outlives them.
    pub fn persistent(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("tsox-iai-{name}"));
        Self::at(root, true)
    }

    fn at(root: PathBuf, keep: bool) -> Self {
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("benchmark directory should be creatable");
        // Canonical, so a file reached through an import and the same file named as an
        // entry are one graph node even when the temp directory is behind a symlink.
        let root = fs::canonicalize(&root).unwrap_or(root);
        Self {
            root,
            entries: Vec::new(),
            keep,
        }
    }

    // Writes or rewrites a file. Only a file written with `entry` set is a graph entry;
    // the others are reached through imports.
    pub fn write(&mut self, name: &str, source: &str, entry: bool) {
        let path = self.root.join(name);
        fs::write(&path, source).expect("benchmark file should be writable");
        if entry {
            self.entries.push(path);
        }
    }

    // `main.ts` imports the first layer, and each file of a layer imports `fan_out`
    // files of the next one. Imports are what the graph is made of, so this is the shape
    // that exercises discovery, resolution and layering. It holds layers * width + 1
    // files.
    pub fn layered(mut self, layers: usize, width: usize, fan_out: usize) -> Self {
        let mut main = String::new();
        for i in 0..width {
            main.push_str(&format!("import {{ v0_{i} }} from \"./l0_{i}\";\n"));
        }
        self.write("main.ts", &main, true);

        for layer in 0..layers {
            for i in 0..width {
                let mut source = String::new();
                if layer + 1 < layers {
                    for step in 0..fan_out {
                        let next = (i + step) % width;
                        source.push_str(&format!(
                            "import {{ v{n}_{next} }} from \"./l{n}_{next}\";\n",
                            n = layer + 1
                        ));
                    }
                }
                source.push_str(&format!("export const v{layer}_{i} = {i};\n"));
                self.write(&format!("l{layer}_{i}.ts"), &source, false);
            }
        }
        self
    }

    // Files with no imports, so the time is spent checking functions and not on the
    // warning each import statement still gets. Every file is an entry and none depends
    // on another, so one layer holds them all and the parallel driver has the most to
    // spread.
    pub fn independent(mut self, files: usize, functions_each: usize) -> Self {
        for n in 0..files {
            let mut source = String::new();
            for i in 0..functions_each {
                source.push_str(&format!(
                    "function add{i}(a: number, b: number): number {{\n\
                     \treturn a + b;\n\
                     }}\n\
                     add{i}(1, 2);\n"
                ));
            }
            self.write(&format!("solo_{n}.ts"), &source, true);
        }
        self
    }

    // Like `independent`, but every function is generic and called, so the work is
    // inference and substitution and not plain arithmetic.
    pub fn independent_generic(mut self, files: usize, functions_each: usize) -> Self {
        for n in 0..files {
            let mut source = String::new();
            for i in 0..functions_each {
                source.push_str(&format!(
                    "function id{i}<T>(value: T): T {{\n\
                     \treturn value;\n\
                     }}\n\
                     function pair{i}<T, U>(first: T, second: U): T {{\n\
                     \treturn first;\n\
                     }}\n\
                     id{i}(1);\n\
                     pair{i}(\"a\", 2);\n"
                ));
            }
            self.write(&format!("generic_{n}.ts"), &source, true);
        }
        self
    }

    // One import cycle of `size` files, each importing the next and the last importing
    // the first. Only `ring_0.ts` is an entry; the rest are found through the cycle.
    pub fn ring(mut self, size: usize) -> Self {
        for i in 0..size {
            let next = (i + 1) % size;
            let source =
                format!("import {{ r{next} }} from \"./ring_{next}\";\nexport const r{i} = {i};\n");
            self.write(&format!("ring_{i}.ts"), &source, i == 0);
        }
        self
    }

    // A single chain, each file importing the next, so the graph is `length` layers of
    // one file each and nothing can run in parallel.
    pub fn chain(mut self, length: usize) -> Self {
        for i in 0..length {
            let mut source = String::new();
            if i + 1 < length {
                source.push_str(&format!(
                    "import {{ c{n} }} from \"./chain_{n}\";\n",
                    n = i + 1
                ));
            }
            source.push_str(&format!("export const c{i} = {i};\n"));
            self.write(&format!("chain_{i}.ts"), &source, i == 0);
        }
        self
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
