# E4 phase 2: the project check on the ready queue

Whole files, to be copied over the repository (paths are relative to the repo root).
Apply on top of the scheduler commit (phase 1, 2a0cdac).

Files: src/module_resolution/{check,graph,discovery,mod}.rs, src/lib.rs, src/scc.rs,
tests/scc_contract.rs, docs/decisions.md, docs/architecture/module-resolution.md.
engine-track-status.md is the status doc, not part of the repo.

Checked: cargo fmt clean, cargo test --all-features 1,208 passed 0 failed, corpus abort test at
stride 1 passes, whole corpus as one project byte-identical to the old driver, 1.0 s against 2.3 s.

One test was changed on purpose: an_unsettled_cycle_reports_no_missing_name_among_its_own_members
now checks for missing name and module errors only (see decision 16).
