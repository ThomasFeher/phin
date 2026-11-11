# `core/src/domain/` — the pure domain model

This subtree holds the domain: the financial transaction, the category tree,
annotations, rules and the rule engine. It is the half of the core that knows
*what a transaction is*, and nothing about *how the data gets in or out*.

## The rule

Nothing in this subtree may perform side effects:

- no file-system access,
- no network access,
- no child processes,
- no environment variables, clock, or randomness,
- no protocol framing.

Those belong to `core/src/technical/`. A function in here must be callable from
a unit test with no setup at all: give it data, get data back.

## Why

Because the domain is pure, every phase-1 feature (amounts, dates, categories,
rules) is unit-testable on its own, and the same domain value can be produced by
an importer, persisted by the data store, decorated by the rule engine and
consumed by an analysis or a plugin without any of them knowing about the others.

## Enforcement

`core/src/technical/mod.rs` contains a test that walks every `.rs` file in this
directory and fails if it finds a reference to the forbidden standard-library
modules. The check is textual, so a mention even in a comment fails the build;
write "the file-system module" instead of naming it.
