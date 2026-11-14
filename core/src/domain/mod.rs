//! The pure domain model of phin: transactions, categories, annotations, rules,
//! and the rule engine that derives a category from data alone.
//!
//! Everything under this module is a pure function over data. It never touches
//! the file system, the network, child processes or the wire protocol; those
//! live in [`core::technical`]. Phase 1 fills this subtree with `money`, `date`,
//! `transaction`, `category`, `rule` and `classify`.
//!
//! The rule is enforced, not just documented: the test
//! `core/src/technical/mod.rs::domain_sources_are_pure` greps every source file
//! in this subtree for forbidden module paths and fails if it finds one.

pub mod date;
pub mod money;
pub mod transaction;

#[test]
fn domain_module_compiles() {}
