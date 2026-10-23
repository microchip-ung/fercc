# Working in this repo

## Dependencies

DO NOT add any new dependencies unless given explicit permission.
dev-only dependencies for developers running tests are allowed if necessary.

## Comments

Don't add new comments. Updating or correcting an existing comment so it stays
accurate is fine; adding a brand-new one to explain code you just wrote is
not (unless specifically given permission per comment) — including doc comments
on new functions/types.
New comments inside test bodies are fine (explaining what a test is checking and why).

Before finishing any change, sweep the diff for comment lines that start with
`+` and check each one: is it new content, or is it an edit to a comment that
already existed at that location before this session? Keep the latter, delete
the former.

## Don't duplicate logic

If the same operation shows up twice with slightly different types around it
(e.g. once operating on owned data, once on a borrowed/ref wrapper), make one
be a thin wrapper around the other rather than writing the logic out twice.
Two implementations of "the same idea" is a bug waiting to happen, not two
pieces of code that happen to look similar.

## Visibility: default to private, check before widening

Only make something `pub`/`pub(crate)` when something outside the module
actually needs it — grep for real call sites first. Rust privacy is
module-scoped, not impl-block-scoped: code anywhere else in the same file can
already see a private field or fn, so `pub(crate)` is usually unnecessary
within a single module. This applies to helper functions too — a function
that only exists to implement one other function's internals doesn't need to
be `pub`, or sometimes even a method at all.

## Don't add code nobody runs

No throwaway example binaries, "manual smoke test" scripts, or scratch
debugging code committed into the repo (`examples/`, etc.) — if it's not
wired into `cargo test` and run on every CI pass, it provides no real
coverage and just rots. Real coverage means an actual `#[test]` with
assertions. Scratch/debugging code during a session belongs in `/tmp`, never
committed.

Conversely: don't add a test for something that's already covered (directly
or transitively) by the existing suite — check first. Do add a test for any
genuinely new public entry point.

## Tests go in test files, not implementation files

Put new tests in the crate's `tests/` directory (e.g. `yang/tests/codec.rs`
for `yang/src/codec.rs`), not in a `#[cfg(test)] mod tests` inside the
implementation file. An in-file test module is only for a test that has to
reach something private to that file. Before writing one, check whether the
behavior can be tested through the public API instead; if it can, it goes in
`tests/`. Don't widen visibility just to move a test out unless told to do that.

## Test coverage is required before committing

Every change needs test coverage before it's committed: either a new
`#[test]` (in `tests/`, see above) or, when
behavior is better exercised against real data, a check against the bundled
real YANG catalog. Check first whether the existing suite already covers the
new behavior (directly or transitively) before adding a redundant test — see
"Don't add code nobody runs" above. A change with no way to verify it is
either untested or untestable; both are a problem to fix before committing,
not after.

## Verify against ground truth, don't assume

Before asserting why some code behaves a certain way, check: the git history,
the actual RFC/spec text, or (for this codebase) pyang's source as the
reference implementation for RFC 7950 semantics. "I believe X" is not the
same as having read the code/spec and confirmed X.

## Real device data is not automatically trustworthy

If a real captured device response contradicts its own YANG schema (e.g. an
`instance-identifier` leaf populated with a value that doesn't satisfy
`require-instance true`), that's a firmware bug, not a case for fercc to
silently tolerate or work around. Prefer fixing/flagging the firmware over
adding client-side accommodation for known-bad device behavior.

## Commit messages

Focus on the *why* and the big picture — what changed conceptually and what
motivated it. Don't narrate the mechanical diff (renamed function X to Y,
added field Z) in the commit message body; that's what the diff itself is
for.

Do not add a `Co-Authored-By` (or similar attribution) line to commit
messages in this repo.

Never describe a bug you introduced and then fixed *within the same session*
as a pre-existing bug the old code had, or as something the commit "found and
fixed" in the codebase. If it didn't exist before this work started, it
doesn't belong in the commit message at all — it's not part of the story of
what changed and why, it's just noise from how the sausage got made.
